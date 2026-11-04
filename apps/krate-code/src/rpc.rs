//! The framing the Language Server Protocol and the Debug Adapter Protocol
//! share: a `Content-Length: N` header, a blank line, then N bytes of JSON.
//!
//! Bytes arrive from a pipe in whatever pieces the system hands over -- half
//! a header, three messages at once -- so [`Framer`] keeps what it has and
//! hands back each whole message as soon as its last byte is in.

use crate::json::{self, Value};
use alloc::string::String;
use alloc::vec::Vec;

/// One message, framed for the wire.
pub fn frame(v: &Value) -> Vec<u8> {
    let body = v.to_json();
    let mut out = Vec::with_capacity(body.len() + 32);
    out.extend_from_slice(b"Content-Length: ");
    out.extend_from_slice(crate::text::num(body.len()).as_bytes());
    out.extend_from_slice(b"\r\n\r\n");
    out.extend_from_slice(body.as_bytes());
    out
}

#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<u8>,
}

/// The most one message may be before it is dropped as nonsense.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

impl Framer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message: `Some(Err)` for one that is not JSON (it is
    /// skipped), `None` while the next one is still arriving.
    pub fn next_message(&mut self) -> Option<Result<Value, String>> {
        let end = find(&self.buf, b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&self.buf[..end]).into_owned();
        let mut len: Option<usize> = None;
        for line in head.split("\r\n") {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().ok();
                }
            }
        }
        let Some(len) = len.filter(|n| *n <= MAX_MESSAGE) else {
            // A header without a length cannot be followed; drop it.
            self.buf.drain(..end + 4);
            return Some(Err(String::from("a message without a Content-Length")));
        };
        let start = end + 4;
        if self.buf.len() < start + len {
            return None;
        }
        let body: Vec<u8> = self.buf.drain(..start + len).skip(start).collect();
        let text = String::from_utf8_lossy(&body);
        Some(json::parse(&text).ok_or_else(|| String::from("a message that is not JSON")))
    }

    /// Every whole message there is now, skipping broken ones.
    pub fn drain(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(m) = self.next_message() {
            if let Ok(v) = m {
                out.push(v);
            }
        }
        out
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::obj;

    #[test]
    fn frames_round_trip_in_any_pieces() {
        let a = obj([("id", 1i64.into()), ("method", "x".into())]);
        let b = obj([("id", 2i64.into()), ("text", "é ünïcode".into())]);
        let mut wire = frame(&a);
        wire.extend_from_slice(&frame(&b));
        // Byte by byte: nothing comes out early, both come out whole.
        let mut f = Framer::default();
        let mut got = Vec::new();
        for byte in &wire {
            f.push(core::slice::from_ref(byte));
            got.extend(f.drain());
        }
        assert_eq!(got, alloc::vec![a.clone(), b.clone()]);
        // All at once.
        let mut f = Framer::default();
        f.push(&wire);
        assert_eq!(f.drain(), alloc::vec![a, b]);
    }

    #[test]
    fn content_length_counts_bytes_and_extra_headers_are_fine() {
        let body = r#"{"s":"é"}"#;
        let mut wire = Vec::new();
        wire.extend_from_slice(
            b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: ",
        );
        wire.extend_from_slice(crate::text::num(body.len()).as_bytes());
        wire.extend_from_slice(b"\r\n\r\n");
        wire.extend_from_slice(body.as_bytes());
        let mut f = Framer::default();
        f.push(&wire);
        assert_eq!(f.drain()[0].get("s").str(), Some("é"));
    }

    #[test]
    fn broken_messages_are_skipped() {
        let mut f = Framer::default();
        f.push(b"X-Nothing: 1\r\n\r\nContent-Length: 3\r\n\r\nabcContent-Length: 2\r\n\r\n{}");
        assert!(matches!(f.next_message(), Some(Err(_))));
        assert!(matches!(f.next_message(), Some(Err(_))));
        assert_eq!(f.next_message(), Some(Ok(Value::Obj(Vec::new()))));
        assert_eq!(f.next_message(), None);
    }
}
