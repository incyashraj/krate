use layer36::{
    io::{
        args, stdio,
        streams::OutputStreamExt,
    },
    net::{self, NetError},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let url = match args::first() {
            Some(url) => url,
            None => {
                let _ = stdio::eprintln("usage: layer36-curl <url>");
                return 2;
            }
        };

        let stderr = stdio::stderr();

        let body = match net::get(url.as_str()) {
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
