use layer36::{
    io::{args, stdio, streams::OutputStreamExt},
    net::{self, NetError},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw_args = args::raw();
        let url = match raw_args.split('\n').find(|arg| !arg.is_empty()) {
            Some(url) => url,
            None => {
                let _ = stdio::eprintln("usage: layer36-curl <url>");
                return 2;
            }
        };

        let stderr = stdio::stderr();

        let body = match net::get(url) {
            Ok(body) => body,
            Err(NetError::PermissionDenied) => {
                let _ = stderr.write_line("layer36-curl: permission denied");
                let _ = stderr.flush();
                return 5;
            }
            Err(NetError::InvalidUrl) => {
                let _ = stderr.write_line("layer36-curl: invalid url");
                let _ = stderr.flush();
                return 20;
            }
            Err(NetError::BodyTooLarge) => {
                let _ = stderr.write_line("layer36-curl: response too large");
                let _ = stderr.flush();
                return 21;
            }
            Err(NetError::Timeout) => {
                let _ = stderr.write_line("layer36-curl: request timed out");
                let _ = stderr.flush();
                return 21;
            }
            Err(NetError::Protocol(_)) => {
                let _ = stderr.write_line("layer36-curl: protocol error");
                let _ = stderr.flush();
                return 21;
            }
            Err(_) => {
                let _ = stderr.write_line("layer36-curl: fetch failed");
                let _ = stderr.flush();
                return 21;
            }
        };

        let stdout = stdio::stdout();
        if stdout.write_all(&body).is_err() || stdout.flush().is_err() {
            return 23;
        }

        0
    }
}

layer36::export!(Component);
