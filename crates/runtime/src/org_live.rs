//! The live socket for an app's team data (SMART-FIXES 7): push instead of
//! poll.
//!
//! While an organisation's app is open, this device holds one WebSocket to
//! the organisation's store on its hub (`/orgs/<slug>/data/<digest>/live`,
//! cloud/worker/src/org-data-live.js). When someone else's sync writes
//! something new for the app, the hub sends `{"t":"changed","s":"<cursor>"}`
//! and this device syncs at once. Nothing secret travels on the socket: the
//! message names no record and no person, and the data still only moves
//! sealed, by sync.
//!
//! **Never in the window's way (K-1107).** The socket lives on its own
//! background thread; the app's thread never touches it and never waits on
//! it. A "changed" only raises a flag and wakes the sync thread, which does
//! the exchange as it always has.
//!
//! The socket opens with a one-use ticket that a sync hands out
//! ([`Live::wants_ticket`] puts `live: true` in the sync's body), so it
//! passes exactly the sync's checks. While it is open and healthy the sync
//! thread polls only every [`LIVE_EVERY`]; when it drops, back to every few
//! seconds, and a new ticket after a backoff that doubles on each quick
//! failure, with jitter. Offline is never an error: it is the poll carrying
//! on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How often the sync thread polls while the socket is open and healthy:
/// only the fallback, since the socket says when something changed.
pub const LIVE_EVERY: Duration = Duration::from_secs(30);
/// The heartbeat: a WebSocket protocol ping this often (answered without
/// waking the organisation's object, and not billed as a message; a
/// browser, which cannot send one, sends the text "ping" instead) ...
const PING: Duration = Duration::from_secs(25);
/// ... and the socket is dead when nothing came back for this long past one.
const PONG_WAIT: Duration = Duration::from_secs(10);
/// The first wait before reconnecting, and the longest.
const BACKOFF: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A socket that stayed open this long starts the waits over.
const STEADY: Duration = Duration::from_secs(30);
/// How often the socket thread looks up from a quiet read.
const TICK: Duration = Duration::from_millis(250);
/// The most a hub message may be: a "changed" is a few dozen bytes.
const MAX_MESSAGE: usize = 4096;

/// A ticket from a sync's answer, and where to use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub url: String,
    pub ticket: String,
    pub protocol: String,
}

#[derive(Debug)]
struct State {
    ticket: Option<Ticket>,
    /// A connect is under way.
    connecting: bool,
    /// No new ticket before this.
    next_try: Instant,
    /// Quick failures in a row.
    fails: u32,
}

/// Paces, so a test can run them in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Paces {
    pub ping: Duration,
    pub pong_wait: Duration,
    pub backoff: Duration,
    pub backoff_max: Duration,
    pub steady: Duration,
    pub tick: Duration,
}

impl Default for Paces {
    fn default() -> Self {
        Paces {
            ping: PING,
            pong_wait: PONG_WAIT,
            backoff: BACKOFF,
            backoff_max: BACKOFF_MAX,
            steady: STEADY,
            tick: TICK,
        }
    }
}

/// What the sync thread and the socket thread share. Plain flags and one
/// lock; neither ever holds it across the network.
#[derive(Debug)]
pub struct Live {
    state: Mutex<State>,
    wake: Condvar,
    open: AtomicBool,
    stop: AtomicBool,
    paces: Paces,
}

impl Live {
    pub fn new(paces: Paces) -> Arc<Live> {
        Arc::new(Live {
            state: Mutex::new(State {
                ticket: None,
                connecting: false,
                next_try: Instant::now(),
                fails: 0,
            }),
            wake: Condvar::new(),
            open: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            paces,
        })
    }

    /// The socket is open and has answered its heartbeat.
    pub fn healthy(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    /// How long the sync thread waits for its next poll.
    pub fn pace(&self, quick: Duration) -> Duration {
        if self.healthy() {
            return LIVE_EVERY.max(quick);
        }
        // Not open: no later than when a new ticket may be asked for, so a
        // dropped socket comes back after its backoff, not a poll later.
        let Ok(state) = self.state.lock() else {
            return quick;
        };
        if self.stop.load(Ordering::SeqCst) || state.connecting || state.ticket.is_some() {
            return quick;
        }
        let until = state.next_try.saturating_duration_since(Instant::now());
        if until.is_zero() {
            quick
        } else {
            quick.min(until)
        }
    }

    /// Whether this sync should ask for a ticket: no socket, none waiting
    /// to be used, none being tried, and the backoff has passed.
    pub fn wants_ticket(&self) -> bool {
        if self.healthy() || self.stop.load(Ordering::SeqCst) {
            return false;
        }
        self.state.lock().is_ok_and(|state| {
            state.ticket.is_none() && !state.connecting && Instant::now() >= state.next_try
        })
    }

    /// A sync's answer carried a ticket: hand it to the socket thread.
    pub fn offer(&self, ticket: Ticket) {
        if let Ok(mut state) = self.state.lock() {
            state.ticket = Some(ticket);
        }
        self.wake.notify_all();
    }

    /// Close the socket and end the thread (the app closed, or the hub
    /// refused this person).
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Wait for a ticket, or the end. None means stop.
    fn next_ticket(&self) -> Option<Ticket> {
        let mut state = self.state.lock().ok()?;
        loop {
            if self.stopped() {
                return None;
            }
            if let Some(ticket) = state.ticket.take() {
                state.connecting = true;
                return Some(ticket);
            }
            state = self
                .wake
                .wait_timeout(state, Duration::from_secs(1))
                .ok()?
                .0;
        }
    }

    /// The socket is gone (or never opened): the next ticket after a wait
    /// that doubles with each quick failure, with jitter so a whole office
    /// does not reconnect in the same second.
    fn dropped(&self, open_for: Option<Duration>) {
        self.open.store(false, Ordering::SeqCst);
        if let Ok(mut state) = self.state.lock() {
            state.connecting = false;
            let steady = open_for.is_some_and(|d| d >= self.paces.steady);
            state.fails = if steady { 0 } else { state.fails + 1 };
            let doubled = self
                .paces
                .backoff
                .saturating_mul(1u32 << state.fails.saturating_sub(1).min(16));
            let wait = doubled.min(self.paces.backoff_max);
            state.next_try = Instant::now() + jitter(wait);
        }
    }

    #[cfg(test)]
    fn fails(&self) -> u32 {
        self.state.lock().map(|s| s.fails).unwrap_or(0)
    }
}

/// Between half and all of `wait`.
fn jitter(wait: Duration) -> Duration {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 2];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    let r = u16::from_le_bytes(b) as f64 / u16::MAX as f64;
    wait.mul_f64(0.5 + 0.5 * r)
}

/// The live URL for a hub: http(s) becomes ws(s).
pub fn live_url(hub: &str, slug: &str, digest: &str) -> String {
    let hub = hub.trim_end_matches('/');
    let ws = if let Some(rest) = hub.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = hub.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        hub.to_string()
    };
    format!("{ws}/orgs/{slug}/data/{digest}/live")
}

/// The ticket a sync's answer carries, if it carries one.
pub fn ticket_of(answer: &serde_json::Value, url: String) -> Option<Ticket> {
    let live = answer.get("live")?;
    let ticket = live.get("ticket")?.as_str()?;
    if ticket.len() != 64 || !ticket.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(Ticket {
        url,
        ticket: ticket.to_string(),
        protocol: live
            .get("protocol")
            .and_then(|p| p.as_str())
            .unwrap_or("krate-live")
            .to_string(),
    })
}

/// Start the socket thread. `changed` runs on it each time the hub says
/// something new is there; `gone` each time a try ends, with whether the
/// socket had opened -- one that did may have missed something, so the
/// sync thread catches up.
pub fn spawn(
    live: Arc<Live>,
    changed: impl Fn() + Send + 'static,
    gone: impl Fn(bool) + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("krate-org-live".into())
        .spawn(move || {
            while let Some(ticket) = live.next_ticket() {
                let opened = run_socket(&live, &ticket, &changed);
                live.dropped(opened);
                if live.stopped() {
                    return;
                }
                gone(opened.is_some());
            }
        })
        .ok()
}

/// One socket, from connect to close. Answers how long it was open, or
/// None when it never opened.
fn run_socket(live: &Live, ticket: &Ticket, changed: &dyn Fn()) -> Option<Duration> {
    use tungstenite::protocol::Message;
    use tungstenite::stream::MaybeTlsStream;

    let protocols = [ticket.protocol.as_str(), ticket.ticket.as_str()];
    let (mut socket, _) = crate::async_ws::connect_with_protocols(&ticket.url, &protocols).ok()?;
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => Some(stream),
        MaybeTlsStream::Rustls(tls) => Some(tls.get_mut()),
        _ => None,
    };
    if let Some(stream) = stream {
        let _ = stream.set_read_timeout(Some(live.paces.tick));
    }
    let opened = Instant::now();
    live.open.store(true, Ordering::SeqCst);
    if let Ok(mut state) = live.state.lock() {
        state.connecting = false;
    }
    let mut heard = Instant::now();
    let mut pinged = Instant::now();
    loop {
        if live.stopped() {
            let _ = socket.close(None);
            let _ = socket.flush();
            return Some(opened.elapsed());
        }
        if heard.elapsed() > live.paces.ping + live.paces.pong_wait {
            // A laptop that slept, a proxy that dropped it: dead.
            return Some(opened.elapsed());
        }
        if pinged.elapsed() >= live.paces.ping {
            pinged = Instant::now();
            if socket.send(Message::Ping(Vec::new())).is_err() {
                return Some(opened.elapsed());
            }
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                heard = Instant::now();
                if text.len() <= MAX_MESSAGE && is_changed(text.as_str()) {
                    changed();
                }
            }
            Ok(Message::Close(_)) => return Some(opened.elapsed()),
            Ok(_) => heard = Instant::now(),
            Err(tungstenite::Error::Io(err))
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return Some(opened.elapsed()),
        }
    }
}

fn is_changed(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .is_some_and(|m| m["t"] == "changed")
}

/// A hub in miniature for tests here and in org_store.rs.
#[cfg(test)]
pub(crate) mod test_hub {
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A hub in miniature: accepts one socket per connection (answering the
    /// krate-live protocol), then runs `serve` on it.
    pub fn hub(
        serve: impl Fn(&mut tungstenite::WebSocket<std::net::TcpStream>) + Send + Sync + 'static,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let count = accepted.clone();
        let serve = Arc::new(serve);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                count.fetch_add(1, Ordering::SeqCst);
                let serve = serve.clone();
                std::thread::spawn(move || {
                    use tungstenite::handshake::server::{ErrorResponse, Request, Response};
                    // The handshake's own error type, as tungstenite gives it.
                    #[allow(clippy::result_large_err)]
                    let answer =
                        |req: &Request, mut res: Response| -> Result<Response, ErrorResponse> {
                            let offered = req
                                .headers()
                                .get("sec-websocket-protocol")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            assert!(offered.starts_with("krate-live, "), "{offered}");
                            res.headers_mut()
                                .insert("sec-websocket-protocol", "krate-live".parse().unwrap());
                            Ok(res)
                        };
                    if let Ok(mut ws) = tungstenite::accept_hdr(stream, answer) {
                        serve(&mut ws);
                    }
                });
            }
        });
        (
            format!("ws://127.0.0.1:{port}/orgs/acme/data/x/live"),
            accepted,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_hub::hub;
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn quick() -> Paces {
        Paces {
            ping: Duration::from_millis(100),
            pong_wait: Duration::from_millis(100),
            backoff: Duration::from_millis(20),
            backoff_max: Duration::from_millis(200),
            steady: Duration::from_secs(30),
            tick: Duration::from_millis(10),
        }
    }

    fn ticket(url: &str) -> Ticket {
        Ticket {
            url: url.to_string(),
            ticket: "ab".repeat(32),
            protocol: "krate-live".into(),
        }
    }

    fn wait_for(what: &str, ok: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn a_changed_message_runs_the_callback_and_nothing_else_does() {
        let (url, _) = hub(|ws| {
            ws.send(tungstenite::Message::text("pong")).unwrap();
            ws.send(tungstenite::Message::text(r#"{"t":"other"}"#))
                .unwrap();
            ws.send(tungstenite::Message::text(r#"{"t":"changed","s":"7"}"#))
                .unwrap();
            loop {
                if ws.read().is_err() {
                    return;
                }
            }
        });
        let live = Live::new(quick());
        let seen = Arc::new(AtomicUsize::new(0));
        let s = seen.clone();
        spawn(
            live.clone(),
            move || {
                s.fetch_add(1, Ordering::SeqCst);
            },
            |_| {},
        );
        assert!(live.wants_ticket());
        live.offer(ticket(&url));
        assert!(!live.wants_ticket(), "one waiting is enough");
        wait_for("the change", || seen.load(Ordering::SeqCst) == 1);
        assert!(live.healthy());
        assert_eq!(
            live.pace(Duration::from_secs(5)),
            LIVE_EVERY,
            "slow while healthy"
        );
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "only the changed message counts"
        );
        live.shutdown();
    }

    #[test]
    fn a_dead_socket_falls_back_to_the_quick_poll_and_reconnects_after_a_backoff() {
        // The hub closes the first socket at once, then keeps the next one.
        let (url, accepted) = hub(|ws| {
            ws.send(tungstenite::Message::text(r#"{"t":"changed","s":"1"}"#))
                .unwrap();
            std::thread::sleep(Duration::from_millis(30));
            let _ = ws.close(None);
            let _ = ws.flush();
        });
        let live = Live::new(quick());
        let gone = Arc::new(AtomicUsize::new(0));
        let g = gone.clone();
        spawn(
            live.clone(),
            || {},
            move |_| {
                g.fetch_add(1, Ordering::SeqCst);
            },
        );
        live.offer(ticket(&url));
        wait_for("open", || live.healthy());
        wait_for("the drop", || gone.load(Ordering::SeqCst) == 1);
        assert!(!live.healthy());
        let quick_poll = Duration::from_secs(5);
        assert!(
            live.pace(quick_poll) <= quick_poll,
            "back to the quick pace"
        );
        assert!(
            live.pace(quick_poll) <= Duration::from_millis(20),
            "no later than the backoff"
        );
        assert_eq!(live.fails(), 1);
        wait_for("the backoff", || live.wants_ticket());
        live.offer(ticket(&url));
        wait_for("the second socket", || accepted.load(Ordering::SeqCst) == 2);
        live.shutdown();
    }

    #[test]
    fn a_silent_socket_is_dead_after_the_heartbeat_and_quick_failures_back_off_further() {
        // Accepts and then never says a word, even to a ping.
        let (url, _) = hub(|_ws| std::thread::sleep(Duration::from_secs(10)));
        let live = Live::new(quick());
        let gone = Arc::new(AtomicUsize::new(0));
        let g = gone.clone();
        spawn(
            live.clone(),
            || {},
            move |_| {
                g.fetch_add(1, Ordering::SeqCst);
            },
        );
        live.offer(ticket(&url));
        wait_for("open", || live.healthy());
        let started = Instant::now();
        wait_for("declared dead", || gone.load(Ordering::SeqCst) == 1);
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "{:?}",
            started.elapsed()
        );
        assert!(!live.healthy());

        // Nobody there at all: never an error, and the waits grow.
        let nobody = Live::new(quick());
        let failed = Arc::new(AtomicUsize::new(0));
        let f = failed.clone();
        spawn(
            nobody.clone(),
            || {},
            move |opened| {
                assert!(!opened);
                f.fetch_add(1, Ordering::SeqCst);
            },
        );
        let mut waits = Vec::new();
        for i in 0..4 {
            wait_for("a try", || nobody.wants_ticket());
            nobody.offer(ticket("ws://127.0.0.1:9/orgs/acme/data/x/live"));
            wait_for("the failure", || failed.load(Ordering::SeqCst) == i + 1);
            let next = nobody.state.lock().unwrap().next_try;
            waits.push(next.saturating_duration_since(Instant::now()));
        }
        assert_eq!(nobody.fails(), 4);
        // 20 ms doubling: the first wait is at most 20 ms, the fourth at
        // least half of 160 ms (jitter keeps half to all of it).
        assert!(waits[0] <= Duration::from_millis(20), "{waits:?}");
        assert!(waits[3] >= Duration::from_millis(60), "{waits:?}");
        nobody.shutdown();
    }

    /// Against a real hub (`wrangler dev`, cloud/worker/test/live-latency.mjs
    /// says how): KRATE_ORG_LIVE_URL and KRATE_ORG_LIVE_TICKET from a sync's
    /// answer. The socket must open with the protocol, stay healthy across
    /// several protocol pings, and hear a "changed" written by someone else.
    #[test]
    fn a_real_hub_socket() {
        let (Ok(url), Ok(t)) = (
            std::env::var("KRATE_ORG_LIVE_URL"),
            std::env::var("KRATE_ORG_LIVE_TICKET"),
        ) else {
            return;
        };
        let paces = Paces {
            ping: Duration::from_secs(1),
            pong_wait: Duration::from_secs(1),
            ..Paces::default()
        };
        let live = Live::new(paces);
        let heard = Arc::new(Mutex::new(None::<Instant>));
        let h = heard.clone();
        spawn(
            live.clone(),
            move || {
                let mut at = h.lock().unwrap();
                if at.is_none() {
                    *at = Some(Instant::now());
                    let ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0);
                    eprintln!("changed heard at unix ms {ms}");
                }
            },
            |_| {},
        );
        live.offer(Ticket {
            url,
            ticket: t,
            protocol: "krate-live".into(),
        });
        wait_for("open", || live.healthy());
        let opened = Instant::now();
        // Healthy past several heartbeats: the hub answers protocol pings.
        std::thread::sleep(Duration::from_secs(5));
        assert!(live.healthy(), "the hub did not answer the pings");
        let end = Instant::now() + Duration::from_secs(30);
        while heard.lock().unwrap().is_none() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        let at = heard.lock().unwrap().expect("no changed message");
        eprintln!("changed heard {:?} after the socket opened", at - opened);
        live.shutdown();
    }

    #[test]
    fn tickets_and_urls() {
        assert_eq!(
            live_url("https://hub.krate.tech/", "acme", "ab"),
            "wss://hub.krate.tech/orgs/acme/data/ab/live"
        );
        assert_eq!(
            live_url("http://127.0.0.1:8787", "acme", "ab"),
            "ws://127.0.0.1:8787/orgs/acme/data/ab/live"
        );
        let t = "c".repeat(64);
        let answer = serde_json::json!({ "live": { "ticket": t, "protocol": "krate-live" } });
        assert_eq!(ticket_of(&answer, "u".into()).unwrap().ticket, t);
        assert!(ticket_of(
            &serde_json::json!({ "live": { "ticket": "nope" } }),
            "u".into()
        )
        .is_none());
        assert!(ticket_of(&serde_json::json!({}), "u".into()).is_none());
        assert!(is_changed(r#"{"t":"changed","s":"3"}"#));
        assert!(!is_changed("pong"));
    }
}
