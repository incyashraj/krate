//! A pretend serial instrument for the workbench (IC-995): a real serial
//! path the app can open, with Studio on the other end.
//!
//! A pseudo-terminal is a character device, which is what Krate takes as a
//! serial port (serial_host.rs `is_serial_device`), so the app is handed it
//! exactly as it would be handed a USB-serial adapter: the person's pick
//! (`KRATE_SERIAL_PICK`), then open, write, read. What the app writes is
//! kept for the workbench to show; what the workbench sends -- a line typed
//! by the developer, or a scale's reading every second -- arrives at the app
//! as if the instrument had sent it.
//!
//! macOS and Linux only: Windows has no pseudo-terminal a serial API opens.

#![cfg(unix)]

use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How much of what the app sent is kept: the newest bytes win.
const KEEP_BYTES: usize = 64 * 1024;
/// How many lines sent to the app are kept.
const KEEP_LINES: usize = 200;

#[derive(Default)]
struct Log {
    from_app: Vec<u8>,
    to_app: Vec<String>,
}

pub struct PretendSerial {
    /// Studio's end: what the app writes is read here, what Studio writes
    /// the app reads.
    master: Mutex<File>,
    /// The app's end, held open so the terminal never hangs up between the
    /// app's opens.
    _slave: File,
    path: String,
    log: Arc<Mutex<Log>>,
    stop: Arc<AtomicBool>,
}

impl PretendSerial {
    /// A new instrument. `scale` makes it a scale that sends a weight
    /// reading every second, as many bench scales do unprompted.
    pub fn open(scale: bool) -> Result<Self, String> {
        let (mut master_fd, mut slave_fd) = (-1, -1);
        let mut name = [0 as libc::c_char; 128];
        // SAFETY: openpty writes two descriptors and a NUL-terminated name
        // into buffers we own; null termios and winsize mean defaults.
        let made = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                name.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if made != 0 {
            return Err("could not make a pretend serial port".to_string());
        }
        // Raw, as a serial line is: no echo, no line editing, no CR/LF
        // rewriting, so the bytes each side sends are the bytes the other
        // gets.
        // SAFETY: a termios read from and written back to our own slave fd.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(slave_fd, &mut t) == 0 {
                libc::cfmakeraw(&mut t);
                libc::tcsetattr(slave_fd, libc::TCSANOW, &t);
            }
            // Studio's end never blocks, so its reader can stop.
            let flags = libc::fcntl(master_fd, libc::F_GETFL);
            libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        // SAFETY: the name buffer was NUL-terminated by openpty.
        let path = unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: both descriptors are fresh and owned by nothing else.
        let (master, slave) =
            unsafe { (File::from_raw_fd(master_fd), File::from_raw_fd(slave_fd)) };
        let log = Arc::new(Mutex::new(Log::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let mut reader = master
            .try_clone()
            .map_err(|e| format!("could not read the pretend serial port: {e}"))?;
        {
            let (log, stop) = (Arc::clone(&log), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while !stop.load(Ordering::Relaxed) {
                    match reader.read(&mut buf) {
                        Ok(n) if n > 0 => {
                            let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
                            log.from_app.extend_from_slice(&buf[..n]);
                            let over = log.from_app.len().saturating_sub(KEEP_BYTES);
                            log.from_app.drain(..over);
                        }
                        _ => std::thread::sleep(Duration::from_millis(40)),
                    }
                }
            });
        }
        let serial = Self {
            master: Mutex::new(master),
            _slave: slave,
            path,
            log,
            stop,
        };
        if scale {
            let mut writer = serial
                .master
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .try_clone()
                .map_err(|e| format!("could not write the pretend serial port: {e}"))?;
            let (log, stop) = (Arc::clone(&serial.log), Arc::clone(&serial.stop));
            std::thread::spawn(move || {
                let mut tick = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    let line = scale_reading(tick);
                    if writer.write_all(format!("{line}\r\n").as_bytes()).is_ok() {
                        keep_sent(&log, line);
                    }
                    tick = tick.wrapping_add(1);
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
        }
        Ok(serial)
    }

    /// The path the app is handed as its picked serial device.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Send one line to the app, as the instrument would, ending CR LF.
    pub fn send(&self, line: &str) -> Result<(), String> {
        let line: String = line
            .chars()
            .filter(|c| *c != '\r' && *c != '\n')
            .take(512)
            .collect();
        self.master
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(format!("{line}\r\n").as_bytes())
            .map_err(|e| format!("the app's end is not reading: {e}"))?;
        keep_sent(&self.log, line);
        Ok(())
    }

    /// What the app has sent (as text, lossy) and the lines sent to it.
    pub fn snapshot(&self) -> (String, Vec<String>) {
        let log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        (
            String::from_utf8_lossy(&log.from_app).into_owned(),
            log.to_app.clone(),
        )
    }
}

impl Drop for PretendSerial {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn keep_sent(log: &Mutex<Log>, line: String) {
    let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
    log.to_app.push(line);
    let over = log.to_app.len().saturating_sub(KEEP_LINES);
    log.to_app.drain(..over);
}

/// A bench scale's reading, the common "ST,GS,+  12.345 kg" shape: stable,
/// gross, a weight that settles around one value with a little wobble.
fn scale_reading(tick: u32) -> String {
    let wobble = [0, 3, -2, 1, 0, -1, 2, 0][(tick % 8) as usize];
    let grams = 12_340 + wobble;
    format!("ST,GS,+{:>8.3} kg", grams as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's end is a real serial path: what the app writes, Studio
    /// keeps; what Studio sends, the app reads byte for byte.
    #[test]
    fn the_app_and_the_pretend_instrument_talk_both_ways() {
        let serial = PretendSerial::open(false).expect("a pty");
        let mut app = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(serial.path())
            .expect("the app opens the path");
        app.write_all(b"W\r\n").unwrap();
        let mut seen = String::new();
        for _ in 0..50 {
            seen = serial.snapshot().0;
            if seen.contains("W\r\n") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(seen, "W\r\n", "Studio got exactly what the app wrote");
        serial.send("ST,GS,+  12.340 kg").unwrap();
        let mut buf = [0u8; 64];
        let n = app.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ST,GS,+  12.340 kg\r\n");
        assert_eq!(serial.snapshot().1, vec!["ST,GS,+  12.340 kg".to_string()]);
    }

    #[test]
    fn a_scale_reads_like_a_scale() {
        assert_eq!(scale_reading(0), "ST,GS,+  12.340 kg");
        assert_eq!(scale_reading(1), "ST,GS,+  12.343 kg");
    }
}
