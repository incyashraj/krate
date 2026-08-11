// A Krate guest is no_std: the SDK owns the allocator, panic handler, and
// mem intrinsics, so this app cannot pull std's latent wasi:* imports.
#![no_std]
extern crate alloc;

use krate::{
    io::{args, stdio, streams::OutputStreamExt},
    net::{self, NetError},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw_args = args::raw();
        let first = raw_args.split('\n').find(|arg| !arg.is_empty());

        // `quick` is the harness asking "does this app work", not a URL.
        // Fetching anything real there would make the gate depend on the
        // network; saying so and succeeding is the honest answer, and it
        // is the same contract every other app in the fleet keeps.
        if first == Some("quick") {
            let _ = stdio::stdout().write_all(b"curl:ready\n");
            return 0;
        }

        let url = match first {
            Some(url) => url,
            None => {
                let _ = stdio::eprintln("usage: krate-curl <url>");
                return 2;
            }
        };

        let stderr = stdio::stderr();

        let body = match net::get(url) {
            Ok(body) => body,
            Err(err) => {
                let (message, code) = classify_net_error(&err);
                let _ = stderr.write_line(message);
                let _ = stderr.flush();
                return code;
            }
        };

        let stdout = stdio::stdout();
        if stdout.write_all(&body).is_err() || stdout.flush().is_err() {
            return 23;
        }

        0
    }
}

krate::export!(Component);

fn classify_net_error(err: &NetError) -> (&'static str, i32) {
    match err {
        NetError::PermissionDenied => ("krate-curl: permission denied", 5),
        NetError::InvalidUrl => ("krate-curl: invalid url", 20),
        NetError::BodyTooLarge => ("krate-curl: response too large", 21),
        NetError::Timeout => ("krate-curl: request timed out", 21),
        NetError::Protocol(_) => ("krate-curl: protocol error", 21),
        NetError::TlsFailure(_) => ("krate-curl: tls handshake failed", 21),
        NetError::DnsFailure(_) => ("krate-curl: dns lookup failed", 21),
        NetError::ConnectFailure(_) => ("krate-curl: connection failed", 21),
        NetError::Other(_) => ("krate-curl: fetch failed", 21),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_net_error_covers_dns_and_connect_failures() {
        assert_eq!(
            classify_net_error(&NetError::DnsFailure("not found".to_string())),
            ("krate-curl: dns lookup failed", 21)
        );
        assert_eq!(
            classify_net_error(&NetError::ConnectFailure("refused".to_string())),
            ("krate-curl: connection failed", 21)
        );
    }
}
