//! MQTT 3.1.1: publish readings to a broker and hear what others publish
//! (IC-994).
//!
//! Turn on the `mqtt` feature of the `krate` dependency and declare the
//! broker as `net.connect:<host>:1883`. There is no grant of its own: the
//! host is the whole permission. Every session is clean (the broker keeps
//! nothing between connections); messages go at QoS 0 or 1. Plain TCP only:
//! Krate's sockets do not do TLS yet, so a broker that only listens on 8883
//! cannot be reached from an app.
//!
//! Nothing blocks for longer than the timeout (five seconds unless the
//! [`Options`] say otherwise). Call [`Client::poll`] from the app's loop at
//! least once every keep-alive period: it hands over what arrived and keeps
//! the connection alive.
//!
//! ```no_run
//! # fn demo() -> Result<(), krate::mqtt::MqttError> {
//! use krate::mqtt::{self, QoS};
//! let options = mqtt::Options::new("oven-7").login("lab", "secret");
//! let mut broker = mqtt::Client::connect("broker.lab.local", 1883, &options)?;
//! broker.publish("lab/oven-7/celsius", b"64.8", QoS::AtLeastOnce)?;
//! broker.subscribe("lab/oven-7/setpoint", QoS::AtLeastOnce)?;
//! while let Some(message) = broker.poll()? {
//!     let _setpoint = message.text();
//! }
//! broker.disconnect();
//! # Ok(()) }
//! ```

use crate::link::{Link, LinkError, TcpLink};
use alloc::{boxed::Box, collections::VecDeque, format, string::String, vec::Vec};

pub mod packet;
use packet::Incoming;

/// How sure a message is to arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QoS {
    /// Sent once; lost if the connection drops on the way.
    AtMostOnce = 0,
    /// The broker confirms it (PUBACK); may arrive twice.
    AtLeastOnce = 1,
}

/// One message on a topic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// Where it was published: `lab/oven-7/setpoint`.
    pub topic: String,
    /// What was published.
    pub payload: Vec<u8>,
    /// How it was delivered.
    pub qos: QoS,
    /// The broker kept it as the topic's last value and sent it on
    /// subscribing.
    pub retained: bool,
}

impl Message {
    /// The payload as text, if it is UTF-8.
    pub fn text(&self) -> Option<&str> {
        core::str::from_utf8(&self.payload).ok()
    }
}

/// Why the broker turned the connection away (CONNACK's return code).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 1: the broker does not speak MQTT 3.1.1.
    ProtocolVersion,
    /// 2: the broker does not accept this client id.
    ClientId,
    /// 3: the broker is up but not serving.
    ServerUnavailable,
    /// 4: the user name or password is wrong.
    BadLogin,
    /// 5: this login may not connect.
    NotAuthorized,
    /// Any other code.
    Other(u8),
}

/// Why an MQTT call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MqttError {
    /// The broker turned the connection away.
    Refused(Refusal),
    /// The broker did not answer in time; the words say what was awaited.
    TimedOut(&'static str),
    /// The broker refused a subscription.
    SubscribeRefused,
    /// The broker sent something MQTT does not allow, with words.
    Protocol(&'static str),
    /// A topic or filter MQTT does not allow, with words.
    BadTopic(&'static str),
    /// Connection options MQTT does not allow, with words.
    BadOptions(&'static str),
    /// A packet larger than the limit in the [`Options`].
    TooLarge,
    /// The connection failed or was closed. Connect again.
    Link(LinkError),
}

impl MqttError {
    /// Words to show the person.
    pub fn message(&self) -> String {
        match self {
            MqttError::Refused(r) => String::from(match r {
                Refusal::ProtocolVersion => "The broker does not speak MQTT 3.1.1.",
                Refusal::ClientId => "The broker does not accept this client id.",
                Refusal::ServerUnavailable => "The broker is not serving right now.",
                Refusal::BadLogin => "The broker says the user name or password is wrong.",
                Refusal::NotAuthorized => "The broker does not let this login connect.",
                Refusal::Other(_) => "The broker turned the connection away.",
            }),
            MqttError::TimedOut(what) => format!("The broker did not answer in time ({what})."),
            MqttError::SubscribeRefused => {
                String::from("The broker refused the subscription to that topic.")
            }
            MqttError::Protocol(w) => {
                format!("The broker sent something MQTT does not allow: {w}.")
            }
            MqttError::BadTopic(w) => format!("That topic is not possible: {w}."),
            MqttError::BadOptions(w) => format!("Those connection settings are not possible: {w}."),
            MqttError::TooLarge => String::from("A message was larger than this app accepts."),
            MqttError::Link(e) => e.message(),
        }
    }
}

impl core::fmt::Display for MqttError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message())
    }
}

impl From<LinkError> for MqttError {
    fn from(e: LinkError) -> Self {
        MqttError::Link(e)
    }
}

/// How to connect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Who this client is to the broker. Empty: the broker picks one.
    pub client_id: String,
    /// Seconds between signs of life (0: none). Default 60.
    pub keep_alive_secs: u16,
    /// A user name, if the broker wants a login.
    pub username: Option<String>,
    /// A password, with the user name.
    pub password: Option<String>,
    /// How long to wait for any answer, in milliseconds. Default 5000.
    pub timeout_ms: u32,
    /// The largest packet accepted from the broker, in bytes. Default 1 MiB.
    pub max_packet: usize,
    /// Messages kept until [`Client::poll`] hands them over; past this the
    /// oldest is dropped and counted. Default 1024.
    pub max_queued: usize,
}

impl Options {
    /// Connect as `client_id`, with the defaults.
    pub fn new(client_id: &str) -> Self {
        Options {
            client_id: String::from(client_id),
            keep_alive_secs: 60,
            username: None,
            password: None,
            timeout_ms: 5_000,
            max_packet: 1 << 20,
            max_queued: 1024,
        }
    }

    /// Log in with a user name and password.
    pub fn login(mut self, username: &str, password: &str) -> Self {
        self.username = Some(String::from(username));
        self.password = Some(String::from(password));
        self
    }

    /// Seconds between signs of life.
    pub fn keep_alive(mut self, secs: u16) -> Self {
        self.keep_alive_secs = secs;
        self
    }

    /// How long to wait for any answer, in milliseconds.
    pub fn timeout(mut self, ms: u32) -> Self {
        self.timeout_ms = ms.max(1);
        self
    }
}

/// A connection to an MQTT broker.
pub struct Client {
    link: Box<dyn Link>,
    buf: Vec<u8>,
    inbox: VecDeque<Message>,
    next_id: u16,
    keep_alive_ms: u64,
    timeout_ms: u32,
    max_packet: usize,
    max_queued: usize,
    last_sent: u64,
    ping_sent: Option<u64>,
    dropped: u64,
    broken: bool,
    session_present: bool,
}

impl core::fmt::Debug for Client {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Client")
            .field("queued", &self.inbox.len())
            .field("dropped", &self.dropped)
            .field("broken", &self.broken)
            .finish_non_exhaustive()
    }
}

/// What a wait for the broker is looking for: the answer to connecting, or
/// to the publish, subscribe or unsubscribe with this packet id.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Await {
    Connect,
    Publish(u16),
    Subscribe(u16),
    Unsubscribe(u16),
}

enum Got {
    ConnAck { session_present: bool, code: u8 },
    SubAck(Vec<u8>),
    Done,
}

impl Client {
    /// Connect to the broker at `host` on `port` (1883 is MQTT's own),
    /// declared in the manifest as `net.connect:<host>:<port>`.
    pub fn connect(host: &str, port: u16, options: &Options) -> Result<Client, MqttError> {
        let link = TcpLink::connect(host, port, options.timeout_ms)?;
        Client::connect_over(link, options)
    }

    /// Connect over any [`Link`]: a broker stub in a test, a tunnel.
    pub fn connect_over(link: impl Link + 'static, options: &Options) -> Result<Client, MqttError> {
        let hello = packet::connect(
            &options.client_id,
            options.keep_alive_secs,
            options.username.as_deref(),
            options.password.as_deref(),
        )?;
        let mut client = Client {
            link: Box::new(link),
            buf: Vec::new(),
            inbox: VecDeque::new(),
            next_id: 1,
            keep_alive_ms: u64::from(options.keep_alive_secs) * 1000,
            timeout_ms: options.timeout_ms.max(1),
            max_packet: options.max_packet.max(64),
            max_queued: options.max_queued.max(1),
            last_sent: 0,
            ping_sent: None,
            dropped: 0,
            broken: false,
            session_present: false,
        };
        client.send(&hello)?;
        match client.wait_for(Await::Connect, "the answer to connecting")? {
            Got::ConnAck {
                session_present,
                code: 0,
            } => {
                client.session_present = session_present;
                Ok(client)
            }
            Got::ConnAck { code, .. } => Err(MqttError::Refused(match code {
                1 => Refusal::ProtocolVersion,
                2 => Refusal::ClientId,
                3 => Refusal::ServerUnavailable,
                4 => Refusal::BadLogin,
                5 => Refusal::NotAuthorized,
                other => Refusal::Other(other),
            })),
            _ => Err(MqttError::Protocol("no CONNACK")),
        }
    }

    /// Publish `payload` to `topic`. At QoS 1 this waits until the broker
    /// confirms it.
    pub fn publish(&mut self, topic: &str, payload: &[u8], qos: QoS) -> Result<(), MqttError> {
        self.publish_with(topic, payload, qos, false)
    }

    /// Publish and ask the broker to keep it as the topic's last value, for
    /// whoever subscribes later.
    pub fn publish_retained(
        &mut self,
        topic: &str,
        payload: &[u8],
        qos: QoS,
    ) -> Result<(), MqttError> {
        self.publish_with(topic, payload, qos, true)
    }

    fn publish_with(
        &mut self,
        topic: &str,
        payload: &[u8],
        qos: QoS,
        retain: bool,
    ) -> Result<(), MqttError> {
        self.usable()?;
        if payload.len() + topic.len() + 4 > self.max_packet {
            return Err(MqttError::TooLarge);
        }
        let id = self.take_id();
        let out = packet::publish(topic, payload, qos, retain, id)?;
        self.send(&out)?;
        if qos == QoS::AtLeastOnce {
            self.wait_for(Await::Publish(id), "the broker's receipt for a message")?;
        }
        Ok(())
    }

    /// Hear messages on topics matching `filter` (`+` one level, `#` all
    /// below), delivered at most at `qos`. Returns the QoS the broker
    /// granted. Messages arrive through [`Client::poll`].
    pub fn subscribe(&mut self, filter: &str, qos: QoS) -> Result<QoS, MqttError> {
        self.usable()?;
        let id = self.take_id();
        let out = packet::subscribe(id, filter, qos)?;
        self.send(&out)?;
        match self.wait_for(Await::Subscribe(id), "the answer to subscribing")? {
            Got::SubAck(codes) => match codes.first() {
                Some(0) => Ok(QoS::AtMostOnce),
                Some(1) | Some(2) => Ok(QoS::AtLeastOnce.min(qos)),
                _ => Err(MqttError::SubscribeRefused),
            },
            _ => Err(MqttError::Protocol("no SUBACK")),
        }
    }

    /// Stop hearing messages on `filter`.
    pub fn unsubscribe(&mut self, filter: &str) -> Result<(), MqttError> {
        self.usable()?;
        let id = self.take_id();
        let out = packet::unsubscribe(id, filter)?;
        self.send(&out)?;
        self.wait_for(Await::Unsubscribe(id), "the answer to unsubscribing")?;
        Ok(())
    }

    /// The next message that arrived, or `None`; never waits. Also keeps
    /// the connection alive: call it from the app's loop.
    pub fn poll(&mut self) -> Result<Option<Message>, MqttError> {
        self.receive(0)
    }

    /// The next message, waiting up to `wait_ms` for one.
    pub fn receive(&mut self, wait_ms: u32) -> Result<Option<Message>, MqttError> {
        if let Some(message) = self.inbox.pop_front() {
            return Ok(Some(message));
        }
        self.usable()?;
        let deadline = self.link.now_ms() + u64::from(wait_ms);
        loop {
            self.keep_alive()?;
            self.read_some(0)?;
            if let Some(message) = self.inbox.pop_front() {
                return Ok(Some(message));
            }
            let now = self.link.now_ms();
            if now >= deadline {
                return Ok(None);
            }
            let wait = (deadline - now).min(self.until_ping_due(now)).max(1);
            self.read_some(wait.min(u64::from(u32::MAX)) as u32)?;
            if let Some(message) = self.inbox.pop_front() {
                return Ok(Some(message));
            }
        }
    }

    /// Say goodbye and close the connection.
    pub fn disconnect(mut self) {
        if !self.broken {
            let _ = self.link.send(&packet::DISCONNECT);
        }
        self.broken = true;
    }

    /// Messages dropped because the app did not take them fast enough.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Whether the broker kept a session from before (never, with the clean
    /// sessions this client asks for).
    pub fn session_present(&self) -> bool {
        self.session_present
    }

    fn usable(&self) -> Result<(), MqttError> {
        if self.broken {
            return Err(MqttError::Link(LinkError::Closed));
        }
        Ok(())
    }

    fn take_id(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id = if id == u16::MAX { 1 } else { id + 1 };
        id
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), MqttError> {
        if let Err(e) = self.link.send(bytes) {
            self.broken = true;
            return Err(e.into());
        }
        self.last_sent = self.link.now_ms();
        Ok(())
    }

    fn until_ping_due(&self, now: u64) -> u64 {
        if self.keep_alive_ms == 0 {
            return u64::MAX;
        }
        match self.ping_sent {
            Some(at) => (at + u64::from(self.timeout_ms)).saturating_sub(now),
            None => (self.last_sent + self.keep_alive_ms).saturating_sub(now),
        }
    }

    /// Send PINGREQ when nothing has gone out for a keep-alive period, and
    /// call the connection lost when its PINGRESP is late.
    fn keep_alive(&mut self) -> Result<(), MqttError> {
        if self.keep_alive_ms == 0 {
            return Ok(());
        }
        let now = self.link.now_ms();
        match self.ping_sent {
            Some(at) if now.saturating_sub(at) >= u64::from(self.timeout_ms) => {
                self.broken = true;
                Err(MqttError::TimedOut(
                    "a sign of life; the connection is lost",
                ))
            }
            Some(_) => Ok(()),
            None if now.saturating_sub(self.last_sent) >= self.keep_alive_ms => {
                self.send(&packet::PINGREQ)?;
                self.ping_sent = Some(now);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Read what arrived within `wait_ms` and act on every whole packet:
    /// queue messages, confirm QoS 1 ones, note PINGRESP.
    fn read_some(&mut self, wait_ms: u32) -> Result<(), MqttError> {
        self.pump(wait_ms, None).map(|_| ())
    }

    /// Act on what is already here; if the packet `want` names is not among
    /// it, read what arrives within `wait_ms` and act on that too.
    fn pump(&mut self, wait_ms: u32, want: Option<Await>) -> Result<Option<Got>, MqttError> {
        if let Some(got) = self.drain(want)? {
            return Ok(Some(got));
        }
        let got = match self.link.receive(8192, wait_ms) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.broken = true;
                return Err(e.into());
            }
        };
        self.buf.extend_from_slice(&got);
        self.drain(want)
    }

    /// Every whole packet in the buffer, in order. Returns the packet `want`
    /// names, if it was there; what follows it waits for the next call.
    fn drain(&mut self, want: Option<Await>) -> Result<Option<Got>, MqttError> {
        let mut found = None;
        loop {
            let parsed = match packet::parse(&self.buf, self.max_packet) {
                Ok(p) => p,
                Err(e) => {
                    // The stream cannot be trusted past a bad packet.
                    self.broken = true;
                    self.buf.clear();
                    return Err(e);
                }
            };
            let Some((incoming, used)) = parsed else {
                break;
            };
            if found.is_some() {
                break;
            }
            self.buf.drain(..used);
            match incoming {
                Incoming::Publish {
                    topic,
                    payload,
                    qos,
                    retained,
                    packet_id,
                    ..
                } => {
                    if let Some(id) = packet_id {
                        self.send(&packet::puback(id))?;
                    }
                    if self.inbox.len() >= self.max_queued {
                        self.inbox.pop_front();
                        self.dropped += 1;
                    }
                    self.inbox.push_back(Message {
                        topic,
                        payload,
                        qos,
                        retained,
                    });
                }
                Incoming::PingResp => self.ping_sent = None,
                Incoming::ConnAck {
                    session_present,
                    code,
                } if want == Some(Await::Connect) && found.is_none() => {
                    found = Some(Got::ConnAck {
                        session_present,
                        code,
                    });
                }
                Incoming::PubAck { packet_id }
                    if want == Some(Await::Publish(packet_id)) && found.is_none() =>
                {
                    found = Some(Got::Done);
                }
                Incoming::SubAck { packet_id, codes }
                    if want == Some(Await::Subscribe(packet_id)) && found.is_none() =>
                {
                    found = Some(Got::SubAck(codes));
                }
                Incoming::UnsubAck { packet_id }
                    if want == Some(Await::Unsubscribe(packet_id)) && found.is_none() =>
                {
                    found = Some(Got::Done);
                }
                Incoming::ConnAck { .. } => {
                    self.broken = true;
                    return Err(MqttError::Protocol("a second CONNACK"));
                }
                // A late receipt for something that already timed out.
                Incoming::PubAck { .. } | Incoming::SubAck { .. } | Incoming::UnsubAck { .. } => {}
            }
        }
        Ok(found)
    }

    fn wait_for(&mut self, want: Await, what: &'static str) -> Result<Got, MqttError> {
        let deadline = self.link.now_ms() + u64::from(self.timeout_ms);
        loop {
            let now = self.link.now_ms();
            let wait = deadline.saturating_sub(now);
            if let Some(got) = self.pump(wait.min(u64::from(u32::MAX)) as u32, Some(want))? {
                return Ok(got);
            }
            if self.link.now_ms() >= deadline {
                if want == Await::Connect {
                    self.broken = true;
                }
                return Err(MqttError::TimedOut(what));
            }
        }
    }
}

#[cfg(test)]
mod tests;
