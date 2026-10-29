// SPDX-License-Identifier: MIT OR Apache-2.0

//! Read-only Windows appearance preferences, cached between frames.

use krate_adapter_common::ui::Appearance;
use std::time::{Duration, Instant};

const CACHE_DURATION: Duration = Duration::from_millis(500);

fn from_raw(
    light_theme: Option<u32>,
    text_scale: Option<u32>,
    contrast: Option<bool>,
) -> Appearance {
    Appearance {
        dark: light_theme == Some(0),
        high_contrast: contrast.unwrap_or_default(),
        text_scale: text_scale
            .filter(|value| (100..=225).contains(value))
            .map_or(1.0, |value| value as f32 / 100.0),
    }
}

#[derive(Default)]
struct AppearanceCache {
    last: Option<(Instant, Appearance)>,
}

impl AppearanceCache {
    fn current(&mut self, now: Instant, read: impl FnOnce() -> Appearance) -> Appearance {
        if let Some((when, look)) = self.last {
            if now.saturating_duration_since(when) < CACHE_DURATION {
                return look;
            }
        }
        let look = read();
        self.last = Some((now, look));
        look
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn current() -> Appearance {
    static LOOK: std::sync::Mutex<AppearanceCache> =
        std::sync::Mutex::new(AppearanceCache { last: None });

    // A poisoned cache must not stop an app from drawing. Failed OS reads are
    // cached too, so unavailable settings are not queried on every frame.
    LOOK.lock()
        .map(|mut cache| cache.current(Instant::now(), read))
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn read() -> Appearance {
    from_raw(
        read_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "AppsUseLightTheme",
        ),
        read_dword(r"Software\Microsoft\Accessibility", "TextScaleFactor"),
        read_high_contrast(),
    )
}

#[cfg(target_os = "windows")]
fn read_dword(key: &str, name: &str) -> Option<u32> {
    use std::mem::size_of;
    use std::ptr::{from_mut, null_mut};
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: both strings are NUL-terminated and live for the call. The output
    // buffer is a DWORD; the type restriction rejects other registry types.
    // RegGetValueW only queries HKCU and does not create or modify a key.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            from_mut(&mut value).cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS && size == size_of::<u32>() as u32).then_some(value)
}

#[cfg(target_os = "windows")]
fn read_high_contrast() -> Option<bool> {
    use std::mem::size_of;
    use std::ptr::{from_mut, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};

    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        dwFlags: 0,
        lpszDefaultScheme: null_mut(),
    };
    // SAFETY: SPI_GETHIGHCONTRAST only reads the setting; the structure has the
    // required size and remains writable for the duration of the call.
    let success = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            from_mut(&mut contrast).cast(),
            0,
        )
    };
    let enabled = (success != 0).then_some(contrast.dwFlags & HCF_HIGHCONTRASTON != 0);
    if !contrast.lpszDefaultScheme.is_null() {
        // SAFETY: Windows owns the returned scheme allocation and documents
        // LocalFree as its matching release operation. We never read its text.
        unsafe { LocalFree(contrast.lpszDefaultScheme.cast()) };
    }
    enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_dark_mode_contrast_and_text_percentage() {
        assert_eq!(
            from_raw(Some(0), Some(135), Some(true)),
            Appearance {
                dark: true,
                high_contrast: true,
                text_scale: 1.35,
            }
        );
        assert_eq!(
            from_raw(Some(1), Some(100), Some(false)),
            Appearance::default()
        );
        assert_eq!(from_raw(Some(0), Some(225), Some(false)).text_scale, 2.25);
    }

    #[test]
    fn missing_or_failed_reads_default_each_setting_independently() {
        assert_eq!(from_raw(None, None, None), Appearance::default());
        let look = from_raw(None, Some(150), Some(true));
        assert!(!look.dark);
        assert!(look.high_contrast);
        assert_eq!(look.text_scale, 1.5);
        assert!(from_raw(Some(0), None, None).dark);
    }

    #[test]
    fn out_of_range_registry_values_use_defaults() {
        for scale in [0, 99, 226, u32::MAX] {
            assert_eq!(from_raw(Some(2), Some(scale), None), Appearance::default());
        }
    }

    #[test]
    fn cache_reads_once_until_half_a_second_then_refreshes() {
        let mut cache = AppearanceCache::default();
        let now = Instant::now();
        let first = from_raw(Some(0), Some(135), Some(true));
        assert_eq!(cache.current(now, || first), first);
        assert_eq!(
            cache.current(now + Duration::from_millis(499), || panic!("cached read")),
            first
        );
        assert_eq!(
            cache.current(now + CACHE_DURATION, Appearance::default),
            Appearance::default()
        );
    }

    #[test]
    fn unavailable_settings_are_cached_but_retried_after_expiry() {
        let mut cache = AppearanceCache::default();
        let now = Instant::now();
        assert_eq!(
            cache.current(now, Appearance::default),
            Appearance::default()
        );
        assert_eq!(
            cache.current(now + Duration::from_millis(499), || panic!(
                "cached failure"
            )),
            Appearance::default()
        );
        let recovered = from_raw(Some(0), Some(200), Some(true));
        assert_eq!(cache.current(now + CACHE_DURATION, || recovered), recovered);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn native_missing_registry_read_uses_default_without_creating_a_key() {
        assert_eq!(
            read_dword(
                r"Software\Krate\Tests\MissingAppearancePreference",
                "Missing"
            ),
            None
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn native_appearance_stays_in_the_supported_text_range() {
        let look = current();
        assert!((1.0..=2.25).contains(&look.text_scale));
    }
}
