//! Modbus: read and write a machine's registers (IC-994).
//!
//! Turn on the `modbus` feature of the `krate` dependency. Modbus RTU runs
//! over the serial port the person picked (`device.serial`); Modbus TCP over
//! a connection to the host the manifest declares
//! (`net.connect:<host>:502`). There is no grant of its own: the port or the
//! host is the whole permission. Every call waits at most the client's
//! timeout (one second unless [`Client::set_timeout`] says otherwise) and
//! fails with words, never a panic.
//!
//! ```no_run
//! # fn demo() -> Result<(), krate::modbus::ModbusError> {
//! use krate::{modbus, serial};
//! let Ok(Some(port)) = serial::pick("Choose the temperature controller") else { return Ok(()) };
//! port.open(&serial::Settings::baud(9600)).ok();
//! let mut device = modbus::Client::rtu(port, 1); // unit address 1
//! let regs = device.read_holding_registers(0, 2)?; // 40001 and 40002
//! let celsius = f32::from(regs[0]) / 10.0;
//! device.write_register(10, 650)?; // set point 65.0
//! # let _ = celsius; Ok(()) }
//! ```
//!
//! Addresses are the protocol's, from 0: the register a manual calls
//! "40001" is holding register 0, "30001" input register 0.

use crate::link::{Link, LinkError, SerialLink, TcpLink};
use alloc::{boxed::Box, format, string::String, vec::Vec};

pub mod frame;
pub use frame::{crc16, Exception, Request, Response};

/// How long a request waits for its answer unless told otherwise.
pub const DEFAULT_TIMEOUT_MS: u32 = 1_000;

/// Why a request did not get its answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModbusError {
    /// Nothing came back in time.
    TimedOut,
    /// The device answered with a refusal.
    Exception(Exception),
    /// The answer arrived damaged: its checksum is wrong.
    BadCrc,
    /// The answer did not make sense, with words saying how.
    BadReply(&'static str),
    /// The request breaks a Modbus limit, with words saying which.
    BadRequest(&'static str),
    /// The serial port or the connection failed.
    Link(LinkError),
}

impl ModbusError {
    /// Words to show the person.
    pub fn message(&self) -> String {
        match self {
            ModbusError::TimedOut => String::from(
                "The device did not answer in time. Check the cable, the device address and \
                 the port settings (baud rate, parity).",
            ),
            ModbusError::Exception(e) => format!("The device refused: {}.", e.words()),
            ModbusError::BadCrc => String::from(
                "The answer arrived damaged (wrong checksum). Check the cable and the baud rate.",
            ),
            ModbusError::BadReply(w) => format!("The device's answer did not make sense: {w}."),
            ModbusError::BadRequest(w) => format!("That request is not possible: {w}."),
            ModbusError::Link(e) => e.message(),
        }
    }
}

impl core::fmt::Display for ModbusError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message())
    }
}

impl From<LinkError> for ModbusError {
    fn from(e: LinkError) -> Self {
        ModbusError::Link(e)
    }
}

enum Framing {
    Rtu,
    Tcp { next: u16, pending: Vec<u8> },
}

/// A Modbus master talking to one device (its unit address) over one link.
pub struct Client {
    link: Box<dyn Link>,
    framing: Framing,
    unit: u8,
    timeout_ms: u32,
}

impl core::fmt::Debug for Client {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Client")
            .field("unit", &self.unit)
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Modbus RTU over the serial port the person picked, already opened
    /// with the device's settings, to the device at `unit` (1 to 247; 0
    /// sends a write to every device and waits for no answer).
    pub fn rtu(port: crate::serial::Port, unit: u8) -> Client {
        Client::rtu_over(SerialLink::new(port), unit)
    }

    /// Modbus TCP to `host` on `port` (502 is Modbus's own), declared in
    /// the manifest as `net.connect:<host>:<port>`. `unit` is the device
    /// behind a gateway, or 1 (some devices want 255) for a device that is
    /// its own server.
    pub fn tcp(host: &str, port: u16, unit: u8) -> Result<Client, ModbusError> {
        let link = TcpLink::connect(host, port, DEFAULT_TIMEOUT_MS * 5)?;
        Ok(Client::tcp_over(link, unit))
    }

    /// Modbus RTU over any [`Link`]: a fake device in a test, or RTU
    /// carried over TCP by a serial gateway.
    pub fn rtu_over(link: impl Link + 'static, unit: u8) -> Client {
        Client {
            link: Box::new(link),
            framing: Framing::Rtu,
            unit,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    /// Modbus TCP framing over any [`Link`].
    pub fn tcp_over(link: impl Link + 'static, unit: u8) -> Client {
        Client {
            link: Box::new(link),
            framing: Framing::Tcp {
                next: 1,
                pending: Vec::new(),
            },
            unit,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    /// Wait up to `ms` for each answer (at least 1).
    pub fn set_timeout(&mut self, ms: u32) {
        self.timeout_ms = ms.max(1);
    }

    /// Talk to the device at another unit address on the same line.
    pub fn set_unit(&mut self, unit: u8) {
        self.unit = unit;
    }

    /// The unit address requests go to.
    pub fn unit(&self) -> u8 {
        self.unit
    }

    /// 01: read `count` coils from `address`.
    pub fn read_coils(&mut self, address: u16, count: u16) -> Result<Vec<bool>, ModbusError> {
        self.bits(Request::ReadCoils { address, count })
    }

    /// 02: read `count` discrete inputs from `address`.
    pub fn read_discrete_inputs(
        &mut self,
        address: u16,
        count: u16,
    ) -> Result<Vec<bool>, ModbusError> {
        self.bits(Request::ReadDiscreteInputs { address, count })
    }

    /// 03: read `count` holding registers from `address`.
    pub fn read_holding_registers(
        &mut self,
        address: u16,
        count: u16,
    ) -> Result<Vec<u16>, ModbusError> {
        self.registers(Request::ReadHoldingRegisters { address, count })
    }

    /// 04: read `count` input registers from `address`.
    pub fn read_input_registers(
        &mut self,
        address: u16,
        count: u16,
    ) -> Result<Vec<u16>, ModbusError> {
        self.registers(Request::ReadInputRegisters { address, count })
    }

    /// 05: switch the coil at `address` on or off.
    pub fn write_coil(&mut self, address: u16, on: bool) -> Result<(), ModbusError> {
        self.call(&Request::WriteSingleCoil { address, on })
            .map(|_| ())
    }

    /// 06: write one holding register.
    pub fn write_register(&mut self, address: u16, value: u16) -> Result<(), ModbusError> {
        self.call(&Request::WriteSingleRegister { address, value })
            .map(|_| ())
    }

    /// 0F: write coils from `address` on.
    pub fn write_coils(&mut self, address: u16, values: &[bool]) -> Result<(), ModbusError> {
        self.call(&Request::WriteMultipleCoils {
            address,
            values: values.to_vec(),
        })
        .map(|_| ())
    }

    /// 10: write holding registers from `address` on.
    pub fn write_registers(&mut self, address: u16, values: &[u16]) -> Result<(), ModbusError> {
        self.call(&Request::WriteMultipleRegisters {
            address,
            values: values.to_vec(),
        })
        .map(|_| ())
    }

    fn bits(&mut self, request: Request) -> Result<Vec<bool>, ModbusError> {
        match self.call(&request)? {
            Response::Bits(bits) => Ok(bits),
            _ => Err(ModbusError::BadReply(
                "the answer is to a different request",
            )),
        }
    }

    fn registers(&mut self, request: Request) -> Result<Vec<u16>, ModbusError> {
        match self.call(&request)? {
            Response::Registers(regs) => Ok(regs),
            _ => Err(ModbusError::BadReply(
                "the answer is to a different request",
            )),
        }
    }

    /// Send one request and wait for its answer.
    pub fn call(&mut self, request: &Request) -> Result<Response, ModbusError> {
        match self.framing {
            Framing::Rtu => self.call_rtu(request),
            Framing::Tcp { .. } => self.call_tcp(request),
        }
    }

    fn call_rtu(&mut self, request: &Request) -> Result<Response, ModbusError> {
        if self.unit > 247 {
            return Err(ModbusError::BadRequest(
                "a device address on a serial line is 1 to 247",
            ));
        }
        if self.unit == 0 && !request.is_write() {
            return Err(ModbusError::BadRequest(
                "a read needs one device's address (1 to 247), not 0",
            ));
        }
        let out = frame::rtu_frame(self.unit, request)?;
        // The rest of a late answer would be read as this one's start.
        self.link.discard_input()?;
        self.link.send(&out)?;
        if self.unit == 0 {
            // A broadcast: every device acts, none answers.
            return Ok(Response::Written);
        }
        let deadline = self.link.now_ms() + u64::from(self.timeout_ms);
        let mut buf: Vec<u8> = Vec::with_capacity(frame::MAX_RTU_FRAME);
        loop {
            let need = frame::rtu_reply_len(&buf)?.unwrap_or(3);
            if buf.len() >= need {
                return frame::parse_rtu_reply(self.unit, request, &buf[..need]);
            }
            let now = self.link.now_ms();
            if now >= deadline {
                return Err(if buf.is_empty() {
                    ModbusError::TimedOut
                } else {
                    ModbusError::BadReply("the answer stopped short")
                });
            }
            let wait = (deadline - now).min(u64::from(u32::MAX)) as u32;
            let got = self.link.receive(need - buf.len(), wait)?;
            let room = need - buf.len();
            buf.extend_from_slice(&got[..got.len().min(room)]);
        }
    }

    fn call_tcp(&mut self, request: &Request) -> Result<Response, ModbusError> {
        let Framing::Tcp { next, pending } = &mut self.framing else {
            return Err(ModbusError::BadRequest("not a Modbus TCP client"));
        };
        let transaction = *next;
        *next = next.wrapping_add(1);
        let mut buf = core::mem::take(pending);
        let result = self.tcp_exchange(request, transaction, &mut buf);
        if let Framing::Tcp { pending, .. } = &mut self.framing {
            // Whatever followed the answer is the start of the next one;
            // after a frame that made no sense, nothing here can be trusted.
            *pending = match result {
                Err(ModbusError::BadReply(_)) => Vec::new(),
                _ => buf,
            };
        }
        result
    }

    fn tcp_exchange(
        &mut self,
        request: &Request,
        transaction: u16,
        buf: &mut Vec<u8>,
    ) -> Result<Response, ModbusError> {
        let out = frame::tcp_frame(transaction, self.unit, request)?;
        self.link.send(&out)?;
        let deadline = self.link.now_ms() + u64::from(self.timeout_ms);
        loop {
            // Every whole frame already here; a late answer to an earlier
            // request that timed out is skipped by its number.
            while let Some(len) = frame::tcp_frame_len(buf)? {
                if buf.len() < len {
                    break;
                }
                let whole: Vec<u8> = buf.drain(..len).collect();
                if u16::from_be_bytes([whole[0], whole[1]]) == transaction {
                    return frame::parse_tcp_reply(self.unit, request, &whole).map(|(_, r)| r);
                }
            }
            let now = self.link.now_ms();
            if now >= deadline {
                return Err(ModbusError::TimedOut);
            }
            let wait = (deadline - now).min(u64::from(u32::MAX)) as u32;
            let got = self.link.receive(frame::MAX_TCP_FRAME, wait)?;
            if buf.len() + got.len() > 4 * frame::MAX_TCP_FRAME {
                return Err(ModbusError::BadReply("too much arrived at once"));
            }
            buf.extend_from_slice(&got);
        }
    }
}

/// A 32-bit float a device keeps in two registers, high word first. If the
/// number reads as nonsense, the device keeps the low word first: swap them.
pub fn f32_from(high: u16, low: u16) -> f32 {
    f32::from_bits(u32_from(high, low))
}

/// A 32-bit unsigned number a device keeps in two registers, high word first.
pub fn u32_from(high: u16, low: u16) -> u32 {
    (u32::from(high) << 16) | u32::from(low)
}

/// A 32-bit signed number a device keeps in two registers, high word first.
pub fn i32_from(high: u16, low: u16) -> i32 {
    u32_from(high, low) as i32
}

/// A float as two registers, high word first, to write with
/// [`Client::write_registers`].
pub fn f32_to(value: f32) -> [u16; 2] {
    let bits = value.to_bits();
    [(bits >> 16) as u16, bits as u16]
}

#[cfg(test)]
mod tests;
