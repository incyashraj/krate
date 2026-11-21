// The smallest app that talks to a USB device the person picks (krate:usb,
// IC-988), so the capability is proved through the binary a person runs.
//
//   krate run usb-probe.krate -- <check>
//
// Each check prints one line, so a test sees exactly what an app is told:
//
//   pick     ask for a device and say what came back
//   talk     pick, claim interface 0, send every byte value out on bulk
//            0x01 and read it back on 0x81, three bytes out on interrupt
//            0x02 and back on 0x82, a vendor request that stores three
//            bytes and one that reads them back, then a read with nothing
//            waiting (empty when its wait runs out), then close and see the
//            handle is spent
//   forge    pick, then try handles the person never gave: the pick itself
//            has no interface yet, the next number and 0 reach nothing
//   hid      pick, then claim interface 1, a HID interface: refused
//   outside  pick, claim 0, then ask for what lies outside interface 0: a
//            vendor request for interface 1, SET_CONFIGURATION, and the HID
//            interface's endpoint 0x83
//   radio    pick, claim 0, set it up the way a Crazyradio is, with vendor
//            requests to the whole device (channel 80, then data rate 2),
//            send the null packet 0xff on bulk 0x01 and read the answer on
//            0x81
//   device   pick, claim 0, then a class request to the whole device (sent),
//            SET_CONFIGURATION to it (refused) and GET_DESCRIPTOR from it
//            (a read: sent)
//   slow     pick, claim 0, write bytes the device never takes: timed out
//   hold     pick, claim 0, write HELD, and leave without closing, so a test
//            can see the run ending released and closed it
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::{
    bindings::krate::usb::devices as raw,
    io::{args, stdio, streams::OutputStreamExt},
    usb::{self, ControlSetup, Device, Recipient, RequestType, UsbError},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw_args = args::raw();
        let check = raw_args
            .split('\n')
            .find(|w| !w.is_empty())
            .unwrap_or("pick");
        let line = match check {
            "pick" => match usb::pick("Choose the test dongle") {
                Ok(Some(device)) => format!(
                    "pick: picked {} {:04x}:{:04x}",
                    device.name(),
                    device.vendor_id(),
                    device.product_id()
                ),
                Ok(None) => String::from("pick: none"),
                Err(err) => format!("pick: {}", refusal(&err)),
            },
            "talk" => with_device("talk", talk),
            "forge" => with_device("forge", forge),
            "hid" => with_device("hid", |device| match device.claim(1) {
                Ok(()) => String::from("CLAIMED"),
                Err(err) => refusal(&err),
            }),
            "outside" => with_device("outside", outside),
            "radio" => with_device("radio", radio),
            "device" => with_device("device", whole_device),
            "slow" => with_device("slow", |device| {
                if let Err(err) = device.claim(0) {
                    return refusal(&err);
                }
                match device.bulk_out(0x01, b"SLOW", 100) {
                    Ok(()) => String::from("TAKEN"),
                    Err(err) => kind(&err).into(),
                }
            }),
            "hold" => with_device("hold", |device| {
                if let Err(err) = device.claim(0) {
                    return refusal(&err);
                }
                if let Err(err) = device.bulk_out(0x01, b"HELD", 1_000) {
                    return refusal(&err);
                }
                // Never closed by the app: the run ending lets it go.
                core::mem::forget(device);
                String::from("left open")
            }),
            other => format!("unknown check {other}"),
        };
        let out = stdio::stdout();
        if out.write_line(&line).is_ok() && out.flush().is_ok() {
            0
        } else {
            20
        }
    }
}

/// Pick, then hand the device to `then`; `label: ...` either way.
fn with_device(label: &str, then: impl FnOnce(Device) -> String) -> String {
    match usb::pick("Choose the test dongle") {
        Ok(Some(device)) => format!("{label}: {}", then(device)),
        Ok(None) => format!("{label}: none"),
        Err(err) => format!("{label}: {}", refusal(&err)),
    }
}

fn talk(device: Device) -> String {
    if let Err(err) = device.claim(0) {
        return format!("claim {}", refusal(&err));
    }
    let every_byte: Vec<u8> = (0..=255u8).collect();
    if let Err(err) = device.bulk_out(0x01, &every_byte, 1_000) {
        return format!("bulk out {}", refusal(&err));
    }
    let mut back: Vec<u8> = Vec::new();
    for _ in 0..16 {
        match device.bulk_in(0x81, 128, 1_000) {
            Ok(bytes) => back.extend_from_slice(&bytes),
            Err(err) => return format!("bulk in {}", refusal(&err)),
        }
        if back.len() >= every_byte.len() {
            break;
        }
    }
    if back != every_byte {
        return format!("bulk came back as {back:?}");
    }
    if let Err(err) = device.interrupt_out(0x02, &[0x00, 0xff, 0x7f], 1_000) {
        return format!("interrupt out {}", refusal(&err));
    }
    let interrupt = match device.interrupt_in(0x82, 8, 1_000) {
        Ok(bytes) if bytes == [0x00, 0xff, 0x7f] => bytes.len(),
        Ok(bytes) => return format!("interrupt came back as {bytes:?}"),
        Err(err) => return format!("interrupt in {}", refusal(&err)),
    };
    if let Err(err) = device.control_out(usb::vendor_request(0x01, 7, 0), &[9, 8, 7], 1_000) {
        return format!("control out {}", refusal(&err));
    }
    let control = match device.control_in(usb::vendor_request(0x01, 7, 0), 64, 1_000) {
        Ok(bytes) if bytes == [9, 8, 7] => bytes.len(),
        Ok(bytes) => return format!("control came back as {bytes:?}"),
        Err(err) => return format!("control in {}", refusal(&err)),
    };
    let quiet = match device.bulk_in(0x81, 64, 100) {
        Ok(bytes) if bytes.is_empty() => "nothing for 100 ms",
        Ok(_) => "UNEXPECTED BYTES",
        Err(_) => "A QUIET READ FAILED",
    };
    let handle = device.handle();
    drop(device);
    let after = match raw::bulk_out(handle, 0x01, b"x", 10) {
        Ok(()) => String::from("STILL OPEN"),
        Err(err) => kind(&err).into(),
    };
    format!(
        "bulk echoed {} bytes, interrupt echoed {interrupt} bytes, control kept {control} \
         bytes, then {quiet}, closed: {after}",
        back.len()
    )
}

fn forge(device: Device) -> String {
    let h = device.handle();
    let unclaimed = match raw::bulk_in(h, 0x81, 64, 10) {
        Ok(_) => "READ",
        Err(err) => kind(&err),
    };
    let next = match raw::claim(h + 1, 0) {
        Ok(()) => "CLAIMED",
        Err(err) => kind(&err),
    };
    let zero = match raw::bulk_out(0, 0x01, b"x", 10) {
        Ok(()) => "WROTE",
        Err(err) => kind(&err),
    };
    format!("unclaimed={unclaimed} next={next} zero={zero}")
}

fn outside(device: Device) -> String {
    if let Err(err) = device.claim(0) {
        return refusal(&err);
    }
    let other_interface = match device.control_out(usb::vendor_request(0x01, 0, 1), b"x", 100) {
        Ok(()) => "SENT",
        Err(err) => kind(&err),
    };
    let set_configuration = ControlSetup {
        request_type: RequestType::Standard,
        recipient: Recipient::Interface,
        request: 0x09,
        value: 1,
        index: 0,
    };
    let reconfigure = match device.control_out(set_configuration, &[], 100) {
        Ok(()) => "SENT",
        Err(err) => kind(&err),
    };
    let hid_endpoint = match device.interrupt_in(0x83, 8, 10) {
        Ok(_) => "READ",
        Err(err) => kind(&err),
    };
    format!(
        "interface-1={other_interface} set-configuration={reconfigure} \
         endpoint-0x83={hid_endpoint}"
    )
}

fn radio(device: Device) -> String {
    if let Err(err) = device.claim(0) {
        return format!("claim {}", refusal(&err));
    }
    // SET_RADIO_CHANNEL and SET_DATA_RATE, each a vendor request to the
    // device with no data.
    for (request, value) in [(0x01u8, 80u16), (0x03, 2)] {
        if let Err(err) =
            device.control_out(usb::vendor_device_request(request, value, 0), &[], 1_000)
        {
            return format!("set-up {request:#04x} {}", refusal(&err));
        }
    }
    if let Err(err) = device.bulk_out(0x01, &[0xff], 1_000) {
        return format!("null packet {}", refusal(&err));
    }
    match device.bulk_in(0x81, 64, 1_000) {
        Ok(ack) => format!("channel 80, data rate 2, null packet answered {ack:02x?}"),
        Err(err) => format!("ack {}", refusal(&err)),
    }
}

fn whole_device(device: Device) -> String {
    if let Err(err) = device.claim(0) {
        return refusal(&err);
    }
    let class = match device.control_out(usb::class_device_request(0x22, 3, 0), &[7], 100) {
        Ok(()) => "sent",
        Err(err) => kind(&err),
    };
    let set_configuration = ControlSetup {
        request_type: RequestType::Standard,
        recipient: Recipient::Device,
        request: 0x09,
        value: 1,
        index: 0,
    };
    let reconfigure = match device.control_out(set_configuration, &[], 100) {
        Ok(()) => "SENT",
        Err(err) => kind(&err),
    };
    let get_descriptor = ControlSetup {
        request_type: RequestType::Standard,
        recipient: Recipient::Device,
        request: 0x06,
        value: 0x0100,
        index: 0,
    };
    let descriptor = match device.control_in(get_descriptor, 18, 100) {
        Ok(bytes) => format!("{} bytes", bytes.len()),
        Err(err) => String::from(kind(&err)),
    };
    format!("class={class} set-configuration={reconfigure} get-descriptor={descriptor}")
}

fn kind(err: &UsbError) -> &'static str {
    match err {
        UsbError::NotAllowed(_) => "not-allowed",
        UsbError::Unavailable(_) => "unavailable",
        UsbError::UnknownHandle => "unknown-handle",
        UsbError::NotClaimed => "not-claimed",
        UsbError::NotForApps(_) => "not-for-apps",
        UsbError::BadRequest(_) => "bad-request",
        UsbError::Busy(_) => "busy",
        UsbError::Disconnected(_) => "disconnected",
        UsbError::Stalled(_) => "stalled",
        UsbError::TimedOut(_) => "timed-out",
        UsbError::TooMany(_) => "too-many",
        UsbError::Failed(_) => "failed",
    }
}

fn refusal(err: &UsbError) -> String {
    format!("{}: {}", kind(err), usb::words(err))
}

krate::export!(Component);
