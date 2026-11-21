//! MQTT 3.1.1 packets, byte for byte.
//!
//! [`Client`](super::Client) uses these; they are public for an app that
//! runs its own loop. Nothing here panics on any input: a packet that does
//! not make sense is an error with words.

use super::{MqttError, QoS};
use alloc::{string::String, vec::Vec};

/// The largest remaining length MQTT can say (four length bytes).
pub const MAX_REMAINING: usize = 268_435_455;

/// A packet a broker sends a client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// The answer to CONNECT: whether a session was kept, and the return
    /// code (0 accepted).
    ConnAck { session_present: bool, code: u8 },
    /// A message on a topic the client subscribed to.
    Publish {
        topic: String,
        payload: Vec<u8>,
        qos: QoS,
        retained: bool,
        duplicate: bool,
        /// Present for QoS 1.
        packet_id: Option<u16>,
    },
    /// A QoS 1 publish arrived at the broker.
    PubAck { packet_id: u16 },
    /// The answer to SUBSCRIBE: one code a filter (0, 1, 2 granted, 0x80
    /// refused).
    SubAck { packet_id: u16, codes: Vec<u8> },
    /// The answer to UNSUBSCRIBE.
    UnsubAck { packet_id: u16 },
    /// The answer to PINGREQ.
    PingResp,
}

/// A topic name or filter as MQTT's UTF-8 string: no NUL, at most 65535
/// bytes, not empty.
fn check_string(s: &str, what: &'static str) -> Result<(), MqttError> {
    if s.is_empty() || s.len() > 65_535 || s.contains('\0') {
        return Err(MqttError::BadTopic(what));
    }
    Ok(())
}

/// A topic a message is published to: no wildcards.
pub fn check_topic(topic: &str) -> Result<(), MqttError> {
    check_string(topic, "a topic is 1 to 65535 bytes with no NUL")?;
    if topic.contains(['+', '#']) {
        return Err(MqttError::BadTopic(
            "a topic to publish to has no + or # in it",
        ));
    }
    Ok(())
}

/// A filter to subscribe with: `+` stands for one whole level, `#` for every
/// level below and only at the end.
pub fn check_filter(filter: &str) -> Result<(), MqttError> {
    check_string(filter, "a filter is 1 to 65535 bytes with no NUL")?;
    let levels: Vec<&str> = filter.split('/').collect();
    for (i, level) in levels.iter().enumerate() {
        if level.contains('#') && (*level != "#" || i + 1 != levels.len()) {
            return Err(MqttError::BadTopic("# stands alone, as the last level"));
        }
        if level.contains('+') && *level != "+" {
            return Err(MqttError::BadTopic("+ stands alone, as a whole level"));
        }
    }
    Ok(())
}

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

/// A fixed header and body as one packet.
fn packet(first: u8, body: &[u8]) -> Result<Vec<u8>, MqttError> {
    if body.len() > MAX_REMAINING {
        return Err(MqttError::TooLarge);
    }
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(first);
    let mut n = body.len();
    loop {
        let mut byte = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(body);
    Ok(out)
}

/// CONNECT with a clean session.
pub fn connect(
    client_id: &str,
    keep_alive_secs: u16,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<Vec<u8>, MqttError> {
    if client_id.len() > 65_535 || client_id.contains('\0') {
        return Err(MqttError::BadOptions(
            "a client id is at most 65535 bytes with no NUL",
        ));
    }
    if password.is_some() && username.is_none() {
        return Err(MqttError::BadOptions(
            "MQTT 3.1.1 sends a password only with a user name",
        ));
    }
    let mut flags = 0x02; // clean session
    let mut body = Vec::new();
    put_str(&mut body, b"MQTT");
    body.push(4); // protocol level: 3.1.1
    if username.is_some() {
        flags |= 0x80;
    }
    if password.is_some() {
        flags |= 0x40;
    }
    body.push(flags);
    body.extend_from_slice(&keep_alive_secs.to_be_bytes());
    put_str(&mut body, client_id.as_bytes());
    for field in [username, password].into_iter().flatten() {
        if field.len() > 65_535 {
            return Err(MqttError::BadOptions(
                "a user name or password is at most 65535 bytes",
            ));
        }
        put_str(&mut body, field.as_bytes());
    }
    packet(0x10, &body)
}

/// PUBLISH. `packet_id` is required for QoS 1 and ignored for QoS 0.
pub fn publish(
    topic: &str,
    payload: &[u8],
    qos: QoS,
    retain: bool,
    packet_id: u16,
) -> Result<Vec<u8>, MqttError> {
    check_topic(topic)?;
    let mut body = Vec::with_capacity(topic.len() + payload.len() + 4);
    put_str(&mut body, topic.as_bytes());
    if qos == QoS::AtLeastOnce {
        body.extend_from_slice(&packet_id.to_be_bytes());
    }
    body.extend_from_slice(payload);
    packet(0x30 | ((qos as u8) << 1) | u8::from(retain), &body)
}

/// PUBACK for a QoS 1 message the client received.
pub fn puback(packet_id: u16) -> Vec<u8> {
    let id = packet_id.to_be_bytes();
    alloc::vec![0x40, 0x02, id[0], id[1]]
}

/// SUBSCRIBE to one filter at most at `qos`.
pub fn subscribe(packet_id: u16, filter: &str, qos: QoS) -> Result<Vec<u8>, MqttError> {
    check_filter(filter)?;
    let mut body = Vec::with_capacity(filter.len() + 5);
    body.extend_from_slice(&packet_id.to_be_bytes());
    put_str(&mut body, filter.as_bytes());
    body.push(qos as u8);
    packet(0x82, &body)
}

/// UNSUBSCRIBE from one filter.
pub fn unsubscribe(packet_id: u16, filter: &str) -> Result<Vec<u8>, MqttError> {
    check_filter(filter)?;
    let mut body = Vec::with_capacity(filter.len() + 4);
    body.extend_from_slice(&packet_id.to_be_bytes());
    put_str(&mut body, filter.as_bytes());
    packet(0xA2, &body)
}

/// PINGREQ.
pub const PINGREQ: [u8; 2] = [0xC0, 0x00];
/// DISCONNECT.
pub const DISCONNECT: [u8; 2] = [0xE0, 0x00];

fn id_of(body: &[u8]) -> Result<u16, MqttError> {
    match body {
        [a, b] => Ok(u16::from_be_bytes([*a, *b])),
        _ => Err(MqttError::Protocol(
            "an acknowledgement of the wrong length",
        )),
    }
}

/// Read one packet from the start of `buf`: `Ok(None)` until the whole
/// packet has arrived, else the packet and how many bytes it took. A packet
/// longer than `max_packet` is an error before any of it is kept.
pub fn parse(buf: &[u8], max_packet: usize) -> Result<Option<(Incoming, usize)>, MqttError> {
    let Some(&first) = buf.first() else {
        return Ok(None);
    };
    let mut remaining: usize = 0;
    let mut header = 1;
    loop {
        let Some(&byte) = buf.get(header) else {
            return Ok(None);
        };
        remaining |= usize::from(byte & 0x7F) << (7 * (header - 1));
        header += 1;
        if byte & 0x80 == 0 {
            break;
        }
        if header == 5 {
            return Err(MqttError::Protocol(
                "a packet length longer than four bytes",
            ));
        }
    }
    if remaining > max_packet {
        return Err(MqttError::TooLarge);
    }
    let total = header + remaining;
    if buf.len() < total {
        return Ok(None);
    }
    let body = &buf[header..total];
    let flags = first & 0x0F;
    let packet = match first >> 4 {
        2 => match body {
            [ack, code] if flags == 0 && ack & 0xFE == 0 => Incoming::ConnAck {
                session_present: ack & 1 == 1,
                code: *code,
            },
            _ => return Err(MqttError::Protocol("a malformed CONNACK")),
        },
        3 => {
            let qos = match (flags >> 1) & 3 {
                0 => QoS::AtMostOnce,
                1 => QoS::AtLeastOnce,
                _ => {
                    return Err(MqttError::Protocol(
                        "a QoS 2 message, which this client never subscribes to",
                    ))
                }
            };
            if body.len() < 2 {
                return Err(MqttError::Protocol("a message with no topic"));
            }
            let topic_len = usize::from(u16::from_be_bytes([body[0], body[1]]));
            let mut at = 2 + topic_len;
            let Some(topic) = body.get(2..at) else {
                return Err(MqttError::Protocol("a message cut short in its topic"));
            };
            let Ok(topic) = core::str::from_utf8(topic) else {
                return Err(MqttError::Protocol("a message whose topic is not UTF-8"));
            };
            if topic.is_empty() || topic.contains(['\0', '+', '#']) {
                return Err(MqttError::Protocol("a message with an impossible topic"));
            }
            let packet_id = if qos == QoS::AtLeastOnce {
                let Some(id) = body.get(at..at + 2) else {
                    return Err(MqttError::Protocol("a message cut short in its id"));
                };
                at += 2;
                let id = u16::from_be_bytes([id[0], id[1]]);
                if id == 0 {
                    return Err(MqttError::Protocol("a message with packet id 0"));
                }
                Some(id)
            } else {
                None
            };
            Incoming::Publish {
                topic: String::from(topic),
                payload: body[at..].to_vec(),
                qos,
                retained: flags & 1 == 1,
                duplicate: flags & 8 == 8,
                packet_id,
            }
        }
        4 if flags == 0 => Incoming::PubAck {
            packet_id: id_of(body)?,
        },
        9 if flags == 0 => {
            if body.len() < 3 {
                return Err(MqttError::Protocol("a malformed SUBACK"));
            }
            Incoming::SubAck {
                packet_id: u16::from_be_bytes([body[0], body[1]]),
                codes: body[2..].to_vec(),
            }
        }
        11 if flags == 0 => Incoming::UnsubAck {
            packet_id: id_of(body)?,
        },
        13 if flags == 0 && body.is_empty() => Incoming::PingResp,
        _ => {
            return Err(MqttError::Protocol(
                "a packet a broker does not send to a client",
            ))
        }
    };
    Ok(Some((packet, total)))
}
