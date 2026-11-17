//! Start menu entries for installed apps (IC-278, Windows per-artifact
//! identity).
//!
//! A shortcut carries the app's AppUserModelID, the same id a run of the
//! app takes ([`crate::identity::adopt`]), so Windows treats the entry, the
//! running window and its notifications as one app: pinning the window pins
//! this entry, and the taskbar button is the app's, not Krate's.

use std::path::Path;

/// What one Start menu entry holds.
pub struct Shortcut<'a> {
    /// Where the `.lnk` is written.
    pub path: &'a Path,
    /// The program it runs.
    pub target: &'a Path,
    /// Its command line, already quoted.
    pub arguments: &'a str,
    /// The tooltip.
    pub description: &'a str,
    /// An `.ico` to show, or the target's own icon when `None`.
    pub icon: Option<&'a Path>,
    /// The app's AppUserModelID.
    pub app_id: &'a str,
}

/// Write `shortcut`, replacing one already at its path.
#[cfg(windows)]
pub fn write(shortcut: &Shortcut<'_>) -> Result<(), String> {
    use std::mem::ManuallyDrop;
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, SHStrDupW, ShellLink};

    // SAFETY: COM is initialised on this thread for the calls below and
    // uninitialised once, only when this call initialised it.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let written = (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(shortcut.target.as_os_str()))?;
            link.SetArguments(&HSTRING::from(shortcut.arguments))?;
            link.SetDescription(&HSTRING::from(shortcut.description))?;
            if let Some(icon) = shortcut.icon {
                link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
            }
            // The id as VT_LPWSTR, the type the shell reads it as; the
            // string is the shell's own allocation, freed by PROPVARIANT's
            // drop (PropVariantClear).
            let text = SHStrDupW(&HSTRING::from(shortcut.app_id))?;
            let value = PROPVARIANT {
                Anonymous: PROPVARIANT_0 {
                    Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                        vt: VT_LPWSTR,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: PROPVARIANT_0_0_0 { pwszVal: text },
                    }),
                },
            };
            let store: IPropertyStore = link.cast()?;
            store.SetValue(&PKEY_AppUserModel_ID, &value)?;
            store.Commit()?;
            let file: IPersistFile = link.cast()?;
            file.Save(&HSTRING::from(shortcut.path.as_os_str()), true)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        written.map_err(|err| format!("Windows would not add it to the Start menu ({err})"))
    }
}

/// The AppUserModelID and arguments a shortcut on disk holds, for checking
/// what [`write`] wrote.
#[cfg(windows)]
pub fn read(path: &Path) -> Result<(String, String), String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    // SAFETY: as in `write`.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let read = (|| -> windows::core::Result<(String, String)> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(&HSTRING::from(path.as_os_str()), STGM_READ)?;
            let store: IPropertyStore = link.cast()?;
            let id = store.GetValue(&PKEY_AppUserModel_ID)?.to_string();
            let mut args = [0u16; 2048];
            link.GetArguments(&mut args)?;
            let len = args.iter().position(|&c| c == 0).unwrap_or(args.len());
            Ok((id, String::from_utf16_lossy(&args[..len])))
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        read.map_err(|err| format!("could not read {} ({err})", path.display()))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// On a real Windows desktop (CI's windows-2022 runner): the entry is
    /// written and reads back with the app's id and its quoted path.
    #[test]
    fn a_shortcut_carries_the_apps_own_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("Notes (1a2b3c4d).lnk");
        let target = std::env::current_exe().expect("exe");
        let arguments = "\"C:\\Users\\a person\\Krate\\apps\\Notes (1a2b3c4d)\\app.krate\"";
        write(&Shortcut {
            path: &path,
            target: &target,
            arguments,
            description: "Made with Krate",
            icon: None,
            app_id: "Krate.App.0123456789abcdef",
        })
        .expect("write");
        let (id, args) = read(&path).expect("read");
        assert_eq!(id, "Krate.App.0123456789abcdef");
        assert_eq!(args, arguments);
    }
}
