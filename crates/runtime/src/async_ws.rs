//! Live WebSocket connections that run while the app keeps drawing.
//!
//! The same shape as `async_fetch`, because it holds the same three
//! properties for the same reasons:
//!
//! - **The capability check happens before this module is reached.** The
//!   host implementation checks the URL's host and port against the
//!   `net.connect` grant on the calling thread, then hands `open` a URL it
//!   has already judged. No thread exists for an ungranted host.
//! - **Only plain data crosses the thread boundary.** The worker owns the
//!   socket; the guest side holds two channels and a flag.
//! - **A connection cannot outlive its handle quietly.** `close` asks the
//!   worker to finish; dropping the table drops the channels, and the worker
//!   notices the hangup on its next tick and exits.
//!
//! Every guest-facing call returns immediately. The worker thread reads with
//! a short socket timeout, so each tick it: forwards a received message,
//! sends anything the guest queued, and notices a requested or remote close.

use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

/// The most connections one app may hold open at once. Each one is an OS
/// thread for as long as it lives, and the thread ceiling is a process-wide
/// crash, not an error (K-137) -- so the cap answers with a refusal the app
/// can handle instead.
const MAX_WS_CONNECTIONS: usize = 16;

/// The largest message either direction, matching the HTTP body cap. A
/// server that streams something bigger fails the connection loudly rather
/// than growing the guest's memory quietly.
pub(crate) const MAX_WS_MESSAGE_BYTES: usize = 1024 * 1024;

/// One message, either direction. Mirrors the WIT `ws-message`.
#[derive(Debug, Clone)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
}

/// What `poll` found. Mirrors the WIT `ws-event`.
#[derive(Debug)]
pub enum WsEvent {
    Pending,
    Opened,
    Message(WsMessage),
    Closed,
    Failed(String),
    UnknownHandle,
}

/// What the worker reports up.
enum Report {
    Opened,
    Message(WsMessage),
    Closed,
    Failed(String),
}

/// What the guest asks the worker to do.
enum Command {
    Send(WsMessage),
    Close,
}

struct Connection {
    reports: Receiver<Report>,
    commands: Sender<Command>,
    /// A terminal report has been delivered; the handle is retired on the
    /// next poll.
    finished: bool,
}

/// The table of live connections, owned by the host.
#[derive(Default)]
pub struct AsyncWs {
    next_handle: u64,
    connections: BTreeMap<u64, Connection>,
}

impl AsyncWs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a connection to a URL the caller has already permission-checked.
    ///
    /// Returns a handle immediately; the handshake happens on the worker and
    /// reports back as `Opened` or `Failed` through `poll`.
    pub fn open(&mut self, url: String) -> Result<u64, String> {
        if self.connections.len() >= MAX_WS_CONNECTIONS {
            return Err(format!(
                "too many live connections: this app already holds {MAX_WS_CONNECTIONS}"
            ));
        }
        let (report_tx, report_rx) = channel::<Report>();
        let (command_tx, command_rx) = channel::<Command>();
        std::thread::spawn(move || run_connection(url, report_tx, command_rx));
        let handle = self.next_handle;
        self.next_handle += 1;
        self.connections.insert(
            handle,
            Connection {
                reports: report_rx,
                commands: command_tx,
                finished: false,
            },
        );
        Ok(handle)
    }

    /// Queue one message. Returns immediately.
    pub fn send(&mut self, handle: u64, message: WsMessage) -> Result<(), String> {
        let size = match &message {
            WsMessage::Text(text) => text.len(),
            WsMessage::Binary(bytes) => bytes.len(),
        };
        if size > MAX_WS_MESSAGE_BYTES {
            return Err(format!(
                "message is {size} bytes; the limit is {MAX_WS_MESSAGE_BYTES}"
            ));
        }
        let Some(connection) = self.connections.get(&handle) else {
            return Err("unknown connection".to_string());
        };
        connection
            .commands
            .send(Command::Send(message))
            .map_err(|_| "the connection is closed".to_string())
    }

    /// The next event, or `Pending`. A terminal event retires the handle.
    pub fn poll(&mut self, handle: u64) -> WsEvent {
        let Some(connection) = self.connections.get_mut(&handle) else {
            return WsEvent::UnknownHandle;
        };
        if connection.finished {
            self.connections.remove(&handle);
            return WsEvent::UnknownHandle;
        }
        match connection.reports.try_recv() {
            Ok(Report::Opened) => WsEvent::Opened,
            Ok(Report::Message(message)) => WsEvent::Message(message),
            Ok(Report::Closed) => {
                connection.finished = true;
                WsEvent::Closed
            }
            Ok(Report::Failed(reason)) => {
                connection.finished = true;
                WsEvent::Failed(reason)
            }
            Err(TryRecvError::Empty) => WsEvent::Pending,
            // The worker is gone without a terminal report -- treat it as a
            // failure rather than pending forever.
            Err(TryRecvError::Disconnected) => {
                connection.finished = true;
                WsEvent::Failed("the connection ended unexpectedly".to_string())
            }
        }
    }

    /// Ask the worker to close. The final `Closed` still arrives via `poll`.
    pub fn close(&mut self, handle: u64) {
        if let Some(connection) = self.connections.get(&handle) {
            let _ = connection.commands.send(Command::Close);
        }
    }
}

/// The worker: owns the socket for the connection's whole life.
fn run_connection(url: String, reports: Sender<Report>, commands: Receiver<Command>) {
    use tungstenite::client::connect;
    use tungstenite::protocol::Message;
    use tungstenite::stream::MaybeTlsStream;

    let (mut socket, _response) = match connect(&url) {
        Ok(pair) => pair,
        Err(err) => {
            let _ = reports.send(Report::Failed(format!("could not connect: {err}")));
            return;
        }
    };
    // A short read timeout turns the blocking read into a tick, so queued
    // sends and close requests are handled within ~50ms even when the server
    // is silent.
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => Some(stream),
        MaybeTlsStream::Rustls(tls) => Some(tls.get_mut()),
        _ => None,
    };
    if let Some(stream) = stream {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));
    }
    if reports.send(Report::Opened).is_err() {
        return;
    }

    let mut closing = false;
    loop {
        // Everything the guest queued since the last tick.
        loop {
            match commands.try_recv() {
                Ok(Command::Send(message)) => {
                    let frame = match message {
                        WsMessage::Text(text) => Message::text(text),
                        WsMessage::Binary(bytes) => Message::binary(bytes),
                    };
                    if let Err(err) = socket.send(frame) {
                        let _ = reports.send(Report::Failed(format!("send failed: {err}")));
                        return;
                    }
                }
                Ok(Command::Close) => {
                    let _ = socket.close(None);
                    closing = true;
                }
                Err(TryRecvError::Empty) => break,
                // The table was dropped: nobody is listening. Close and go.
                Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return;
                }
            }
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                if text.len() > MAX_WS_MESSAGE_BYTES {
                    let _ = reports.send(Report::Failed("message too large".to_string()));
                    return;
                }
                if reports
                    .send(Report::Message(WsMessage::Text(text.to_string())))
                    .is_err()
                {
                    return;
                }
            }
            Ok(Message::Binary(bytes)) => {
                if bytes.len() > MAX_WS_MESSAGE_BYTES {
                    let _ = reports.send(Report::Failed("message too large".to_string()));
                    return;
                }
                if reports
                    .send(Report::Message(WsMessage::Binary(bytes.to_vec())))
                    .is_err()
                {
                    return;
                }
            }
            // Ping/pong are answered by tungstenite itself on the next
            // read/write; frames other than data need nothing from us.
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) => {
                let _ = reports.send(Report::Closed);
                return;
            }
            Err(tungstenite::Error::Io(err))
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                // A quiet tick. If we asked to close and the server never
                // answers, the close still completes from our side.
                if closing {
                    let _ = reports.send(Report::Closed);
                    return;
                }
            }
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                let _ = reports.send(Report::Closed);
                return;
            }
            Err(err) => {
                // After we asked to close, servers routinely drop the TCP
                // without finishing the close handshake. That is a close the
                // app requested, not a failure to report.
                if closing {
                    let _ = reports.send(Report::Closed);
                } else {
                    let _ = reports.send(Report::Failed(format!("connection error: {err}")));
                }
                return;
            }
        }
    }
}

/// Parse a `ws://` or `wss://` URL into the host and port the capability
/// check needs. Refuses other schemes, so an `http://` URL cannot slip into
/// a socket open.
pub fn ws_url_endpoint(url: &str) -> Result<(String, u16), String> {
    let (rest, default_port) = if let Some(rest) = url.strip_prefix("wss://") {
        (rest, 443)
    } else if let Some(rest) = url.strip_prefix("ws://") {
        (rest, 80)
    } else {
        return Err("the URL must start with ws:// or wss://".to_string());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let endpoint = krate_adapter_common::net::parse_url_endpoint_with_default(rest, default_port)
        .map_err(|_| "the URL's host could not be read".to_string())?;
    if authority.is_empty() {
        return Err("the URL names no host".to_string());
    }
    Ok((endpoint.host, endpoint.port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_urls_map_to_the_hosts_the_wall_checks() {
        assert_eq!(
            ws_url_endpoint("wss://example.com/socket").unwrap(),
            ("example.com".to_string(), 443)
        );
        assert_eq!(
            ws_url_endpoint("ws://localhost:9001").unwrap(),
            ("localhost".to_string(), 9001)
        );
        assert!(ws_url_endpoint("https://example.com").is_err());
    }

    #[test]
    fn an_unknown_handle_says_so() {
        let mut table = AsyncWs::new();
        assert!(matches!(table.poll(7), WsEvent::UnknownHandle));
        assert!(table.send(7, WsMessage::Text("x".into())).is_err());
    }

    #[test]
    fn the_connection_cap_refuses_rather_than_growing() {
        let mut table = AsyncWs::new();
        for _ in 0..MAX_WS_CONNECTIONS {
            // The URL never resolves; the worker fails on its own thread.
            table.open("ws://localhost:1".to_string()).unwrap();
        }
        assert!(table.open("ws://localhost:1".to_string()).is_err());
    }

    /// Opt-in: proves wss:// against the production relay -- real TLS,
    /// real internet, the actual room two players would meet in. Run with
    /// KRATE_WS_LIVE_TEST=1.
    #[test]
    fn a_wss_round_trip_through_the_production_relay() {
        if std::env::var("KRATE_WS_LIVE_TEST").as_deref() != Ok("1") {
            return;
        }
        let mut table = AsyncWs::new();
        let a = table
            .open("wss://hub.krate.tech/play/krateselftest".to_string())
            .unwrap();
        let b = table
            .open("wss://hub.krate.tech/play/krateselftest".to_string())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut b_open = false;
        let mut relayed = None;
        while std::time::Instant::now() < deadline && relayed.is_none() {
            match table.poll(a) {
                WsEvent::Opened => {}
                WsEvent::Message(WsMessage::Text(text)) if text.contains("peer-joined") => {
                    table
                        .send(a, WsMessage::Text("{\"t\":\"hello\"}".to_string()))
                        .unwrap();
                }
                WsEvent::Failed(reason) => panic!("a failed: {reason}"),
                _ => {}
            }
            match table.poll(b) {
                WsEvent::Opened => b_open = true,
                WsEvent::Message(WsMessage::Text(text)) if text.contains("hello") => {
                    relayed = Some(text);
                }
                WsEvent::Failed(reason) => panic!("b failed: {reason}"),
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(b_open, "second connection never opened");
        assert!(relayed.is_some(), "the relay never delivered the message");
        table.close(a);
        table.close(b);
    }

    #[test]
    fn a_live_round_trip_against_a_local_server() {
        use std::net::TcpListener;
        // A one-connection echo server on an OS-chosen port.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            loop {
                match socket.read() {
                    Ok(message) if message.is_text() || message.is_binary() => {
                        if socket.send(message).is_err() {
                            break;
                        }
                    }
                    Ok(tungstenite::protocol::Message::Close(_)) => {
                        let _ = socket.close(None);
                        let _ = socket.flush();
                        break;
                    }
                    Err(_) => break,
                    Ok(_) => {}
                }
            }
        });

        let mut table = AsyncWs::new();
        let handle = table.open(format!("ws://127.0.0.1:{port}")).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut opened = false;
        let mut echoed = None;
        while std::time::Instant::now() < deadline {
            match table.poll(handle) {
                WsEvent::Opened => {
                    opened = true;
                    table
                        .send(handle, WsMessage::Text("hello".to_string()))
                        .unwrap();
                }
                WsEvent::Message(WsMessage::Text(text)) => {
                    echoed = Some(text);
                    break;
                }
                WsEvent::Pending => std::thread::sleep(std::time::Duration::from_millis(10)),
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(opened, "the connection never opened");
        assert_eq!(echoed.as_deref(), Some("hello"));

        table.close(handle);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match table.poll(handle) {
                WsEvent::Closed => break,
                WsEvent::Pending if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                other => panic!("expected Closed, got {other:?}"),
            }
        }
    }
}
