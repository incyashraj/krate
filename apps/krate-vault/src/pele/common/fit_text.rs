//! Ports diagrams/common/fit-text.ts.

use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use crate::pele::js::{jmax, trim_end};
use crate::pele::text::measurer::Measure;

/// Ports `interface Fitted`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted {
    pub text: String,
    pub size: f64,
    pub width: f64,
}

/// Ports `fitText`: fits one line of text into a width, first by shrinking
/// the font down to `min`, then by cutting the text and adding an
/// ellipsis. None when not even one character fits.
pub fn fit_text(
    text: &str,
    max: f64,
    size: f64,
    min: f64,
    measurer: &dyn Measure,
    style: u32,
) -> Option<Fitted> {
    if text.is_empty() || max <= 0.0 {
        return None;
    }
    let mut width = measurer.width(text, size, style);
    if width <= max {
        return Some(Fitted {
            text: text.into(),
            size,
            width,
        });
    }

    // Width grows with font size, so one division lands on the right size or just above it.
    let mut fitted = jmax(min, ((size * max) / width).floor());
    while fitted >= min {
        width = measurer.width(text, fitted, style);
        if width <= max {
            return Some(Fitted {
                text: text.into(),
                size: fitted,
                width,
            });
        }
        fitted -= 1.0;
    }

    let chars: Vec<char> = text.chars().collect();
    let cut_at = |k: usize| -> String {
        let s: String = chars[..k].iter().collect();
        String::from(trim_end(&s)) + "\u{2026}"
    };
    let mut low = 0usize;
    let mut high = chars.len() as isize - 1;
    while (low as isize) < high {
        let mid = (low + high as usize + 1) >> 1;
        if measurer.width(&cut_at(mid), min, style) <= max {
            low = mid;
        } else {
            high = mid as isize - 1;
        }
    }
    if low == 0 {
        return None;
    }
    let cut = cut_at(low);
    width = measurer.width(&cut, min, style);
    if width <= max {
        Some(Fitted {
            text: cut,
            size: min,
            width,
        })
    } else {
        None
    }
}
