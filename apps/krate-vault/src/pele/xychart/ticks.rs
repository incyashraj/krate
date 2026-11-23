//! Ports diagrams/xychart/ticks.ts.

use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use crate::pele::js::{jmax, jround};

/// `10 ** k` for a whole k.
fn pow10(k: f64) -> f64 {
    10f64.powf(k)
}

/// Ports `tickStep`: a step of 1, 2 or 5 times a power of ten that divides
/// `span` into about `count` parts. A step below one comes back as the
/// negative of its inverse, which is a whole number, so that tick values
/// are found by division and print without rounding noise.
pub fn tick_step(span: f64, count: f64) -> f64 {
    let raw = span / jmax(count, 1.0);
    let power = raw.log10().floor();
    let unit = raw / pow10(power);
    let factor = if unit >= 7.07 {
        10.0
    } else if unit >= 3.16 {
        5.0
    } else if unit >= 1.41 {
        2.0
    } else {
        1.0
    };
    if power < 0.0 {
        -pow10(-power) / factor
    } else {
        factor * pow10(power)
    }
}

/// Ports `tickIndex`.
pub fn tick_index(step: f64, value: f64) -> f64 {
    if step > 0.0 {
        value / step
    } else {
        value * -step
    }
}

/// Ports `tickValue`.
pub fn tick_value(step: f64, index: f64) -> f64 {
    if step > 0.0 {
        index * step
    } else {
        index / -step
    }
}

/// Ports `ticksBetween`: the multiples of a step from `lo` to `hi`, both included.
pub fn ticks_between(lo: f64, hi: f64, step: f64) -> Vec<f64> {
    let mut first = jround(tick_index(step, lo));
    if tick_value(step, first) < lo {
        first += 1.0;
    }
    let mut last = jround(tick_index(step, hi));
    if tick_value(step, last) > hi {
        last -= 1.0;
    }
    let mut out = Vec::new();
    let mut i = first;
    while i <= last && out.len() < 100 {
        out.push(tick_value(step, i));
        i += 1.0;
    }
    out
}
