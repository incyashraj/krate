//! USB devices the person picks (`krate:usb`, IC-988): the operating-system
//! half.
//!
//! This file knows nothing of grants or manifests. The Phase 2 host admits
//! every call first -- the manifest names the device's ids as
//! `device.usb:<vendor>:<product>` and the person allowed it -- and hands
//! this file the rule (`admit`) a listed device must pass before it is shown.
//! What it guarantees:
//!
//! - **The app never enumerates.** [`UsbDevices::pick`] shows the person
//!   only connected devices whose ids pass `admit`, and keeps the one they
//!   chose under a fresh handle. No call here takes a device's address, bus
//!   or path from the app.
//! - **Never a device the computer's own drivers serve.** A keyboard or
//!   mouse (any HID interface), storage, audio, video and hubs are refused
//!   whatever the manifest names ([`kept_by_the_computer`]).
//! - **One pick, one device, one interface.** A handle reaches the device
//!   it was picked for. It claims one interface, and every transfer goes to
//!   that interface: bulk and interrupt transfers on its own endpoints, of
//!   the endpoint's own kind and direction, and control requests addressed
//!   to it or one of its endpoints -- never to the whole device, and of the
//!   standard requests only the ones that read.
//! - **Bounded.** At most [`MAX_PICKED`] devices held, [`MAX_TRANSFER_BYTES`]
//!   a bulk or interrupt transfer, [`MAX_CONTROL_BYTES`] a control transfer,
//!   and every transfer waits between 1 ms and [`MAX_WAIT_MS`].
//! - **Everything is let go with the run.** [`UsbDevices`] owns every
//!   device; dropping it, which the run ending does, releases every claimed
//!   interface and closes every device.
//! - **One backend seam.** Everything the system does is behind
//!   [`Backend`] and [`Link`]: nusb on macOS, Windows and Linux, and a fake
//!   ([`fake::FakeUsb`]) for tests, which a debug build also takes from
//!   [`FAKE_ENV`] so a test can drive it through the binary.

use std::{collections::BTreeMap, time::Duration};

/// Devices one run may hold at once.
pub const MAX_PICKED: usize = 8;
/// The most bytes one bulk or interrupt transfer moves.
pub const MAX_TRANSFER_BYTES: usize = 65_536;
/// The most bytes one control transfer moves.
pub const MAX_CONTROL_BYTES: usize = 4_096;
/// The longest wait a transfer may ask for, in milliseconds. The shortest is
/// 1: a transfer with no wait would be cancelled as it starts.
pub const MAX_WAIT_MS: u32 = 60_000;
/// The longest title an app may put on the list, in characters.
pub const MAX_TITLE_CHARS: usize = 200;

/// The person's pick, made by the environment rather than a dialog: a test,
/// or someone driving Krate from a script. The value is the device's ids,
/// `1915:7777`, or `cancel` (or nothing) for a person who cancels. Only the
/// person running Krate sets it; an app cannot. It still passes the same
/// grant, and must name a connected device the app may have.
pub const PICK_ENV: &str = "KRATE_USB_PICK";

/// A debug build only: the devices are [`fake::FakeUsb::standard`]'s instead
/// of the computer's, and what they see is written, a line an event, to the
/// file this names. For a test that drives the fake through the binary. A
/// release build never reads it.
#[cfg(debug_assertions)]
pub const FAKE_ENV: &str = "KRATE_USB_FAKE";

#[cfg(test)]
thread_local! {
    /// A test's scripted pick, for this thread only: the environment is the
    /// whole process's, and tests run side by side.
    pub static TEST_PICK: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The scripted pick ([`PICK_ENV`]), if the person running Krate set one.
pub fn scripted_pick() -> Option<String> {
    #[cfg(test)]
    if let Some(value) = TEST_PICK.with(|pick| pick.borrow().clone()) {
        return Some(value);
    }
    std::env::var(PICK_ENV).ok()
}

/// Why a USB call did not work, in words for a person where it has any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// The app may not have this device (from the caller's `admit`).
    NotAllowed(String),
    /// No device the app may have is connected.
    Unavailable(String),
    /// The handle was never picked this run, or was closed.
    UnknownHandle,
    /// No interface claimed yet.
    NotClaimed,
    /// The computer's own drivers serve it.
    NotForApps(String),
    /// A request Krate does not send.
    BadRequest(String),
    /// Another program or a driver has it.
    Busy(String),
    /// The device went away.
    Disconnected(String),
    /// The device refused the transfer.
    Stalled(String),
    /// A transfer out did not finish in its wait.
    TimedOut(String),
    /// [`MAX_PICKED`] devices are already held.
    TooMany(String),
    /// The computer refused.
    Failed(String),
}

/// A connected device as the system lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// How the backend finds it again. Never handed to the app.
    pub token: String,
    pub vendor_id: u16,
    pub product_id: u16,
    /// bDeviceClass.
    pub class: u8,
    /// What the person is shown.
    pub name: String,
    /// Each interface's number and class, as the system lists them.
    pub interfaces: Vec<(u8, u8)>,
}

/// An endpoint's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bulk,
    Interrupt,
    /// Isochronous or control: never reached by an app.
    Other,
}

/// One endpoint of an interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    /// The address: the number, with 0x80 set for IN.
    pub address: u8,
    pub kind: Kind,
    /// The largest packet it moves, in bytes.
    pub max_packet: usize,
}

impl Endpoint {
    fn is_in(&self) -> bool {
        self.address & 0x80 != 0
    }
}

/// One interface of the device's active configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    /// bInterfaceClass.
    pub class: u8,
    pub endpoints: Vec<Endpoint>,
}

/// The kind of a control request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestType {
    Standard,
    Class,
    Vendor,
}

/// Who a control request is for: never the whole device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipient {
    Interface,
    Endpoint,
}

/// A control request's setup, the direction aside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setup {
    pub request_type: RequestType,
    pub recipient: Recipient,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

/// The standard requests an app may send, all of them reads: GET_STATUS
/// and GET_DESCRIPTOR. Every other standard request -- SET_ADDRESS,
/// SET_CONFIGURATION, SET_INTERFACE, SET_FEATURE, CLEAR_FEATURE,
/// SET_DESCRIPTOR -- changes how the computer and the device agree to talk,
/// which is the computer's to decide. A stalled endpoint is cleared by
/// Krate, not by the app.
const STANDARD_READS: [u8; 2] = [0x00, 0x06];

/// What the computer's own drivers serve, by USB class code, in words: such
/// a device or interface is never handed to an app, whatever ids the
/// manifest names. Keyboards and mice are HID; so are security keys, which
/// are a later item of their own.
pub fn kept_by_the_computer(class: u8) -> Option<&'static str> {
    match class {
        0x01 => Some("an audio device"),
        0x03 => Some("a keyboard, mouse or other input device"),
        0x08 => Some("a storage device"),
        0x09 => Some("a USB hub"),
        0x0e => Some("a camera or other video device"),
        0x10 => Some("an audio and video device"),
        _ => None,
    }
}

/// Is this device one the computer keeps? Its own class says so, or (for a
/// device whose class is per interface) every interface it has does.
fn kept_device(listed: &Listed) -> Option<&'static str> {
    if let Some(what) = kept_by_the_computer(listed.class) {
        return Some(what);
    }
    let mut kept = listed
        .interfaces
        .iter()
        .map(|&(_, class)| kept_by_the_computer(class));
    match kept.next() {
        Some(Some(first)) if kept.all(|other| other.is_some()) => Some(first),
        _ => None,
    }
}

/// The operating system's USB, or a stand-in for it.
pub trait Backend: Send {
    /// The connected devices.
    fn list(&self) -> Result<Vec<Listed>, Fault>;
    /// Open one, for this run.
    fn open(&self, listed: &Listed) -> Result<Box<dyn Link>, Fault>;
}

/// One open device. Dropping it releases its interface and closes it.
pub trait Link: Send {
    /// Interface `number` of the active configuration, before any claim.
    fn interface(&self, number: u8) -> Option<Interface>;
    /// Claim interface `number` for this run.
    fn claim(&mut self, number: u8) -> Result<(), Fault>;
    /// Read up to `len` bytes (a whole number of packets) from `endpoint`,
    /// within `wait`. What arrived, empty when nothing did.
    fn read(&mut self, endpoint: Endpoint, len: usize, wait: Duration) -> Result<Vec<u8>, Fault>;
    /// Write every byte to `endpoint` within `wait`.
    fn write(&mut self, endpoint: Endpoint, bytes: &[u8], wait: Duration) -> Result<(), Fault>;
    /// A control request that reads up to `length` bytes.
    fn control_in(&mut self, setup: Setup, length: u16, wait: Duration) -> Result<Vec<u8>, Fault>;
    /// A control request that writes `data`.
    fn control_out(&mut self, setup: Setup, data: &[u8], wait: Duration) -> Result<(), Fault>;
}

/// The backend for this run: in a debug build [`FAKE_ENV`] chooses the fake,
/// otherwise the computer's own USB.
pub fn system_backend() -> Box<dyn Backend> {
    #[cfg(debug_assertions)]
    if let Some(log) = std::env::var_os(FAKE_ENV).filter(|log| !log.is_empty()) {
        return Box::new(fake::FakeUsb::standard().with_log_file(log.into()));
    }
    Box::new(native::Nusb::default())
}

/// One device the person picked this run.
struct Held {
    listed: Listed,
    link: Box<dyn Link>,
    claimed: Option<Interface>,
}

/// The devices one run picked, by handle. Dropped with the run, which
/// releases and closes every one.
pub struct UsbDevices {
    backend: Box<dyn Backend>,
    next: u64,
    held: BTreeMap<u64, Held>,
}

impl UsbDevices {
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self {
            backend,
            next: 1,
            held: BTreeMap::new(),
        }
    }

    /// The ids of the device behind `handle`, so the caller can check the
    /// grant again on every call.
    pub fn ids(&self, handle: u64) -> Option<(u16, u16)> {
        self.held
            .get(&handle)
            .map(|held| (held.listed.vendor_id, held.listed.product_id))
    }

    /// Ask the person to choose one device that passes `admit`, open it,
    /// and keep it under a fresh handle: `(handle, device)`, or None when
    /// they cancelled or nobody is there to ask. `scripted` is the
    /// [`PICK_ENV`] pick when one is set: it answers for the person, and the
    /// device it names must pass the same rules.
    pub fn pick(
        &mut self,
        title: &str,
        nobody_to_ask: bool,
        scripted: Option<&str>,
        admit: &dyn Fn(u16, u16) -> Result<(), Fault>,
    ) -> Result<Option<(u64, Listed)>, Fault> {
        if self.held.len() >= MAX_PICKED {
            return Err(Fault::TooMany(format!(
                "this app already holds {MAX_PICKED} USB devices; close one first"
            )));
        }
        let chosen = match scripted {
            Some(value) => self.scripted_pick(value, admit)?,
            None if nobody_to_ask || !crate::serial_host::display_present() => None,
            None => {
                let shown = candidates(self.backend.list()?, admit)?;
                let title: String = title.trim().chars().take(MAX_TITLE_CHARS).collect();
                crate::serial_host::walk(shown.len(), |at| {
                    ask_person(&title, &shown[at].name, at, shown.len())
                })
                .map(|at| shown[at].clone())
            }
        };
        let Some(listed) = chosen else {
            return Ok(None);
        };
        let link = self.backend.open(&listed)?;
        let handle = self.next;
        self.next += 1;
        self.held.insert(
            handle,
            Held {
                listed: listed.clone(),
                link,
                claimed: None,
            },
        );
        Ok(Some((handle, listed)))
    }

    /// The pick [`PICK_ENV`] makes: ids that must name a connected device
    /// the app may have, or a cancel.
    fn scripted_pick(
        &self,
        value: &str,
        admit: &dyn Fn(u16, u16) -> Result<(), Fault>,
    ) -> Result<Option<Listed>, Fault> {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("cancel") {
            return Ok(None);
        }
        let (vendor, product) = krate_manifest::validate_usb_ids(value).map_err(|_| {
            Fault::Unavailable(format!(
                "{PICK_ENV} names `{value}`, which is not a USB device's ids (such as \
                 1915:7777) or `cancel`"
            ))
        })?;
        admit(vendor, product)?;
        let listed = self
            .backend
            .list()?
            .into_iter()
            .find(|listed| listed.vendor_id == vendor && listed.product_id == product)
            .ok_or_else(|| {
                Fault::Unavailable(format!(
                    "No USB device {} is connected to this computer.",
                    krate_manifest::usb_ids(vendor, product)
                ))
            })?;
        if let Some(what) = kept_device(&listed) {
            return Err(not_for_apps(&listed.name, what));
        }
        Ok(Some(listed))
    }

    /// Claim interface `number` of a picked device. One a pick: the same one
    /// again is nothing to do, another is refused.
    pub fn claim(&mut self, handle: u64, number: u8) -> Result<(), Fault> {
        let held = self.held.get_mut(&handle).ok_or(Fault::UnknownHandle)?;
        if let Some(claimed) = &held.claimed {
            if claimed.number == number {
                return Ok(());
            }
            return Err(Fault::BadRequest(format!(
                "this pick already claimed interface {}; one interface a pick, so pick the \
                 device again to reach interface {number}",
                claimed.number
            )));
        }
        let interface = held.link.interface(number).ok_or_else(|| {
            Fault::BadRequest(format!("{} has no interface {number}", held.listed.name))
        })?;
        if let Some(what) = kept_by_the_computer(interface.class) {
            return Err(Fault::NotForApps(format!(
                "interface {number} of {} is {what}, which the computer's own drivers serve; \
                 Krate does not hand it to an app",
                held.listed.name
            )));
        }
        held.link.claim(number)?;
        held.claimed = Some(interface);
        Ok(())
    }

    /// Read from bulk or interrupt endpoint `address` of the claimed
    /// interface: up to `max` bytes rounded up to whole packets, waiting up
    /// to `wait_ms`. Empty when the wait ran out.
    pub fn read(
        &mut self,
        handle: u64,
        kind: Kind,
        address: u8,
        max: u32,
        wait_ms: u32,
    ) -> Result<Vec<u8>, Fault> {
        let wait = check_wait(wait_ms)?;
        let held = self.held.get_mut(&handle).ok_or(Fault::UnknownHandle)?;
        let endpoint = own_endpoint(held, kind, address, true)?;
        if max == 0 {
            return Ok(Vec::new());
        }
        let packet = endpoint.max_packet.max(1);
        // USB delivers whole packets: ask for whole ones, never past the
        // limit.
        let len = (max as usize)
            .min(MAX_TRANSFER_BYTES)
            .div_ceil(packet)
            .saturating_mul(packet)
            .min(MAX_TRANSFER_BYTES / packet * packet)
            .max(packet);
        let mut got = held.link.read(endpoint, len, wait)?;
        got.truncate(len);
        Ok(got)
    }

    /// Write `bytes` to bulk or interrupt endpoint `address` of the claimed
    /// interface within `wait_ms`.
    pub fn write(
        &mut self,
        handle: u64,
        kind: Kind,
        address: u8,
        bytes: &[u8],
        wait_ms: u32,
    ) -> Result<(), Fault> {
        let wait = check_wait(wait_ms)?;
        if bytes.len() > MAX_TRANSFER_BYTES {
            return Err(Fault::BadRequest(format!(
                "a transfer is at most {MAX_TRANSFER_BYTES} bytes, not {}; send the rest in \
                 another call",
                bytes.len()
            )));
        }
        let held = self.held.get_mut(&handle).ok_or(Fault::UnknownHandle)?;
        let endpoint = own_endpoint(held, kind, address, false)?;
        held.link.write(endpoint, bytes, wait)
    }

    /// A control request that reads, for the claimed interface or one of its
    /// endpoints.
    pub fn control_in(
        &mut self,
        handle: u64,
        setup: Setup,
        length: u16,
        wait_ms: u32,
    ) -> Result<Vec<u8>, Fault> {
        let wait = check_wait(wait_ms)?;
        if usize::from(length) > MAX_CONTROL_BYTES {
            return Err(Fault::BadRequest(format!(
                "a control transfer is at most {MAX_CONTROL_BYTES} bytes, not {length}"
            )));
        }
        let held = self.held.get_mut(&handle).ok_or(Fault::UnknownHandle)?;
        check_setup(held, &setup, true)?;
        let mut got = held.link.control_in(setup, length, wait)?;
        got.truncate(usize::from(length));
        Ok(got)
    }

    /// A control request that writes, for the claimed interface or one of
    /// its endpoints.
    pub fn control_out(
        &mut self,
        handle: u64,
        setup: Setup,
        data: &[u8],
        wait_ms: u32,
    ) -> Result<(), Fault> {
        let wait = check_wait(wait_ms)?;
        if data.len() > MAX_CONTROL_BYTES {
            return Err(Fault::BadRequest(format!(
                "a control transfer is at most {MAX_CONTROL_BYTES} bytes, not {}",
                data.len()
            )));
        }
        let held = self.held.get_mut(&handle).ok_or(Fault::UnknownHandle)?;
        check_setup(held, &setup, false)?;
        held.link.control_out(setup, data, wait)
    }

    /// Release the interface, close the device and spend the pick. An
    /// unknown handle is nothing to do.
    pub fn close(&mut self, handle: u64) {
        self.held.remove(&handle);
    }

    /// How many devices this run holds.
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.held.len()
    }
}

/// The devices to show: those `admit` lets the app have and the computer
/// does not keep. None to show is a refusal in words: the kept device's
/// when one the app may have was the computer's, else nothing connected.
fn candidates(
    listed: Vec<Listed>,
    admit: &dyn Fn(u16, u16) -> Result<(), Fault>,
) -> Result<Vec<Listed>, Fault> {
    let mut kept = None;
    let mut shown = Vec::new();
    for device in listed {
        if admit(device.vendor_id, device.product_id).is_err() {
            continue;
        }
        match kept_device(&device) {
            Some(what) => {
                kept.get_or_insert_with(|| not_for_apps(&device.name, what));
            }
            None => shown.push(device),
        }
    }
    if shown.is_empty() {
        return Err(kept.unwrap_or_else(|| {
            Fault::Unavailable(
                "No USB device this app is for is connected to this computer. Plug it in, \
                 then try again."
                    .to_string(),
            )
        }));
    }
    Ok(shown)
}

fn not_for_apps(name: &str, what: &str) -> Fault {
    Fault::NotForApps(format!(
        "{name} is {what}, which the computer's own drivers serve; Krate does not hand it to an \
         app"
    ))
}

fn check_wait(ms: u32) -> Result<Duration, Fault> {
    if !(1..=MAX_WAIT_MS).contains(&ms) {
        return Err(Fault::BadRequest(format!(
            "a wait is from 1 to {MAX_WAIT_MS} milliseconds, not {ms}"
        )));
    }
    Ok(Duration::from_millis(u64::from(ms)))
}

/// The claimed interface's endpoint at `address`, if it is of this kind and
/// direction.
fn own_endpoint(held: &Held, kind: Kind, address: u8, is_in: bool) -> Result<Endpoint, Fault> {
    let claimed = held.claimed.as_ref().ok_or(Fault::NotClaimed)?;
    let endpoint = claimed
        .endpoints
        .iter()
        .find(|endpoint| endpoint.address == address)
        .copied()
        .ok_or_else(|| {
            Fault::BadRequest(format!(
                "endpoint {address:#04x} is not one of interface {}'s",
                claimed.number
            ))
        })?;
    let named = |kind: Kind| match kind {
        Kind::Bulk => "bulk",
        Kind::Interrupt => "interrupt",
        Kind::Other => "isochronous or control",
    };
    if endpoint.kind != kind {
        return Err(Fault::BadRequest(format!(
            "endpoint {address:#04x} is a {} endpoint, and this call is for {} endpoints",
            named(endpoint.kind),
            named(kind)
        )));
    }
    if endpoint.is_in() != is_in {
        return Err(Fault::BadRequest(if endpoint.is_in() {
            format!("endpoint {address:#04x} is an IN endpoint: read from it")
        } else {
            format!("endpoint {address:#04x} is an OUT endpoint: write to it")
        }));
    }
    Ok(endpoint)
}

/// A control request goes to the claimed interface or one of its endpoints
/// (the low byte of the index names which), and of the standard requests
/// only the reads are sent.
fn check_setup(held: &Held, setup: &Setup, is_in: bool) -> Result<(), Fault> {
    let claimed = held.claimed.as_ref().ok_or(Fault::NotClaimed)?;
    let target = (setup.index & 0x00ff) as u8;
    match setup.recipient {
        Recipient::Interface if target != claimed.number => {
            return Err(Fault::BadRequest(format!(
                "a control request for interface {target}: this pick claimed interface {}, and \
                 a request reaches only that one",
                claimed.number
            )));
        }
        Recipient::Endpoint
            if !claimed
                .endpoints
                .iter()
                .any(|endpoint| endpoint.address == target) =>
        {
            return Err(Fault::BadRequest(format!(
                "a control request for endpoint {target:#04x}, which is not one of interface \
                 {}'s",
                claimed.number
            )));
        }
        _ => {}
    }
    if setup.request_type == RequestType::Standard
        && !(is_in && STANDARD_READS.contains(&setup.request))
    {
        return Err(Fault::BadRequest(format!(
            "standard request {:#04x} is the computer's to send; an app sends class and vendor \
             requests, and of the standard ones only GET_STATUS and GET_DESCRIPTOR",
            setup.request
        )));
    }
    Ok(())
}

/// One step of Krate's list, as a dialog: the device, then Use, Next (when
/// there is more than one) and Cancel.
fn ask_person(title: &str, name: &str, at: usize, count: usize) -> crate::serial_host::Step {
    use crate::serial_host::Step;
    const USE: &str = "Use this device";
    const NEXT: &str = "Next device";
    const CANCEL: &str = "Cancel";
    let heading = if title.is_empty() {
        "Choose a USB device"
    } else {
        title
    };
    let mut body = format!(
        "An app asked to talk to a USB device. It reaches only the one you choose, until it \
         closes.\n\n{name}"
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
        rfd::MessageDialogResult::Yes | rfd::MessageDialogResult::Ok => Step::Use,
        rfd::MessageDialogResult::No if count > 1 => Step::Next,
        _ => Step::Cancel,
    }
}

/// The computer's own USB, through nusb.
mod native {
    use super::{
        Backend, Endpoint, Fault, Interface, Kind, Link, Listed, Recipient, RequestType, Setup,
    };
    use nusb::{
        transfer::{
            Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Interrupt, Out, TransferError,
        },
        MaybeFuture as _,
    };
    use std::{collections::HashMap, sync::Mutex, time::Duration};

    /// The system's devices, and the last list's entries by token so a pick
    /// opens the device it showed.
    #[derive(Default)]
    pub struct Nusb {
        seen: Mutex<HashMap<String, nusb::DeviceInfo>>,
    }

    impl Backend for Nusb {
        fn list(&self) -> Result<Vec<Listed>, Fault> {
            let devices = nusb::list_devices().wait().map_err(|err| {
                Fault::Failed(format!("Krate could not list the USB devices: {err}"))
            })?;
            let mut seen = self
                .seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            seen.clear();
            let mut listed = Vec::new();
            for info in devices {
                let token = format!("{:?}", info.id());
                let ids = krate_manifest::usb_ids(info.vendor_id(), info.product_id());
                let label = info
                    .product_string()
                    .or(info.manufacturer_string())
                    .map(str::trim)
                    .filter(|label| !label.is_empty());
                let name = match label {
                    Some(label) => format!("{label} ({ids})"),
                    None => format!("USB device {ids}"),
                };
                listed.push(Listed {
                    token: token.clone(),
                    vendor_id: info.vendor_id(),
                    product_id: info.product_id(),
                    class: info.class(),
                    name,
                    interfaces: info
                        .interfaces()
                        .map(|interface| (interface.interface_number(), interface.class()))
                        .collect(),
                });
                seen.insert(token, info);
            }
            Ok(listed)
        }

        fn open(&self, listed: &Listed) -> Result<Box<dyn Link>, Fault> {
            let info = self
                .seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&listed.token)
                .cloned()
                .ok_or_else(|| gone(&listed.name))?;
            let device = info
                .open()
                .wait()
                .map_err(|err| open_fault(&listed.name, listed, &err))?;
            Ok(Box::new(NusbLink {
                device,
                interface: None,
                endpoints: HashMap::new(),
                listed: listed.clone(),
            }))
        }
    }

    /// An endpoint opened for transfers, by its kind and direction.
    enum Ep {
        BulkIn(nusb::Endpoint<Bulk, In>),
        BulkOut(nusb::Endpoint<Bulk, Out>),
        InterruptIn(nusb::Endpoint<Interrupt, In>),
        InterruptOut(nusb::Endpoint<Interrupt, Out>),
    }

    /// One open device. Dropping it drops the endpoints, then the claimed
    /// interface (which releases it), then the device (which closes it).
    struct NusbLink {
        endpoints: HashMap<u8, Ep>,
        interface: Option<nusb::Interface>,
        device: nusb::Device,
        listed: Listed,
    }

    impl NusbLink {
        fn endpoint(&mut self, endpoint: Endpoint) -> Result<&mut Ep, Fault> {
            let name = self.listed.name.clone();
            let interface = self.interface.as_ref().ok_or(Fault::NotClaimed)?;
            if !self.endpoints.contains_key(&endpoint.address) {
                let address = endpoint.address;
                let fail = |err: nusb::Error| Fault::Failed(format!("{name}: {err}"));
                let opened = match (endpoint.kind, endpoint.address & 0x80 != 0) {
                    (Kind::Bulk, true) => Ep::BulkIn(interface.endpoint(address).map_err(fail)?),
                    (Kind::Bulk, false) => Ep::BulkOut(interface.endpoint(address).map_err(fail)?),
                    (Kind::Interrupt, true) => {
                        Ep::InterruptIn(interface.endpoint(address).map_err(fail)?)
                    }
                    (Kind::Interrupt, false) => {
                        Ep::InterruptOut(interface.endpoint(address).map_err(fail)?)
                    }
                    (Kind::Other, _) => {
                        return Err(Fault::BadRequest(format!(
                            "endpoint {address:#04x} is not a bulk or interrupt endpoint"
                        )))
                    }
                };
                self.endpoints.insert(address, opened);
            }
            Ok(self
                .endpoints
                .get_mut(&endpoint.address)
                .expect("opened above"))
        }
    }

    impl Link for NusbLink {
        fn interface(&self, number: u8) -> Option<Interface> {
            let config = self.device.active_configuration().ok()?;
            let group = config
                .interfaces()
                .find(|group| group.interface_number() == number)?;
            // A claimed interface starts at its first alternate setting, and
            // an app cannot change it (SET_INTERFACE is the computer's).
            let alt = group.first_alt_setting();
            Some(Interface {
                number,
                class: alt.class(),
                endpoints: alt
                    .endpoints()
                    .map(|endpoint| Endpoint {
                        address: endpoint.address(),
                        kind: match endpoint.transfer_type() {
                            nusb::descriptors::TransferType::Bulk => Kind::Bulk,
                            nusb::descriptors::TransferType::Interrupt => Kind::Interrupt,
                            _ => Kind::Other,
                        },
                        max_packet: endpoint.max_packet_size(),
                    })
                    .collect(),
            })
        }

        fn claim(&mut self, number: u8) -> Result<(), Fault> {
            // Never detaches a driver the computer bound: a device a driver
            // holds is busy, not taken.
            let interface = self
                .device
                .claim_interface(number)
                .wait()
                .map_err(|err| claim_fault(&self.listed.name, number, &err))?;
            self.interface = Some(interface);
            Ok(())
        }

        fn read(
            &mut self,
            endpoint: Endpoint,
            len: usize,
            wait: Duration,
        ) -> Result<Vec<u8>, Fault> {
            let name = self.listed.name.clone();
            let completion = match self.endpoint(endpoint)? {
                Ep::BulkIn(ep) => ep.transfer_blocking(Buffer::new(len), wait),
                Ep::InterruptIn(ep) => ep.transfer_blocking(Buffer::new(len), wait),
                _ => unreachable!("an IN endpoint is opened IN"),
            };
            let actual = completion.actual_len;
            match completion.status {
                Ok(()) => Ok(completion.buffer.into_vec()),
                // The wait ran out: what arrived before it, maybe nothing.
                Err(TransferError::Cancelled) => {
                    let mut got = completion.buffer.into_vec();
                    got.truncate(actual);
                    Ok(got)
                }
                Err(TransferError::Stall) => {
                    self.clear_stall(endpoint.address);
                    Err(stalled(&name, endpoint.address))
                }
                Err(err) => Err(transfer_fault(&name, err)),
            }
        }

        fn write(&mut self, endpoint: Endpoint, bytes: &[u8], wait: Duration) -> Result<(), Fault> {
            let name = self.listed.name.clone();
            let buffer = Buffer::from(bytes.to_vec());
            let completion = match self.endpoint(endpoint)? {
                Ep::BulkOut(ep) => ep.transfer_blocking(buffer, wait),
                Ep::InterruptOut(ep) => ep.transfer_blocking(buffer, wait),
                _ => unreachable!("an OUT endpoint is opened OUT"),
            };
            match completion.status {
                Ok(()) => Ok(()),
                Err(TransferError::Cancelled) => Err(Fault::TimedOut(format!(
                    "{name} took {} of {} bytes in the wait",
                    completion.actual_len,
                    bytes.len()
                ))),
                Err(TransferError::Stall) => {
                    self.clear_stall(endpoint.address);
                    Err(stalled(&name, endpoint.address))
                }
                Err(err) => Err(transfer_fault(&name, err)),
            }
        }

        fn control_in(
            &mut self,
            setup: Setup,
            length: u16,
            wait: Duration,
        ) -> Result<Vec<u8>, Fault> {
            let interface = self.interface.as_ref().ok_or(Fault::NotClaimed)?;
            interface
                .control_in(
                    ControlIn {
                        control_type: control_type(setup.request_type),
                        recipient: recipient(setup.recipient),
                        request: setup.request,
                        value: setup.value,
                        index: setup.index,
                        length,
                    },
                    wait,
                )
                .wait()
                .map_err(|err| control_fault(&self.listed.name, err))
        }

        fn control_out(&mut self, setup: Setup, data: &[u8], wait: Duration) -> Result<(), Fault> {
            let interface = self.interface.as_ref().ok_or(Fault::NotClaimed)?;
            interface
                .control_out(
                    ControlOut {
                        control_type: control_type(setup.request_type),
                        recipient: recipient(setup.recipient),
                        request: setup.request,
                        value: setup.value,
                        index: setup.index,
                        data,
                    },
                    wait,
                )
                .wait()
                .map_err(|err| control_fault(&self.listed.name, err))
        }
    }

    impl NusbLink {
        /// Clear a stalled endpoint, so the next transfer can go. Best
        /// effort: a device that stays stalled answers the next one stalled.
        fn clear_stall(&mut self, address: u8) {
            let cleared = match self.endpoints.get_mut(&address) {
                Some(Ep::BulkIn(ep)) => ep.clear_halt().wait(),
                Some(Ep::BulkOut(ep)) => ep.clear_halt().wait(),
                Some(Ep::InterruptIn(ep)) => ep.clear_halt().wait(),
                Some(Ep::InterruptOut(ep)) => ep.clear_halt().wait(),
                None => Ok(()),
            };
            if cleared.is_err() {
                self.endpoints.remove(&address);
            }
        }
    }

    fn control_type(request_type: RequestType) -> ControlType {
        match request_type {
            RequestType::Standard => ControlType::Standard,
            RequestType::Class => ControlType::Class,
            RequestType::Vendor => ControlType::Vendor,
        }
    }

    fn recipient(recipient: Recipient) -> nusb::transfer::Recipient {
        match recipient {
            Recipient::Interface => nusb::transfer::Recipient::Interface,
            Recipient::Endpoint => nusb::transfer::Recipient::Endpoint,
        }
    }

    fn gone(name: &str) -> Fault {
        Fault::Disconnected(format!("{name} is not connected any more"))
    }

    fn stalled(name: &str, address: u8) -> Fault {
        Fault::Stalled(format!(
            "{name} refused the transfer on endpoint {address:#04x}; Krate cleared it, so the \
             next transfer can go"
        ))
    }

    fn transfer_fault(name: &str, err: TransferError) -> Fault {
        match err {
            TransferError::Disconnected => gone(name),
            TransferError::Stall => Fault::Stalled(format!("{name} refused the transfer")),
            TransferError::Cancelled => Fault::TimedOut(format!("{name} did not answer in time")),
            other => Fault::Failed(format!("{name}: {other}")),
        }
    }

    fn control_fault(name: &str, err: TransferError) -> Fault {
        match err {
            TransferError::Stall => Fault::Stalled(format!("{name} refused the request")),
            TransferError::Cancelled => {
                Fault::TimedOut(format!("{name} did not answer the request in time"))
            }
            other => transfer_fault(name, other),
        }
    }

    fn open_fault(name: &str, listed: &Listed, err: &nusb::Error) -> Fault {
        match err.kind() {
            nusb::ErrorKind::Disconnected | nusb::ErrorKind::NotFound => gone(name),
            nusb::ErrorKind::Busy => Fault::Busy(format!("{name} is in use by another program")),
            nusb::ErrorKind::PermissionDenied => Fault::Failed(if cfg!(target_os = "linux") {
                format!(
                    "this computer does not let your account open {name}. On Linux, allow it \
                     with a udev rule: put the line SUBSYSTEM==\"usb\", \
                     ATTRS{{idVendor}}==\"{:04x}\", ATTRS{{idProduct}}==\"{:04x}\", \
                     TAG+=\"uaccess\" in a file such as /etc/udev/rules.d/70-krate-usb.rules, \
                     run `sudo udevadm control --reload`, then unplug the device and plug it \
                     back in",
                    listed.vendor_id, listed.product_id
                )
            } else {
                format!("this computer does not let Krate open {name}")
            }),
            _ => Fault::Failed(format!("{name}: {err}")),
        }
    }

    fn claim_fault(name: &str, number: u8, err: &nusb::Error) -> Fault {
        match err.kind() {
            nusb::ErrorKind::Disconnected => gone(name),
            nusb::ErrorKind::Busy => Fault::Busy(format!(
                "interface {number} of {name} is in use by another program or a driver of the \
                 computer's own"
            )),
            nusb::ErrorKind::NotFound | nusb::ErrorKind::Unsupported if cfg!(windows) => {
                Fault::Failed(format!(
                    "Windows cannot hand interface {number} of {name} to an app: it needs the \
                     WinUSB driver (the device maker's installer, or Zadig, sets it up)"
                ))
            }
            nusb::ErrorKind::NotFound => {
                Fault::BadRequest(format!("{name} has no interface {number}"))
            }
            _ => Fault::Failed(format!("{name}: {err}")),
        }
    }
}

/// A stand-in for the computer's USB: tests, and a debug build's binary
/// under [`FAKE_ENV`].
#[cfg(any(test, debug_assertions))]
pub mod fake {
    use super::{Backend, Endpoint, Fault, Interface, Kind, Link, Listed, RequestType, Setup};
    use std::{
        collections::{BTreeMap, BTreeSet, VecDeque},
        io::Write as _,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };

    /// Bytes written with this content never arrive: the device takes
    /// nothing, and the write runs out its wait.
    pub const NEVER_TAKEN: &[u8] = b"SLOW";
    /// Bytes written with this content are refused: the endpoint stalls.
    pub const REFUSED: &[u8] = b"STALL";

    /// What the devices saw, shared by every link the backend opens.
    #[derive(Debug, Default)]
    pub struct Log {
        /// One line an event: `open 1915:7777`, `claim 1915:7777 0`,
        /// `out 1915:7777 0x01 50494e47`, `release 1915:7777 0`,
        /// `close 1915:7777`.
        pub events: Vec<String>,
        /// Interfaces claimed and not yet released, by device ids.
        pub claimed: BTreeSet<(String, u8)>,
        /// Devices open now.
        pub open: usize,
        file: Option<PathBuf>,
    }

    impl Log {
        fn note(&mut self, event: String) {
            if let Some(file) = &self.file {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(file)
                {
                    let _ = writeln!(file, "{event}");
                }
            }
            self.events.push(event);
        }
    }

    /// One fake device: what it lists as, and its interfaces.
    #[derive(Debug, Clone)]
    pub struct FakeDevice {
        pub listed: Listed,
        pub interfaces: Vec<Interface>,
    }

    /// The fake backend.
    pub struct FakeUsb {
        devices: Vec<FakeDevice>,
        log: Arc<Mutex<Log>>,
    }

    fn endpoint(address: u8, kind: Kind, max_packet: usize) -> Endpoint {
        Endpoint {
            address,
            kind,
            max_packet,
        }
    }

    /// A device with a vendor interface 0 that echoes: bytes out on 0x01
    /// come back on 0x81 (bulk), out on 0x02 back on 0x82 (interrupt).
    fn echo(vendor: u16, product: u16, name: &str, extra: Vec<Interface>) -> FakeDevice {
        let mut interfaces = vec![Interface {
            number: 0,
            class: 0xff,
            endpoints: vec![
                endpoint(0x01, Kind::Bulk, 64),
                endpoint(0x81, Kind::Bulk, 64),
                endpoint(0x02, Kind::Interrupt, 8),
                endpoint(0x82, Kind::Interrupt, 8),
            ],
        }];
        interfaces.extend(extra);
        device(vendor, product, 0xff, name, interfaces)
    }

    fn device(
        vendor: u16,
        product: u16,
        class: u8,
        name: &str,
        interfaces: Vec<Interface>,
    ) -> FakeDevice {
        FakeDevice {
            listed: Listed {
                token: format!("fake:{vendor:04x}:{product:04x}"),
                vendor_id: vendor,
                product_id: product,
                class,
                name: name.to_string(),
                interfaces: interfaces.iter().map(|i| (i.number, i.class)).collect(),
            },
            interfaces,
        }
    }

    fn hid(number: u8, address: u8) -> Interface {
        Interface {
            number,
            class: 0x03,
            endpoints: vec![endpoint(address, Kind::Interrupt, 8)],
        }
    }

    impl FakeUsb {
        pub fn new(devices: Vec<FakeDevice>) -> Self {
            Self {
                devices,
                log: Arc::default(),
            }
        }

        /// The devices a test meets:
        ///
        /// - `1915:7777` "Radio dongle": vendor class, an echoing interface
        ///   0, and a HID interface 1 (interrupt IN 0x83).
        /// - `1915:7778` "Second dongle": the same, a different model.
        /// - `1915:0101` "Keyboard": every interface HID.
        /// - `1915:0202` "Thumb drive": a storage interface.
        /// - `1915:0909` "Hub": a hub.
        pub fn standard() -> Self {
            Self::new(vec![
                echo(
                    0x1915,
                    0x7777,
                    "Radio dongle (1915:7777)",
                    vec![hid(1, 0x83)],
                ),
                echo(0x1915, 0x7778, "Second dongle (1915:7778)", Vec::new()),
                device(
                    0x1915,
                    0x0101,
                    0x00,
                    "Keyboard (1915:0101)",
                    vec![hid(0, 0x81)],
                ),
                device(
                    0x1915,
                    0x0202,
                    0x00,
                    "Thumb drive (1915:0202)",
                    vec![Interface {
                        number: 0,
                        class: 0x08,
                        endpoints: vec![
                            endpoint(0x01, Kind::Bulk, 512),
                            endpoint(0x81, Kind::Bulk, 512),
                        ],
                    }],
                ),
                device(0x1915, 0x0909, 0x09, "Hub (1915:0909)", Vec::new()),
            ])
        }

        /// Also write every event to `file`, a line each.
        pub fn with_log_file(self, file: PathBuf) -> Self {
            self.log.lock().expect("log").file = Some(file);
            self
        }

        /// What the devices saw.
        pub fn log(&self) -> Arc<Mutex<Log>> {
            self.log.clone()
        }
    }

    impl Backend for FakeUsb {
        fn list(&self) -> Result<Vec<Listed>, Fault> {
            Ok(self.devices.iter().map(|d| d.listed.clone()).collect())
        }

        fn open(&self, listed: &Listed) -> Result<Box<dyn Link>, Fault> {
            let device = self
                .devices
                .iter()
                .find(|d| d.listed.token == listed.token)
                .cloned()
                .ok_or_else(|| Fault::Disconnected(format!("{} is gone", listed.name)))?;
            let ids = ids(&device.listed);
            {
                let mut log = self.log.lock().expect("log");
                log.open += 1;
                log.note(format!("open {ids}"));
            }
            Ok(Box::new(FakeLink {
                ids,
                device,
                claimed: None,
                queues: BTreeMap::new(),
                stored: BTreeMap::new(),
                log: self.log.clone(),
            }))
        }
    }

    fn ids(listed: &Listed) -> String {
        krate_manifest::usb_ids(listed.vendor_id, listed.product_id)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    struct FakeLink {
        ids: String,
        device: FakeDevice,
        claimed: Option<u8>,
        /// Bytes waiting on each IN endpoint.
        queues: BTreeMap<u8, VecDeque<u8>>,
        /// What vendor control requests stored, by request and value.
        stored: BTreeMap<(u8, u16), Vec<u8>>,
        log: Arc<Mutex<Log>>,
    }

    impl FakeLink {
        fn note(&self, event: String) {
            self.log.lock().expect("log").note(event);
        }
    }

    impl Link for FakeLink {
        fn interface(&self, number: u8) -> Option<Interface> {
            self.device
                .interfaces
                .iter()
                .find(|i| i.number == number)
                .cloned()
        }

        fn claim(&mut self, number: u8) -> Result<(), Fault> {
            self.claimed = Some(number);
            let mut log = self.log.lock().expect("log");
            log.claimed.insert((self.ids.clone(), number));
            log.note(format!("claim {} {number}", self.ids));
            Ok(())
        }

        fn read(
            &mut self,
            endpoint: Endpoint,
            len: usize,
            wait: Duration,
        ) -> Result<Vec<u8>, Fault> {
            let queue = self.queues.entry(endpoint.address).or_default();
            if queue.is_empty() {
                // Nothing to send: the transfer waits its whole wait.
                std::thread::sleep(wait);
                return Ok(Vec::new());
            }
            let take = len.min(queue.len());
            let got: Vec<u8> = queue.drain(..take).collect();
            self.note(format!(
                "in {} {:#04x} {}",
                self.ids,
                endpoint.address,
                hex(&got)
            ));
            Ok(got)
        }

        fn write(&mut self, endpoint: Endpoint, bytes: &[u8], wait: Duration) -> Result<(), Fault> {
            if bytes == NEVER_TAKEN {
                std::thread::sleep(wait);
                return Err(Fault::TimedOut(format!(
                    "{} took 0 of {} bytes in the wait",
                    self.device.listed.name,
                    bytes.len()
                )));
            }
            if bytes == REFUSED {
                return Err(Fault::Stalled(format!(
                    "{} refused the transfer on endpoint {:#04x}",
                    self.device.listed.name, endpoint.address
                )));
            }
            self.note(format!(
                "out {} {:#04x} {}",
                self.ids,
                endpoint.address,
                hex(bytes)
            ));
            // The echo: OUT n comes back on IN n.
            self.queues
                .entry(endpoint.address | 0x80)
                .or_default()
                .extend(bytes.iter().copied());
            Ok(())
        }

        fn control_in(
            &mut self,
            setup: Setup,
            length: u16,
            _wait: Duration,
        ) -> Result<Vec<u8>, Fault> {
            self.note(format!(
                "control-in {} {:?} {:?} {:#04x} {:#06x} {:#06x} {length}",
                self.ids,
                setup.request_type,
                setup.recipient,
                setup.request,
                setup.value,
                setup.index
            ));
            Ok(match setup.request_type {
                RequestType::Standard => vec![0, 0],
                _ => self
                    .stored
                    .get(&(setup.request, setup.value))
                    .cloned()
                    .unwrap_or_default(),
            })
        }

        fn control_out(&mut self, setup: Setup, data: &[u8], _wait: Duration) -> Result<(), Fault> {
            self.note(format!(
                "control-out {} {:?} {:?} {:#04x} {:#06x} {:#06x} {}",
                self.ids,
                setup.request_type,
                setup.recipient,
                setup.request,
                setup.value,
                setup.index,
                hex(data)
            ));
            self.stored
                .insert((setup.request, setup.value), data.to_vec());
            Ok(())
        }
    }

    impl Drop for FakeLink {
        fn drop(&mut self) {
            let mut log = self.log.lock().expect("log");
            if let Some(number) = self.claimed {
                log.claimed.remove(&(self.ids.clone(), number));
                log.note(format!("release {} {number}", self.ids));
            }
            log.open -= 1;
            log.note(format!("close {}", self.ids));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeUsb, NEVER_TAKEN, REFUSED};
    use super::*;
    use std::sync::{Arc, Mutex};

    /// The manifest named 1915:7777, 1915:0101 (a keyboard), 1915:0202 (a
    /// thumb drive) and 1915:0909 (a hub); not 1915:7778.
    fn admit(vendor: u16, product: u16) -> Result<(), Fault> {
        match (vendor, product) {
            (0x1915, 0x7777 | 0x0101 | 0x0202 | 0x0909) => Ok(()),
            _ => Err(Fault::NotAllowed(format!(
                "this app did not ask for device.usb:{}",
                krate_manifest::usb_ids(vendor, product)
            ))),
        }
    }

    fn devices() -> (UsbDevices, Arc<Mutex<super::fake::Log>>) {
        let fake = FakeUsb::standard();
        let log = fake.log();
        (UsbDevices::new(Box::new(fake)), log)
    }

    fn picked(devices: &mut UsbDevices, ids: &str) -> u64 {
        devices
            .pick("Choose", false, Some(ids), &admit)
            .expect("pick")
            .expect("a device")
            .0
    }

    fn vendor(request: u8, recipient: Recipient, index: u16) -> Setup {
        Setup {
            request_type: RequestType::Vendor,
            recipient,
            request,
            value: 0x0042,
            index,
        }
    }

    /// The list the person sees holds only devices whose ids the manifest
    /// names and the computer does not keep: never 1915:7778 (not named),
    /// never the keyboard, the drive or the hub (named, and the computer's).
    #[test]
    fn the_list_shows_only_named_devices_the_computer_does_not_keep() {
        let shown = candidates(FakeUsb::standard().list().expect("list"), &admit).expect("one");
        let ids: Vec<(u16, u16)> = shown.iter().map(|d| (d.vendor_id, d.product_id)).collect();
        assert_eq!(ids, vec![(0x1915, 0x7777)]);

        // Only kept devices named: refused in words that say what they are.
        let only_kept = |v: u16, p: u16| {
            if (v, p) == (0x1915, 0x0101) {
                Ok(())
            } else {
                Err(Fault::NotAllowed("no".into()))
            }
        };
        match candidates(FakeUsb::standard().list().expect("list"), &only_kept) {
            Err(Fault::NotForApps(words)) => {
                assert!(words.contains("keyboard, mouse"), "{words}")
            }
            other => panic!("a keyboard is never shown, got {other:?}"),
        }
        // Nothing named is connected.
        let nothing = |_: u16, _: u16| Err(Fault::NotAllowed("no".into()));
        assert!(matches!(
            candidates(FakeUsb::standard().list().expect("list"), &nothing),
            Err(Fault::Unavailable(_))
        ));
    }

    /// A scripted pick names a connected device the app may have, or
    /// cancels: an id the manifest never named is refused by the caller's
    /// rule, a kept class is refused even when named, a device that is not
    /// connected is unavailable, and anything but ids is refused.
    #[test]
    fn a_scripted_pick_passes_the_same_rules() {
        let (mut devices, _log) = devices();
        assert_eq!(devices.pick("t", false, Some("cancel"), &admit), Ok(None));
        assert_eq!(devices.pick("t", false, Some(""), &admit), Ok(None));
        assert!(matches!(
            devices.pick("t", false, Some("1915:7778"), &admit),
            Err(Fault::NotAllowed(_))
        ));
        for (kept, says) in [
            ("1915:0101", "keyboard"),
            ("1915:0202", "storage"),
            ("1915:0909", "hub"),
        ] {
            match devices.pick("t", false, Some(kept), &admit) {
                Err(Fault::NotForApps(words)) => assert!(words.contains(says), "{words}"),
                other => panic!("{kept} must be refused as the computer's, got {other:?}"),
            }
        }
        let anything = |_: u16, _: u16| Ok(());
        assert!(matches!(
            devices.pick("t", false, Some("1915:1234"), &anything),
            Err(Fault::Unavailable(_))
        ));
        for not_ids in ["/dev/bus/usb/001/002", "1915", "*:*", "0x1915:0x7777"] {
            assert!(
                matches!(
                    devices.pick("t", false, Some(not_ids), &anything),
                    Err(Fault::Unavailable(_))
                ),
                "{not_ids}"
            );
        }
        assert_eq!(devices.held(), 0, "no refusal holds a device");
        let (h, listed) = devices
            .pick("t", false, Some("1915:7777"), &admit)
            .expect("pick")
            .expect("device");
        assert_eq!((listed.vendor_id, listed.product_id), (0x1915, 0x7777));
        assert_eq!(devices.ids(h), Some((0x1915, 0x7777)));
    }

    /// A run nobody is at shows no list: the pick is a cancel, never a
    /// dialog that waits for ever.
    #[test]
    fn nobody_to_ask_means_no_list() {
        let (mut devices, log) = devices();
        assert_eq!(devices.pick("t", true, None, &admit), Ok(None));
        assert!(log.lock().expect("log").events.is_empty(), "nothing opened");
    }

    /// Bulk and interrupt transfers arrive byte for byte, out and back, on
    /// the claimed interface's own endpoints; a read with nothing waiting
    /// waits its wait and ends empty; a read never answers more than asked
    /// (rounded up to whole packets).
    #[test]
    fn transfers_are_byte_exact_and_reads_end_with_their_wait() {
        let (mut devices, log) = devices();
        let h = picked(&mut devices, "1915:7777");
        assert_eq!(
            devices.read(h, Kind::Bulk, 0x81, 64, 10),
            Err(Fault::NotClaimed),
            "nothing before a claim"
        );
        devices.claim(h, 0).expect("claim");
        devices
            .claim(h, 0)
            .expect("the same claim again is nothing");

        let every_byte: Vec<u8> = (0..=255).collect();
        devices
            .write(h, Kind::Bulk, 0x01, &every_byte, 1_000)
            .expect("bulk out");
        let mut back = Vec::new();
        while back.len() < every_byte.len() {
            let more = devices
                .read(h, Kind::Bulk, 0x81, 100, 1_000)
                .expect("bulk in");
            assert!(more.len() <= 128, "100 rounds up to two 64-byte packets");
            assert!(!more.is_empty());
            back.extend(more);
        }
        assert_eq!(back, every_byte, "byte for byte");

        devices
            .write(h, Kind::Interrupt, 0x02, &[0x00, 0xff, 0x7f], 1_000)
            .expect("interrupt out");
        assert_eq!(
            devices.read(h, Kind::Interrupt, 0x82, 1, 1_000),
            Ok(vec![0x00, 0xff, 0x7f]),
            "one asked, one whole 8-byte packet answered"
        );

        let started = std::time::Instant::now();
        assert_eq!(devices.read(h, Kind::Bulk, 0x81, 64, 150), Ok(Vec::new()));
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(140) && waited < Duration::from_secs(5),
            "waited {waited:?} for 150 ms"
        );
        assert_eq!(devices.read(h, Kind::Bulk, 0x81, 0, 10), Ok(Vec::new()));

        let log = log.lock().expect("log");
        assert!(log
            .events
            .contains(&"out 1915:7777 0x02 00ff7f".to_string()));
    }

    /// Waits, sizes, endpoints, kinds and directions out of bounds are
    /// refused before anything reaches the device, and a write the device
    /// never takes ends with its wait, in words.
    #[test]
    fn requests_out_of_bounds_are_refused_and_writes_time_out() {
        let (mut devices, log) = devices();
        let h = picked(&mut devices, "1915:7777");
        devices.claim(h, 0).expect("claim");
        let refused = |result: Result<Vec<u8>, Fault>, says: &str| match result {
            Err(Fault::BadRequest(words)) => assert!(words.contains(says), "{words}"),
            other => panic!("refused ({says}), got {other:?}"),
        };
        refused(devices.read(h, Kind::Bulk, 0x81, 64, 0), "a wait");
        refused(
            devices.read(h, Kind::Bulk, 0x81, 64, MAX_WAIT_MS + 1),
            "a wait",
        );
        // Not this interface's: the HID interface's endpoint, and one that
        // does not exist.
        refused(devices.read(h, Kind::Interrupt, 0x83, 8, 10), "not one of");
        refused(devices.read(h, Kind::Bulk, 0x85, 8, 10), "not one of");
        // The wrong kind, and the wrong direction.
        refused(
            devices.read(h, Kind::Interrupt, 0x81, 8, 10),
            "a bulk endpoint",
        );
        refused(devices.read(h, Kind::Bulk, 0x01, 8, 10), "OUT endpoint");
        let refused_write = |result: Result<(), Fault>, says: &str| match result {
            Err(Fault::BadRequest(words)) => assert!(words.contains(says), "{words}"),
            other => panic!("refused ({says}), got {other:?}"),
        };
        refused_write(devices.write(h, Kind::Bulk, 0x81, b"x", 10), "IN endpoint");
        refused_write(
            devices.write(h, Kind::Bulk, 0x01, &vec![0; MAX_TRANSFER_BYTES + 1], 10),
            "at most",
        );
        refused_write(devices.write(h, Kind::Bulk, 0x01, b"x", 0), "a wait");
        assert!(
            log.lock()
                .expect("log")
                .events
                .iter()
                .all(|event| !event.starts_with("out") && !event.starts_with("in")),
            "nothing reached the device"
        );

        // A device that takes nothing: the write ends with its wait.
        let started = std::time::Instant::now();
        assert!(matches!(
            devices.write(h, Kind::Bulk, 0x01, NEVER_TAKEN, 100),
            Err(Fault::TimedOut(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(
            devices.write(h, Kind::Bulk, 0x01, REFUSED, 100),
            Err(Fault::Stalled(_))
        ));
    }

    /// Control requests reach the claimed interface or one of its endpoints
    /// and nothing else: another interface, another endpoint, and every
    /// standard request but the two reads are refused before they are sent.
    #[test]
    fn control_requests_stay_inside_the_claimed_interface() {
        let (mut devices, log) = devices();
        let h = picked(&mut devices, "1915:7777");
        assert_eq!(
            devices.control_out(h, vendor(0x01, Recipient::Interface, 0), b"x", 100),
            Err(Fault::NotClaimed)
        );
        devices.claim(h, 0).expect("claim");

        devices
            .control_out(
                h,
                vendor(0x01, Recipient::Interface, 0x0000),
                &[1, 2, 3],
                100,
            )
            .expect("vendor request to the interface");
        assert_eq!(
            devices.control_in(h, vendor(0x01, Recipient::Interface, 0x0000), 64, 100),
            Ok(vec![1, 2, 3])
        );
        devices
            .control_in(h, vendor(0x02, Recipient::Endpoint, 0x0081), 8, 100)
            .expect("vendor request to its endpoint");
        devices
            .control_in(
                h,
                Setup {
                    request_type: RequestType::Standard,
                    recipient: Recipient::Interface,
                    request: 0x00,
                    value: 0,
                    index: 0,
                },
                2,
                100,
            )
            .expect("GET_STATUS reads");
        let sent = log.lock().expect("log").events.len();

        let refused = |result: Result<(), Fault>, says: &str| match result {
            Err(Fault::BadRequest(words)) => assert!(words.contains(says), "{words}"),
            other => panic!("refused ({says}), got {other:?}"),
        };
        // Another interface: the HID one, and one that does not exist.
        refused(
            devices.control_out(h, vendor(0x01, Recipient::Interface, 0x0001), b"x", 100),
            "interface 1",
        );
        refused(
            devices.control_out(h, vendor(0x01, Recipient::Interface, 0x0007), b"x", 100),
            "interface 7",
        );
        // The HID interface's endpoint, and endpoint 0 (the device's own).
        refused(
            devices.control_out(h, vendor(0x01, Recipient::Endpoint, 0x0083), b"x", 100),
            "endpoint 0x83",
        );
        refused(
            devices.control_out(h, vendor(0x01, Recipient::Endpoint, 0x0000), b"x", 100),
            "endpoint 0x00",
        );
        // Standard requests that change things: SET_CONFIGURATION,
        // SET_INTERFACE, CLEAR_FEATURE, SET_FEATURE, SET_ADDRESS.
        for request in [0x09, 0x0b, 0x01, 0x03, 0x05] {
            let setup = Setup {
                request_type: RequestType::Standard,
                ..vendor(request, Recipient::Interface, 0)
            };
            refused(
                devices.control_out(h, setup, &[], 100),
                "computer's to send",
            );
        }
        refused(
            devices.control_out(
                h,
                vendor(0x01, Recipient::Interface, 0),
                &[0; MAX_CONTROL_BYTES + 1],
                100,
            ),
            "at most",
        );
        assert!(matches!(
            devices.control_in(
                h,
                vendor(0x01, Recipient::Interface, 0),
                MAX_CONTROL_BYTES as u16 + 1,
                100
            ),
            Err(Fault::BadRequest(_))
        ));
        assert_eq!(
            log.lock().expect("log").events.len(),
            sent,
            "nothing refused reached the device"
        );
    }

    /// One interface a pick, never one the computer keeps: the composite
    /// dongle's HID interface is refused, and once interface 0 is claimed a
    /// second interface is refused too.
    #[test]
    fn one_interface_a_pick_and_never_a_kept_one() {
        let (mut devices, log) = devices();
        let h = picked(&mut devices, "1915:7777");
        match devices.claim(h, 1) {
            Err(Fault::NotForApps(words)) => assert!(words.contains("interface 1"), "{words}"),
            other => panic!("the HID interface is the computer's, got {other:?}"),
        }
        assert!(matches!(devices.claim(h, 9), Err(Fault::BadRequest(_))));
        devices.claim(h, 0).expect("claim");
        assert!(matches!(devices.claim(h, 1), Err(Fault::BadRequest(_))));
        let log = log.lock().expect("log");
        assert_eq!(
            log.claimed.iter().cloned().collect::<Vec<_>>(),
            vec![("1915:7777".to_string(), 0)]
        );
    }

    /// Handles the person never gave reach nothing; a closed handle is
    /// spent; and when the run ends (the devices dropped) nothing is left
    /// claimed or open.
    #[test]
    fn forged_and_spent_handles_reach_nothing_and_the_run_lets_go() {
        let (mut devices, log) = devices();
        let h = picked(&mut devices, "1915:7777");
        devices.claim(h, 0).expect("claim");
        for forged in [0, h + 1, u64::MAX] {
            assert_eq!(devices.claim(forged, 0), Err(Fault::UnknownHandle));
            assert_eq!(
                devices.read(forged, Kind::Bulk, 0x81, 64, 10),
                Err(Fault::UnknownHandle)
            );
            assert_eq!(
                devices.write(forged, Kind::Bulk, 0x01, b"x", 10),
                Err(Fault::UnknownHandle)
            );
            assert_eq!(
                devices.control_out(forged, vendor(1, Recipient::Interface, 0), b"x", 10),
                Err(Fault::UnknownHandle)
            );
            assert_eq!(devices.ids(forged), None);
        }
        devices.close(h);
        assert_eq!(
            devices.write(h, Kind::Bulk, 0x01, b"x", 10),
            Err(Fault::UnknownHandle),
            "a closed handle is spent"
        );
        {
            let log = log.lock().expect("log");
            assert!(log.claimed.is_empty(), "close released the interface");
            assert_eq!(log.open, 0, "close closed the device");
        }

        // Two picks left open: the run ending lets both go.
        let a = picked(&mut devices, "1915:7777");
        let b = picked(&mut devices, "1915:7777");
        assert_ne!(a, b, "handles are never reused");
        devices.claim(a, 0).expect("claim");
        devices.claim(b, 0).expect("claim");
        assert_eq!(log.lock().expect("log").open, 2);
        drop(devices);
        let log = log.lock().expect("log");
        assert!(
            log.claimed.is_empty(),
            "nothing left claimed: {:?}",
            log.claimed
        );
        assert_eq!(log.open, 0, "nothing left open");
        assert_eq!(
            log.events.last().map(String::as_str),
            Some("close 1915:7777")
        );
    }

    /// At most [`MAX_PICKED`] devices held at once, refused before a list is
    /// shown.
    #[test]
    fn a_run_holds_at_most_eight_devices() {
        let (mut devices, _log) = devices();
        let handles: Vec<u64> = (0..MAX_PICKED)
            .map(|_| picked(&mut devices, "1915:7777"))
            .collect();
        assert!(matches!(
            devices.pick("t", false, Some("1915:7777"), &admit),
            Err(Fault::TooMany(_))
        ));
        devices.close(handles[0]);
        let again = picked(&mut devices, "1915:7777");
        assert_eq!(again, MAX_PICKED as u64 + 1, "handles are never reused");
    }

    /// Only the six classes the computer keeps are refused; a vendor
    /// device, a composite whose class is per interface, and a printer are
    /// not.
    #[test]
    fn the_kept_classes_are_audio_input_storage_hubs_and_video() {
        for kept in [0x01, 0x03, 0x08, 0x09, 0x0e, 0x10] {
            assert!(kept_by_the_computer(kept).is_some(), "{kept:#04x}");
        }
        for free in [0x00, 0x02, 0x07, 0x0a, 0xef, 0xfe, 0xff] {
            assert!(kept_by_the_computer(free).is_none(), "{free:#04x}");
        }
    }
}
