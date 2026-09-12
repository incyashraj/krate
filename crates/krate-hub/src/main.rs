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
use std::time::Duration;

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

/// How long a connection may produce NOTHING before it is dropped (K-279).
///
/// Without it, a client that connects and stalls -- one byte then silence,
/// or a partial header and nothing more -- held a thread open forever, and
/// enough of them exhaust the server for everybody. This bounds SILENCE, not
/// total transfer: it is the socket read timeout, so every chunk that
/// arrives resets it. A real 5 MiB upload over a poor link keeps it alive;
/// a stalled one is released. Same reasoning and roughly the same number as
/// the download side's FETCH_SILENCE_TIMEOUT (K-275), so the two read
/// together.
const CONNECTION_SILENCE_TIMEOUT: Duration = Duration::from_secs(30);

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
                    if let Err(err) = handle(stream, &config, CONNECTION_SILENCE_TIMEOUT) {
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
fn handle(mut stream: TcpStream, config: &Config, silence: Duration) -> io::Result<()> {
    // Drop a connection that goes silent, before a byte of it is read
    // (K-279). A read past the timeout returns WouldBlock/TimedOut, which
    // surfaces as the ordinary connection error the accept loop already
    // logs and moves on from. Passed in rather than read from the constant
    // so a test can use a short budget instead of the real 30 seconds.
    stream.set_read_timeout(Some(silence))?;

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
        ("GET", "/apps") => handle_list(&mut stream, config),
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

    // Metadata travels in headers rather than a multipart body: the body is
    // the bundle, and a publisher without any of these still gets a working
    // upload rather than a rejection.
    let meta_field = |name: &str, fallback: &str| {
        header(headers, name)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback)
            .to_string()
    };
    let meta = Metadata {
        name: meta_field("x-krate-name", "Untitled app"),
        description: meta_field("x-krate-description", ""),
        author: meta_field("x-krate-author", "anonymous"),
        author_url: meta_field("x-krate-author-url", ""),
        published: now_iso8601(),
        size: body.len(),
    };
    let meta_path = config.data_dir.join(format!("{hash}.json"));
    if !meta_path.exists() {
        let _ = std::fs::write(&meta_path, meta.to_json());
    }

    let json = publish_response(&config.public_base, &hash, &body);
    write_response(stream, 200, "application/json", json.as_bytes())
}

/// What a publisher gets back, with each number named for what it is
/// (IC-212). `id` is the STORE KEY: a plain sha256 over the uploaded bytes,
/// which is also the path under /a/. `archive` is the archive identity the
/// client computes for the same bytes -- the value `krate run --json` and
/// the trust screen show -- so a person can match the file in their hand
/// to the one at the URL without knowing that the two hashes differ by a
/// schema tag. Neither is the execution identity: what runs is a third
/// number, and the hub does not pretend to know it.
fn publish_response(public_base: &str, hash: &str, body: &[u8]) -> String {
    let url = format!("{}/a/{hash}", public_base.trim_end_matches('/'));
    let archive = krate_bundle::provenance::digest_archive_bytes(body).digest;
    format!("{{\"url\":\"{url}\",\"id\":\"{hash}\",\"archive\":\"{archive}\"}}")
}

/// What a published app carries beyond its bytes.
struct Metadata {
    name: String,
    description: String,
    author: String,
    author_url: String,
    published: String,
    size: usize,
}

impl Metadata {
    fn to_json(&self) -> String {
        format!(
            "{{\"name\":{},\"description\":{},\"author\":{},\"author_url\":{},\"published\":{},\"size\":{}}}",
            json_string(&self.name),
            json_string(&self.description),
            json_string(&self.author),
            json_string(&self.author_url),
            json_string(&self.published),
            self.size
        )
    }
}

/// Escape a string for JSON. Small enough to do by hand, and it keeps the hub
/// free of a serialisation dependency it otherwise does not need.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Seconds since the epoch, as a date. Good enough to sort and show.
fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Every published app, newest first, as JSON.
///
/// This is what makes a cloud page possible: without it an app can only be
/// reached by someone who already has its exact hash.
fn handle_list(stream: &mut TcpStream, config: &Config) -> io::Result<()> {
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(dir) = std::fs::read_dir(&config.data_dir) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(hash) = name.strip_suffix(".json") else {
                continue;
            };
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                entries.push((hash.to_string(), text));
            }
        }
    }
    // Newest first: the published timestamp is inside the JSON, and sorting by
    // it here saves every reader from doing so.
    entries.sort_by(|a, b| b.1.cmp(&a.1));

    let items: Vec<String> = entries
        .iter()
        .map(|(hash, meta)| {
            let url = format!("{}/a/{hash}", config.public_base.trim_end_matches('/'));
            format!(
                "{{\"id\":{},\"url\":{},\"meta\":{meta}}}",
                json_string(hash),
                json_string(&url)
            )
        })
        .collect();

    let json = format!("{{\"apps\":[{}]}}", items.join(","));
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

/// Full admission validation: the same open the recipient will run (IC-833).
///
/// This was once a cheap structural check -- readable zip, two names -- and
/// the comment still said "not a full validation". It is now exactly a full
/// validation: `open_bytes` is what `krate run` runs, so the hub cannot hand
/// out a URL to a bundle a client would refuse (K-278). It returns Err on
/// every malformed input rather than panicking, which is what keeps a bad
/// upload from taking down the connection thread (K-280).
fn looks_like_krate(bytes: &[u8]) -> Result<(), String> {
    // Admission is the same open the recipient will run (IC-833). This used
    // to be a two-name scan -- "accepts any PK 03 04 byte string containing
    // two filename strings", as the register's audit put it -- so every
    // refusal the client makes (duplicate paths, names outside ASCII, paths
    // too deep to unpack, forged sizes, a damaged format line) was absent
    // exactly where a curl could reach past the client (K-278). One shared
    // validator means the hub can never admit what `krate run` refuses.
    let opened = krate_bundle::open_bytes(bytes)
        .map_err(|err| err.user_message().unwrap_or_else(|| err.to_string()))?;

    // And the component itself, to the same bar `krate pack` holds: a real
    // component header, and imports that parse. A core module or a text file
    // named code.wasm gets its URL refused here rather than its recipients
    // getting exit 2 later (the K-272 shape, server-side).
    let component = std::fs::read(opened.component_path())
        .map_err(|err| format!("could not read the component back: {err}"))?;
    if !krate_bundle::imports::is_component(&component) {
        return Err("code.wasm is a core WebAssembly module, not a component. \
             Build it with `cargo component build` rather than `cargo build`."
            .to_string());
    }
    krate_bundle::imports::component_imports(&component)
        .map(|_| ())
        .map_err(|detail| format!("code.wasm is not a readable component: {detail}"))
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
    /// A real manifest and a real COMPONENT header.
    ///
    /// The old fixture wrote `[app]` alone and the module header
    /// `\0asm\x01\0\0\0` -- fine for the two-name scan this file used to
    /// do, and exactly the K-272 trap once admission became the shared
    /// validator: a module is not a component, and an empty [app] table is
    /// not a manifest. A fixture that is not the thing it claims to be
    /// tests something other than what it says.
    const MANIFEST: &str = "[app]\nid = \"com.example.hub\"\nname = \"Hub\"\n\
                            version = \"1.0.0\"\nentry = \"code.wasm\"\n\
                            world = \"krate:app/cli@0.1.0\"\n";
    // A real component with a `run` export (IC-210): admission runs the
    // same validator as open, and a bare header is a component that could
    // never run, which it now rightly refuses.
    const MINIMAL_COMPONENT: &[u8] = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");

    fn krate_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            for (name, bytes) in entries {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    fn make_krate(manifest: bool, component: bool) -> Vec<u8> {
        let mut entries: Vec<(&str, &[u8])> = Vec::new();
        if manifest {
            entries.push(("manifest.toml", MANIFEST.as_bytes()));
        }
        if component {
            entries.push(("code.wasm", MINIMAL_COMPONENT));
        }
        krate_of(&entries)
    }

    #[test]
    fn accepts_a_well_formed_krate() {
        assert!(looks_like_krate(&make_krate(true, true)).is_ok());
    }

    /// Admission is the same open the recipient will run (K-278 / IC-833).
    ///
    /// Each of these is refused by `krate run` and by `krate publish`, and
    /// each was admitted here with an HTTP 200 -- measured against a live
    /// hub before the shared validator went in. A curl straight at /publish
    /// skips the client, so the client's discipline has to live here too.
    /// A client that stalls mid-request is dropped, not held forever (K-279).
    ///
    /// The hub gives every connection its own thread, so a stalled one that
    /// is never released is a thread leaked, and enough of them stop the
    /// server for everybody. `handle` takes its silence budget as a
    /// parameter so this can use a short one instead of the real 30s: a
    /// client connects, sends a partial request, and goes quiet; the read
    /// must fail within the budget rather than blocking on the socket.
    #[test]
    fn a_stalled_connection_is_dropped_within_the_silence_budget() {
        use std::io::Write as _;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let config = Config {
            addr: addr.to_string(),
            data_dir: std::env::temp_dir().join(format!("krate-hub-test-{}", std::process::id())),
            public_base: format!("http://{addr}"),
        };
        std::fs::create_dir_all(&config.data_dir).ok();

        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let started = std::time::Instant::now();
            // A short budget so the test is quick; the production caller
            // passes CONNECTION_SILENCE_TIMEOUT.
            let result = handle(stream, &config, Duration::from_millis(400));
            (started.elapsed(), result)
        });

        // Connect, send a partial head promising a body, then go silent.
        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .write_all(b"POST /publish HTTP/1.0\r\nContent-Length: 100\r\n\r\nten bytes.")
            .expect("write partial");

        let (elapsed, result) = server.join().expect("server thread");
        assert!(
            elapsed < Duration::from_secs(5),
            "the hub must give up on a silent client near its budget, not \
             block on the socket: waited {elapsed:?}"
        );
        assert!(
            result.is_err(),
            "a stalled read must surface as a connection error the accept \
             loop logs and moves past, not a success",
        );
        // Keep the client alive until here so the OS does not close the
        // socket for us and mask what is being tested.
        drop(client);
    }

    /// Malformed archives are refused, never crash the validator (K-280).
    ///
    /// Admission now runs the full opener on untrusted bytes -- it parses a
    /// zip, reads an end-of-central-directory count, writes to a temp dir.
    /// A panic there would be caught by the per-connection thread, but a
    /// validator that aborts on a hostile input still costs the upload and
    /// can leave state behind. IC-833 names "validator crash" as a case to
    /// prove safe, so this feeds it the shapes that break naive zip readers
    /// and requires every one to come back as a plain Err.
    #[test]
    fn a_malformed_archive_is_refused_not_a_crash() {
        let cases: [(&str, &[u8]); 6] = [
            ("one byte", b"P"),
            ("zip magic only", b"PK\x03\x04"),
            (
                "truncated EOCD",
                b"PK\x05\x06\x00\x00\x00\x00\x00\x00\x00\x00",
            ),
            // EOCD claiming 65535 records, none present.
            (
                "huge declared count",
                b"PK\x05\x06\x00\x00\x00\x00\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00",
            ),
            (
                "zip64 marker of 0xff",
                b"PK\x06\x06\xff\xff\xff\xff\xff\xff\xff\xff",
            ),
            // Many local-file signatures, no directory: a reader that trusts
            // them loops or over-allocates.
            (
                "repeated local headers",
                &[0x50, 0x4b, 0x03, 0x04].repeat(500),
            ),
        ];
        for (what, bytes) in cases {
            // The contract is total: Err, never a panic. A panic here fails
            // the test loudly rather than being swallowed by a thread.
            let result = looks_like_krate(bytes);
            assert!(
                result.is_err(),
                "{what} must be refused, and it must be refused as an Err \
                 rather than crash the validator",
            );
        }
    }

    #[test]
    fn refuses_what_the_client_refuses() {
        // Names one file twice: what a reviewer reads is not what runs.
        //
        // The zip WRITER refuses to produce this (the K-713 lesson: our own
        // tools cannot build the archive the rule is about), so it is a
        // committed fixture assembled by Python's zipfile with one name
        // byte-patched to collide -- exactly what a hostile publisher does.
        // The opener catches it via the record-count path, which is the one
        // that fires for a real writer's duplicate.
        const DUPLICATE: &[u8] = include_bytes!("../tests/fixtures/duplicate-source-path.krate");
        let refusal = looks_like_krate(DUPLICATE).expect_err("a duplicate must be refused");
        assert!(
            refusal.contains("names the same file twice"),
            "and refused AS a duplicate: {refusal}"
        );

        // A 200-directory path: unpacks nowhere portable.
        let deep_name = format!(
            "source/{}/lib.rs",
            (0..200)
                .map(|i| format!("d{i}"))
                .collect::<Vec<_>>()
                .join("/")
        );
        let deep = krate_of(&[
            ("manifest.toml", MANIFEST.as_bytes()),
            ("code.wasm", MINIMAL_COMPONENT),
            (deep_name.as_str(), b"x"),
        ]);
        assert!(
            looks_like_krate(&deep).is_err(),
            "a path too deep to unpack everywhere must be refused"
        );

        // A damaged format line.
        let damaged = krate_of(&[
            ("krate-profile", b"banana"),
            ("manifest.toml", MANIFEST.as_bytes()),
            ("code.wasm", MINIMAL_COMPONENT),
        ]);
        assert!(
            looks_like_krate(&damaged).is_err(),
            "a damaged format line must be refused"
        );

        // A core module where the component belongs: the K-272 shape,
        // server-side. The recipient would get exit 2; the publisher gets
        // told the build command instead.
        let module = krate_of(&[
            ("manifest.toml", MANIFEST.as_bytes()),
            (
                "code.wasm",
                &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
            ),
        ]);
        let refusal = looks_like_krate(&module).expect_err("a module must be refused");
        assert!(
            refusal.contains("cargo component build"),
            "and told the command that fixes it: {refusal}"
        );

        // A component that instantiates but is not a Krate app: it exports
        // more than `run`. Nothing before the validator objects to it, so
        // this is the case that proves admission runs the validator and not
        // merely the header check (IC-210).
        let chatty = krate_of(&[
            ("manifest.toml", MANIFEST.as_bytes()),
            (
                "code.wasm",
                include_bytes!("../../bundle/tests/fixtures/extra-export.wasm"),
            ),
        ]);
        let refusal = looks_like_krate(&chatty).expect_err("an extra export must be refused");
        assert!(
            refusal.contains("exports more than `run`") && refusal.contains("debug-hook"),
            "and refused in the validator's words: {refusal}"
        );

        // A compression bomb, and specifically a FORGED one: its source
        // entries declare ~1 byte and each hold 32 MiB, 384 MiB expanded,
        // ~393 KB on the wire. IC-833 names the bomb. The declared-size
        // preflight would catch an HONEST oversize bundle earlier; this
        // committed fixture declares small, so it exercises the guard that
        // counts bytes actually WRITTEN (K-255). Refused before it can fill
        // the store -- verified against a live hub at 393 KB in, 0 written.
        const BOMB: &[u8] = include_bytes!("../tests/fixtures/forged-size-bomb.krate");
        assert!(
            BOMB.len() < 5 * 1024 * 1024,
            "the bomb must be under the upload cap, or it is refused for its \
             wire size rather than its expansion: {} bytes",
            BOMB.len(),
        );
        assert!(
            looks_like_krate(BOMB).is_err(),
            "a bundle whose source expands past the limit must be refused, \
             however small it is on the wire",
        );
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

    /// The hub names the same archive identity the client computes, beside
    /// its own store key, and identical bytes are one artifact: same key,
    /// same URL, same identity, however many times or from wherever they
    /// arrive (IC-212: "identical bytes at two URLs" cannot happen here,
    /// because the URL IS the bytes).
    #[test]
    fn the_hub_names_the_archive_identity_the_client_computes() {
        let body = make_krate(true, true);
        let hash = sha256_hex(&body);
        let first = publish_response("https://hub.example", &hash, &body);
        let again = publish_response("https://hub.example/", &sha256_hex(&body), &body);
        assert_eq!(
            first, again,
            "the same bytes get the same answer every time"
        );
        let expected = krate_bundle::provenance::digest_archive_bytes(&body).digest;
        assert!(
            first.contains(&format!("\"archive\":\"{expected}\"")),
            "the archive identity must be the client's number: {first}"
        );
        assert!(
            first.contains(&format!("\"id\":\"{hash}\""))
                && first.contains(&format!("/a/{hash}\"")),
            "the store key and the URL are the raw sha256: {first}"
        );
        assert_ne!(
            hash, expected,
            "store key and archive identity are two numbers, and both are named"
        );
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
