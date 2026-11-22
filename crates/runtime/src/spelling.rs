//! Spelling (krate:ui/spelling, K-1340): the computer's own spell checker,
//! so an editor underlines what the person's other apps underline and
//! offers the same corrections.
//!
//! A Mac asks NSSpellChecker, Windows its spell-checking API, Linux the
//! Hunspell dictionaries installed for the person's language (read with
//! spellbook). Where there is none -- a phone, a Linux machine with no
//! dictionary -- `available` is false and nothing is ever marked wrong.
//! Offsets an app sees are UTF-8 bytes into the text it passed; the system
//! checkers count UTF-16 units, converted here.

/// One word the checker does not know: `start` and `len` in UTF-8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Misspelling {
    pub start: u32,
    pub len: u32,
}

/// The UTF-8 byte offset of UTF-16 unit `unit` in `text` (clamped to the end).
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn utf8_offset(text: &str, unit: usize) -> usize {
    let mut seen = 0usize;
    for (byte, ch) in text.char_indices() {
        if seen >= unit {
            return byte;
        }
        seen += ch.len_utf16();
    }
    text.len()
}

/// The words in `text`, as (byte start, word): runs of letters, with an
/// apostrophe inside a word kept ("don't"). Words with digits are skipped,
/// as spell checkers skip them.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn words(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut digit = false;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(byte, ch)) in chars.iter().enumerate() {
        let inner_apostrophe = (ch == '\'' || ch == '\u{2019}')
            && start.is_some()
            && chars.get(i + 1).is_some_and(|(_, n)| n.is_alphabetic());
        if ch.is_alphanumeric() || inner_apostrophe {
            if start.is_none() {
                start = Some(byte);
                digit = false;
            }
            digit |= ch.is_numeric();
        } else if let Some(s) = start.take() {
            if !digit {
                out.push((s, &text[s..byte]));
            }
        }
    }
    if let Some(s) = start {
        if !digit {
            out.push((s, &text[s..]));
        }
    }
    out
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{utf8_offset, Misspelling};
    use objc2_app_kit::NSSpellChecker;
    use objc2_foundation::{NSRange, NSString};

    pub(crate) fn available() -> bool {
        true
    }

    pub(crate) fn check(text: &str) -> Vec<Misspelling> {
        let checker = NSSpellChecker::sharedSpellChecker();
        // The person's own spelling language. Left to guess from the text,
        // the checker takes a short note with one accented word for French
        // and marks nothing.
        let lang = checker.language();
        let s = NSString::from_str(text);
        let total = s.length();
        let mut out = Vec::new();
        let mut from = 0usize;
        while from < total {
            // SAFETY: a null word count is allowed; the string outlives the call.
            let r: NSRange = unsafe {
                checker.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                    &s,
                    from as isize,
                    Some(&lang),
                    false,
                    0,
                    std::ptr::null_mut(),
                )
            };
            // NSNotFound (isize::MAX) when nothing more is wrong.
            if r.length == 0 || r.location >= total || r.location < from {
                break;
            }
            let start = utf8_offset(text, r.location);
            let end = utf8_offset(text, r.location + r.length);
            out.push(Misspelling {
                start: start as u32,
                len: (end - start) as u32,
            });
            from = r.location + r.length;
        }
        out
    }

    pub(crate) fn suggest(word: &str) -> Vec<String> {
        let checker = NSSpellChecker::sharedSpellChecker();
        let s = NSString::from_str(word);
        let range = NSRange::new(0, s.length());
        checker
            .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                range,
                &s,
                Some(&checker.language()),
                0,
            )
            .map(|list| list.iter().map(|g| g.to_string()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn learn(word: &str) {
        NSSpellChecker::sharedSpellChecker().learnWord(&NSString::from_str(word));
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{utf8_offset, Misspelling};
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Globalization::{
        GetUserDefaultLocaleName, ISpellChecker, ISpellCheckerFactory, SpellCheckerFactory,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };

    fn checker() -> Option<ISpellChecker> {
        // SAFETY: COM is set up for this thread before the factory is made;
        // a second CoInitializeEx on the same thread is harmless.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let factory: ISpellCheckerFactory =
                CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let mut name = [0u16; 85];
            let n = GetUserDefaultLocaleName(&mut name);
            let lang = if n > 1 {
                String::from_utf16_lossy(&name[..(n as usize - 1)])
            } else {
                "en-US".to_string()
            };
            let lang = HSTRING::from(lang);
            if factory.IsSupported(&lang).ok()?.as_bool() {
                factory.CreateSpellChecker(&lang).ok()
            } else {
                factory.CreateSpellChecker(&HSTRING::from("en-US")).ok()
            }
        }
    }

    pub(crate) fn available() -> bool {
        checker().is_some()
    }

    pub(crate) fn check(text: &str) -> Vec<Misspelling> {
        let Some(checker) = checker() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // SAFETY: the HSTRING outlives the enumeration; errors end the walk.
        unsafe {
            let Ok(errors) = checker.Check(&HSTRING::from(text)) else {
                return out;
            };
            loop {
                let mut one = None;
                if errors.Next(&mut one).is_err() {
                    break;
                }
                let Some(e) = one else { break };
                let (Ok(at), Ok(len)) = (e.StartIndex(), e.Length()) else {
                    break;
                };
                let start = utf8_offset(text, at as usize);
                let end = utf8_offset(text, (at + len) as usize);
                out.push(Misspelling {
                    start: start as u32,
                    len: (end - start) as u32,
                });
            }
        }
        out
    }

    pub(crate) fn suggest(word: &str) -> Vec<String> {
        let Some(checker) = checker() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // SAFETY: each string the enumerator hands over is ours to free.
        unsafe {
            let Ok(list) = checker.Suggest(&HSTRING::from(word)) else {
                return out;
            };
            loop {
                let mut item = [PWSTR::null()];
                let mut got = 0u32;
                if list.Next(&mut item, Some(&mut got)).is_err() || got == 0 {
                    break;
                }
                if let Ok(s) = item[0].to_string() {
                    out.push(s);
                }
                CoTaskMemFree(Some(item[0].0 as *const _));
            }
        }
        out
    }

    pub(crate) fn learn(word: &str) {
        if let Some(checker) = checker() {
            // SAFETY: a plain COM call with an owned string.
            unsafe {
                let _ = checker.Add(&HSTRING::from(word));
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{words, Misspelling};
    use std::sync::{Mutex, OnceLock};

    /// The person's language's Hunspell dictionary, read once.
    fn dictionary() -> &'static Option<Mutex<spellbook::Dictionary>> {
        static DICT: OnceLock<Option<Mutex<spellbook::Dictionary>>> = OnceLock::new();
        DICT.get_or_init(|| {
            let lang = std::env::var("LC_ALL")
                .ok()
                .filter(|v| !v.is_empty())
                .or_else(|| std::env::var("LC_MESSAGES").ok().filter(|v| !v.is_empty()))
                .or_else(|| std::env::var("LANG").ok())
                .unwrap_or_default();
            let lang = lang.split('.').next().unwrap_or("").to_string();
            let mut names = vec![lang.clone()];
            if let Some((base, _)) = lang.split_once('_') {
                names.push(base.to_string());
            }
            names.push("en_US".to_string());
            for name in names.into_iter().filter(|n| !n.is_empty()) {
                for dir in [
                    "/usr/share/hunspell",
                    "/usr/share/myspell",
                    "/usr/share/myspell/dicts",
                ] {
                    let aff = std::fs::read_to_string(format!("{dir}/{name}.aff"));
                    let dic = std::fs::read_to_string(format!("{dir}/{name}.dic"));
                    if let (Ok(aff), Ok(dic)) = (aff, dic) {
                        if let Ok(d) = spellbook::Dictionary::new(&aff, &dic) {
                            return Some(Mutex::new(d));
                        }
                    }
                }
            }
            None
        })
    }

    pub(crate) fn available() -> bool {
        dictionary().is_some()
    }

    pub(crate) fn check(text: &str) -> Vec<Misspelling> {
        let Some(dict) = dictionary() else {
            return Vec::new();
        };
        let Ok(dict) = dict.lock() else {
            return Vec::new();
        };
        words(text)
            .into_iter()
            .filter(|(_, w)| !dict.check(w))
            .map(|(start, w)| Misspelling {
                start: start as u32,
                len: w.len() as u32,
            })
            .collect()
    }

    pub(crate) fn suggest(word: &str) -> Vec<String> {
        let Some(dict) = dictionary() else {
            return Vec::new();
        };
        let Ok(dict) = dict.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        dict.suggest(word, &mut out);
        out
    }

    pub(crate) fn learn(word: &str) {
        if let Some(Ok(mut dict)) = dictionary().as_ref().map(|d| d.lock()) {
            let _ = dict.add(word);
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod imp {
    use super::Misspelling;

    pub(crate) fn available() -> bool {
        false
    }
    pub(crate) fn check(_text: &str) -> Vec<Misspelling> {
        Vec::new()
    }
    pub(crate) fn suggest(_word: &str) -> Vec<String> {
        Vec::new()
    }
    pub(crate) fn learn(_word: &str) {}
}

pub(crate) use imp::{available, check, learn, suggest};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_units_become_utf8_bytes() {
        let text = "caf\u{e9} \u{1F600} teh";
        // "café " is 5 UTF-16 units and 6 bytes; the emoji is 2 units, 4 bytes.
        assert_eq!(utf8_offset(text, 0), 0);
        assert_eq!(utf8_offset(text, 5), 6);
        assert_eq!(utf8_offset(text, 8), 11);
        assert_eq!(&text[utf8_offset(text, 8)..], "teh");
        assert_eq!(utf8_offset(text, 99), text.len());
    }

    #[test]
    fn words_keep_inner_apostrophes_and_skip_numbers() {
        let got: Vec<&str> = words("Don't  say 'hello' to r2d2, okay?")
            .into_iter()
            .map(|(_, w)| w)
            .collect();
        assert_eq!(got, ["Don't", "say", "hello", "to", "okay"]);
        let text = "a\u{e9}b c";
        assert_eq!(words(text), [(0, "a\u{e9}b"), (5, "c")]);
    }

    /// The Mac's own checker: "teh" is wrong, "the" is right, and the offsets
    /// are bytes into the text even after a non-ASCII word.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_macs_checker_marks_a_typo_by_its_bytes() {
        assert!(available());
        let text = "Caf\u{e9} is teh place";
        let wrong = check(text);
        assert_eq!(wrong.len(), 1, "{wrong:?}");
        let m = wrong[0];
        assert_eq!(&text[m.start as usize..(m.start + m.len) as usize], "teh");
        assert!(suggest("teh").iter().any(|s| s == "the"));
        assert!(check("The cat sat on the mat.").is_empty());
    }
}
