//! An MQTT 3.1.1 broker in the test, byte for byte (IC-994).
//!
//! The broker below reads the client's packets on its own (it does not use
//! `packet.rs`), keeps subscriptions, routes messages -- the client's own and
//! ones "another client" publishes -- confirms QoS 1, answers pings, and can
//! be told to misbehave: drop a receipt, stop answering pings, refuse a
//! login or a filter, send a packet too large, close. The link runs on a
//! clock of its own, so keep-alive and timeouts are measured, not slept.

extern crate std;

use super::*;
use crate::link::{Link, LinkError};
use alloc::{rc::Rc, string::String, vec, vec::Vec};
use core::cell::RefCell;
use std::collections::VecDeque;

#[derive(Default)]
struct Broker {
    /// A login the broker insists on.
    login: Option<(String, String)>,
    drop_puback: bool,
    mute_pings: bool,
    closed: bool,
    subs: Vec<(String, u8)>,
    next_id: u16,
    /// What the broker saw, packet by packet (type, body).
    seen: Vec<(u8, Vec<u8>)>,
    published: Vec<(String, Vec<u8>, u8, bool)>,
    pubacks_from_client: Vec<u16>,
    pings: usize,
    connect: Option<ConnectSeen>,
}

#[derive(Clone, Debug, Default)]
struct ConnectSeen {
    flags: u8,
    keep_alive: u16,
    client_id: String,
    username: Option<String>,
    password: Option<String>,
}

fn matches(filter: &str, topic: &str) -> bool {
    let f: Vec<&str> = filter.split('/').collect();
    let t: Vec<&str> = topic.split('/').collect();
    for (i, level) in f.iter().enumerate() {
        if *level == "#" {
            return true;
        }
        match t.get(i) {
            Some(tl) if *level == "+" || level == tl => {}
            _ => return false,
        }
    }
    f.len() == t.len()
}

fn frame(first: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![first];
    let mut n = body.len();
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(body);
    out
}

fn read_str(body: &[u8], at: &mut usize) -> String {
    let n = ((body[*at] as usize) << 8) | body[*at + 1] as usize;
    let s = String::from_utf8(body[*at + 2..*at + 2 + n].to_vec()).unwrap();
    *at += 2 + n;
    s
}

impl Broker {
    /// A message from "another client" to whoever subscribed.
    fn deliver(&mut self, topic: &str, payload: &[u8], qos: u8, retain: bool) -> Vec<u8> {
        let granted = self
            .subs
            .iter()
            .filter(|(f, _)| matches(f, topic))
            .map(|(_, q)| *q)
            .max();
        let Some(granted) = granted else {
            return Vec::new();
        };
        let qos = qos.min(granted);
        let mut body = Vec::new();
        body.extend((topic.len() as u16).to_be_bytes());
        body.extend(topic.as_bytes());
        if qos == 1 {
            self.next_id += 1;
            body.extend(self.next_id.to_be_bytes());
        }
        body.extend(payload);
        frame(0x30 | (qos << 1) | retain as u8, &body)
    }

    /// Whole packets in; what the broker sends back.
    fn take(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let first = bytes[at];
            let (mut len, mut mul, mut i) = (0usize, 1usize, at + 1);
            loop {
                let b = bytes[i];
                len += (b & 0x7F) as usize * mul;
                mul *= 128;
                i += 1;
                if b & 0x80 == 0 {
                    break;
                }
            }
            let body = &bytes[i..i + len];
            at = i + len;
            self.seen.push((first >> 4, body.to_vec()));
            match first >> 4 {
                1 => {
                    let mut p = 0;
                    assert_eq!(read_str(body, &mut p), "MQTT");
                    assert_eq!(body[p], 4, "protocol level 3.1.1");
                    let flags = body[p + 1];
                    let keep_alive = u16::from_be_bytes([body[p + 2], body[p + 3]]);
                    p += 4;
                    let client_id = read_str(body, &mut p);
                    let username = (flags & 0x80 != 0).then(|| read_str(body, &mut p));
                    let password = (flags & 0x40 != 0).then(|| read_str(body, &mut p));
                    assert_eq!(p, body.len(), "CONNECT has trailing bytes");
                    let ok = match &self.login {
                        None => true,
                        Some((u, pw)) => {
                            username.as_deref() == Some(u) && password.as_deref() == Some(pw)
                        }
                    };
                    self.connect = Some(ConnectSeen {
                        flags,
                        keep_alive,
                        client_id,
                        username,
                        password,
                    });
                    out.extend([0x20, 0x02, 0x00, if ok { 0 } else { 4 }]);
                }
                3 => {
                    let qos = (first >> 1) & 3;
                    let retain = first & 1 == 1;
                    let mut p = 0;
                    let topic = read_str(body, &mut p);
                    let id = (qos == 1).then(|| {
                        let id = u16::from_be_bytes([body[p], body[p + 1]]);
                        p += 2;
                        id
                    });
                    let payload = body[p..].to_vec();
                    self.published
                        .push((topic.clone(), payload.clone(), qos, retain));
                    if let Some(id) = id {
                        if !self.drop_puback {
                            out.extend([0x40, 0x02]);
                            out.extend(id.to_be_bytes());
                        }
                    }
                    let routed = self.deliver(&topic, &payload, qos, false);
                    out.extend(routed);
                }
                4 => self
                    .pubacks_from_client
                    .push(u16::from_be_bytes([body[0], body[1]])),
                8 => {
                    assert_eq!(first & 0x0F, 0x02, "SUBSCRIBE flags");
                    let id = [body[0], body[1]];
                    let mut p = 2;
                    let filter = read_str(body, &mut p);
                    let qos = body[p];
                    let code = if filter.starts_with("forbidden") {
                        0x80
                    } else {
                        self.subs.push((filter, qos.min(1)));
                        qos.min(1)
                    };
                    out.extend([0x90, 0x03, id[0], id[1], code]);
                }
                10 => {
                    assert_eq!(first & 0x0F, 0x02, "UNSUBSCRIBE flags");
                    let mut p = 2;
                    let filter = read_str(body, &mut p);
                    self.subs.retain(|(f, _)| *f != filter);
                    out.extend([0xB0, 0x02, body[0], body[1]]);
                }
                12 => {
                    self.pings += 1;
                    if !self.mute_pings {
                        out.extend([0xD0, 0x00]);
                    }
                }
                14 => self.closed = true,
                other => panic!("the client sent packet type {other}"),
            }
        }
        out
    }
}

struct Wire {
    broker: Broker,
    line: VecDeque<u8>,
    clock: u64,
    chunk: usize,
    latency: u64,
    ready_at: u64,
    hang_up: bool,
}

#[derive(Clone)]
struct FakeLink(Rc<RefCell<Wire>>);

impl Link for FakeLink {
    fn send(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        let mut w = self.0.borrow_mut();
        if w.hang_up {
            return Err(LinkError::Closed);
        }
        let back = w.broker.take(bytes);
        if !back.is_empty() {
            w.line.extend(back);
            w.ready_at = w.clock + w.latency;
        }
        Ok(())
    }

    fn receive(&mut self, max: usize, wait_ms: u32) -> Result<Vec<u8>, LinkError> {
        let mut w = self.0.borrow_mut();
        if w.line.is_empty() && w.hang_up {
            return Err(LinkError::Closed);
        }
        if w.line.is_empty() || w.ready_at > w.clock + u64::from(wait_ms) {
            w.clock += u64::from(wait_ms);
            return Ok(Vec::new());
        }
        w.clock = w.clock.max(w.ready_at);
        let n = max.min(w.chunk).min(w.line.len());
        Ok(w.line.drain(..n).collect())
    }

    fn now_ms(&mut self) -> u64 {
        self.0.borrow().clock
    }
}

fn wire() -> (Rc<RefCell<Wire>>, FakeLink) {
    let w = Rc::new(RefCell::new(Wire {
        broker: Broker::default(),
        line: VecDeque::new(),
        clock: 5_000,
        chunk: 4096,
        latency: 15,
        ready_at: 0,
        hang_up: false,
    }));
    (w.clone(), FakeLink(w))
}

fn connected() -> (Rc<RefCell<Wire>>, Client) {
    let (w, link) = wire();
    let c = Client::connect_over(link, &Options::new("oven-7")).unwrap();
    (w, c)
}

/// "Another client" publishes; the broker routes it to the line.
fn from_elsewhere(w: &Rc<RefCell<Wire>>, topic: &str, payload: &[u8], qos: u8, retain: bool) {
    let mut w = w.borrow_mut();
    let bytes = w.broker.deliver(topic, payload, qos, retain);
    w.line.extend(bytes);
    w.ready_at = w.clock;
}

#[test]
fn connect_sends_a_clean_3_1_1_session() {
    let (w, c) = connected();
    let seen = w.borrow().broker.connect.clone().unwrap();
    assert_eq!(seen.flags, 0x02, "clean session, no login");
    assert_eq!(seen.keep_alive, 60);
    assert_eq!(seen.client_id, "oven-7");
    assert!(!c.session_present());
    // Exact bytes for the smallest CONNECT.
    assert_eq!(
        packet::connect("", 0, None, None).unwrap(),
        [0x10, 12, 0, 4, b'M', b'Q', b'T', b'T', 4, 0x02, 0, 0, 0, 0]
    );
}

#[test]
fn a_login_is_sent_and_a_wrong_one_is_refused_with_words() {
    let (w, link) = wire();
    w.borrow_mut().broker.login = Some((String::from("lab"), String::from("s3cret")));
    let ok = Client::connect_over(link.clone(), &Options::new("a").login("lab", "s3cret"));
    assert!(ok.is_ok());
    let seen = w.borrow().broker.connect.clone().unwrap();
    assert_eq!(seen.flags, 0xC2);
    assert_eq!(seen.username.as_deref(), Some("lab"));
    assert_eq!(seen.password.as_deref(), Some("s3cret"));

    let e = Client::connect_over(link, &Options::new("a").login("lab", "wrong")).unwrap_err();
    assert_eq!(e, MqttError::Refused(Refusal::BadLogin));
    assert!(e.message().contains("user name or password is wrong"));

    let mut no_user = Options::new("a");
    no_user.password = Some(String::from("x"));
    let (_, link) = wire();
    assert!(matches!(
        Client::connect_over(link, &no_user),
        Err(MqttError::BadOptions(_))
    ));
}

#[test]
fn a_silent_broker_times_out_connecting() {
    let (w, link) = wire();
    w.borrow_mut().latency = 60_000;
    let start = w.borrow().clock;
    let e = Client::connect_over(link, &Options::new("x").timeout(2_000)).unwrap_err();
    assert_eq!(e, MqttError::TimedOut("the answer to connecting"));
    let spent = w.borrow().clock - start;
    assert!((2_000..=2_010).contains(&spent), "waited {spent} ms");
}

#[test]
fn qos0_goes_once_without_a_receipt() {
    let (w, mut c) = connected();
    c.publish("lab/oven-7/celsius", b"64.8", QoS::AtMostOnce)
        .unwrap();
    let b = &w.borrow().broker;
    assert_eq!(
        b.published,
        [(
            String::from("lab/oven-7/celsius"),
            b"64.8".to_vec(),
            0,
            false
        )]
    );
}

#[test]
fn qos1_waits_for_the_brokers_receipt() {
    let (w, mut c) = connected();
    c.publish("lab/oven-7/celsius", b"64.8", QoS::AtLeastOnce)
        .unwrap();
    c.publish_retained("lab/oven-7/state", b"on", QoS::AtLeastOnce)
        .unwrap();
    let b = &w.borrow().broker;
    assert_eq!(b.published.len(), 2);
    assert_eq!(b.published[0].2, 1);
    assert!(b.published[1].3, "retain flag");
    // Two publishes, two different packet ids.
    let ids: Vec<u16> = b
        .seen
        .iter()
        .filter(|(t, _)| *t == 3)
        .map(|(_, body)| {
            let n = ((body[0] as usize) << 8) | body[1] as usize;
            u16::from_be_bytes([body[2 + n], body[3 + n]])
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert!(ids.iter().all(|&id| id != 0));
}

#[test]
fn qos1_without_a_receipt_times_out() {
    let (w, mut c) = connected();
    w.borrow_mut().broker.drop_puback = true;
    let start = w.borrow().clock;
    let e = c
        .publish("lab/oven-7/celsius", b"64.8", QoS::AtLeastOnce)
        .unwrap_err();
    assert_eq!(e, MqttError::TimedOut("the broker's receipt for a message"));
    let spent = w.borrow().clock - start;
    assert!((5_000..=5_010).contains(&spent), "waited {spent} ms");
    // A receipt for some other packet does not count either.
    w.borrow_mut().broker.drop_puback = false;
    w.borrow_mut().line.extend([0x40, 0x02, 0x7F, 0x7F]);
    c.publish("t", b"x", QoS::AtLeastOnce).unwrap();
}

#[test]
fn subscribe_and_receive_messages_confirming_qos1() {
    let (w, mut c) = connected();
    assert_eq!(
        c.subscribe("lab/+/setpoint", QoS::AtLeastOnce).unwrap(),
        QoS::AtLeastOnce
    );
    assert_eq!(c.poll().unwrap(), None);
    from_elsewhere(&w, "lab/oven-7/setpoint", b"65.0", 1, true);
    from_elsewhere(&w, "lab/oven-9/setpoint", b"40", 0, false);
    from_elsewhere(&w, "lab/oven-7/other", b"no", 1, false);
    let m = c.poll().unwrap().unwrap();
    assert_eq!(m.topic, "lab/oven-7/setpoint");
    assert_eq!(m.text(), Some("65.0"));
    assert_eq!(m.qos, QoS::AtLeastOnce);
    assert!(m.retained);
    let m = c.poll().unwrap().unwrap();
    assert_eq!(m.topic, "lab/oven-9/setpoint");
    assert_eq!(m.qos, QoS::AtMostOnce);
    assert_eq!(c.poll().unwrap(), None);
    // The QoS 1 message was confirmed to the broker with its own id.
    assert_eq!(w.borrow().broker.pubacks_from_client, [1]);

    // Its own publish comes back through the subscription.
    c.subscribe("echo/#", QoS::AtMostOnce).unwrap();
    c.publish("echo/a/b", b"hi", QoS::AtLeastOnce).unwrap();
    let m = c.receive(100).unwrap().unwrap();
    assert_eq!(
        (m.topic.as_str(), m.payload.as_slice()),
        ("echo/a/b", &b"hi"[..])
    );

    c.unsubscribe("echo/#").unwrap();
    c.publish("echo/a/b", b"again", QoS::AtMostOnce).unwrap();
    assert_eq!(c.receive(100).unwrap(), None);
}

#[test]
fn messages_that_arrive_while_waiting_for_a_receipt_are_kept() {
    let (w, mut c) = connected();
    c.subscribe("#", QoS::AtLeastOnce).unwrap();
    from_elsewhere(&w, "a", b"1", 1, false);
    c.publish("b", b"2", QoS::AtLeastOnce).unwrap();
    assert_eq!(c.poll().unwrap().unwrap().topic, "a");
    assert_eq!(c.poll().unwrap().unwrap().topic, "b");
}

#[test]
fn a_refused_subscription_is_an_error() {
    let (_, mut c) = connected();
    assert_eq!(
        c.subscribe("forbidden/x", QoS::AtLeastOnce),
        Err(MqttError::SubscribeRefused)
    );
    // The connection is still good.
    c.publish("t", b"x", QoS::AtLeastOnce).unwrap();
}

#[test]
fn keep_alive_pings_and_a_silent_broker_is_noticed() {
    let (w, link) = wire();
    let mut c = Client::connect_over(link, &Options::new("k").keep_alive(10)).unwrap();
    // Nine seconds idle: nothing to say yet.
    w.borrow_mut().clock += 9_000;
    assert_eq!(c.poll().unwrap(), None);
    assert_eq!(w.borrow().broker.pings, 0);
    // Past ten: a PINGREQ, and the PINGRESP clears it.
    w.borrow_mut().clock += 1_500;
    assert_eq!(c.poll().unwrap(), None);
    assert_eq!(w.borrow().broker.pings, 1);
    assert_eq!(c.receive(100).unwrap(), None);
    assert!(c.ping_sent.is_none(), "PINGRESP not taken");
    // A long wait pings by itself, on time.
    assert_eq!(c.receive(25_000).unwrap(), None);
    assert_eq!(w.borrow().broker.pings, 3);
    // The broker goes quiet: the next ping goes unanswered, and after the
    // timeout the connection is called lost.
    w.borrow_mut().broker.mute_pings = true;
    w.borrow_mut().clock += 11_000;
    assert_eq!(c.poll().unwrap(), None);
    w.borrow_mut().clock += 5_000;
    let e = c.poll().unwrap_err();
    assert!(matches!(e, MqttError::TimedOut(_)), "{e:?}");
    assert_eq!(
        c.publish("t", b"x", QoS::AtMostOnce),
        Err(MqttError::Link(LinkError::Closed))
    );
}

#[test]
fn bad_topics_and_filters_never_reach_the_broker() {
    let (w, mut c) = connected();
    let before = w.borrow().broker.seen.len();
    for t in ["", "a/+", "a/#", "nul\0"] {
        assert!(matches!(
            c.publish(t, b"x", QoS::AtMostOnce),
            Err(MqttError::BadTopic(_))
        ));
    }
    for f in ["", "a/#/b", "a/b#", "a+/b", "x\0"] {
        assert!(
            matches!(c.subscribe(f, QoS::AtMostOnce), Err(MqttError::BadTopic(_))),
            "{f}"
        );
    }
    assert_eq!(w.borrow().broker.seen.len(), before);
    for f in ["#", "+", "a/+/b", "a/#", "+/+/#"] {
        c.subscribe(f, QoS::AtMostOnce).unwrap();
    }
}

#[test]
fn a_packet_past_the_limit_is_refused_before_it_is_kept() {
    let (w, link) = wire();
    let mut options = Options::new("x");
    options.max_packet = 1024;
    let mut c = Client::connect_over(link, &options).unwrap();
    // A header that announces 200 MB arrives; nothing that size is kept.
    w.borrow_mut().line.extend([0x30, 0xFF, 0xFF, 0xFF, 0x5F]);
    assert_eq!(c.poll(), Err(MqttError::TooLarge));
    assert!(c.buf.capacity() < 64 * 1024);
    assert!(matches!(
        c.publish("t", &[0u8; 2000], QoS::AtMostOnce),
        Err(MqttError::TooLarge) | Err(MqttError::Link(_))
    ));
}

#[test]
fn a_closed_connection_says_so() {
    let (w, mut c) = connected();
    w.borrow_mut().hang_up = true;
    assert_eq!(c.poll(), Err(MqttError::Link(LinkError::Closed)));
    assert!(c.poll().is_err());
}

#[test]
fn the_queue_is_bounded_and_counts_what_it_dropped() {
    let (w, link) = wire();
    let mut options = Options::new("x");
    options.max_queued = 4;
    let mut c = Client::connect_over(link, &options).unwrap();
    c.subscribe("t/#", QoS::AtMostOnce).unwrap();
    for i in 0..10u8 {
        from_elsewhere(&w, "t/x", &[i], 0, false);
    }
    // A wait for a receipt reads all ten while the app is not looking.
    c.publish("other", b"x", QoS::AtLeastOnce).unwrap();
    assert_eq!(c.dropped(), 6);
    let kept: Vec<u8> = core::iter::from_fn(|| c.poll().unwrap())
        .map(|m| m.payload[0])
        .collect();
    assert_eq!(kept, [6, 7, 8, 9]);
}

#[test]
fn packets_split_byte_by_byte_still_arrive_whole() {
    let (w, link) = wire();
    w.borrow_mut().chunk = 1;
    let mut c = Client::connect_over(link, &Options::new("x")).unwrap();
    c.subscribe("t", QoS::AtLeastOnce).unwrap();
    from_elsewhere(&w, "t", &[7u8; 300], 1, false);
    let m = c.receive(1_000).unwrap().unwrap();
    assert_eq!(m.payload.len(), 300);
}

#[test]
fn disconnect_says_goodbye() {
    let (w, c) = connected();
    c.disconnect();
    assert!(w.borrow().broker.closed);
}

#[test]
fn large_lengths_encode_in_four_bytes() {
    let p = packet::publish("t", &vec![1u8; 2_097_152], QoS::AtMostOnce, false, 0).unwrap();
    // 3 + 2_097_152 = 0x200003: four length bytes.
    assert_eq!(&p[..4], &[0x30, 0x83, 0x80, 0x80]);
    let (back, used) = packet::parse(&p, 4 << 20).unwrap().unwrap();
    assert_eq!(used, p.len());
    assert!(
        matches!(back, packet::Incoming::Publish { ref payload, .. } if payload.len() == 2_097_152)
    );
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// A link that answers CONNECT properly and then only noise.
struct Noise {
    rng: Rng,
    clock: u64,
    hello: bool,
}

impl Link for Noise {
    fn send(&mut self, _: &[u8]) -> Result<(), LinkError> {
        Ok(())
    }
    fn receive(&mut self, max: usize, wait_ms: u32) -> Result<Vec<u8>, LinkError> {
        self.clock += 1 + u64::from(wait_ms.min(50));
        if !self.hello {
            self.hello = true;
            return Ok(vec![0x20, 0x02, 0x00, 0x00]);
        }
        let n = (self.rng.next() % 64) as usize;
        Ok((0..n.min(max))
            .map(|_| {
                // Mostly the types a broker sends, so the parsers behind the
                // header are reached.
                let b = self.rng.next() as u8;
                if self.rng.next().is_multiple_of(4) {
                    [0x20, 0x30, 0x32, 0x40, 0x90, 0xB0, 0xD0, 0x00][(b % 8) as usize]
                } else {
                    b
                }
            })
            .collect())
    }
    fn now_ms(&mut self) -> u64 {
        self.clock
    }
}

/// The parser, fed random bytes, answers with an error, "not yet" or a
/// packet no longer than what it was given -- never a panic -- and a client
/// on a line of noise always comes back.
#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for _ in 0..50_000 {
        let n = (rng.next() % 200) as usize;
        let mut bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        if let Some(first) = bytes.first_mut() {
            *first =
                [0x20, 0x30, 0x32, 0x34, 0x40, 0x90, 0xB0, 0xD0, *first][(rng.next() % 9) as usize];
        }
        if bytes.len() > 1 && rng.next().is_multiple_of(2) {
            bytes[1] = (rng.next() % (bytes.len() as u64)) as u8 & 0x7F;
        }
        if let Ok(Some((_, used))) = packet::parse(&bytes, 1 << 20) {
            assert!(used <= bytes.len());
        }
        let _ = packet::check_filter(core::str::from_utf8(&bytes).unwrap_or("x"));
    }
    for seed in 1..300u64 {
        let link = Noise {
            rng: Rng(seed),
            clock: 0,
            hello: false,
        };
        let Ok(mut c) = Client::connect_over(link, &Options::new("n").keep_alive(1)) else {
            continue;
        };
        for _ in 0..50 {
            if c.receive(20).is_err() {
                break;
            }
        }
        let _ = c.subscribe("a/#", QoS::AtLeastOnce);
        let _ = c.publish("a", b"x", QoS::AtLeastOnce);
        assert!(c.buf.len() <= (1 << 20) + 8192);
    }
}
