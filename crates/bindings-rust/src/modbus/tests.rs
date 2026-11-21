//! A simulated Modbus device, byte for byte (IC-994).
//!
//! The device below is written from the Modbus specification on its own,
//! not from `frame.rs`: it checks the CRC, answers RTU and MBAP frames,
//! raises exceptions, and can be told to misbehave -- a damaged checksum,
//! silence, an answer cut short, an answer from another device, noise on
//! the line, a late answer to an earlier request, bytes one at a time. The
//! link runs on a clock of its own, so a timeout is measured, not slept.

extern crate std;

use super::*;
use crate::link::{Link, LinkError};
use alloc::{rc::Rc, vec, vec::Vec};
use core::cell::RefCell;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq)]
enum Fault {
    None,
    /// Flip a bit in the answer's CRC.
    BadCrc,
    /// Answer nothing.
    Silent,
    /// Send only the first n bytes of the answer.
    Short(usize),
    /// Answer as another unit address.
    WrongUnit,
    /// Send an MBAP length of 0x0400.
    ImpossibleLength,
    /// Before the answer, a whole answer to an older transaction.
    StaleFirst,
    /// Answer with random bytes from this seed.
    Noise(u64),
}

struct Device {
    unit: u8,
    coils: [bool; 64],
    discrete: [bool; 64],
    holding: [u16; 64],
    input: [u16; 64],
    fault: Fault,
    seen: Vec<Vec<u8>>,
}

/// CRC-16/Modbus by a lookup-free shift, written apart from `frame::crc16`.
fn device_crc(bytes: &[u8]) -> [u8; 2] {
    let mut crc = 0xFFFFu16;
    for b in bytes {
        crc ^= *b as u16;
        let mut i = 0;
        while i < 8 {
            let lsb = crc & 1;
            crc >>= 1;
            if lsb == 1 {
                crc ^= 0xA001;
            }
            i += 1;
        }
    }
    [(crc & 0xFF) as u8, (crc >> 8) as u8]
}

impl Device {
    fn new(unit: u8) -> Self {
        let mut d = Device {
            unit,
            coils: [false; 64],
            discrete: [false; 64],
            holding: [0; 64],
            input: [0; 64],
            fault: Fault::None,
            seen: Vec::new(),
        };
        for i in 0..64 {
            d.holding[i] = 1000 + i as u16;
            d.input[i] = 2000 + i as u16;
            d.discrete[i] = i % 3 == 0;
        }
        d
    }

    fn be(pdu: &[u8], at: usize) -> usize {
        ((pdu[at] as usize) << 8) | pdu[at + 1] as usize
    }

    /// Answer a PDU (function code first): the reply PDU, or an exception.
    fn answer(&mut self, pdu: &[u8]) -> Vec<u8> {
        let fc = pdu[0];
        let exc = |code: u8| vec![fc | 0x80, code];
        match fc {
            1..=4 => {
                if pdu.len() != 5 {
                    return exc(3);
                }
                let (addr, count) = (Self::be(pdu, 1), Self::be(pdu, 3));
                let max = if fc <= 2 { 2000 } else { 125 };
                if count == 0 || count > max {
                    return exc(3);
                }
                if addr + count > 64 {
                    return exc(2);
                }
                let mut out = vec![fc];
                if fc <= 2 {
                    let src = if fc == 1 { &self.coils } else { &self.discrete };
                    let mut bytes = vec![0u8; count.div_ceil(8)];
                    for i in 0..count {
                        if src[addr + i] {
                            bytes[i / 8] |= 1 << (i % 8);
                        }
                    }
                    out.push(bytes.len() as u8);
                    out.extend(bytes);
                } else {
                    let src = if fc == 3 { &self.holding } else { &self.input };
                    out.push((count * 2) as u8);
                    for i in 0..count {
                        out.extend(src[addr + i].to_be_bytes());
                    }
                }
                out
            }
            5 => {
                let addr = Self::be(pdu, 1);
                let value = Self::be(pdu, 3);
                if value != 0xFF00 && value != 0 {
                    return exc(3);
                }
                if addr >= 64 {
                    return exc(2);
                }
                self.coils[addr] = value == 0xFF00;
                pdu.to_vec()
            }
            6 => {
                let addr = Self::be(pdu, 1);
                if addr >= 64 {
                    return exc(2);
                }
                self.holding[addr] = Self::be(pdu, 3) as u16;
                pdu.to_vec()
            }
            15 => {
                let (addr, count, n) = (Self::be(pdu, 1), Self::be(pdu, 3), pdu[5] as usize);
                if n != count.div_ceil(8) || pdu.len() != 6 + n {
                    return exc(3);
                }
                if addr + count > 64 {
                    return exc(2);
                }
                for i in 0..count {
                    self.coils[addr + i] = pdu[6 + i / 8] & (1 << (i % 8)) != 0;
                }
                pdu[..5].to_vec()
            }
            16 => {
                let (addr, count, n) = (Self::be(pdu, 1), Self::be(pdu, 3), pdu[5] as usize);
                if n != count * 2 || pdu.len() != 6 + n {
                    return exc(3);
                }
                if addr + count > 64 {
                    return exc(2);
                }
                for i in 0..count {
                    self.holding[addr + i] = Self::be(pdu, 6 + 2 * i) as u16;
                }
                pdu[..5].to_vec()
            }
            _ => exc(1),
        }
    }

    fn noise(seed: u64) -> Vec<u8> {
        let mut rng = Rng(seed);
        let n = (rng.next() % 300) as usize;
        (0..n).map(|_| rng.next() as u8).collect()
    }

    /// An RTU frame in; the bytes the device puts on the line.
    fn rtu(&mut self, frame: &[u8]) -> Vec<u8> {
        self.seen.push(frame.to_vec());
        let n = frame.len();
        if n < 4 || device_crc(&frame[..n - 2]) != [frame[n - 2], frame[n - 1]] {
            return Vec::new(); // a damaged request is ignored, as on a real line
        }
        if frame[0] != self.unit && frame[0] != 0 {
            return Vec::new();
        }
        let pdu = self.answer(&frame[1..n - 2]);
        if frame[0] == 0 {
            return Vec::new(); // broadcast: act, never answer
        }
        let unit = if self.fault == Fault::WrongUnit {
            self.unit + 1
        } else {
            self.unit
        };
        let mut out = vec![unit];
        out.extend(pdu);
        let crc = device_crc(&out);
        out.extend(crc);
        match self.fault.clone() {
            Fault::BadCrc => {
                let last = out.len() - 1;
                out[last] ^= 0x01;
            }
            Fault::Silent => out.clear(),
            Fault::Short(k) => out.truncate(k),
            Fault::Noise(seed) => out = Self::noise(seed),
            _ => {}
        }
        out
    }

    /// An MBAP frame in; the bytes the device sends back.
    fn tcp(&mut self, frame: &[u8]) -> Vec<u8> {
        self.seen.push(frame.to_vec());
        assert!(frame.len() >= 8, "client sent a short MBAP frame");
        assert_eq!(&frame[2..4], &[0, 0], "protocol id");
        assert_eq!(Self::be(frame, 4), frame.len() - 6, "MBAP length");
        let pdu = self.answer(&frame[7..]);
        let wrap = |tid: [u8; 2], pdu: &[u8]| {
            let mut out = tid.to_vec();
            out.extend([0, 0]);
            out.extend(((pdu.len() + 1) as u16).to_be_bytes());
            out.push(frame[6]);
            out.extend_from_slice(pdu);
            out
        };
        let mut out = Vec::new();
        if self.fault == Fault::StaleFirst {
            let old = u16::from_be_bytes([frame[0], frame[1]]).wrapping_sub(1);
            out.extend(wrap(old.to_be_bytes(), &[3, 2, 0xDE, 0xAD]));
        }
        out.extend(wrap([frame[0], frame[1]], &pdu));
        match self.fault.clone() {
            Fault::Silent => out.clear(),
            Fault::Short(k) => out.truncate(k),
            Fault::ImpossibleLength => {
                out[4] = 0x04;
                out[5] = 0x00;
            }
            Fault::Noise(seed) => out = Self::noise(seed),
            _ => {}
        }
        out
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// The wire between the client and the device, on a clock of its own.
struct Wire {
    device: Device,
    tcp: bool,
    line: VecDeque<u8>,
    clock: u64,
    /// Bytes handed over per receive (1: one at a time).
    chunk: usize,
    /// How long the device takes to answer.
    latency: u64,
    ready_at: u64,
}

#[derive(Clone)]
struct FakeLink(Rc<RefCell<Wire>>);

impl Link for FakeLink {
    fn send(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        let mut w = self.0.borrow_mut();
        let answer = if w.tcp {
            w.device.tcp(bytes)
        } else {
            w.device.rtu(bytes)
        };
        w.line.extend(answer);
        w.ready_at = w.clock + w.latency;
        Ok(())
    }

    fn receive(&mut self, max: usize, wait_ms: u32) -> Result<Vec<u8>, LinkError> {
        let mut w = self.0.borrow_mut();
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

fn wire(tcp: bool) -> (Rc<RefCell<Wire>>, FakeLink) {
    let w = Rc::new(RefCell::new(Wire {
        device: Device::new(7),
        tcp,
        line: VecDeque::new(),
        clock: 1_000,
        chunk: 64,
        latency: 20,
        ready_at: 0,
    }));
    (w.clone(), FakeLink(w))
}

fn rtu() -> (Rc<RefCell<Wire>>, Client) {
    let (w, link) = wire(false);
    (w, Client::rtu_over(link, 7))
}

fn tcp() -> (Rc<RefCell<Wire>>, Client) {
    let (w, link) = wire(true);
    (w, Client::tcp_over(link, 7))
}

#[test]
fn crc_matches_the_specifications_example() {
    // Modbus over Serial Line V1.02's read of ten holding registers.
    assert_eq!(crc16(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x0A]), 0xCDC5);
    let frame = frame::rtu_frame(
        1,
        &Request::ReadHoldingRegisters {
            address: 0,
            count: 10,
        },
    )
    .unwrap();
    assert_eq!(frame, [0x01, 0x03, 0x00, 0x00, 0x00, 0x0A, 0xC5, 0xCD]);
}

#[test]
fn every_function_code_round_trips_over_rtu_and_tcp() {
    for (w, mut c) in [rtu(), tcp()] {
        assert_eq!(c.read_holding_registers(2, 3).unwrap(), [1002, 1003, 1004]);
        assert_eq!(c.read_input_registers(0, 2).unwrap(), [2000, 2001]);
        assert_eq!(
            c.read_discrete_inputs(0, 7).unwrap(),
            [true, false, false, true, false, false, true]
        );
        c.write_coil(5, true).unwrap();
        c.write_coils(
            8,
            &[true, false, true, true, false, false, false, false, true],
        )
        .unwrap();
        let coils = c.read_coils(4, 14).unwrap();
        assert_eq!(coils.len(), 14);
        assert_eq!(
            coils,
            [
                false, true, false, false, true, false, true, true, false, false, false, false,
                true, false
            ]
        );
        c.write_register(1, 0xBEEF).unwrap();
        c.write_registers(10, &[1, 2, 65535]).unwrap();
        assert_eq!(c.read_holding_registers(0, 2).unwrap(), [1000, 0xBEEF]);
        assert_eq!(c.read_holding_registers(10, 3).unwrap(), [1, 2, 65535]);
        assert_eq!(w.borrow().device.holding[1], 0xBEEF);
    }
}

#[test]
fn a_refusal_is_a_typed_exception_with_words() {
    for (_, mut c) in [rtu(), tcp()] {
        let e = c.read_holding_registers(60, 10).unwrap_err();
        assert_eq!(e, ModbusError::Exception(Exception::IllegalDataAddress));
        assert!(e.message().contains("address does not exist"), "{e}");
        let e = c.write_register(64, 1).unwrap_err();
        assert_eq!(e, ModbusError::Exception(Exception::IllegalDataAddress));
    }
    // An unknown function code, straight through `call`'s parser.
    let r = Request::ReadCoils {
        address: 0,
        count: 1,
    };
    assert_eq!(
        r.parse_reply(&[0x81, 0x01]),
        Err(ModbusError::Exception(Exception::IllegalFunction))
    );
    assert_eq!(
        r.parse_reply(&[0x81, 0x77]),
        Err(ModbusError::Exception(Exception::Other(0x77)))
    );
}

#[test]
fn a_damaged_checksum_is_refused() {
    let (w, mut c) = rtu();
    w.borrow_mut().device.fault = Fault::BadCrc;
    assert_eq!(c.read_holding_registers(0, 4), Err(ModbusError::BadCrc));
    assert_eq!(c.write_register(0, 5), Err(ModbusError::BadCrc));
    // Back to health, the same client works.
    w.borrow_mut().device.fault = Fault::None;
    assert_eq!(c.read_holding_registers(0, 1).unwrap(), [5]);
}

#[test]
fn silence_times_out_on_time() {
    for (w, mut c) in [rtu(), tcp()] {
        w.borrow_mut().device.fault = Fault::Silent;
        c.set_timeout(250);
        let start = w.borrow().clock;
        assert_eq!(c.read_input_registers(0, 1), Err(ModbusError::TimedOut));
        let spent = w.borrow().clock - start;
        assert!((250..=260).contains(&spent), "waited {spent} ms");
        assert!(ModbusError::TimedOut.message().contains("did not answer"));
    }
}

#[test]
fn an_answer_cut_short_is_an_error_not_a_hang() {
    for k in [1, 2, 3, 6] {
        let (w, mut c) = rtu();
        w.borrow_mut().device.fault = Fault::Short(k);
        let e = c.read_holding_registers(0, 4).unwrap_err();
        assert_eq!(
            e,
            ModbusError::BadReply("the answer stopped short"),
            "k={k}"
        );
    }
    for k in [3, 7, 9] {
        let (w, mut c) = tcp();
        w.borrow_mut().device.fault = Fault::Short(k);
        assert_eq!(c.read_holding_registers(0, 4), Err(ModbusError::TimedOut));
    }
}

#[test]
fn an_answer_from_another_device_is_refused() {
    let (w, mut c) = rtu();
    w.borrow_mut().device.fault = Fault::WrongUnit;
    assert_eq!(
        c.read_holding_registers(0, 1),
        Err(ModbusError::BadReply(
            "the answer came from a different device"
        ))
    );
}

#[test]
fn noise_before_the_request_is_thrown_away() {
    let (w, mut c) = rtu();
    w.borrow_mut().line.extend([0x07, 0x03, 0x02, 0x99]); // half a late answer
    w.borrow_mut().ready_at = 0;
    assert_eq!(c.read_holding_registers(0, 1).unwrap(), [1000]);
}

#[test]
fn bytes_one_at_a_time_still_make_a_frame() {
    for (w, mut c) in [rtu(), tcp()] {
        w.borrow_mut().chunk = 1;
        assert_eq!(c.read_holding_registers(0, 3).unwrap(), [1000, 1001, 1002]);
        c.write_registers(0, &[9; 10]).unwrap();
    }
}

#[test]
fn a_late_answer_to_an_old_transaction_is_skipped() {
    let (w, mut c) = tcp();
    w.borrow_mut().device.fault = Fault::StaleFirst;
    assert_eq!(c.read_holding_registers(0, 2).unwrap(), [1000, 1001]);
}

#[test]
fn an_impossible_tcp_length_is_refused() {
    let (w, mut c) = tcp();
    w.borrow_mut().device.fault = Fault::ImpossibleLength;
    assert_eq!(
        c.read_holding_registers(0, 2),
        Err(ModbusError::BadReply("the answer has an impossible length"))
    );
    w.borrow_mut().device.fault = Fault::None;
    assert_eq!(c.read_holding_registers(0, 2).unwrap(), [1000, 1001]);
}

#[test]
fn tcp_transactions_count_up() {
    let (w, mut c) = tcp();
    for _ in 0..3 {
        c.read_coils(0, 1).unwrap();
    }
    let ids: Vec<u16> = w
        .borrow()
        .device
        .seen
        .iter()
        .map(|f| u16::from_be_bytes([f[0], f[1]]))
        .collect();
    assert_eq!(ids, [1, 2, 3]);
}

#[test]
fn a_broadcast_write_waits_for_nothing() {
    let (w, link) = wire(false);
    let mut c = Client::rtu_over(link, 0);
    let start = w.borrow().clock;
    c.write_register(3, 77).unwrap();
    assert_eq!(w.borrow().clock, start, "a broadcast waited");
    assert_eq!(w.borrow().device.holding[3], 77);
    assert!(matches!(
        c.read_holding_registers(0, 1),
        Err(ModbusError::BadRequest(_))
    ));
}

#[test]
fn requests_past_modbus_limits_never_reach_the_wire() {
    let (w, mut c) = rtu();
    for e in [
        c.read_holding_registers(0, 0),
        c.read_holding_registers(0, 126),
        c.read_holding_registers(65_535, 2),
    ] {
        assert!(matches!(e, Err(ModbusError::BadRequest(_))), "{e:?}");
    }
    assert!(matches!(
        c.read_coils(0, 2001),
        Err(ModbusError::BadRequest(_))
    ));
    assert!(matches!(
        c.write_registers(0, &[0; 124]),
        Err(ModbusError::BadRequest(_))
    ));
    assert!(matches!(
        c.write_coils(0, &[false; 1969]),
        Err(ModbusError::BadRequest(_))
    ));
    assert!(matches!(
        c.write_registers(0, &[]),
        Err(ModbusError::BadRequest(_))
    ));
    c.set_unit(248);
    assert!(matches!(
        c.read_coils(0, 1),
        Err(ModbusError::BadRequest(_))
    ));
    assert!(w.borrow().device.seen.is_empty());
    // The largest legal reads do go.
    c.set_unit(7);
    assert!(matches!(
        c.read_holding_registers(0, 125),
        Err(ModbusError::Exception(Exception::IllegalDataAddress))
    ));
}

#[test]
fn two_register_numbers() {
    let [hi, lo] = f32_to(64.8);
    assert_eq!(f32_from(hi, lo), 64.8);
    assert_eq!(u32_from(0x0001, 0x0002), 65_538);
    assert_eq!(i32_from(0xFFFF, 0xFFFE), -2);
}

/// Every parser, fed random bytes, answers with an error or a value --
/// never a panic -- and a client on a line of noise always comes back.
#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let requests = [
        Request::ReadCoils {
            address: 0,
            count: 13,
        },
        Request::ReadDiscreteInputs {
            address: 3,
            count: 1,
        },
        Request::ReadHoldingRegisters {
            address: 0,
            count: 4,
        },
        Request::ReadInputRegisters {
            address: 9,
            count: 125,
        },
        Request::WriteSingleCoil {
            address: 1,
            on: true,
        },
        Request::WriteSingleRegister {
            address: 1,
            value: 2,
        },
        Request::WriteMultipleCoils {
            address: 1,
            values: vec![true; 9],
        },
        Request::WriteMultipleRegisters {
            address: 1,
            values: vec![7; 3],
        },
    ];
    for round in 0..20_000 {
        let n = (rng.next() % 300) as usize;
        let mut bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        // Half the RTU frames carry a correct CRC, so the parser behind it
        // is reached, and some start like a real answer.
        if round % 2 == 0 && bytes.len() >= 2 {
            bytes[0] = 7;
            bytes[1] = [1, 2, 3, 4, 5, 6, 0x0F, 0x10, 0x83, 0x99][(rng.next() % 10) as usize];
            let body = bytes.len() - 2;
            let crc = crc16(&bytes[..body]).to_le_bytes();
            bytes[body..].copy_from_slice(&crc);
        }
        let _ = frame::rtu_reply_len(&bytes);
        let _ = frame::tcp_frame_len(&bytes);
        for r in &requests {
            let _ = r.parse_reply(&bytes);
            let _ = frame::parse_rtu_reply(7, r, &bytes);
            let _ = frame::parse_tcp_reply(7, r, &bytes);
        }
        if let Ok(Some(len)) = frame::rtu_reply_len(&bytes) {
            assert!(len <= frame::MAX_RTU_FRAME);
        }
        if let Ok(Some(len)) = frame::tcp_frame_len(&bytes) {
            assert!(len <= frame::MAX_TCP_FRAME);
        }
    }
    for seed in 1..400u64 {
        for (w, mut c) in [rtu(), tcp()] {
            w.borrow_mut().device.fault = Fault::Noise(seed);
            let start = w.borrow().clock;
            let _ = c.read_holding_registers(0, 4);
            let _ = c.write_coils(0, &[true; 3]);
            assert!(w.borrow().clock - start <= 2 * u64::from(DEFAULT_TIMEOUT_MS) + 100);
        }
    }
}
