//! Krate Hub: the smallest honest place to publish a `.krate` and get a URL
//! back that `krate run <url>` can fetch.
//!
//! This is a v1 to make one-click share real for the demo, not production
//! infra. The design decisions that follow from that:
//!
//! - **Content-addressed.** The store key is the sha256 of the uploaded bytes,
//!   so the same app always lands at the same URL. That is why there is no auth
//!   and no database: there is nothing to overwrite and nothing to look up but
//!   a file on disk.
//! - **The filesystem is the store.** One directory, one file per hash. Losing
//!   the directory loses the store, which is fine for what this is.
//! - **Hand-rolled HTTP.** A single `TcpListener` and a thread per connection,
//!   no async runtime and no web framework, because the surface is three routes
//!   and keeping the dependency list to `sha2` + `zip` keeps this auditable.
//!
//! Routes:
//!   POST /publish   -> stores the body, returns JSON { "url", "id" }
//!   GET  /a/<hash>  -> returns the stored .krate bytes
//!   GET  /health    -> "ok"

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};

/// (method, path, headers) parsed from a request line and its header block.
type RequestHead = (String, String, Vec<(String, String)>);

/// Largest upload accepted. A real `.krate` is tens of kilobytes; 5 MiB is
/// generous headroom while still refusing anything that is obviously not one.
const MAX_UPLOAD_BYTES: usize = 5 * 1024 * 1024;

/// How long a request line + headers may be before we give up reading them.
/// The routes here take no large headers, so anything past this is junk or an
/// attempt to make us buffer forever.
const MAX_HEADER_BYTES: usize = 16 * 1024;

struct Config {
    addr: String,
    /// Where uploaded bundles are stored, one file per content hash.
    data_dir: PathBuf,
    /// The origin used to build the returned URL, e.g. `http://127.0.0.1:8787`.
    /// Configurable so a deployment behind a real hostname hands out links that
    /// actually resolve from elsewhere.
    public_base: String,
}

fn main() {
    let config = Config {
        addr: env_or("KRATE_HUB_ADDR", "127.0.0.1:8787"),
        data_dir: PathBuf::from(env_or("KRATE_HUB_DIR", "./hub-data")),
        public_base: env_or("KRATE_HUB_PUBLIC_URL", "http://127.0.0.1:8787"),
    };

    if let Err(err) = std::fs::create_dir_all(&config.data_dir) {
        eprintln!(
            "krate-hub: cannot create data dir {}: {err}",
            config.data_dir.display()
        );
        std::process::exit(1);
    }

    let listener = match TcpListener::bind(&config.addr) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("krate-hub: cannot bind {}: {err}", config.addr);
            std::process::exit(1);
        }
    };

    eprintln!(
        "krate-hub listening on {} (data: {}, public: {})",
        config.addr,
        config.data_dir.display(),
        config.public_base
    );

    let config = Arc::new(config);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let config = Arc::clone(&config);
                std::thread::spawn(move || {
                    if let Err(err) = handle(stream, &config) {
                        // A dropped connection is normal; log at a low volume
                        // rather than crashing the server over one client.
                        eprintln!("krate-hub: connection error: {err}");
                    }
                });
            }
            Err(err) => eprintln!("krate-hub: accept error: {err}"),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// One request/response cycle. The connection is closed after (HTTP/1.0-style)
/// because the routes are one-shot and a keep-alive loop would be more code for
/// no benefit here.
fn handle(mut stream: TcpStream, config: &Config) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let (method, path, headers) = match read_request_head(&mut reader) {
        Ok(head) => head,
        Err(RequestError::TooLarge) => {
            return write_response(&mut stream, 431, "text/plain", b"request header too large");
        }
        Err(RequestError::Malformed) => {
            return write_response(&mut stream, 400, "text/plain", b"malformed request");
        }
        Err(RequestError::Io(err)) => return Err(err),
    };

    match (method.as_str(), path.as_str()) {
        ("GET", "/health") => write_response(&mut stream, 200, "text/plain", b"ok"),
        ("POST", "/publish") => handle_publish(&mut stream, &mut reader, &headers, config),
        ("GET", p) if p.starts_with("/a/") => handle_fetch(&mut stream, p, config),
        _ => write_response(&mut stream, 404, "text/plain", b"not found"),
    }
}

enum RequestError {
    TooLarge,
    Malformed,
    Io(io::Error),
}

impl From<io::Error> for RequestError {
    fn from(err: io::Error) -> Self {
        RequestError::Io(err)
    }
}

/// Read the request line and headers. Returns (method, path, headers) with the
/// reader positioned at the start of the body.
fn read_request_head(reader: &mut BufReader<TcpStream>) -> Result<RequestHead, RequestError> {
    let mut line = String::new();
    let mut total = 0;

    // Request line: METHOD SP PATH SP VERSION
    if reader.read_line(&mut line)? == 0 {
        return Err(RequestError::Malformed);
    }
    total += line.len();
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or(RequestError::Malformed)?.to_string();
    let path = parts.next().ok_or(RequestError::Malformed)?.to_string();

    let mut headers = Vec::new();
    loop {
        let mut header_line = String::new();
        if reader.read_line(&mut header_line)? == 0 {
            return Err(RequestError::Malformed);
        }
        total += header_line.len();
        if total > MAX_HEADER_BYTES {
            return Err(RequestError::TooLarge);
        }
        let trimmed = header_line.trim_end();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    Ok((method, path, headers))
}

/// Store an uploaded bundle and return its URL.
fn handle_publish(
    stream: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    headers: &[(String, String)],
    config: &Config,
) -> io::Result<()> {
    let declared_len = header(headers, "content-length").and_then(|v| v.parse::<usize>().ok());

    // Refuse an oversize upload from the declared length before reading a byte
    // of it, so a huge Content-Length cannot make us buffer megabytes to then
    // reject them.
    if let Some(len) = declared_len {
        if len > MAX_UPLOAD_BYTES {
            return write_response(stream, 413, "text/plain", b"bundle too large (5 MiB max)");
        }
    }

    let mut body = Vec::new();
    // Read exactly Content-Length when given; otherwise read until EOF, capped.
    // Cap at MAX+1 so we can tell "exactly at the limit" from "over it".
    let cap = (MAX_UPLOAD_BYTES + 1) as u64;
    match declared_len {
        Some(len) => {
            reader.take(len as u64).read_to_end(&mut body)?;
        }
        None => {
            reader.take(cap).read_to_end(&mut body)?;
        }
    }

    if body.len() > MAX_UPLOAD_BYTES {
        return write_response(stream, 413, "text/plain", b"bundle too large (5 MiB max)");
    }
    if body.is_empty() {
        return write_response(stream, 400, "text/plain", b"empty body");
    }

    // It must actually be a .krate: a zip carrying manifest.toml + code.wasm.
    // Refusing here keeps the store from filling with things `krate run` will
    // only reject later, and it is the one bit of validation worth doing.
    if let Err(reason) = looks_like_krate(&body) {
        let msg = format!("not a valid .krate bundle: {reason}");
        return write_response(stream, 422, "text/plain", msg.as_bytes());
    }

    let hash = sha256_hex(&body);
    let stored = config.data_dir.join(&hash);

    // Content-addressed: if this exact bundle is already here, this is a no-op
    // and the same URL comes back. Write to a temp file then rename so a reader
    // never sees a half-written bundle.
    if !stored.exists() {
        let tmp = config.data_dir.join(format!(".{hash}.tmp"));
        std::fs::write(&tmp, &body)?;
        std::fs::rename(&tmp, &stored)?;
    }

    let url = format!("{}/a/{hash}", config.public_base.trim_end_matches('/'));
    let json = format!("{{\"url\":\"{url}\",\"id\":\"{hash}\"}}");
    write_response(stream, 200, "application/json", json.as_bytes())
}

/// Return a stored bundle by its hash.
fn handle_fetch(stream: &mut TcpStream, path: &str, config: &Config) -> io::Result<()> {
    let hash = &path["/a/".len()..];

    // The hash is the filename, so it must be a bare hex string. Rejecting
    // anything else closes off `../` and every other path-traversal shape
    // before it can touch the filesystem.
    if !is_hex_hash(hash) {
        return write_response(stream, 400, "text/plain", b"bad id");
    }

    let stored = config.data_dir.join(hash);
    match std::fs::read(&stored) {
        Ok(bytes) => write_response(stream, 200, "application/octet-stream", &bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            write_response(stream, 404, "text/plain", b"not found")
        }
        Err(err) => Err(err),
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn is_hex_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Cheap structural check that the bytes are a `.krate`: a readable zip that
/// contains both `manifest.toml` and `code.wasm`. Not a full validation -- the
/// runtime does that at run time -- just enough to refuse obvious non-bundles.
fn looks_like_krate(bytes: &[u8]) -> Result<(), String> {
    let cursor = io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|_| "not a readable zip archive".to_string())?;

    let mut has_manifest = false;
    let mut has_component = false;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|err| format!("corrupt zip entry: {err}"))?;
        match entry.name() {
            "manifest.toml" => has_manifest = true,
            "code.wasm" => has_component = true,
            _ => {}
        }
    }

    if !has_manifest {
        return Err("missing manifest.toml".to_string());
    }
    if !has_component {
        return Err("missing code.wasm".to_string());
    }
    Ok(())
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid `.krate` in memory: a zip with the two required
    /// entries. Enough to exercise `looks_like_krate` without a real component.
    fn make_krate(manifest: bool, component: bool) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            if manifest {
                zip.start_file("manifest.toml", opts).unwrap();
                zip.write_all(b"[app]\n").unwrap();
            }
            if component {
                zip.start_file("code.wasm", opts).unwrap();
                zip.write_all(b"\0asm\x01\0\0\0").unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn accepts_a_well_formed_krate() {
        assert!(looks_like_krate(&make_krate(true, true)).is_ok());
    }

    #[test]
    fn rejects_missing_manifest() {
        assert!(looks_like_krate(&make_krate(false, true)).is_err());
    }

    #[test]
    fn rejects_missing_component() {
        assert!(looks_like_krate(&make_krate(true, false)).is_err());
    }

    #[test]
    fn rejects_non_zip() {
        assert!(looks_like_krate(b"not a zip at all").is_err());
    }

    #[test]
    fn hash_is_stable_and_hex() {
        let a = sha256_hex(b"hello");
        let b = sha256_hex(b"hello");
        assert_eq!(a, b);
        assert!(is_hex_hash(&a));
    }

    #[test]
    fn rejects_traversal_ids() {
        assert!(!is_hex_hash("../etc/passwd"));
        assert!(!is_hex_hash("abc"));
        assert!(!is_hex_hash(&"z".repeat(64)));
    }
}
