//! A byte link a machine protocol runs over (IC-994): the serial port the
//! person picked, or a TCP connection to a host the manifest declares.
//!
//! [`krate::modbus`](crate::modbus) and [`krate::mqtt`](crate::mqtt) talk
//! through a [`Link`], so they need no grant of their own: `device.serial`
//! (the person's pick) or `net.connect:<host>:<port>` is the whole
//! permission. An app rarely touches this module; it is here for a link of
//! its own (a fake device in a test, a radio modem) and for the two links
//! Krate gives: [`SerialLink`] and [`TcpLink`].

use alloc::{format, string::String, vec::Vec};

/// Bytes in and out, with a clock. Everything a protocol needs.
pub trait Link {
    /// Send every byte, or say why not.
    fn send(&mut self, bytes: &[u8]) -> Result<(), LinkError>;

    /// Wait up to `wait_ms` for bytes to arrive, then answer with what came
    /// (at most `max`, at least one). Empty when the wait ran out with
    /// nothing; `wait_ms == 0` answers at once.
    fn receive(&mut self, max: usize, wait_ms: u32) -> Result<Vec<u8>, LinkError>;

    /// Milliseconds on a clock that never goes back.
    fn now_ms(&mut self) -> u64;

    /// Throw away whatever has already arrived and was not asked for: the
    /// rest of a late answer, line noise. Bounded, so a chattering line
    /// cannot hold the app here.
    fn discard_input(&mut self) -> Result<(), LinkError> {
        for _ in 0..16 {
            if self.receive(256, 0)?.is_empty() {
                break;
            }
        }
        Ok(())
    }
}

/// Why a link could not carry bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The other end closed the connection.
    Closed,
    /// The link failed, with words to show the person.
    Failed(String),
}

impl LinkError {
    /// Words to show the person.
    pub fn message(&self) -> String {
        match self {
            LinkError::Closed => String::from("The other end closed the connection."),
            LinkError::Failed(words) => words.clone(),
        }
    }
}

impl core::fmt::Display for LinkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message())
    }
}

fn monotonic_ms() -> u64 {
    crate::time::monotonic_nanos() / 1_000_000
}

/// The serial port the person picked, as a [`Link`]. Open it with the
/// settings the device expects before handing it over.
#[derive(Debug)]
pub struct SerialLink {
    port: crate::serial::Port,
}

impl SerialLink {
    /// Talk through `port` (already opened with its settings).
    pub fn new(port: crate::serial::Port) -> Self {
        SerialLink { port }
    }

    /// The port, to show its name or change its settings.
    pub fn port(&self) -> &crate::serial::Port {
        &self.port
    }

    /// Give the port back.
    pub fn into_port(self) -> crate::serial::Port {
        self.port
    }
}

impl Link for SerialLink {
    fn send(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        for chunk in bytes.chunks(65_536) {
            self.port
                .write(chunk)
                .map_err(|e| LinkError::Failed(String::from(crate::serial::words(&e))))?;
        }
        Ok(())
    }

    fn receive(&mut self, max: usize, wait_ms: u32) -> Result<Vec<u8>, LinkError> {
        let max = max.clamp(1, 65_536) as u32;
        self.port
            .read(max, Some(wait_ms.min(60_000)))
            .map_err(|e| LinkError::Failed(String::from(crate::serial::words(&e))))
    }

    fn now_ms(&mut self) -> u64 {
        monotonic_ms()
    }
}

/// A TCP connection to a host the manifest declares
/// (`net.connect:<host>:<port>`), as a [`Link`]. Closed when dropped.
///
/// Plain TCP only: the sockets Krate gives an app do not do TLS yet.
#[derive(Debug)]
pub struct TcpLink {
    handle: u64,
    timeout_ms: u32,
    host: String,
    port: u16,
}

/// Words for a refused or failed connection.
/// Only the reason, from the host's words. The host says "could not connect
/// to 127.0.0.1:502: Connection refused (os error 61)", and `net_words`
/// already names the address, so the person read it twice (K-1310). The
/// operating system's error number means nothing to them either.
fn cause(words: &str) -> String {
    let lower = words.to_ascii_lowercase();
    let mut rest = words;
    for lead in ["could not connect to ", "could not find "] {
        if lower.starts_with(lead) {
            if let Some(at) = words.find(": ") {
                rest = &words[at + 2..];
            }
            break;
        }
    }
    let rest = match rest.rfind(" (os error ") {
        Some(at) if rest.ends_with(')') => &rest[..at],
        _ => rest,
    };
    let mut out = String::from(rest.trim());
    if let Some(first) = out.get(..1) {
        let lowered = first.to_ascii_lowercase();
        out.replace_range(..1, &lowered);
    }
    out
}

pub(crate) fn net_words(err: &crate::net::NetError, host: &str, port: u16) -> String {
    use crate::net::NetError;
    match err {
        NetError::PermissionDenied => format!(
            "This app may not connect to {host}:{port}. Declare `net.connect:{host}:{port}` in its manifest."
        ),
        NetError::DnsFailure(w) => format!("{host} could not be found: {}", cause(w)),
        NetError::ConnectFailure(w) => format!("Could not connect to {host}:{port}: {}", cause(w)),
        NetError::TlsFailure(w) => format!("A secure connection to {host} failed: {w}"),
        NetError::Timeout => format!("{host}:{port} did not answer in time."),
        NetError::InvalidUrl => format!("{host}:{port} is not an address that can be reached."),
        NetError::BodyTooLarge => String::from("Too much arrived at once."),
        NetError::Protocol(w) | NetError::Other(w) => format!("{host}:{port}: {w}"),
    }
}

impl TcpLink {
    /// Connect to `host` (a name or an IP) on `port`, waiting up to
    /// `timeout_ms` for the connection to open. Sending later waits up to
    /// the same time for room to send.
    pub fn connect(host: &str, port: u16, timeout_ms: u32) -> Result<TcpLink, LinkError> {
        use crate::net::sockets::{close, tcp_connect, tcp_status, ConnectStatus};
        let handle =
            tcp_connect(host, port).map_err(|e| LinkError::Failed(net_words(&e, host, port)))?;
        let start = monotonic_ms();
        loop {
            match tcp_status(handle) {
                ConnectStatus::Connected(_) => {
                    return Ok(TcpLink {
                        handle,
                        timeout_ms,
                        host: String::from(host),
                        port,
                    })
                }
                ConnectStatus::Pending => {}
                ConnectStatus::Failed(e) => {
                    close(handle);
                    return Err(LinkError::Failed(net_words(&e, host, port)));
                }
                ConnectStatus::UnknownHandle => {
                    return Err(LinkError::Failed(format!(
                        "The connection to {host}:{port} went away."
                    )))
                }
            }
            if monotonic_ms().saturating_sub(start) >= u64::from(timeout_ms) {
                close(handle);
                return Err(LinkError::Failed(format!(
                    "{host}:{port} did not answer within {} seconds.",
                    timeout_ms.div_ceil(1000)
                )));
            }
            crate::time::sleep_millis(2);
        }
    }

    /// The handle the host knows the connection by.
    pub fn handle(&self) -> u64 {
        self.handle
    }
}

impl Link for TcpLink {
    fn send(&mut self, mut bytes: &[u8]) -> Result<(), LinkError> {
        use crate::net::sockets::tcp_write;
        let start = monotonic_ms();
        while !bytes.is_empty() {
            let taken = tcp_write(self.handle, bytes)
                .map_err(|e| LinkError::Failed(net_words(&e, &self.host, self.port)))?
                as usize;
            bytes = &bytes[taken.min(bytes.len())..];
            if bytes.is_empty() {
                break;
            }
            if monotonic_ms().saturating_sub(start) >= u64::from(self.timeout_ms) {
                return Err(LinkError::Failed(format!(
                    "{}:{} stopped taking data.",
                    self.host, self.port
                )));
            }
            crate::time::sleep_millis(1);
        }
        Ok(())
    }

    fn receive(&mut self, max: usize, wait_ms: u32) -> Result<Vec<u8>, LinkError> {
        use crate::net::sockets::{tcp_read, ReadResult};
        let max = max.clamp(1, 65_536) as u32;
        let start = monotonic_ms();
        loop {
            match tcp_read(self.handle, max) {
                ReadResult::Data(bytes) => return Ok(bytes),
                ReadResult::Pending => {}
                ReadResult::Closed => return Err(LinkError::Closed),
                ReadResult::Failed(e) => {
                    return Err(LinkError::Failed(net_words(&e, &self.host, self.port)))
                }
            }
            if monotonic_ms().saturating_sub(start) >= u64::from(wait_ms) {
                return Ok(Vec::new());
            }
            crate::time::sleep_millis(1);
        }
    }

    fn now_ms(&mut self) -> u64 {
        monotonic_ms()
    }
}

impl Drop for TcpLink {
    fn drop(&mut self) {
        crate::net::sockets::close(self.handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::NetError;

    /// The words an oven board showed when its sensor was down (K-1310):
    /// the address once, the reason once, no operating-system number.
    #[test]
    fn a_refused_connection_names_the_address_once() {
        let host_words =
            String::from("could not connect to 127.0.0.1:502: Connection refused (os error 61)");
        let said = net_words(&NetError::ConnectFailure(host_words), "127.0.0.1", 502);
        assert_eq!(
            said,
            "Could not connect to 127.0.0.1:502: connection refused"
        );
        assert_eq!(said.matches("127.0.0.1").count(), 1, "{said}");
    }

    #[test]
    fn a_name_that_cannot_be_found_is_said_once() {
        let host_words =
            String::from("could not find oven.local: failed to lookup address information");
        let said = net_words(&NetError::DnsFailure(host_words), "oven.local", 502);
        assert_eq!(
            said,
            "oven.local could not be found: failed to lookup address information"
        );
    }

    #[test]
    fn words_without_the_hosts_lead_pass_through() {
        assert_eq!(cause("Network is unreachable"), "network is unreachable");
        assert_eq!(cause("timed out (os error 60)"), "timed out");
        assert_eq!(cause(""), "");
    }
}
