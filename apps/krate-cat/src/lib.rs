use krate::{
    fs::{self, FsError, OpenMode},
    io::{
        args, stdio,
        streams::{OutputStream, OutputStreamExt},
    },
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw_args = args::raw();
        let stderr = stdio::stderr();

        if raw_args.is_empty() {
            let _ = stderr.write_line("usage: krate-cat <file> [file...]");
            let _ = stderr.flush();
            return 2;
        }

        let stdout = stdio::stdout();
        for path in raw_args.split('\n').filter(|path| !path.is_empty()) {
            let file = match fs::open(path, OpenMode::Read) {
                Ok(file) => file,
                Err(FsError::PermissionDenied) => {
                    let _ = write_error(&stderr, "permission denied", path);
                    let _ = stderr.flush();
                    return 5;
                }
                Err(FsError::NotFound) => {
                    let _ = write_error(&stderr, "not found", path);
                    let _ = stderr.flush();
                    return 20;
                }
                Err(_) => {
                    let _ = write_error(&stderr, "could not open", path);
                    let _ = stderr.flush();
                    return 21;
                }
            };

            loop {
                let bytes = match file.read(8192) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        let _ = write_error(&stderr, "could not read", path);
                        let _ = stderr.flush();
                        return 22;
                    }
                };

                if bytes.is_empty() {
                    break;
                }

                if stdout.write_all(&bytes).is_err() {
                    return 23;
                }
            }
        }

        if stdout.flush().is_err() {
            return 24;
        }

        0
    }
}

fn write_error(stream: &OutputStream, message: &str, path: &str) -> bool {
    stream.write_text("krate-cat: ").is_ok()
        && stream.write_text(message).is_ok()
        && stream.write_text(": ").is_ok()
        && stream.write_line(path).is_ok()
}

krate::export!(Component);
