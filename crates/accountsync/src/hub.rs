//! The device's side of the hub's `/account/sync` routes
//! (cloud/worker/src/account-sync.js). Every call carries the person's
//! sign-in: the hub decides whose account it is from that, never from
//! anything the device says.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubError {
    /// No answer from the hub: offline is a normal state.
    Offline,
    /// The hub refused, with its stable reason and words.
    Refused {
        status: u16,
        reason: String,
        message: String,
    },
    /// The hub answered something that is not a hub's answer.
    Hub(String),
}

impl std::fmt::Display for HubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HubError::Offline => f.write_str("the hub could not be reached"),
            HubError::Refused {
                message, reason, ..
            } if !message.is_empty() => write!(f, "{message} ({reason})"),
            HubError::Refused { reason, .. } => write!(f, "the hub refused: {reason}"),
            HubError::Hub(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for HubError {}

/// The most of one hub answer the device reads: a whole bucket (the hub
/// keeps at most 24 MiB in one, cloud/worker/src/account-sync.js
/// BUCKET_MAX) and its JSON around it. ureq's `into_string` stops at
/// 10 MiB, so an app keeping a few MB synced once and then read every
/// later answer as "offline", for good (K-1221).
pub const ANSWER_MAX: u64 = 26 * 1024 * 1024;

/// An answer's text, up to `ANSWER_MAX`; longer is not a hub's answer.
fn answer_text(url: &str, response: ureq::Response) -> Result<String, HubError> {
    use std::io::Read;
    let mut text = String::new();
    response
        .into_reader()
        .take(ANSWER_MAX + 1)
        .read_to_string(&mut text)
        .map_err(|_| HubError::Offline)?;
    if text.len() as u64 > ANSWER_MAX {
        return Err(HubError::Hub(format!(
            "{url} answered more than {} MB",
            ANSWER_MAX / (1024 * 1024)
        )));
    }
    Ok(text)
}

/// One signed-in person's connection to the hub.
pub struct Hub {
    base: String,
    authorization: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hub({})", self.base)
    }
}

impl Hub {
    /// `token` is the sign-in the player keeps (a Krate session or a GitHub
    /// token); it is only ever sent to `base`.
    pub fn new(base: &str, token: &str) -> Hub {
        let base = base.trim_end_matches('/').to_string();
        let agent = krate_http::builder(&base)
            .timeout_connect(std::time::Duration::from_secs(4))
            .timeout_read(std::time::Duration::from_secs(20))
            .build();
        Hub {
            base,
            authorization: format!("Bearer {}", token.trim()),
            agent,
        }
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, HubError> {
        let url = format!("{}/account/sync{path}", self.base);
        let request = self
            .agent
            .request(method, &url)
            .set("accept", "application/json")
            .set("authorization", &self.authorization);
        let sent = match body {
            Some(body) => request
                .set("content-type", "application/json")
                .send_string(&body.to_string()),
            None => request.call(),
        };
        Self::answered(&url, sent)
    }

    /// The account's keys as the hub holds them, for `device` (when given):
    /// `{account, key, device, envelope, devices, recovery}`.
    pub fn view(&self, device: Option<&str>) -> Result<Value, HubError> {
        match device {
            Some(id) => self.call("GET", &format!("/keys?device={id}"), None),
            None => self.call("GET", "/keys", None),
        }
    }

    /// Register this device's public key under the account.
    pub fn register(&self, device_id: &str, public_hex: &str, name: &str) -> Result<(), HubError> {
        self.call(
            "PUT",
            &format!("/devices/{device_id}"),
            Some(&serde_json::json!({ "public_key": public_hex, "name": name })),
        )
        .map(|_| ())
    }

    /// Forget a device: its envelope goes.
    pub fn remove(&self, device_id: &str) -> Result<(), HubError> {
        self.call("DELETE", &format!("/devices/{device_id}"), None)
            .map(|_| ())
    }

    /// The account's first key, made on this device: its public half, this
    /// device's envelope and the recovery envelope.
    pub fn init(&self, body: &Value) -> Result<(), HubError> {
        self.call("POST", "/init", Some(body)).map(|_| ())
    }

    /// Pass the key to `device_id`, sealed to its public key, from
    /// `by_device` (or `"recovery"`).
    pub fn grant(
        &self,
        device_id: &str,
        version: u32,
        blob: &str,
        by_device: &str,
    ) -> Result<(), HubError> {
        self.call(
            "POST",
            "/grants",
            Some(&serde_json::json!({
                "device_id": device_id, "version": version, "blob": blob, "by_device": by_device,
            })),
        )
        .map(|_| ())
    }

    /// The recovery envelope and its Argon2id parameters.
    pub fn recovery(&self) -> Result<Value, HubError> {
        self.call("GET", "/recovery", None)
    }

    /// One app's bucket: `{rev, entries}`, or `{rev, unchanged: true}` when
    /// it is still at `since`.
    pub fn bucket(&self, bucket: &str, since: Option<u64>) -> Result<Value, HubError> {
        match since {
            Some(rev) => self.call("GET", &format!("/buckets/{bucket}?since={rev}"), None),
            None => self.call("GET", &format!("/buckets/{bucket}"), None),
        }
    }

    /// Put one sealed file body (S3) under `id`, the SHA-256 of its bytes,
    /// in the app's `bucket`. The hub keeps it once: a body it already
    /// holds costs no write.
    pub fn put_blob(&self, bucket: &str, id: &str, sealed: &[u8]) -> Result<(), HubError> {
        let url = format!("{}/account/sync/blobs/{bucket}/{id}", self.base);
        let sent = self
            .agent
            .put(&url)
            .set("accept", "application/json")
            .set("authorization", &self.authorization)
            .set("content-type", "application/octet-stream")
            .send_bytes(sealed);
        Self::answered(&url, sent).map(|_| ())
    }

    /// One sealed file body, at most `max` bytes.
    pub fn blob(&self, bucket: &str, id: &str, max: u64) -> Result<Vec<u8>, HubError> {
        use std::io::Read;
        let url = format!("{}/account/sync/blobs/{bucket}/{id}", self.base);
        let sent = self
            .agent
            .get(&url)
            .set("authorization", &self.authorization)
            .call();
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Self::answered(&url, Err(error)).map(|_| Vec::new()),
        };
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(max.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| HubError::Offline)?;
        if bytes.len() as u64 > max {
            return Err(HubError::Hub(format!("{url} sent more than a file body")));
        }
        Ok(bytes)
    }

    fn answered(url: &str, sent: Result<ureq::Response, ureq::Error>) -> Result<Value, HubError> {
        match sent {
            Ok(response) => {
                let text = answer_text(url, response)?;
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                serde_json::from_str(&text)
                    .map_err(|_| HubError::Hub(format!("{url} did not answer as a Krate hub")))
            }
            Err(ureq::Error::Status(status, response)) if status < 500 && status != 429 => {
                let body: Value =
                    serde_json::from_str(&answer_text(url, response).unwrap_or_default())
                        .unwrap_or_default();
                Err(HubError::Refused {
                    status,
                    reason: body["error"].as_str().unwrap_or("refused").to_string(),
                    message: body["message"].as_str().unwrap_or("").to_string(),
                })
            }
            Err(_) => Err(HubError::Offline),
        }
    }

    /// Send `writes` (`[{name, vv, by, kind, k, c}]`) and answer the bucket
    /// as it stands after them, in one request and one hub write.
    pub fn put(&self, bucket: &str, writes: &[Value]) -> Result<Value, HubError> {
        self.call(
            "PUT",
            &format!("/buckets/{bucket}"),
            Some(&serde_json::json!({ "writes": writes })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A one-request hub on localhost answering `body` with 200.
    fn serve(body: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        });
        format!("http://{address}")
    }

    #[test]
    fn a_bucket_over_ten_megabytes_is_read_not_called_offline() {
        // A bucket the hub would keep (under its 24 MiB), past ureq's
        // 10 MiB string cap: the answer the device read as "offline".
        let filler = "x".repeat(12 * 1024 * 1024);
        let body = serde_json::json!({ "entries": [], "pad": filler }).to_string();
        let hub = Hub::new(&serve(body.into_bytes()), "token");
        let answer = hub
            .call("GET", "/buckets/b", None)
            .expect("a 12 MB answer is read");
        assert_eq!(answer["pad"].as_str().map(str::len), Some(12 * 1024 * 1024));
    }

    #[test]
    fn an_answer_past_the_ceiling_is_named_not_called_offline() {
        let body = vec![b' '; (ANSWER_MAX + 16) as usize];
        let hub = Hub::new(&serve(body), "token");
        match hub.call("GET", "/buckets/b", None) {
            Err(HubError::Hub(words)) => assert!(words.contains("more than"), "{words}"),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }
}
