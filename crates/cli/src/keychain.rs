//! The Mac login keychain, through `/usr/bin/security`: where Krate keeps
//! the keys this computer holds for the person -- the publisher key every
//! app they make is signed with (IC-983) and the key their apps' secrets are
//! sealed under (IC-038).
//!
//! Used only when this is a Mac and Krate's home is the account's own: the
//! keychain belongs to the account, so a Krate home anywhere else (a test, a
//! moved `HOME`) keeps its keys in its own folder and never writes into the
//! person's keychain.

#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::{bail, Result};
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

/// One kind of key Krate keeps in the keychain.
// Read only where there is a keychain, which only a Mac has.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct Item {
    /// The keychain service name.
    pub service: &'static str,
    /// What the person sees in Keychain Access.
    pub label: &'static str,
    /// Keychain Access's comment: what removing it costs.
    pub comment: &'static str,
    /// How a sentence names it: "your maker key".
    pub said: &'static str,
}

/// Whether the keychain is used here.
pub(crate) fn usable() -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(account_home) = account_home() else {
            return false;
        };
        let Some(home) = crate::home_dir() else {
            return false;
        };
        let same = match (account_home.canonicalize(), home.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        same && Path::new("/usr/bin/security").exists()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The account's home as the system knows it, not as `HOME` says.
#[cfg(target_os = "macos")]
fn account_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    // SAFETY: getpwuid_r writes into the buffers we own and hands back a
    // pointer into them; we read pw_dir before they go out of scope.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let dir = CStr::from_ptr(pwd.pw_dir).to_string_lossy().into_owned();
        Some(PathBuf::from(dir))
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The item's bytes: `Ok(None)` when there is no such item, an error when
/// the keychain could not be asked (locked, or the person said no) -- the
/// two must never be confused, or a locked keychain would make a second key.
pub(crate) fn read(item: &Item) -> Result<Option<Vec<u8>>> {
    #[cfg(target_os = "macos")]
    {
        if !usable() {
            return Ok(None);
        }
        let out = std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                item.service,
                "-a",
                "krate",
                "-w",
            ])
            .output()
            .with_context(|| format!("could not ask the keychain for {}", item.said))?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            return unhex(&text)
                .map(Some)
                .with_context(|| format!("{} in your keychain is damaged", item.said));
        }
        // 44: errSecItemNotFound. Anything else is the keychain refusing.
        if out.status.code() == Some(44) {
            return Ok(None);
        }
        bail!(
            "your keychain did not hand over {} ({}). Unlock it -- open Keychain Access, \
             or run `security unlock-keychain` -- and try again.",
            item.said,
            String::from_utf8_lossy(&out.stderr).trim()
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = item;
        Ok(None)
    }
}

/// Put the item in the keychain. The secret goes in on `security -i`'s
/// standard input, never on a command line another process could read.
/// Never `-U`: an item already there is somebody's key, not ours to
/// overwrite.
pub(crate) fn add(item: &Item, bytes: &[u8]) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        let mut child = std::process::Command::new("/usr/bin/security")
            .arg("-i")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("could not reach the keychain")?;
        let line = format!(
            "add-generic-password -s {} -a krate -l \"{}\" -j \"{}\" -w {}\n",
            item.service,
            item.label,
            item.comment,
            hex(bytes)
        );
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(line.as_bytes())?;
        }
        let out = child.wait_with_output()?;
        let said = String::from_utf8_lossy(&out.stderr);
        if out.status.success() && !said.contains("rror") {
            Ok(())
        } else {
            bail!("the keychain did not take {}: {}", item.said, said.trim())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (item, bytes);
        bail!("no keychain here")
    }
}

/// Remove the item. Gone already is not an error; a keychain that will not
/// be asked (locked, or the person said no) is.
pub(crate) fn remove(item: &Item) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if !usable() {
            return Ok(());
        }
        let out = std::process::Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", item.service, "-a", "krate"])
            .output()
            .with_context(|| format!("could not ask the keychain to remove {}", item.said))?;
        // 44: errSecItemNotFound -- nothing to remove.
        if out.status.success() || out.status.code() == Some(44) {
            return Ok(());
        }
        bail!(
            "your keychain did not let go of {} ({}). Unlock it and try again.",
            item.said,
            String::from_utf8_lossy(&out.stderr).trim()
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = item;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_refuses_what_is_not_hex() {
        let bytes = [0u8, 1, 0xab, 0xff];
        assert_eq!(unhex(&hex(&bytes)).as_deref(), Some(&bytes[..]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex(""), None);
    }
}
