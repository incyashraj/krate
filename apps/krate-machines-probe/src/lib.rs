// The smallest app that talks Modbus and MQTT with krate::modbus and
// krate::mqtt (IC-994), so the whole path -- frames, CRCs, MBAP headers,
// exceptions, timeouts, CONNECT, QoS 1 receipts, subscriptions, keep-alive
// -- is proved inside the sandbox, as WebAssembly, with nothing but krate:*
// imports.
//
//   krate check-app apps/krate-machines-probe
//
// The device and the broker are simulated inside the app behind a
// `krate::link::Link`, on a clock of their own, so it needs no serial port,
// no network and no grant beyond stdout. It prints one line per check, then
// `machines=ok` or `machines=FAILED`.
#![no_std]
extern crate alloc;

use alloc::{format, rc::Rc, string::String, vec, vec::Vec};
use core::cell::RefCell;
use krate::{
    io::{stdio, streams::OutputStreamExt},
    link::{Link, LinkError},
    modbus::{self, Exception, ModbusError},
    mqtt::{self, MqttError, QoS},
    Guest,
};

struct Component;

/// What the far end of the line is.
enum Far {
    /// A Modbus device at unit 3, RTU or TCP framing.
    Device {
        tcp: bool,
        holding: [u16; 16],
        damage_crc: bool,
        silent: bool,
    },
    /// An MQTT broker that routes messages back to its one client.
    Broker {
        subs: Vec<String>,
        pings: u32,
        receipts_from_client: u32,
    },
}

struct Bench {
    far: Far,
    line: Vec<u8>,
    clock: u64,
}

#[derive(Clone)]
struct Wire(Rc<RefCell<Bench>>);

fn crc(bytes: &[u8]) -> [u8; 2] {
    modbus::crc16(bytes).to_le_bytes()
}

fn be(b: &[u8], at: usize) -> usize {
    (usize::from(b[at]) << 8) | usize::from(b[at + 1])
}

fn device_answer(holding: &mut [u16; 16], pdu: &[u8]) -> Vec<u8> {
    let (fc, addr) = (pdu[0], be(pdu, 1));
    match fc {
        3 => {
            let n = be(pdu, 3);
            if addr + n > holding.len() {
                return vec![0x83, 2];
            }
            let mut out = vec![3, (n * 2) as u8];
            for v in &holding[addr..addr + n] {
                out.extend_from_slice(&v.to_be_bytes());
            }
            out
        }
        6 if addr < holding.len() => {
            holding[addr] = be(pdu, 3) as u16;
            pdu.to_vec()
        }
        6 => vec![0x86, 2],
        _ => vec![fc | 0x80, 1],
    }
}

fn topic_matches(filter: &str, topic: &str) -> bool {
    let (f, t): (Vec<&str>, Vec<&str>) = (filter.split('/').collect(), topic.split('/').collect());
    for (i, level) in f.iter().enumerate() {
        if *level == "#" {
            return true;
        }
        if t.get(i).is_none_or(|tl| *level != "+" && level != tl) {
            return false;
        }
    }
    f.len() == t.len()
}

fn broker_answer(
    subs: &mut Vec<String>,
    pings: &mut u32,
    receipts: &mut u32,
    pkt: &[u8],
) -> Vec<u8> {
    let body = &pkt[2..]; // every packet here is under 128 bytes
    match pkt[0] >> 4 {
        1 => vec![0x20, 2, 0, 0],
        3 => {
            let qos = (pkt[0] >> 1) & 3;
            let n = be(body, 0);
            let topic = String::from_utf8_lossy(&body[2..2 + n]).into_owned();
            let mut out = Vec::new();
            let mut at = 2 + n;
            if qos == 1 {
                out.extend_from_slice(&[0x40, 2, body[at], body[at + 1]]);
                at += 2;
            }
            if subs.iter().any(|f| topic_matches(f, &topic)) {
                // Back to the client at QoS 1, packet id 9.
                let mut b = Vec::new();
                b.extend_from_slice(&(n as u16).to_be_bytes());
                b.extend_from_slice(topic.as_bytes());
                b.extend_from_slice(&[0, 9]);
                b.extend_from_slice(&body[at..]);
                out.push(0x32);
                out.push(b.len() as u8);
                out.extend(b);
            }
            out
        }
        4 => {
            *receipts += 1;
            Vec::new()
        }
        8 => {
            let n = be(body, 2);
            subs.push(String::from_utf8_lossy(&body[4..4 + n]).into_owned());
            vec![0x90, 3, body[0], body[1], 1]
        }
        12 => {
            *pings += 1;
            vec![0xD0, 0]
        }
        _ => Vec::new(),
    }
}

impl Link for Wire {
    fn send(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        let mut b = self.0.borrow_mut();
        let answer = match &mut b.far {
            Far::Device {
                tcp,
                holding,
                damage_crc,
                silent,
            } => {
                if *silent {
                    Vec::new()
                } else if *tcp {
                    let pdu = device_answer(holding, &bytes[7..]);
                    let mut out = bytes[..4].to_vec();
                    out.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
                    out.push(bytes[6]);
                    out.extend(pdu);
                    out
                } else {
                    let n = bytes.len();
                    if crc(&bytes[..n - 2]) != [bytes[n - 2], bytes[n - 1]] {
                        Vec::new()
                    } else {
                        let mut out = vec![bytes[0]];
                        out.extend(device_answer(holding, &bytes[1..n - 2]));
                        let c = crc(&out);
                        out.extend_from_slice(&c);
                        if *damage_crc {
                            let last = out.len() - 1;
                            out[last] ^= 0x40;
                        }
                        out
                    }
                }
            }
            Far::Broker {
                subs,
                pings,
                receipts_from_client,
            } => broker_answer(subs, pings, receipts_from_client, bytes),
        };
        b.line.extend(answer);
        Ok(())
    }

    fn receive(&mut self, max: usize, wait_ms: u32) -> Result<Vec<u8>, LinkError> {
        let mut b = self.0.borrow_mut();
        if b.line.is_empty() {
            b.clock += u64::from(wait_ms);
            return Ok(Vec::new());
        }
        b.clock += 2; // the device takes a moment
        let n = max.min(b.line.len());
        Ok(b.line.drain(..n).collect())
    }

    fn now_ms(&mut self) -> u64 {
        self.0.borrow().clock
    }
}

fn bench(far: Far) -> (Rc<RefCell<Bench>>, Wire) {
    let b = Rc::new(RefCell::new(Bench {
        far,
        line: Vec::new(),
        clock: 1_000,
    }));
    (b.clone(), Wire(b))
}

fn device(tcp: bool) -> (Rc<RefCell<Bench>>, Wire) {
    let mut holding = [0u16; 16];
    holding[0] = 648; // 64.8 degrees, in tenths
    bench(Far::Device {
        tcp,
        holding,
        damage_crc: false,
        silent: false,
    })
}

fn set_fault(b: &Rc<RefCell<Bench>>, crc: bool, quiet: bool) {
    if let Far::Device {
        damage_crc, silent, ..
    } = &mut b.borrow_mut().far
    {
        *damage_crc = crc;
        *silent = quiet;
    }
}

fn modbus_checks(say: &mut dyn FnMut(&str, bool)) {
    for tcp in [false, true] {
        let label = if tcp { "modbus tcp" } else { "modbus rtu" };
        let (b, wire) = device(tcp);
        let mut plc = if tcp {
            modbus::Client::tcp_over(wire, 3)
        } else {
            modbus::Client::rtu_over(wire, 3)
        };
        let read = plc.read_holding_registers(0, 2);
        say(
            &format!("{label} read 40001 -> {read:?}"),
            read == Ok(vec![648, 0]),
        );
        let wrote = plc
            .write_register(5, 650)
            .and_then(|_| plc.read_holding_registers(5, 1));
        say(
            &format!("{label} write then read -> {wrote:?}"),
            wrote == Ok(vec![650]),
        );
        let refused = plc.read_holding_registers(10, 10);
        say(
            &format!("{label} past the end -> {}", refused.clone().unwrap_err()),
            refused == Err(ModbusError::Exception(Exception::IllegalDataAddress)),
        );
        if !tcp {
            set_fault(&b, true, false);
            let damaged = plc.read_holding_registers(0, 1);
            say(
                &format!("{label} damaged crc -> {damaged:?}"),
                damaged == Err(ModbusError::BadCrc),
            );
        }
        set_fault(&b, false, true);
        plc.set_timeout(300);
        let start = b.borrow().clock;
        let silent = plc.read_holding_registers(0, 1);
        let waited = b.borrow().clock - start;
        say(
            &format!("{label} silent device -> {silent:?} after {waited} ms"),
            silent == Err(ModbusError::TimedOut) && (300..=310).contains(&waited),
        );
    }
    let float = modbus::f32_from(0x4281, 0x999A);
    say(
        &format!("modbus two-register float -> {float}"),
        float == 64.8,
    );
}

fn mqtt_checks(say: &mut dyn FnMut(&str, bool)) {
    let (b, wire) = bench(Far::Broker {
        subs: Vec::new(),
        pings: 0,
        receipts_from_client: 0,
    });
    let options = mqtt::Options::new("oven-7").keep_alive(10);
    let mut broker = match mqtt::Client::connect_over(wire, &options) {
        Ok(c) => c,
        Err(e) => {
            say(&format!("mqtt connect -> {}", e.message()), false);
            return;
        }
    };
    say("mqtt connect -> accepted", true);
    let sub = broker.subscribe("lab/+/celsius", QoS::AtLeastOnce);
    say(
        &format!("mqtt subscribe -> {sub:?}"),
        sub == Ok(QoS::AtLeastOnce),
    );
    let sent = broker.publish("lab/oven-7/celsius", b"64.8", QoS::AtLeastOnce);
    say(&format!("mqtt publish qos1 -> {sent:?}"), sent.is_ok());
    let heard = broker.receive(100);
    let good = matches!(&heard, Ok(Some(m)) if m.topic == "lab/oven-7/celsius" && m.text() == Some("64.8"));
    say(
        &format!(
            "mqtt heard -> {:?}",
            heard
                .as_ref()
                .ok()
                .and_then(|m| m.as_ref())
                .and_then(|m| m.text())
        ),
        good,
    );
    let receipts = match &b.borrow().far {
        Far::Broker {
            receipts_from_client,
            ..
        } => *receipts_from_client,
        _ => 0,
    };
    say(&format!("mqtt receipts sent -> {receipts}"), receipts == 1);
    b.borrow_mut().clock += 11_000;
    let idle = broker.poll();
    let pings = match &b.borrow().far {
        Far::Broker { pings, .. } => *pings,
        _ => 0,
    };
    say(
        &format!("mqtt keep-alive after 11 s idle -> {pings} ping"),
        idle == Ok(None) && pings == 1,
    );
    let bad = broker.publish("lab/#", b"x", QoS::AtMostOnce);
    say(
        &format!("mqtt wildcard topic -> {}", bad.clone().unwrap_err()),
        matches!(bad, Err(MqttError::BadTopic(_))),
    );
    broker.disconnect();
}

impl Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        let mut ok = true;
        let mut say = |line: &str, good: bool| {
            ok &= good;
            let _ = out.write_line(&format!("{} {line}", if good { "ok  " } else { "FAIL" }));
            let _ = out.flush();
        };
        modbus_checks(&mut say);
        mqtt_checks(&mut say);
        let _ = out.write_line(if ok { "machines=ok" } else { "machines=FAILED" });
        let _ = out.flush();
        if ok {
            0
        } else {
            1
        }
    }
}

krate::export!(Component);
