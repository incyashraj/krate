//! TCP and UDP for apps that speak their own protocol (IC-935).
//!
//! The shape is `async_ws`'s, for its reasons:
//!
//! - **The grant is judged before this module is reached.** The host
//!   implementation checks every connect and every datagram's destination
//!   against `net.connect` on the calling thread; nothing here dials a host
//!   it was not handed already judged.
//! - **Every guest-facing call returns at once.** A connection lives on two
//!   host threads (one reading, one writing); a lookup on one. The guest
//!   asks how they are doing from its own loop.
//! - **A socket cannot outlive its handle.** `close` shuts the connection
//!   down; dropping the table drops every socket and its threads notice.
//!
//! An address a lookup returned stands for the name looked up: the table
//! remembers which names each address came from, so a grant for a name
//! covers its addresses (and only those).
//!
//! Listening (IC-987) lives here too. The caller has judged the port and the
//! scope; this binds exactly that port, on the loopback address for this
//! computer or every address for the network, refuses a port something
//! already listens on, and hands each device that connects over as a
//! connection the same reader and writer threads serve. Connections from
//! other devices are counted and bounded: [`MAX_PEERS`] open at once, each
//! holding at most [`PEER_QUEUE_BYTES`] in each direction before it pushes
//! back. A listener is a plain socket in the table, so closing it or
//! dropping the table (the run ending) closes the port.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The most sockets and lookups one app may hold at once. Each connection is
/// two OS threads for as long as it lives (K-137: the thread ceiling is a
/// process-wide crash, not an error), so the cap is a refusal the app can
/// handle.
pub(crate) const MAX_SOCKETS: usize = 64;

/// How much one connection holds in each direction before it pushes back:
/// writes take fewer bytes, and reading from the network pauses.
pub(crate) const QUEUE_BYTES: usize = 4 * 1024 * 1024;

/// How long a connect may take before it fails.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The most listeners one app may hold at once (IC-987).
pub(crate) const MAX_LISTENERS: usize = 8;

/// The most connections from other devices one app may hold open at once
/// (IC-987). Past it `tcp-accept` answers too-many and the device waits in
/// the system's queue until the app closes one.
pub(crate) const MAX_PEERS: usize = 32;

/// How much one connection from another device holds in each direction
/// before it pushes back (IC-987): smaller than a dialled connection's, as
/// the app did not choose who sends it bytes.
pub(crate) const PEER_QUEUE_BYTES: usize = 1024 * 1024;

/// The lowest port a listener opens (IC-987): below it the ports are the
/// system's.
pub(crate) const LOWEST_LISTEN_PORT: u16 = krate_manifest::LOWEST_LISTEN_PORT;

/// How long the in-use probe waits for something already on the port to
/// answer (IC-987). Something listening on this computer answers within a
/// millisecond; a free port is refused at once on macOS and Linux, and
/// Windows, which retries a refused connect, is cut off here.
const IN_USE_PROBE: Duration = Duration::from_millis(250);

/// The largest UDP payload there is.
const MAX_DATAGRAM: usize = 65_535;

/// How a lookup is doing. Mirrors the WIT `lookup-status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupStatus {
    Pending,
    Ready(Vec<String>),
    Failed(String),
    UnknownHandle,
}

/// How a connection is doing. Mirrors the WIT `connect-status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectStatus {
    Pending,
    Connected(String),
    Failed(String),
    UnknownHandle,
}

/// What a read found. Mirrors the WIT `read-result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadResult {
    Data(Vec<u8>),
    Pending,
    Closed,
    Failed(String),
}

/// Why a listener could not be opened or used. Mirrors the WIT
/// `listen-error`, less the permission case the caller judges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenFault {
    InUse(String),
    BadPort(String),
    TooMany(String),
    UnknownHandle,
    Failed(String),
}

/// One datagram that arrived. Mirrors the WIT `datagram`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    pub host: String,
    pub port: u16,
    pub data: Vec<u8>,
}

#[derive(Default)]
struct TcpInner {
    /// Some once connected: the peer.
    peer: Option<String>,
    /// Some once it failed or broke.
    error: Option<String>,
    inbox: VecDeque<u8>,
    outbox: VecDeque<u8>,
    /// The other end finished sending.
    eof: bool,
    /// The guest closed it.
    closed: bool,
    /// The writer holds bytes it took from the outbox and has not finished
    /// sending: a close must wait for them too.
    sending: bool,
    stream: Option<std::net::TcpStream>,
}

struct TcpShared {
    inner: Mutex<TcpInner>,
    changed: Condvar,
    /// How much it holds in each direction before it pushes back.
    queue_bytes: usize,
    /// Another device connected to one of this app's listeners (IC-987):
    /// counted against [`MAX_PEERS`].
    accepted: bool,
}

impl TcpShared {
    fn new(queue_bytes: usize, accepted: bool) -> Self {
        Self {
            inner: Mutex::new(TcpInner::default()),
            changed: Condvar::new(),
            queue_bytes,
            accepted,
        }
    }
}

/// A listening socket (IC-987). Non-blocking, so an accept never holds the
/// app's loop.
struct Listener {
    socket: std::net::TcpListener,
    port: u16,
    /// Bound to the loopback address: only this computer reaches it.
    this_computer: bool,
}

struct Udp {
    socket: std::net::UdpSocket,
    /// Addresses this socket has sent to: the only ones it hears from.
    peers: HashSet<IpAddr>,
}

enum Socket {
    Tcp(Arc<TcpShared>),
    Udp(Udp),
    Listener(Listener),
}

/// Where a lookup's thread sends its answer.
type LookupAnswer = Receiver<Result<Vec<IpAddr>, String>>;

/// The table of an app's sockets and lookups, owned by its host.
#[derive(Default)]
pub struct AsyncSockets {
    next_handle: u64,
    sockets: BTreeMap<u64, Socket>,
    lookups: BTreeMap<u64, (String, LookupAnswer)>,
    /// Each address a lookup returned, with the names it came from.
    resolved: HashMap<IpAddr, BTreeSet<String>>,
}

impl AsyncSockets {
    pub fn new() -> Self {
        Self::default()
    }

    fn handle(&mut self) -> Result<u64, String> {
        if self.sockets.len() + self.lookups.len() >= MAX_SOCKETS {
            return Err(format!(
                "too many sockets: this app already holds {MAX_SOCKETS}"
            ));
        }
        self.next_handle += 1;
        Ok(self.next_handle)
    }

    /// The names an address stands for, from this app's own lookups.
    pub fn names_for(&self, host: &str) -> Vec<String> {
        host.parse::<IpAddr>()
            .ok()
            .and_then(|ip| self.resolved.get(&ip))
            .map(|names| names.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Start resolving a name the caller has judged.
    pub fn lookup(&mut self, host: String) -> Result<u64, String> {
        let handle = self.handle()?;
        let (tx, rx) = channel();
        let name = host.clone();
        std::thread::spawn(move || {
            let answer = (name.as_str(), 0)
                .to_socket_addrs()
                .map(|addrs| {
                    let mut ips: Vec<IpAddr> = Vec::new();
                    for a in addrs {
                        if !ips.contains(&a.ip()) {
                            ips.push(a.ip());
                        }
                    }
                    ips
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(answer);
        });
        self.lookups.insert(handle, (host, rx));
        Ok(handle)
    }

    pub fn lookup_status(&mut self, handle: u64) -> LookupStatus {
        let Some((_, rx)) = self.lookups.get(&handle) else {
            return LookupStatus::UnknownHandle;
        };
        let answer = match rx.try_recv() {
            Err(TryRecvError::Empty) => return LookupStatus::Pending,
            Err(TryRecvError::Disconnected) => Err("the lookup stopped".to_string()),
            Ok(answer) => answer,
        };
        let (name, _) = self.lookups.remove(&handle).expect("present above");
        match answer {
            Ok(ips) if ips.is_empty() => LookupStatus::Failed(format!("{name} has no address")),
            Ok(ips) => {
                for ip in &ips {
                    self.resolved
                        .entry(*ip)
                        .or_default()
                        .insert(name.to_ascii_lowercase());
                }
                LookupStatus::Ready(ips.iter().map(|ip| ip.to_string()).collect())
            }
            Err(why) => LookupStatus::Failed(why),
        }
    }

    /// Start a connection to an endpoint the caller has judged.
    pub fn tcp_connect(&mut self, host: String, port: u16) -> Result<u64, String> {
        let handle = self.handle()?;
        let shared = Arc::new(TcpShared::new(QUEUE_BYTES, false));
        let worker = Arc::clone(&shared);
        std::thread::spawn(move || run_connection(host, port, worker));
        self.sockets.insert(handle, Socket::Tcp(shared));
        Ok(handle)
    }

    pub fn tcp_status(&self, handle: u64) -> ConnectStatus {
        let Some(Socket::Tcp(shared)) = self.sockets.get(&handle) else {
            return ConnectStatus::UnknownHandle;
        };
        let inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
        match (&inner.error, &inner.peer) {
            (Some(why), _) => ConnectStatus::Failed(why.clone()),
            (None, Some(peer)) => ConnectStatus::Connected(peer.clone()),
            (None, None) => ConnectStatus::Pending,
        }
    }

    pub fn tcp_write(&self, handle: u64, bytes: &[u8]) -> Result<u32, String> {
        let Some(Socket::Tcp(shared)) = self.sockets.get(&handle) else {
            return Err("no such connection".to_string());
        };
        let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(why) = &inner.error {
            return Err(why.clone());
        }
        let room = shared.queue_bytes.saturating_sub(inner.outbox.len());
        let taken = bytes.len().min(room);
        inner.outbox.extend(&bytes[..taken]);
        drop(inner);
        shared.changed.notify_all();
        Ok(taken as u32)
    }

    pub fn tcp_read(&self, handle: u64, max_bytes: u32) -> ReadResult {
        let Some(Socket::Tcp(shared)) = self.sockets.get(&handle) else {
            return ReadResult::Failed("no such connection".to_string());
        };
        let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
        if !inner.inbox.is_empty() {
            let n = inner.inbox.len().min(max_bytes.max(1) as usize);
            let data: Vec<u8> = inner.inbox.drain(..n).collect();
            drop(inner);
            // The reader may be waiting for room.
            shared.changed.notify_all();
            return ReadResult::Data(data);
        }
        if let Some(why) = &inner.error {
            return ReadResult::Failed(why.clone());
        }
        if inner.eof {
            return ReadResult::Closed;
        }
        ReadResult::Pending
    }

    pub fn udp_bind(&mut self, ipv6: bool) -> Result<u64, String> {
        let handle = self.handle()?;
        let any: SocketAddr = if ipv6 {
            "[::]:0".parse().expect("an address")
        } else {
            "0.0.0.0:0".parse().expect("an address")
        };
        let socket = std::net::UdpSocket::bind(any).map_err(|e| e.to_string())?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        self.sockets.insert(
            handle,
            Socket::Udp(Udp {
                socket,
                peers: HashSet::new(),
            }),
        );
        Ok(handle)
    }

    pub fn udp_local_port(&self, handle: u64) -> Result<u16, String> {
        match self.sockets.get(&handle) {
            Some(Socket::Udp(udp)) => udp
                .socket
                .local_addr()
                .map(|a| a.port())
                .map_err(|e| e.to_string()),
            _ => Err("no such UDP socket".to_string()),
        }
    }

    /// Send to an endpoint the caller has judged. A name is resolved here
    /// (and remembered, as a lookup would be).
    pub fn udp_send(
        &mut self,
        handle: u64,
        host: &str,
        port: u16,
        data: &[u8],
    ) -> Result<(), String> {
        if data.len() > MAX_DATAGRAM {
            return Err(format!("a datagram carries at most {MAX_DATAGRAM} bytes"));
        }
        let ipv6 = match self.sockets.get(&handle) {
            Some(Socket::Udp(udp)) => udp
                .socket
                .local_addr()
                .map(|a| a.is_ipv6())
                .unwrap_or(false),
            _ => return Err("no such UDP socket".to_string()),
        };
        let ip = match host.parse::<IpAddr>() {
            Ok(ip) => ip,
            Err(_) => {
                // A name: the first of its addresses this socket's family can
                // reach ("localhost" is ::1 first, which an IPv4 socket cannot).
                let ip = (host, port)
                    .to_socket_addrs()
                    .map_err(|e| e.to_string())?
                    .map(|a| a.ip())
                    .find(|ip| ip.is_ipv6() == ipv6)
                    .ok_or_else(|| {
                        format!(
                            "{host} has no {} address",
                            if ipv6 { "IPv6" } else { "IPv4" }
                        )
                    })?;
                self.resolved
                    .entry(ip)
                    .or_default()
                    .insert(host.to_ascii_lowercase());
                ip
            }
        };
        let Some(Socket::Udp(udp)) = self.sockets.get_mut(&handle) else {
            return Err("no such UDP socket".to_string());
        };
        udp.peers.insert(ip);
        match udp.socket.send_to(data, SocketAddr::new(ip, port)) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                Err("the send queue is full; the datagram was dropped".to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn udp_receive(&mut self, handle: u64) -> Option<Datagram> {
        let Some(Socket::Udp(udp)) = self.sockets.get_mut(&handle) else {
            return None;
        };
        let mut buf = vec![0u8; MAX_DATAGRAM];
        loop {
            match udp.socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    // Only what came from an address this socket sent to.
                    if !udp.peers.contains(&from.ip()) {
                        continue;
                    }
                    return Some(Datagram {
                        host: from.ip().to_string(),
                        port: from.port(),
                        data: buf[..n].to_vec(),
                    });
                }
                // WouldBlock is "nothing yet"; a connection-refused ICMP is
                // also only "nothing", as UDP has no connection.
                Err(_) => return None,
            }
        }
    }

    /// Listen on `port`, which the caller has judged for this scope
    /// (IC-987). `this_computer` binds the loopback address; otherwise every
    /// IPv4 address of this computer. Refuses a port below
    /// [`LOWEST_LISTEN_PORT`] and one anything already listens on, in words.
    pub fn tcp_listen(&mut self, port: u16, this_computer: bool) -> Result<u64, ListenFault> {
        if port < LOWEST_LISTEN_PORT {
            return Err(ListenFault::BadPort(format!(
                "port {port} is below {LOWEST_LISTEN_PORT}: those ports belong to the system, so \
                 an app listens on {LOWEST_LISTEN_PORT} or above"
            )));
        }
        let listeners = self
            .sockets
            .values()
            .filter(|s| matches!(s, Socket::Listener(_)))
            .count();
        if listeners >= MAX_LISTENERS {
            return Err(ListenFault::TooMany(format!(
                "too many listeners: this app already listens on {MAX_LISTENERS} ports"
            )));
        }
        if self
            .sockets
            .values()
            .any(|s| matches!(s, Socket::Listener(l) if l.port == port))
        {
            return Err(ListenFault::InUse(format!(
                "this app already listens on port {port}"
            )));
        }
        let in_use = || {
            ListenFault::InUse(format!(
                "another program on this computer is already listening on port {port}; \
                 close it, or choose another port"
            ))
        };
        // Something already listening on the port answers here. The bind
        // alone does not say so everywhere: with the address reuse Unix
        // listeners carry, macOS lets a loopback bind sit beside another
        // program's listener on every address and takes its connections.
        let probe = SocketAddr::from(([127, 0, 0, 1], port));
        if std::net::TcpStream::connect_timeout(&probe, IN_USE_PROBE).is_ok() {
            return Err(in_use());
        }
        let handle = self.handle().map_err(ListenFault::TooMany)?;
        let at = if this_computer {
            SocketAddr::from(([127, 0, 0, 1], port))
        } else {
            SocketAddr::from(([0, 0, 0, 0], port))
        };
        let socket = std::net::TcpListener::bind(at).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse {
                in_use()
            } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                ListenFault::Failed(format!(
                    "this computer did not let the app listen on port {port}: {e}"
                ))
            } else {
                ListenFault::Failed(format!("could not listen on port {port}: {e}"))
            }
        })?;
        socket
            .set_nonblocking(true)
            .map_err(|e| ListenFault::Failed(e.to_string()))?;
        self.sockets.insert(
            handle,
            Socket::Listener(Listener {
                socket,
                port,
                this_computer,
            }),
        );
        Ok(handle)
    }

    /// The next connection waiting on a listener, or none (IC-987). Never
    /// blocks. The connection gets its own reader and writer threads, as a
    /// dialled one does, and a smaller queue.
    pub fn tcp_accept(&mut self, handle: u64) -> Result<Option<u64>, ListenFault> {
        let Some(Socket::Listener(_)) = self.sockets.get(&handle) else {
            return Err(ListenFault::UnknownHandle);
        };
        let peers = self
            .sockets
            .values()
            .filter(|s| matches!(s, Socket::Tcp(shared) if shared.accepted))
            .count();
        if peers >= MAX_PEERS {
            return Err(ListenFault::TooMany(format!(
                "this app already has {MAX_PEERS} connections from other devices open; \
                 close one to let the next in"
            )));
        }
        if self.sockets.len() + self.lookups.len() >= MAX_SOCKETS {
            return Err(ListenFault::TooMany(format!(
                "too many sockets: this app already holds {MAX_SOCKETS}"
            )));
        }
        let Some(Socket::Listener(listener)) = self.sockets.get(&handle) else {
            return Err(ListenFault::UnknownHandle);
        };
        let (stream, from) = match listener.socket.accept() {
            Ok(accepted) => accepted,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(None)
            }
            Err(e) => return Err(ListenFault::Failed(format!("could not accept: {e}"))),
        };
        // Bound to loopback, only this computer can arrive; checked anyway,
        // so the scope does not rest on the bind alone.
        if listener.this_computer && !from.ip().is_loopback() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return Ok(None);
        }
        // An accepted socket inherits the listener's non-blocking mode on
        // macOS and Windows; its threads block.
        if stream.set_nonblocking(false).is_err() {
            return Ok(None);
        }
        let handle = self.handle().map_err(ListenFault::TooMany)?;
        let shared = Arc::new(TcpShared::new(PEER_QUEUE_BYTES, true));
        if !start_streams(stream, from.to_string(), &shared) {
            return Ok(None);
        }
        self.sockets.insert(handle, Socket::Tcp(shared));
        Ok(Some(handle))
    }

    /// The local port of a listener, a connected TCP connection or a UDP
    /// socket.
    pub fn tcp_local_port(&self, handle: u64) -> Result<u16, ListenFault> {
        match self.sockets.get(&handle) {
            Some(Socket::Listener(listener)) => Ok(listener.port),
            Some(Socket::Udp(udp)) => udp
                .socket
                .local_addr()
                .map(|a| a.port())
                .map_err(|e| ListenFault::Failed(e.to_string())),
            Some(Socket::Tcp(shared)) => {
                let inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
                match &inner.stream {
                    Some(stream) => stream
                        .local_addr()
                        .map(|a| a.port())
                        .map_err(|e| ListenFault::Failed(e.to_string())),
                    None => Err(ListenFault::Failed(
                        "the connection is not connected yet".to_string(),
                    )),
                }
            }
            None => Err(ListenFault::UnknownHandle),
        }
    }

    /// A listener's port, and whether it is for this computer alone
    /// (IC-990): what the host judges before saying where it is reached or
    /// announcing it. Any other handle is unknown here.
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "linux", target_os = "windows")),
        allow(dead_code)
    )]
    pub fn listener(&self, handle: u64) -> Result<(u16, bool), ListenFault> {
        match self.sockets.get(&handle) {
            Some(Socket::Listener(listener)) => Ok((listener.port, listener.this_computer)),
            _ => Err(ListenFault::UnknownHandle),
        }
    }

    /// How many listeners are open: what a test asks after a run.
    #[cfg(test)]
    pub fn listeners(&self) -> usize {
        self.sockets
            .values()
            .filter(|s| matches!(s, Socket::Listener(_)))
            .count()
    }

    pub fn close(&mut self, handle: u64) {
        self.lookups.remove(&handle);
        // A listener or a UDP socket closes as it drops.
        if let Some(Socket::Tcp(shared)) = self.sockets.remove(&handle) {
            close_tcp(&shared);
        }
    }
}

impl Drop for AsyncSockets {
    fn drop(&mut self) {
        for socket in self.sockets.values() {
            if let Socket::Tcp(shared) = socket {
                close_tcp(shared);
            }
        }
    }
}

/// How long a closed connection may spend sending what the app wrote before
/// it closed (IC-987): a server writes its answer and closes at once, and the
/// answer must still arrive -- but a device that stops reading cannot hold
/// the connection open for ever.
const FLUSH_ON_CLOSE: Duration = Duration::from_secs(5);

fn close_tcp(shared: &TcpShared) {
    let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
    inner.closed = true;
    if let Some(stream) = &inner.stream {
        if (inner.outbox.is_empty() && !inner.sending) || inner.error.is_some() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        } else {
            // The writer sends what is queued, then shuts the connection
            // down; a write stalled past the deadline ends it.
            let _ = stream.set_write_timeout(Some(FLUSH_ON_CLOSE));
        }
    }
    drop(inner);
    shared.changed.notify_all();
}

/// The connection's own thread: dial, then read until the end, while a
/// second thread writes what the app queues.
fn run_connection(host: String, port: u16, shared: Arc<TcpShared>) {
    let fail = |why: String| {
        let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.error.is_none() {
            inner.error = Some(why);
        }
        drop(inner);
        shared.changed.notify_all();
    };
    let addrs: Vec<SocketAddr> = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(e) => return fail(format!("could not find {host}: {e}")),
    };
    let mut last = format!("{host} has no address");
    let mut stream = None;
    for addr in addrs {
        match std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = format!("could not connect to {addr}: {e}"),
        }
    }
    let Some(stream) = stream else {
        return fail(last);
    };
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
    if !start_streams(stream, peer, &shared) {
        fail("could not share the connection between its threads".to_string());
    }
}

/// A connected stream, dialled or accepted: mark it connected, then a
/// writer thread sends what the app queues and a reader thread fills the
/// inbox. False when the stream could not be shared between its threads.
fn start_streams(stream: std::net::TcpStream, peer: String, shared: &Arc<TcpShared>) -> bool {
    let _ = stream.set_nodelay(true);
    let (Ok(writer), Ok(control)) = (stream.try_clone(), stream.try_clone()) else {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return false;
    };
    {
        let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.closed {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return true;
        }
        inner.peer = Some(peer);
        inner.stream = Some(control);
    }
    shared.changed.notify_all();
    let writer_shared = Arc::clone(shared);
    std::thread::spawn(move || run_writer(writer, writer_shared));
    let reader_shared = Arc::clone(shared);
    std::thread::spawn(move || run_reader(stream, &reader_shared));
    true
}

fn run_reader(mut stream: std::net::TcpStream, shared: &TcpShared) {
    use std::io::Read;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        // Hold off while the app has not taken what arrived.
        {
            let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
            while inner.inbox.len() >= shared.queue_bytes && !inner.closed {
                inner = shared
                    .changed
                    .wait(inner)
                    .unwrap_or_else(|p| p.into_inner());
            }
            if inner.closed {
                return;
            }
        }
        match stream.read(&mut buf) {
            Ok(0) => {
                let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
                inner.eof = true;
                return;
            }
            Ok(n) => {
                let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
                inner.inbox.extend(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
                if !inner.closed && inner.error.is_none() {
                    inner.error = Some(format!("the connection broke: {e}"));
                }
                return;
            }
        }
    }
}

fn run_writer(mut stream: std::net::TcpStream, shared: Arc<TcpShared>) {
    use std::io::Write;
    loop {
        let chunk: Vec<u8> = {
            let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
            while inner.outbox.is_empty() && !inner.closed && inner.error.is_none() {
                inner = shared
                    .changed
                    .wait(inner)
                    .unwrap_or_else(|p| p.into_inner());
            }
            if inner.error.is_some() {
                return;
            }
            if inner.closed && inner.outbox.is_empty() {
                // Closed by the app, and everything it wrote is sent.
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
            let n = inner.outbox.len().min(256 * 1024);
            inner.sending = true;
            inner.outbox.drain(..n).collect()
        };
        shared.changed.notify_all();
        let sent = stream.write_all(&chunk);
        shared
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .sending = false;
        if let Err(e) = sent {
            let mut inner = shared.inner.lock().unwrap_or_else(|p| p.into_inner());
            if inner.closed {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            } else if inner.error.is_none() {
                inner.error = Some(format!("the connection broke: {e}"));
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait<T>(mut f: impl FnMut() -> Option<T>) -> T {
        for _ in 0..500 {
            if let Some(v) = f() {
                return v;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out");
    }

    #[test]
    fn a_connection_carries_bytes_both_ways_and_ends() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut s, _) = listener.accept().unwrap();
            let mut got = [0u8; 5];
            s.read_exact(&mut got).unwrap();
            s.write_all(b"world").unwrap();
            got
        });
        let mut table = AsyncSockets::new();
        let h = table.tcp_connect("127.0.0.1".into(), port).unwrap();
        // Written before the connection is up: sent once it is.
        assert_eq!(table.tcp_write(h, b"hello").unwrap(), 5);
        let peer = wait(|| match table.tcp_status(h) {
            ConnectStatus::Connected(p) => Some(p),
            ConnectStatus::Pending => None,
            other => panic!("{other:?}"),
        });
        assert_eq!(peer, format!("127.0.0.1:{port}"));
        let mut got = Vec::new();
        let end = wait(|| match table.tcp_read(h, 2) {
            ReadResult::Data(d) => {
                got.extend(d);
                None
            }
            ReadResult::Pending => None,
            other => Some(other),
        });
        assert_eq!(got, b"world");
        assert_eq!(end, ReadResult::Closed);
        assert_eq!(&server.join().unwrap(), b"hello");
        table.close(h);
        assert_eq!(table.tcp_status(h), ConnectStatus::UnknownHandle);
    }

    #[test]
    fn a_refused_connection_fails_with_the_reason() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let mut table = AsyncSockets::new();
        let h = table.tcp_connect("127.0.0.1".into(), port).unwrap();
        let why = wait(|| match table.tcp_status(h) {
            ConnectStatus::Failed(why) => Some(why),
            _ => None,
        });
        assert!(why.contains("could not connect"), "{why}");
    }

    #[test]
    fn a_udp_socket_hears_only_addresses_it_sent_to() {
        let echo = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        let mut table = AsyncSockets::new();
        let h = table.udp_bind(false).unwrap();
        let mine = table.udp_local_port(h).unwrap();
        // A stranger's datagram before we sent anywhere: not heard.
        let stranger = std::net::UdpSocket::bind("127.0.0.2:0")
            .or_else(|_| std::net::UdpSocket::bind("[::1]:0"))
            .unwrap();
        let _ = stranger.send_to(b"psst", ("127.0.0.1", mine));
        // By name: "localhost" is ::1 first, and this IPv4 socket picks 127.0.0.1.
        table.udp_send(h, "localhost", echo_port, b"ping").unwrap();
        let mut buf = [0u8; 16];
        let (n, from) = echo.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ping");
        echo.send_to(b"pong", from).unwrap();
        let got = wait(|| table.udp_receive(h));
        assert_eq!(got.data, b"pong");
        assert_eq!(got.host, "127.0.0.1");
        assert_eq!(got.port, echo_port);
        assert!(table.udp_receive(h).is_none());
    }

    /// A port nothing listens on, at or above 1024.
    fn free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    /// Nothing listens on `port` any more: a connect is refused and the
    /// port binds again.
    fn port_is_free(port: u16) -> bool {
        std::net::TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(250),
        )
        .is_err()
            && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    }

    /// IC-987: listen on loopback, a device (this test) connects, bytes go
    /// both ways through the ordinary read and write calls, and closing the
    /// listener closes the port.
    #[test]
    fn a_listener_takes_a_connection_that_carries_bytes_both_ways() {
        use std::io::{Read, Write};
        let port = free_port();
        let mut table = AsyncSockets::new();
        let l = table.tcp_listen(port, true).expect("listen");
        assert_eq!(table.tcp_local_port(l), Ok(port));
        // Nobody yet: none, at once.
        assert_eq!(table.tcp_accept(l), Ok(None));
        let mut device = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let c = wait(|| table.tcp_accept(l).expect("accept"));
        assert!(
            matches!(table.tcp_status(c), ConnectStatus::Connected(peer) if peer.starts_with("127.0.0.1:"))
        );
        assert_eq!(table.tcp_local_port(c), Ok(port));
        device.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        let mut got = Vec::new();
        wait(|| match table.tcp_read(c, 4096) {
            ReadResult::Data(d) => {
                got.extend(d);
                got.ends_with(b"\r\n\r\n").then_some(())
            }
            ReadResult::Pending => None,
            other => panic!("{other:?}"),
        });
        assert_eq!(got, b"GET / HTTP/1.0\r\n\r\n");
        // A server's shape: write the answer and close at once. The answer
        // still arrives, then the end.
        let page = vec![b'x'; 256 * 1024];
        assert_eq!(table.tcp_write(c, &page).unwrap(), page.len() as u32);
        table.close(c);
        let mut back = Vec::new();
        device.read_to_end(&mut back).unwrap();
        assert_eq!(
            back.len(),
            page.len(),
            "everything written before close arrives"
        );
        table.close(l);
        assert_eq!(table.tcp_accept(l), Err(ListenFault::UnknownHandle));
        assert!(port_is_free(port), "a closed listener closes its port");
    }

    /// A port something already listens on is refused in words, whether it
    /// is another program's or this app's own; a system port is refused
    /// before anything binds.
    #[test]
    fn a_port_in_use_or_below_1024_is_refused_in_words() {
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = other.local_addr().unwrap().port();
        let mut table = AsyncSockets::new();
        for this_computer in [true, false] {
            match table.tcp_listen(taken, this_computer) {
                Err(ListenFault::InUse(words)) => assert!(
                    words.contains("another program") && words.contains(&taken.to_string()),
                    "{words}"
                ),
                other => panic!("in use, got {other:?}"),
            }
        }
        let port = free_port();
        table.tcp_listen(port, true).expect("listen");
        assert!(matches!(
            table.tcp_listen(port, false),
            Err(ListenFault::InUse(words)) if words == format!("this app already listens on port {port}")
        ));
        assert!(matches!(
            table.tcp_listen(80, true),
            Err(ListenFault::BadPort(words)) if words.contains("below 1024")
        ));
        assert!(matches!(
            table.tcp_listen(0, true),
            Err(ListenFault::BadPort(_))
        ));
    }

    /// The run ending (the table dropped) closes every listener and every
    /// connection it accepted.
    #[test]
    fn every_listener_and_its_connections_close_when_the_run_ends() {
        use std::io::Read;
        let (a, b) = (free_port(), free_port());
        let mut table = AsyncSockets::new();
        let la = table.tcp_listen(a, true).expect("listen a");
        table.tcp_listen(b, true).expect("listen b");
        assert_eq!(table.listeners(), 2);
        let mut device = std::net::TcpStream::connect(("127.0.0.1", a)).expect("connect");
        wait(|| table.tcp_accept(la).expect("accept"));
        drop(table);
        device
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut rest = Vec::new();
        device
            .read_to_end(&mut rest)
            .expect("the connection ends, it does not hang");
        assert!(port_is_free(a), "port {a} is closed after the run");
        assert!(port_is_free(b), "port {b} is closed after the run");
    }

    /// Connections from other devices are bounded: past [`MAX_PEERS`] the
    /// next waits, and closing one lets it in.
    #[test]
    fn connections_from_other_devices_are_bounded() {
        let port = free_port();
        let mut table = AsyncSockets::new();
        let l = table.tcp_listen(port, true).expect("listen");
        let devices: Vec<_> = (0..=MAX_PEERS)
            .map(|_| std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect"))
            .collect();
        let mut accepted = Vec::new();
        while accepted.len() < MAX_PEERS {
            if let Some(c) = table.tcp_accept(l).expect("accept") {
                accepted.push(c);
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(matches!(table.tcp_accept(l), Err(ListenFault::TooMany(_))));
        table.close(accepted[0]);
        let next = wait(|| table.tcp_accept(l).expect("room again"));
        assert!(matches!(
            table.tcp_status(next),
            ConnectStatus::Connected(_)
        ));
        drop(devices);
    }

    #[test]
    fn a_lookup_remembers_which_name_an_address_stands_for() {
        let mut table = AsyncSockets::new();
        let h = table.lookup("localhost".into()).unwrap();
        let ips = wait(|| match table.lookup_status(h) {
            LookupStatus::Ready(ips) => Some(ips),
            LookupStatus::Pending => None,
            other => panic!("{other:?}"),
        });
        assert!(!ips.is_empty());
        assert_eq!(table.names_for(&ips[0]), vec!["localhost".to_string()]);
        assert!(table.names_for("10.9.8.7").is_empty());
        assert_eq!(table.lookup_status(h), LookupStatus::UnknownHandle);
    }
}
