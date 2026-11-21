//! Modbus frames, byte for byte: build a request, read an answer.
//!
//! [`Client`](super::Client) uses these; they are public for an app that
//! runs its own loop. Nothing here panics on any input: a frame that does
//! not make sense is an error with words.

use super::ModbusError;
use alloc::{vec, vec::Vec};

/// The largest RTU frame: address, 253 bytes of PDU, two of CRC.
pub const MAX_RTU_FRAME: usize = 256;
/// The largest Modbus TCP frame: the 7-byte MBAP header and 253 of PDU.
pub const MAX_TCP_FRAME: usize = 260;

/// CRC-16/Modbus (polynomial 0xA001 reflected, start 0xFFFF). On the wire
/// it follows the frame low byte first.
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= u16::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// One request, by function code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// 01: read 1 to 2000 coils (on/off outputs).
    ReadCoils { address: u16, count: u16 },
    /// 02: read 1 to 2000 discrete inputs (on/off inputs).
    ReadDiscreteInputs { address: u16, count: u16 },
    /// 03: read 1 to 125 holding registers (16-bit, read and write).
    ReadHoldingRegisters { address: u16, count: u16 },
    /// 04: read 1 to 125 input registers (16-bit, read only).
    ReadInputRegisters { address: u16, count: u16 },
    /// 05: switch one coil on or off.
    WriteSingleCoil { address: u16, on: bool },
    /// 06: write one holding register.
    WriteSingleRegister { address: u16, value: u16 },
    /// 0F (15): write 1 to 1968 coils.
    WriteMultipleCoils { address: u16, values: Vec<bool> },
    /// 10 (16): write 1 to 123 holding registers.
    WriteMultipleRegisters { address: u16, values: Vec<u16> },
}

/// What a device answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// Coils or discrete inputs, exactly as many as asked for.
    Bits(Vec<bool>),
    /// Registers, exactly as many as asked for.
    Registers(Vec<u16>),
    /// A write the device confirmed.
    Written,
}

/// The device's refusal: a Modbus exception code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exception {
    /// 01: the device does not do that function.
    IllegalFunction,
    /// 02: no such address on the device.
    IllegalDataAddress,
    /// 03: a value the device does not accept.
    IllegalDataValue,
    /// 04: the device failed while doing it.
    ServerDeviceFailure,
    /// 05: accepted, still working on it.
    Acknowledge,
    /// 06: busy with something else; ask again later.
    ServerDeviceBusy,
    /// 08: the device's memory failed a parity check.
    MemoryParityError,
    /// 0A: a gateway has no path to the device.
    GatewayPathUnavailable,
    /// 0B: a gateway reached nothing at that address.
    GatewayTargetFailedToRespond,
    /// Any other code.
    Other(u8),
}

impl Exception {
    /// From the code on the wire.
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => Exception::IllegalFunction,
            2 => Exception::IllegalDataAddress,
            3 => Exception::IllegalDataValue,
            4 => Exception::ServerDeviceFailure,
            5 => Exception::Acknowledge,
            6 => Exception::ServerDeviceBusy,
            8 => Exception::MemoryParityError,
            0x0A => Exception::GatewayPathUnavailable,
            0x0B => Exception::GatewayTargetFailedToRespond,
            other => Exception::Other(other),
        }
    }

    /// The code on the wire.
    pub fn code(&self) -> u8 {
        match self {
            Exception::IllegalFunction => 1,
            Exception::IllegalDataAddress => 2,
            Exception::IllegalDataValue => 3,
            Exception::ServerDeviceFailure => 4,
            Exception::Acknowledge => 5,
            Exception::ServerDeviceBusy => 6,
            Exception::MemoryParityError => 8,
            Exception::GatewayPathUnavailable => 0x0A,
            Exception::GatewayTargetFailedToRespond => 0x0B,
            Exception::Other(code) => *code,
        }
    }

    /// What it means, in words.
    pub fn words(&self) -> &'static str {
        match self {
            Exception::IllegalFunction => "the device does not support that request",
            Exception::IllegalDataAddress => "that address does not exist on the device",
            Exception::IllegalDataValue => "the device does not accept that value",
            Exception::ServerDeviceFailure => "the device failed while doing it",
            Exception::Acknowledge => "the device accepted it and is still working on it",
            Exception::ServerDeviceBusy => "the device is busy; try again shortly",
            Exception::MemoryParityError => "the device's memory failed a check",
            Exception::GatewayPathUnavailable => "the gateway has no path to that device",
            Exception::GatewayTargetFailedToRespond => {
                "the gateway reached nothing at that device address"
            }
            Exception::Other(_) => "the device refused with a code Modbus does not define",
        }
    }
}

fn check_count(count: usize, max: usize, what: &'static str) -> Result<(), ModbusError> {
    if count == 0 || count > max {
        return Err(ModbusError::BadRequest(what));
    }
    Ok(())
}

fn check_span(address: u16, count: usize) -> Result<(), ModbusError> {
    if usize::from(address) + count > 0x1_0000 {
        return Err(ModbusError::BadRequest(
            "the range runs past the last address (65535)",
        ));
    }
    Ok(())
}

fn pack_bits(values: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; values.len().div_ceil(8)];
    for (i, &on) in values.iter().enumerate() {
        if on {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

impl Request {
    /// The function code.
    pub fn function(&self) -> u8 {
        match self {
            Request::ReadCoils { .. } => 0x01,
            Request::ReadDiscreteInputs { .. } => 0x02,
            Request::ReadHoldingRegisters { .. } => 0x03,
            Request::ReadInputRegisters { .. } => 0x04,
            Request::WriteSingleCoil { .. } => 0x05,
            Request::WriteSingleRegister { .. } => 0x06,
            Request::WriteMultipleCoils { .. } => 0x0F,
            Request::WriteMultipleRegisters { .. } => 0x10,
        }
    }

    /// Whether it writes (and so may go to every device at once, unit 0).
    pub fn is_write(&self) -> bool {
        self.function() >= 0x05
    }

    /// The protocol data unit: function code and data, checked against the
    /// limits Modbus sets.
    pub fn pdu(&self) -> Result<Vec<u8>, ModbusError> {
        let mut out = vec![self.function()];
        match self {
            Request::ReadCoils { address, count }
            | Request::ReadDiscreteInputs { address, count } => {
                check_count(usize::from(*count), 2000, "read 1 to 2000 bits at a time")?;
                check_span(*address, usize::from(*count))?;
                out.extend_from_slice(&address.to_be_bytes());
                out.extend_from_slice(&count.to_be_bytes());
            }
            Request::ReadHoldingRegisters { address, count }
            | Request::ReadInputRegisters { address, count } => {
                check_count(
                    usize::from(*count),
                    125,
                    "read 1 to 125 registers at a time",
                )?;
                check_span(*address, usize::from(*count))?;
                out.extend_from_slice(&address.to_be_bytes());
                out.extend_from_slice(&count.to_be_bytes());
            }
            Request::WriteSingleCoil { address, on } => {
                out.extend_from_slice(&address.to_be_bytes());
                out.extend_from_slice(if *on { &[0xFF, 0x00] } else { &[0x00, 0x00] });
            }
            Request::WriteSingleRegister { address, value } => {
                out.extend_from_slice(&address.to_be_bytes());
                out.extend_from_slice(&value.to_be_bytes());
            }
            Request::WriteMultipleCoils { address, values } => {
                check_count(values.len(), 1968, "write 1 to 1968 coils at a time")?;
                check_span(*address, values.len())?;
                let packed = pack_bits(values);
                out.extend_from_slice(&address.to_be_bytes());
                out.extend_from_slice(&(values.len() as u16).to_be_bytes());
                out.push(packed.len() as u8);
                out.extend_from_slice(&packed);
            }
            Request::WriteMultipleRegisters { address, values } => {
                check_count(values.len(), 123, "write 1 to 123 registers at a time")?;
                check_span(*address, values.len())?;
                out.extend_from_slice(&address.to_be_bytes());
                out.extend_from_slice(&(values.len() as u16).to_be_bytes());
                out.push((values.len() * 2) as u8);
                for v in values {
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
        }
        Ok(out)
    }

    /// Read a reply PDU (function code and data) to this request: the
    /// values, the confirmation, or the device's exception.
    pub fn parse_reply(&self, pdu: &[u8]) -> Result<Response, ModbusError> {
        let function = self.function();
        let Some(&got) = pdu.first() else {
            return Err(ModbusError::BadReply("the answer was empty"));
        };
        if got == function | 0x80 {
            return match pdu.get(1) {
                Some(&code) if pdu.len() == 2 => {
                    Err(ModbusError::Exception(Exception::from_code(code)))
                }
                _ => Err(ModbusError::BadReply("a refusal of the wrong length")),
            };
        }
        if got != function {
            return Err(ModbusError::BadReply(
                "the answer is to a different request",
            ));
        }
        let data = &pdu[1..];
        match self {
            Request::ReadCoils { count, .. } | Request::ReadDiscreteInputs { count, .. } => {
                let count = usize::from(*count);
                let want = count.div_ceil(8);
                if data.first().map(|&n| usize::from(n)) != Some(want) || data.len() != want + 1 {
                    return Err(ModbusError::BadReply(
                        "the device sent a different number of bits than asked",
                    ));
                }
                let bits = (0..count)
                    .map(|i| data[1 + i / 8] & (1 << (i % 8)) != 0)
                    .collect();
                Ok(Response::Bits(bits))
            }
            Request::ReadHoldingRegisters { count, .. }
            | Request::ReadInputRegisters { count, .. } => {
                let want = usize::from(*count) * 2;
                if data.first().map(|&n| usize::from(n)) != Some(want) || data.len() != want + 1 {
                    return Err(ModbusError::BadReply(
                        "the device sent a different number of registers than asked",
                    ));
                }
                let regs = data[1..]
                    .chunks_exact(2)
                    .map(|p| u16::from_be_bytes([p[0], p[1]]))
                    .collect();
                Ok(Response::Registers(regs))
            }
            Request::WriteSingleCoil { .. } | Request::WriteSingleRegister { .. } => {
                // The device echoes the request.
                let sent = self.pdu()?;
                if pdu != sent.as_slice() {
                    return Err(ModbusError::BadReply(
                        "the device confirmed a different write",
                    ));
                }
                Ok(Response::Written)
            }
            Request::WriteMultipleCoils { address, values } => {
                echo_range(data, *address, values.len())
            }
            Request::WriteMultipleRegisters { address, values } => {
                echo_range(data, *address, values.len())
            }
        }
    }
}

fn echo_range(data: &[u8], address: u16, count: usize) -> Result<Response, ModbusError> {
    let mut want = Vec::with_capacity(4);
    want.extend_from_slice(&address.to_be_bytes());
    want.extend_from_slice(&(count as u16).to_be_bytes());
    if data != want.as_slice() {
        return Err(ModbusError::BadReply(
            "the device confirmed a different write",
        ));
    }
    Ok(Response::Written)
}

/// A whole RTU frame: unit address, PDU, CRC (low byte first).
pub fn rtu_frame(unit: u8, request: &Request) -> Result<Vec<u8>, ModbusError> {
    let mut out = vec![unit];
    out.extend_from_slice(&request.pdu()?);
    let crc = crc16(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    Ok(out)
}

/// How long the RTU answer that starts with `head` is, once enough of it
/// has arrived to tell (`None` until then). Never more than
/// [`MAX_RTU_FRAME`].
pub fn rtu_reply_len(head: &[u8]) -> Result<Option<usize>, ModbusError> {
    let Some(&function) = head.get(1) else {
        return Ok(None);
    };
    if function & 0x80 != 0 {
        return Ok(Some(5));
    }
    match function {
        // A read answers at most 250 data bytes (125 registers, 2000 bits).
        0x01..=0x04 => match head.get(2) {
            Some(&n) if n > 250 => {
                Err(ModbusError::BadReply("an answer longer than Modbus allows"))
            }
            Some(&n) => Ok(Some(5 + usize::from(n))),
            None => Ok(None),
        },
        0x05 | 0x06 | 0x0F | 0x10 => Ok(Some(8)),
        _ => Err(ModbusError::BadReply(
            "an answer with an unknown function code",
        )),
    }
}

/// Check and read a whole RTU answer from `unit` to `request`.
pub fn parse_rtu_reply(unit: u8, request: &Request, frame: &[u8]) -> Result<Response, ModbusError> {
    if frame.len() < 4 {
        return Err(ModbusError::BadReply("the answer stopped short"));
    }
    let (body, crc) = frame.split_at(frame.len() - 2);
    if crc16(body).to_le_bytes() != [crc[0], crc[1]] {
        return Err(ModbusError::BadCrc);
    }
    if body[0] != unit {
        return Err(ModbusError::BadReply(
            "the answer came from a different device",
        ));
    }
    request.parse_reply(&body[1..])
}

/// A whole Modbus TCP frame: the MBAP header (transaction, protocol 0,
/// length, unit) and the PDU.
pub fn tcp_frame(transaction: u16, unit: u8, request: &Request) -> Result<Vec<u8>, ModbusError> {
    let pdu = request.pdu()?;
    let mut out = Vec::with_capacity(7 + pdu.len());
    out.extend_from_slice(&transaction.to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
    out.push(unit);
    out.extend_from_slice(&pdu);
    Ok(out)
}

/// How long the Modbus TCP frame that starts with `head` is, once its
/// header has arrived (`None` until then). An impossible length is an error,
/// never a huge buffer.
pub fn tcp_frame_len(head: &[u8]) -> Result<Option<usize>, ModbusError> {
    if head.len() < 6 {
        return Ok(None);
    }
    if head[2] != 0 || head[3] != 0 {
        return Err(ModbusError::BadReply(
            "the answer is not Modbus (protocol id)",
        ));
    }
    let len = usize::from(u16::from_be_bytes([head[4], head[5]]));
    if !(2..=254).contains(&len) {
        return Err(ModbusError::BadReply("the answer has an impossible length"));
    }
    Ok(Some(6 + len))
}

/// Read a whole Modbus TCP answer: its transaction number and what it says
/// for `request` from `unit`.
pub fn parse_tcp_reply(
    unit: u8,
    request: &Request,
    frame: &[u8],
) -> Result<(u16, Response), ModbusError> {
    let Some(len) = tcp_frame_len(frame)? else {
        return Err(ModbusError::BadReply("the answer stopped short"));
    };
    if frame.len() != len {
        return Err(ModbusError::BadReply("the answer has the wrong length"));
    }
    let transaction = u16::from_be_bytes([frame[0], frame[1]]);
    if frame[6] != unit {
        return Err(ModbusError::BadReply(
            "the answer came from a different device",
        ));
    }
    Ok((transaction, request.parse_reply(&frame[7..])?))
}
