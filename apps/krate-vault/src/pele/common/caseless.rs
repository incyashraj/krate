//! Ports diagrams/common/caseless.ts.

use super::chars::Text;

/// Ports `wordAt`: whether `word`, written in lower case, is at `at`. Only
/// ASCII letters match in either case, as with the `i` flag on a regular
/// expression without the `u` flag.
pub fn word_at(src: &Text, at: usize, word: &str) -> bool {
    for (k, w) in word.chars().enumerate() {
        let c = src.c(at + k);
        let w = w as u32;
        if c != w && !((97..=122).contains(&w) && c == w - 32) {
            return false;
        }
    }
    true
}
