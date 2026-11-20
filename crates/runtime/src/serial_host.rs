//! Serial devices the person picks (`krate:serial`, IC-985): the
//! operating-system half.
//!
//! This file knows nothing of grants or manifests. The Phase 2 host admits
//! every call first -- the manifest declares `device.serial` and the person
//! allowed it -- and only then asks this file to show the list or touch a
//! port. What it guarantees:
//!
//! - **The app never names a port.** [`choose`] shows the person the
//!   system's list and returns the one they chose; [`SerialPorts`] keeps its
//!   path under a fresh handle, and the path never leaves this file except
//!   as the name shown to the person. No call here takes a path from the app.
//! - **One pick, one port.** A handle reaches the port it was picked for and
//!   nothing else. Closing it spends the pick.
//! - **Bounded.** At most [`MAX_PICKED`] ports held, [`MAX_IO_BYTES`] a read
//!   or a write, and a wait of at most [`MAX_WAIT_MS`].
//! - **Everything closes with the run.** [`SerialPorts`] owns every open
//!   port; dropping it, which the run ending does, closes them all and lets
//!   go of the exclusive lock each one holds while open.
//!
//! A port is opened exclusively (TIOCEXCL and an flock on macOS and Linux,
//! an unshared handle on Windows), so two apps -- or an app and another
//! program -- never talk over each other on one wire.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    time::Duration,
};

use serialport::SerialPort as _;

/// Ports one run may hold at once, picked or open.
pub const MAX_PICKED: usize = 8;
/// The most bytes one read returns or one write takes.
pub const MAX_IO_BYTES: usize = 65_536;
/// The longest wait a read or a write may ask for, in milliseconds.
pub const MAX_WAIT_MS: u32 = 60_000;
/// The slowest and fastest line speeds Krate accepts, in bits a second.
pub const MIN_BAUD: u32 = 50;
pub const MAX_BAUD: u32 = 4_000_000;
/// The longest title an app may put on the list, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// How long a write may wait for the device to take its bytes, at least.
const MIN_WRITE_WAIT: Duration = Duration::from_secs(1);
/// How many times round the list the person may go before it counts as
/// cancelled: a list that never ends is not a choice.
const WALK_ROUNDS: usize = 3;

/// The person's pick, made by the environment rather than a dialog: a test,
/// or someone driving Krate from a script. The value is the device's path
/// (`/dev/ttys004`, `/dev/ttyUSB0`, `COM3`), or `cancel` (or nothing) for a
/// person who cancels. Only the person running Krate sets it; an app cannot,
/// and it still passes the same grant, and must name a real serial device.
pub const PICK_ENV: &str = "KRATE_SERIAL_PICK";

/// Why a serial call did not work, in words for a person where it has any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// No serial device is connected.
    Unavailable(String),
    /// The handle was never picked this run, or was closed.
    UnknownHandle,
    /// Picked and not opened yet.
    NotOpen,
    /// A setting, a wait or a length Krate does not accept.
    BadSettings(String),
    /// Another program has the device open.
    Busy(String),
    /// The device went away.
    Disconnected(String),
    /// [`MAX_PICKED`] ports are already held.
    TooMany(String),
    /// The computer refused.
    Failed(String),
}

/// Parity, as the app asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    None,
    Odd,
    Even,
}

/// Stop bits, as the app asked for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBits {
    One,
    Two,
}

/// How to talk to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub read_timeout_ms: u32,
}

impl Settings {
    /// Refuse what Krate does not accept, with words that say the range.
    pub fn check(&self) -> Result<(), Fault> {
        if !(MIN_BAUD..=MAX_BAUD).contains(&self.baud_rate) {
            return Err(Fault::BadSettings(format!(
                "a baud rate is from {MIN_BAUD} to {MAX_BAUD}, not {}",
                self.baud_rate
            )));
        }
        if !(5..=8).contains(&self.data_bits) {
            return Err(Fault::BadSettings(format!(
                "data bits are 5, 6, 7 or 8, not {}",
                self.data_bits
            )));
        }
        check_wait(self.read_timeout_ms)
    }

    fn data_bits(&self) -> serialport::DataBits {
        match self.data_bits {
            5 => serialport::DataBits::Five,
            6 => serialport::DataBits::Six,
            7 => serialport::DataBits::Seven,
            _ => serialport::DataBits::Eight,
        }
    }

    fn parity(&self) -> serialport::Parity {
        match self.parity {
            Parity::None => serialport::Parity::None,
            Parity::Odd => serialport::Parity::Odd,
            Parity::Even => serialport::Parity::Even,
        }
    }

    fn stop_bits(&self) -> serialport::StopBits {
        match self.stop_bits {
            StopBits::One => serialport::StopBits::One,
            StopBits::Two => serialport::StopBits::Two,
        }
    }
}

fn check_wait(ms: u32) -> Result<(), Fault> {
    if ms > MAX_WAIT_MS {
        return Err(Fault::BadSettings(format!(
            "a wait is at most {MAX_WAIT_MS} milliseconds, not {ms}"
        )));
    }
    Ok(())
}

/// A port as this system opens it.
#[cfg(unix)]
type Native = serialport::TTYPort;
#[cfg(windows)]
type Native = serialport::COMPort;

/// One port the person picked this run.
struct Held {
    /// Where the device is. Never handed to the app.
    path: String,
    /// What the person was shown, for words.
    name: String,
    /// Open, once the app opened it.
    port: Option<Native>,
    /// The wait a read uses when it names none.
    read_timeout: Duration,
    /// The line speed it was opened at, for how long a write may take.
    baud: u32,
}

/// The ports one run picked, by handle. Dropped with the run, which closes
/// every one.
pub struct SerialPorts {
    next: u64,
    held: BTreeMap<u64, Held>,
}

impl Default for SerialPorts {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialPorts {
    pub fn new() -> Self {
        Self {
            next: 1,
            held: BTreeMap::new(),
        }
    }

    /// Keep the person's pick under a fresh handle. Numbers are never
    /// reused within a run.
    pub fn remember(&mut self, path: String, name: String) -> Result<u64, Fault> {
        if self.held.len() >= MAX_PICKED {
            return Err(Fault::TooMany(format!(
                "this app already holds {MAX_PICKED} serial devices; close one first"
            )));
        }
        let handle = self.next;
        self.next += 1;
        self.held.insert(
            handle,
            Held {
                path,
                name,
                port: None,
                read_timeout: Duration::ZERO,
                baud: MAX_BAUD,
            },
        );
        Ok(handle)
    }

    /// Open a picked port, or change an open one's settings in place.
    pub fn open(&mut self, handle: u64, settings: &Settings) -> Result<(), Fault> {
        settings.check()?;
        let held = self.held.get_mut(&handle).ok_or(Fault::UnknownHandle)?;
        match held.port.as_mut() {
            Some(port) => reconfigure(port, &held.name, settings)?,
            None => held.port = Some(open_native(&held.path, &held.name, settings)?),
        }
        held.read_timeout = Duration::from_millis(u64::from(settings.read_timeout_ms));
        held.baud = settings.baud_rate;
        Ok(())
    }

    /// Wait up to `timeout_ms` (or the port's own wait) for the first byte,
    /// then return at most `max` of what has arrived. Empty when the wait
    /// ran out.
    pub fn read(
        &mut self,
        handle: u64,
        max: u32,
        timeout_ms: Option<u32>,
    ) -> Result<Vec<u8>, Fault> {
        if let Some(ms) = timeout_ms {
            check_wait(ms)?;
        }
        let held = self.held.get_mut(&handle).ok_or(Fault::UnknownHandle)?;
        let wait = timeout_ms
            .map(|ms| Duration::from_millis(u64::from(ms)))
            .unwrap_or(held.read_timeout);
        let name = held.name.clone();
        let port = held.port.as_mut().ok_or(Fault::NotOpen)?;
        let max = (max as usize).min(MAX_IO_BYTES);
        if max == 0 {
            return Ok(Vec::new());
        }
        port.set_timeout(wait).map_err(|err| failed(&name, err))?;
        let mut buf = vec![0u8; max];
        match port.read(&mut buf) {
            // A terminal line that reads nothing has been hung up.
            Ok(0) => Err(gone(&name)),
            Ok(n) => {
                buf.truncate(n);
                Ok(buf)
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(Vec::new())
            }
            Err(err) => Err(io_fault(&name, &err)),
        }
    }

    /// Write every byte, waiting up to the port's own wait (at least a
    /// second) plus the time the bytes take on the wire at the port's speed
    /// for the device to take them. Never longer: a device that stops
    /// taking bytes ends the write with words, it does not hold the app.
    pub fn write(&mut self, handle: u64, bytes: &[u8]) -> Result<(), Fault> {
        if bytes.len() > MAX_IO_BYTES {
            return Err(Fault::BadSettings(format!(
                "a write is at most {MAX_IO_BYTES} bytes, not {}; write the rest in another call",
                bytes.len()
            )));
        }
        let held = self.held.get_mut(&handle).ok_or(Fault::UnknownHandle)?;
        let name = held.name.clone();
        // Ten bits a character (start, eight data, stop) is near enough
        // for any framing, and the floor keeps a slow line from timing out
        // on its own bytes.
        let on_the_wire = Duration::from_millis(
            (bytes.len() as u64 * 10 * 1000).div_ceil(u64::from(held.baud.max(1))),
        );
        let deadline =
            std::time::Instant::now() + held.read_timeout.max(MIN_WRITE_WAIT) + on_the_wire;
        let port = held.port.as_mut().ok_or(Fault::NotOpen)?;
        let late = || Fault::Failed(format!("{name} did not take the bytes in time"));
        let mut rest = bytes;
        while !rest.is_empty() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(late());
            }
            port.set_timeout(left).map_err(|err| failed(&name, err))?;
            match port.write(rest) {
                Ok(0) => return Err(gone(&name)),
                Ok(n) => rest = &rest[n..],
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    // Full for now: let it drain a moment and try again,
                    // until the deadline.
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(err) => return Err(io_fault(&name, &err)),
            }
        }
        // Queued is not taken: wait, by the same deadline, until the output
        // queue is empty, so a port closed straight after a write (or by the
        // run ending) loses nothing. A driver that cannot say how much is
        // queued is taken at its word.
        while port.bytes_to_write().is_ok_and(|queued| queued > 0) {
            if std::time::Instant::now() >= deadline {
                return Err(late());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    /// Close the port and spend the pick. An unknown handle is nothing to do.
    pub fn close(&mut self, handle: u64) {
        self.held.remove(&handle);
    }

    /// How many ports this run holds, picked or open.
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.held.len()
    }
}

/// Open `path` exclusively with `settings`.
fn open_native(path: &str, name: &str, settings: &Settings) -> Result<Native, Fault> {
    // macOS sets a line speed through an ioctl a pseudo-terminal (a virtual
    // port) does not have, and serialport makes that call whenever the
    // speed is not 0. So the port opens at 0 and the speed is set after,
    // with the termios speed as the fallback (`set_speed`).
    let first_speed = if cfg!(target_os = "macos") {
        0
    } else {
        settings.baud_rate
    };
    let builder = serialport::new(path, first_speed)
        .data_bits(settings.data_bits())
        .parity(settings.parity())
        .stop_bits(settings.stop_bits())
        .flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_millis(u64::from(settings.read_timeout_ms)));
    let mut port = builder.open_native().map_err(|err| open_fault(name, err))?;
    // A terminal write blocks until every byte is queued, which on a device
    // that stops reading is for ever, whatever the timeout. Non-blocking, a
    // write takes what fits and `write` above waits by its own deadline.
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd as _;
        let fd = port.as_raw_fd();
        // SAFETY: fcntl on the descriptor this port owns.
        let set = unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
        };
        if !set {
            return Err(failed(name, std::io::Error::last_os_error()));
        }
    }
    if cfg!(target_os = "macos") {
        set_speed(&mut port, name, settings.baud_rate)?;
    }
    Ok(port)
}

/// Change an open port's settings in place.
fn reconfigure(port: &mut Native, name: &str, settings: &Settings) -> Result<(), Fault> {
    set_speed(port, name, settings.baud_rate)?;
    port.set_data_bits(settings.data_bits())
        .and_then(|()| port.set_parity(settings.parity()))
        .and_then(|()| port.set_stop_bits(settings.stop_bits()))
        .map_err(|err| failed(name, err))
}

/// Set the line speed. On macOS: the ioctl that takes any speed, then the
/// termios speed (a pseudo-terminal, or a driver without the ioctl).
fn set_speed(port: &mut Native, name: &str, baud: u32) -> Result<(), Fault> {
    if port.set_baud_rate(baud).is_ok() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd as _;
        let fd = port.as_raw_fd();
        let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `fd` is the open port's descriptor, and `termios` is
        // written by tcgetattr before it is read.
        let set = unsafe {
            libc::tcgetattr(fd, termios.as_mut_ptr()) == 0 && {
                let mut termios = termios.assume_init();
                libc::cfsetspeed(&mut termios, libc::speed_t::from(baud)) == 0
                    && libc::tcsetattr(fd, libc::TCSANOW, &termios) == 0
            }
        };
        if set {
            return Ok(());
        }
    }
    Err(Fault::BadSettings(format!(
        "{name} does not take {baud} baud"
    )))
}

fn open_fault(name: &str, err: serialport::Error) -> Fault {
    match err.kind() {
        serialport::ErrorKind::NoDevice => Fault::Busy(format!(
            "{name} is in use by another program, or was unplugged"
        )),
        serialport::ErrorKind::Io(std::io::ErrorKind::NotFound) => gone(name),
        serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => {
            Fault::Failed(if cfg!(target_os = "linux") {
                format!(
                    "this computer does not let you open {name}: on Linux, add yourself to the \
                     group that owns it (often `dialout`), then sign in again"
                )
            } else {
                format!("this computer does not let you open {name}")
            })
        }
        serialport::ErrorKind::InvalidInput => {
            Fault::BadSettings(format!("{name} does not take these settings: {err}"))
        }
        _ => failed(name, err),
    }
}

fn failed(name: &str, err: impl std::fmt::Display) -> Fault {
    Fault::Failed(format!("{name}: {err}"))
}

fn gone(name: &str) -> Fault {
    Fault::Disconnected(format!("{name} is not connected any more"))
}

/// A failed read or write: the device gone, or the computer's own words.
fn io_fault(name: &str, err: &std::io::Error) -> Fault {
    use std::io::ErrorKind as K;
    if matches!(
        err.kind(),
        K::BrokenPipe
            | K::NotConnected
            | K::UnexpectedEof
            | K::WriteZero
            | K::ConnectionReset
            | K::ConnectionAborted
    ) {
        return gone(name);
    }
    // EIO, ENXIO and ENODEV on macOS and Linux; the device-gone errors on
    // Windows (ERROR_BAD_COMMAND, ERROR_GEN_FAILURE, ERROR_DEVICE_NOT_CONNECTED).
    #[cfg(unix)]
    let gone_codes = [libc::EIO, libc::ENXIO, libc::ENODEV];
    #[cfg(windows)]
    let gone_codes = [22, 31, 1167];
    if err
        .raw_os_error()
        .is_some_and(|code| gone_codes.contains(&code))
    {
        return gone(name);
    }
    failed(name, err)
}

/// What one step of the list answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Use the device shown.
    Use,
    /// Show the next one.
    Next,
    /// Cancel: no device.
    Cancel,
}

/// Go through `count` devices one at a time, asking `ask` about each, until
/// the person uses one or cancels. Round the list at most [`WALK_ROUNDS`]
/// times; then it is a cancel.
pub fn walk(count: usize, mut ask: impl FnMut(usize) -> Step) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let mut at = 0;
    for _ in 0..count.saturating_mul(WALK_ROUNDS) {
        match ask(at) {
            Step::Use => return Some(at),
            Step::Next => at = (at + 1) % count,
            Step::Cancel => return None,
        }
    }
    None
}

/// Ask the person to choose one serial device: `(path, name)`, or None when
/// they cancelled or nobody is there to ask. `nobody_to_ask` is a run with
/// no window and no person (a headless run); a Linux session with no
/// display has nobody to show a list to either.
pub fn choose(title: &str, nobody_to_ask: bool) -> Result<Option<(String, String)>, Fault> {
    if let Some(scripted) = std::env::var_os(PICK_ENV) {
        return scripted_pick(&scripted.to_string_lossy());
    }
    if nobody_to_ask || !display_present() {
        return Ok(None);
    }
    let ports = system_ports()?;
    if ports.is_empty() {
        return Err(Fault::Unavailable(
            "No serial device is connected to this computer. Plug it in, then try again."
                .to_string(),
        ));
    }
    let title: String = title.trim().chars().take(MAX_TITLE_CHARS).collect();
    let picked = walk(ports.len(), |at| {
        ask_person(&title, &ports[at].1, at, ports.len())
    });
    Ok(picked.map(|at| ports[at].clone()))
}

/// The pick [`PICK_ENV`] makes: a path that must be a serial device here,
/// or a cancel.
pub fn scripted_pick(value: &str) -> Result<Option<(String, String)>, Fault> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("cancel") {
        return Ok(None);
    }
    if !is_serial_device(value) {
        return Err(Fault::Unavailable(format!(
            "{PICK_ENV} names `{value}`, which is not a serial device on this computer"
        )));
    }
    Ok(Some((value.to_string(), value.to_string())))
}

/// A character device (macOS, Linux): what every serial port, USB adapter
/// and pseudo-terminal is, and what no ordinary file or folder is.
#[cfg(unix)]
fn is_serial_device(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt as _;
    std::fs::metadata(path).is_ok_and(|meta| meta.file_type().is_char_device())
}

/// `COM3` or `\\.\COM12` (Windows).
#[cfg(windows)]
fn is_serial_device(path: &str) -> bool {
    let name = path.strip_prefix(r"\\.\").unwrap_or(path);
    name.len() > 3
        && name[..3].eq_ignore_ascii_case("COM")
        && name[3..].chars().all(|c| c.is_ascii_digit())
}

/// Linux needs a display for a dialog; macOS and Windows always have one.
/// The USB pick (IC-988) asks the same question.
pub(crate) fn display_present() -> bool {
    if cfg!(target_os = "linux") {
        ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()))
    } else {
        true
    }
}

/// The system's serial devices as `(path, name to show)`. On macOS each
/// device appears twice, as `/dev/tty.*` (which waits for a carrier) and
/// `/dev/cu.*` (which does not); only the `cu` one is listed.
fn system_ports() -> Result<Vec<(String, String)>, Fault> {
    let ports = serialport::available_ports()
        .map_err(|err| Fault::Failed(format!("Krate could not list the serial devices: {err}")))?;
    let has_cu = ports.iter().any(|p| p.port_name.starts_with("/dev/cu."));
    Ok(ports
        .into_iter()
        .filter(|p| !(has_cu && p.port_name.starts_with("/dev/tty.")))
        .map(|p| {
            let name = match &p.port_type {
                serialport::SerialPortType::UsbPort(usb) => {
                    match usb.product.as_deref().or(usb.manufacturer.as_deref()) {
                        Some(label) if !label.trim().is_empty() => {
                            format!("{} ({})", label.trim(), p.port_name)
                        }
                        _ => p.port_name.clone(),
                    }
                }
                _ => p.port_name.clone(),
            };
            (p.port_name, name)
        })
        .collect())
}

/// One step of Krate's list, as a dialog: the device, then Use, Next
/// (when there is more than one) and Cancel.
fn ask_person(title: &str, name: &str, at: usize, count: usize) -> Step {
    const USE: &str = "Use this device";
    const NEXT: &str = "Next device";
    const CANCEL: &str = "Cancel";
    let heading = if title.is_empty() {
        "Choose a serial device"
    } else {
        title
    };
    let mut body = format!(
        "An app asked to talk to a serial device. It reaches only the one you choose, \
         until it closes.\n\n{name}"
    );
    if count > 1 {
        body.push_str(&format!("\n\nDevice {} of {count}.", at + 1));
    }
    let buttons = if count > 1 {
        rfd::MessageButtons::YesNoCancelCustom(USE.into(), NEXT.into(), CANCEL.into())
    } else {
        rfd::MessageButtons::OkCancelCustom(USE.into(), CANCEL.into())
    };
    let answer = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title(heading)
        .set_description(&body)
        .set_buttons(buttons)
        .show();
    match answer {
        rfd::MessageDialogResult::Custom(label) if label == USE => Step::Use,
        rfd::MessageDialogResult::Custom(label) if label == NEXT => Step::Next,
        // Some systems answer a custom button with the plain one it stands
        // in for.
        rfd::MessageDialogResult::Yes | rfd::MessageDialogResult::Ok => Step::Use,
        rfd::MessageDialogResult::No if count > 1 => Step::Next,
        _ => Step::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eight_n_one(read_timeout_ms: u32) -> Settings {
        Settings {
            baud_rate: 115_200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: StopBits::One,
            read_timeout_ms,
        }
    }

    /// The list goes round, uses what the person uses, and a person who
    /// only ever says "next" has cancelled.
    #[test]
    fn the_list_walks_to_the_device_the_person_uses() {
        let mut seen = Vec::new();
        let picked = walk(3, |at| {
            seen.push(at);
            if seen.len() == 5 {
                Step::Use
            } else {
                Step::Next
            }
        });
        assert_eq!(picked, Some(1), "0 1 2 0 1: the fifth shown is device 1");
        assert_eq!(walk(3, |_| Step::Cancel), None);
        let mut asked = 0;
        assert_eq!(
            walk(2, |_| {
                asked += 1;
                Step::Next
            }),
            None
        );
        assert_eq!(asked, 2 * WALK_ROUNDS, "the list ends");
        assert_eq!(walk(0, |_| Step::Use), None, "nothing to choose");
    }

    /// Settings out of range are refused with the range in words.
    #[test]
    fn settings_out_of_range_are_refused_in_words() {
        assert!(eight_n_one(0).check().is_ok());
        for (bad, says) in [
            (
                Settings {
                    baud_rate: 0,
                    ..eight_n_one(0)
                },
                "baud rate",
            ),
            (
                Settings {
                    baud_rate: MAX_BAUD + 1,
                    ..eight_n_one(0)
                },
                "baud rate",
            ),
            (
                Settings {
                    data_bits: 9,
                    ..eight_n_one(0)
                },
                "data bits",
            ),
            (
                Settings {
                    data_bits: 4,
                    ..eight_n_one(0)
                },
                "data bits",
            ),
            (eight_n_one(MAX_WAIT_MS + 1), "a wait"),
        ] {
            match bad.check() {
                Err(Fault::BadSettings(words)) => assert!(words.contains(says), "{words}"),
                other => panic!("{bad:?} must be refused, got {other:?}"),
            }
        }
    }

    /// A scripted pick names a real serial device or nothing: an ordinary
    /// file, a folder and a missing path are refused, and `cancel` is the
    /// person cancelling.
    #[cfg(unix)]
    #[test]
    fn a_scripted_pick_names_a_serial_device_or_cancels() {
        assert_eq!(scripted_pick("cancel"), Ok(None));
        assert_eq!(scripted_pick(""), Ok(None));
        let file = tempfile::NamedTempFile::new().expect("file");
        let dir = tempfile::tempdir().expect("dir");
        for not_a_port in [
            file.path().to_string_lossy().to_string(),
            dir.path().to_string_lossy().to_string(),
            "/no/such/port".to_string(),
        ] {
            assert!(
                matches!(scripted_pick(&not_a_port), Err(Fault::Unavailable(_))),
                "{not_a_port} is not a serial device"
            );
        }
        let (_master, slave) = serialport::TTYPort::pair().expect("pty");
        let path = slave.name().expect("pty name");
        assert_eq!(scripted_pick(&path), Ok(Some((path.clone(), path))));
    }

    /// A pseudo-terminal stands in for the device (IC-985): what the app
    /// writes arrives at the device, what the device sends the app reads, a
    /// read with nothing waiting ends empty when its wait runs out, and a
    /// closed handle is spent.
    #[cfg(unix)]
    #[test]
    fn a_picked_port_writes_reads_times_out_and_closes() {
        let (mut device, slave) = serialport::TTYPort::pair().expect("pty");
        let path = slave.name().expect("pty name");
        drop(slave);
        device
            .set_timeout(Duration::from_secs(5))
            .expect("device wait");

        let mut ports = SerialPorts::new();
        let h = ports.remember(path.clone(), path.clone()).expect("pick");
        assert_eq!(ports.read(h, 16, Some(0)), Err(Fault::NotOpen));
        assert_eq!(ports.write(h, b"x"), Err(Fault::NotOpen));
        ports.open(h, &eight_n_one(200)).expect("open");

        // A write returns once the device has taken the bytes, so the
        // device reads while the app writes.
        let got = std::thread::scope(|scope| {
            let hearing = scope.spawn(|| {
                let mut got = [0u8; 5];
                device.read_exact(&mut got).map(|()| got)
            });
            ports.write(h, b"PING\n").expect("write");
            hearing.join().expect("device thread")
        })
        .expect("the device hears it");
        assert_eq!(&got, b"PING\n");

        device.write_all(b"PONG\n").expect("device answers");
        let mut heard = Vec::new();
        while heard.len() < 5 {
            let more = ports.read(h, 64, Some(2_000)).expect("read");
            assert!(!more.is_empty(), "the answer arrives within the wait");
            heard.extend(more);
        }
        assert_eq!(heard, b"PONG\n");

        // Nothing waiting: the wait runs out and the read is empty, after
        // the port's own wait when the read names none.
        let started = std::time::Instant::now();
        assert_eq!(ports.read(h, 64, None), Ok(Vec::new()));
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(150) && waited < Duration::from_secs(5),
            "waited {waited:?} for the port's 200 ms"
        );
        assert_eq!(ports.read(h, 64, Some(0)), Ok(Vec::new()));

        // Bounded: a read never returns more than asked, a write over the
        // limit is refused whole, a wait over a minute is refused.
        device.write_all(b"abcdef").expect("device sends");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(ports.read(h, 2, Some(1_000)).expect("read").len(), 2);
        assert!(matches!(
            ports.write(h, &vec![0u8; MAX_IO_BYTES + 1]),
            Err(Fault::BadSettings(_))
        ));
        assert!(matches!(
            ports.read(h, 1, Some(MAX_WAIT_MS + 1)),
            Err(Fault::BadSettings(_))
        ));
        // A device that stops taking bytes ends the write with words in
        // bounded time; it never holds the app. (64 KiB fills a
        // pseudo-terminal nobody reads.)
        let started = std::time::Instant::now();
        let mut stuck = None;
        for _ in 0..8 {
            if let Err(fault) = ports.write(h, &vec![b'z'; MAX_IO_BYTES]) {
                stuck = Some(fault);
                break;
            }
        }
        assert!(
            matches!(&stuck, Some(Fault::Failed(words)) if words.contains("in time")),
            "{stuck:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the write ended by its deadline, took {:?}",
            started.elapsed()
        );
        // Let the device catch up before the next checks.
        let mut sink = vec![0u8; 1 << 16];
        device
            .set_timeout(Duration::from_millis(50))
            .expect("device wait");
        while device.read(&mut sink).is_ok_and(|n| n > 0) {}
        device
            .set_timeout(Duration::from_secs(5))
            .expect("device wait");

        // New settings on an open port, in place.
        ports
            .open(
                h,
                &Settings {
                    baud_rate: 9_600,
                    data_bits: 7,
                    parity: Parity::Even,
                    stop_bits: StopBits::Two,
                    read_timeout_ms: 50,
                },
            )
            .expect("reconfigure");

        ports.close(h);
        assert_eq!(ports.read(h, 1, Some(0)), Err(Fault::UnknownHandle));
        assert_eq!(ports.write(h, b"x"), Err(Fault::UnknownHandle));
        assert_eq!(ports.open(h, &eight_n_one(0)), Err(Fault::UnknownHandle));
        assert_eq!(ports.held(), 0);
    }

    /// One pick reaches one port (IC-985): with two devices, the handle the
    /// person gave reaches the first only; a handle never given reaches
    /// neither; and a port held open is the run's alone until it lets go.
    #[cfg(unix)]
    #[test]
    fn one_pick_never_reaches_a_second_port() {
        let (mut first, first_slave) = serialport::TTYPort::pair().expect("pty");
        let (mut second, second_slave) = serialport::TTYPort::pair().expect("pty");
        let first_path = first_slave.name().expect("name");
        let second_path = second_slave.name().expect("name");
        drop((first_slave, second_slave));
        first.set_timeout(Duration::from_secs(5)).expect("wait");
        second
            .set_timeout(Duration::from_millis(300))
            .expect("wait");

        let mut ports = SerialPorts::new();
        let h = ports
            .remember(first_path.clone(), first_path.clone())
            .expect("pick");
        ports.open(h, &eight_n_one(100)).expect("open");
        let got = std::thread::scope(|scope| {
            let hearing = scope.spawn(|| {
                let mut got = [0u8; 10];
                first.read_exact(&mut got).map(|()| got)
            });
            ports.write(h, b"only-first").expect("write");
            hearing.join().expect("device thread")
        })
        .expect("first hears it");
        assert_eq!(&got, b"only-first");
        let mut stray = [0u8; 1];
        assert!(
            second.read(&mut stray).is_err(),
            "the second device hears nothing"
        );

        for forged in [0, h + 1, u64::MAX] {
            assert_eq!(
                ports.open(forged, &eight_n_one(0)),
                Err(Fault::UnknownHandle)
            );
            assert_eq!(ports.write(forged, b"x"), Err(Fault::UnknownHandle));
            assert_eq!(ports.read(forged, 1, Some(0)), Err(Fault::UnknownHandle));
        }

        // Open exclusively: nobody else gets the line while the run holds it.
        // (A speed of 0 on macOS, where a pseudo-terminal refuses the speed
        // ioctl, so only the lock can make this open fail.)
        let probe_speed = if cfg!(target_os = "macos") { 0 } else { 9_600 };
        assert!(
            serialport::new(&first_path, probe_speed)
                .open_native()
                .is_err(),
            "a port held open is the run's alone"
        );
        // Not even shared: a program that opens the device and asks for a
        // shared lock is refused too (or cannot open it at all).
        let shared_too = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&first_path)
            .map(|file| {
                use std::os::fd::AsRawFd as _;
                // SAFETY: flock on a descriptor this test owns.
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) == 0 }
            })
            .unwrap_or(false);
        assert!(!shared_too, "a held port is not shared with anyone");
        // The run ends: every port closes and the lock goes with it.
        drop(ports);
        serialport::new(&first_path, probe_speed)
            .open_native()
            .expect("free again once the run is over");
        let _ = second_path;
    }

    /// A run nobody is at (a headless window) shows no list: the pick is a
    /// cancel, never a dialog that waits for ever.
    #[test]
    fn nobody_to_ask_means_no_list() {
        if std::env::var_os(PICK_ENV).is_some() {
            return; // a scripted pick decides instead
        }
        assert_eq!(choose("Choose", true), Ok(None));
    }

    /// At most [`MAX_PICKED`] ports held at once.
    #[test]
    fn a_run_holds_at_most_eight_ports() {
        let mut ports = SerialPorts::new();
        for n in 0..MAX_PICKED {
            ports
                .remember(format!("/dev/fake{n}"), format!("fake {n}"))
                .expect("room");
        }
        assert!(matches!(
            ports.remember("/dev/one-more".into(), "one more".into()),
            Err(Fault::TooMany(_))
        ));
        ports.close(1);
        let again = ports
            .remember("/dev/again".into(), "again".into())
            .expect("room again");
        assert_eq!(again, MAX_PICKED as u64 + 1, "handles are never reused");
    }
}
