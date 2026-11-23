//! Ports svg/ink.ts: which text colour reads on a fill a style sets.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use crate::pele::js::{self, jmax, jmin};

/// Named colours that #fff contrasts with more than #1f1f1f does (`DARK`).
const DARK: &str = "black blue blueviolet brown crimson darkblue darkcyan darkgreen darkmagenta darkolivegreen darkorchid darkred darkslateblue darkslategray darkviolet dimgray firebrick forestgreen green indigo maroon mediumblue mediumslateblue mediumvioletred midnightblue navy olive purple rebeccapurple royalblue saddlebrown seagreen sienna slateblue steelblue teal";
/// ... and the rest (`LIGHT`).
const LIGHT: &str = "aliceblue antiquewhite aqua aquamarine azure beige bisque blanchedalmond burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk cyan darkgoldenrod darkgray darkkhaki darkorange darksalmon darkseagreen darkturquoise deeppink deepskyblue dodgerblue floralwhite fuchsia gainsboro ghostwhite gold goldenrod gray greenyellow honeydew hotpink indianred ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightpink lightsalmon lightseagreen lightskyblue lightslategray lightsteelblue lightyellow lime limegreen linen magenta mediumaquamarine mediumorchid mediumpurple mediumseagreen mediumspringgreen mediumturquoise mintcream mistyrose moccasin navajowhite oldlace olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue red rosybrown salmon sandybrown seashell silver skyblue slategray snow springgreen tan thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen";

fn in_set(set: &str, name: &str) -> bool {
    set.split(' ').any(|n| n == name)
}

pub const ON_LIGHT: &str = "var(--pele-fg-on-light,#1f1f1f)";
pub const ON_DARK: &str = "var(--pele-fg-on-dark,#fff)";

/// `RE_FILL.exec(s)?.[1]`: /[";]fill:([^;"]*)/.
pub fn style_fill(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if (b[i] == b'"' || b[i] == b';') && s[i + 1..].starts_with("fill:") {
            let start = i + 6;
            let end = s[start..]
                .find([';', '"'])
                .map(|k| start + k)
                .unwrap_or(s.len());
            return Some(&s[start..end]);
        }
        i += 1;
    }
    None
}

/// `[...shape.matchAll(RE_OPACITIES)]`: /[";](?:fill-)?opacity:([^;"]*)/g.
fn opacities(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' || b[i] == b';' {
            let rest = &s[i + 1..];
            let skip = if rest.starts_with("opacity:") {
                8
            } else if rest.starts_with("fill-opacity:") {
                13
            } else {
                0
            };
            if skip > 0 {
                let start = i + 1 + skip;
                let end = s[start..]
                    .find([';', '"'])
                    .map(|k| start + k)
                    .unwrap_or(s.len());
                out.push(&s[start..end]);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `RE_HEX.test(value)`: /^#(?:[\da-f]{3,4}|[\da-f]{6}|[\da-f]{8})$/i.
fn is_hex(value: &str) -> bool {
    let Some(d) = value.strip_prefix('#') else {
        return false;
    };
    matches!(d.len(), 3 | 4 | 6 | 8) && d.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `RE_NUMBER.exec(raw)`: /^[+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:e[+-]?\d+)?(%|deg|grad|rad|turn)?$/i.
/// Some(unit) on a match ("" for none).
fn number_unit(raw: &str) -> Option<String> {
    let b = raw.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let d = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i > d {
        if i < b.len() && b[i] == b'.' {
            let f = i + 1;
            let mut k = f;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k > f {
                i = k;
            }
        }
    } else if i < b.len() && b[i] == b'.' {
        let f = i + 1;
        let mut k = f;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        if k == f {
            return None;
        }
        i = k;
    } else {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut k = i + 1;
        if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
            k += 1;
        }
        let e = k;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        if k > e {
            i = k;
        }
    }
    let unit = &raw[i..];
    let lower = unit.to_ascii_lowercase();
    if matches!(lower.as_str(), "" | "%" | "deg" | "grad" | "rad" | "turn") {
        Some(unit.to_string())
    } else {
        None
    }
}

/// Ports `linear`.
fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Ports `luminance`.
fn luminance(r: f64, g: f64, b: f64) -> f64 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// Ports `clamp`.
fn clamp(n: f64) -> f64 {
    jmin(jmax(n, 0.0), 1.0)
}

/// Luminance where #1f1f1f and #fff have equal contrast (`EVEN`).
fn even() -> f64 {
    (1.05 * (luminance(31.0 / 255.0, 31.0 / 255.0, 31.0 / 255.0) + 0.05)).sqrt() - 0.05
}

/// Ports `amount`.
fn amount(raw: &str, full: f64) -> f64 {
    match number_unit(raw) {
        None => f64::NAN,
        Some(unit) if !unit.is_empty() && unit != "%" => f64::NAN,
        Some(unit) => clamp(js::parse_float(raw) / if unit.is_empty() { full } else { 100.0 }),
    }
}

/// Ports `hue`.
fn hue(raw: &str) -> f64 {
    let Some(unit) = number_unit(raw) else {
        return f64::NAN;
    };
    if unit == "%" {
        return f64::NAN;
    }
    let unit = unit.to_ascii_lowercase();
    let n = js::parse_float(raw);
    match unit.as_str() {
        "turn" => n * 360.0,
        "rad" => (n * 180.0) / core::f64::consts::PI,
        "grad" => n * 0.9,
        _ => n,
    }
}

/// `main.trim().split(/\s+/)`.
fn split_ws(s: &str) -> Vec<String> {
    let t = js::trim(s);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_ws = false;
    for c in t.chars() {
        if js::is_space(c) {
            if !in_ws {
                out.push(core::mem::take(&mut cur));
                in_ws = true;
            }
        } else {
            cur.push(c);
            in_ws = false;
        }
    }
    out.push(cur);
    out
}

/// Ports `args`: the comma syntax takes no `none`, rgb() channels all or
/// none in %, and hsl() saturation and lightness in %.
fn args(body: &str, hsl: bool) -> Option<Vec<String>> {
    let legacy = body.contains(',');
    let (main, alpha) = if legacy {
        (body, None)
    } else {
        let parts: Vec<&str> = body.split('/').collect();
        if parts.len() > 2 {
            return None;
        }
        (parts[0], parts.get(1).copied())
    };
    let mut list: Vec<String> = if legacy {
        js::trim(main)
            .split(',')
            .map(|p| js::trim(p).to_string())
            .collect()
    } else {
        split_ws(main)
    };
    if legacy {
        if list.len() < 3 || list.len() > 4 {
            return None;
        }
    } else if list.len() != 3 {
        return None;
    }
    if let Some(a) = alpha {
        list.push(js::trim(a).to_string());
    }
    if !legacy {
        return Some(
            list.into_iter()
                .map(|c| if c == "none" { "0".to_string() } else { c })
                .collect(),
        );
    }
    let from = if hsl { 1 } else { 0 };
    let percent = list[from..3].iter().filter(|c| c.ends_with('%')).count();
    if percent == (if hsl { 2 } else { 3 }) || (!hsl && percent == 0) {
        Some(list)
    } else {
        None
    }
}

/// Ports `plainColor`: a hex colour or one of CSS's named colours.
pub fn plain_color(raw: &str) -> bool {
    let value = js::trim(raw).to_lowercase();
    let name = js::replace_first(&value, "grey", "gray");
    is_hex(&value)
        || in_set(DARK, &name)
        || in_set(LIGHT, &name)
        || value == "transparent"
        || value == "currentcolor"
}

/// `RE_FUNCTION.exec(value)`: /^(rgba?|hsla?)\(([^()]*)\)$/i (value is lower case).
fn function(value: &str) -> Option<(&str, &str)> {
    let open = value.find('(')?;
    let name = &value[..open];
    if !matches!(name, "rgb" | "rgba" | "hsl" | "hsla") {
        return None;
    }
    let body = value[open + 1..].strip_suffix(')')?;
    if body.contains(['(', ')']) {
        return None;
    }
    Some((name, body))
}

/// Ports `color`: luminance and alpha; named colours use 0 or 1 to
/// preserve the contrast choice.
fn color(raw: &str) -> Option<(f64, f64)> {
    let value = js::trim(raw).to_lowercase();
    let name = js::replace_first(&value, "grey", "gray");
    if in_set(DARK, &name) {
        return Some((0.0, 1.0));
    }
    if in_set(LIGHT, &name) {
        return Some((1.0, 1.0));
    }
    if is_hex(&value) {
        let digits: String = if value.len() < 6 {
            value[1..].chars().flat_map(|c| [c, c]).collect()
        } else {
            value[1..].to_string()
        };
        let n = |i: usize| js::parse_int(&digits[2 * i..2 * i + 2], 16) / 255.0;
        return Some((
            luminance(n(0), n(1), n(2)),
            if digits.len() == 8 { n(3) } else { 1.0 },
        ));
    }
    let (fname, body) = function(&value)?;
    let hsl = fname.starts_with("hsl");
    let list = args(body, hsl)?;
    let alpha = if list.len() == 4 {
        amount(&list[3], 1.0)
    } else {
        1.0
    };
    let rgb: [f64; 3] = if !hsl {
        [
            amount(&list[0], 255.0),
            amount(&list[1], 255.0),
            amount(&list[2], 255.0),
        ]
    } else {
        let h = (((hue(&list[0]) % 360.0) + 360.0) % 360.0) / 30.0;
        let s = amount(&list[1], 100.0);
        let l = amount(&list[2], 100.0);
        let a = s * jmin(l, 1.0 - l);
        let f = |k: f64| {
            let t = (k + h) % 12.0;
            l - a * jmax(-1.0, jmin(jmin(t - 3.0, 9.0 - t), 1.0))
        };
        [f(0.0), f(8.0), f(4.0)]
    };
    let lum = luminance(rgb[0], rgb[1], rgb[2]);
    if (lum + alpha).is_nan() {
        None
    } else {
        Some((lum, alpha))
    }
}

/// Ports `opacity`: how opaque a style's fill-opacity and opacity leave
/// its fill, or NaN when either isn't a plain number.
fn opacity(shape: &str) -> f64 {
    let mut product = 1.0;
    for value in opacities(shape) {
        product *= amount(js::trim(value), 1.0);
    }
    product
}

/// Ports `ink` (without its cache, which only saves time): the text
/// colour for a style's fill, where its contrast is known. Fills below half
/// opacity depend on an unknown background.
pub fn ink(shape: &str, text: &str) -> Option<&'static str> {
    pick(shape, text)
}

/// Ports `pick`.
fn pick(shape: &str, text: &str) -> Option<&'static str> {
    let fill = style_fill(shape)?;
    if style_fill(text).is_some() {
        return None;
    }
    let found = color(fill)?;
    // An opacity that isn't a plain number, such as calc() or var(), may still be low.
    // NaN (an opacity that is no plain number) fails the test too.
    if (found.1 * opacity(shape))
        .partial_cmp(&0.5)
        .is_none_or(|o| o == core::cmp::Ordering::Less)
    {
        return None;
    }
    Some(if found.0 > even() { ON_LIGHT } else { ON_DARK })
}

/// Ports `paints`: unknown fill colours and opacities are treated as opaque.
pub fn paints(shape: &str) -> bool {
    let Some(fill) = style_fill(shape) else {
        return false;
    };
    let fill = js::trim(fill).to_lowercase();
    if fill.is_empty() || fill == "none" || fill == "transparent" {
        return false;
    }
    let scale = opacity(shape);
    color(&fill).map(|c| c.1).unwrap_or(1.0) * (if scale.is_nan() { 1.0 } else { scale }) > 0.0
}

/// Ports `inkFill`.
pub fn ink_fill(shape: &str, text: &str, fallback: Option<&str>) -> String {
    match ink(shape, text).or(fallback) {
        Some(c) if !c.is_empty() => format!(" fill=\"{}\"", c),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inks() {
        assert_eq!(ink(" style=\"fill:#000;\"", ""), Some(ON_DARK));
        assert_eq!(ink(" style=\"fill:#fff;\"", ""), Some(ON_LIGHT));
        assert_eq!(ink(" style=\"fill:rgb(255, 255, 0);\"", ""), Some(ON_LIGHT));
        assert_eq!(ink(" style=\"fill:hsl(240 100% 20%);\"", ""), Some(ON_DARK));
        assert_eq!(ink(" style=\"fill:#000;opacity:0.2;\"", ""), None);
        assert!(paints(" style=\"fill:#f9f;\""));
        assert!(!paints(" style=\"fill:none;\""));
        assert!(plain_color("Grey"));
    }
}
