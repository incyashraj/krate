//! Ports svg/builder.ts: escaping, Pele's number formatter and the SVG of
//! a measured label.

use alloc::format;
use alloc::string::{String, ToString};
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use crate::pele::js::{self, jround};
use crate::pele::text::entities::decode_entities;
use crate::pele::text::label::{Icons, Label};
use crate::pele::text::measurer::style;

/// A character XML cannot hold, or DEL (`RE_INVALID_XML`, one character).
pub fn invalid_xml(c: char) -> bool {
    let c = c as u32;
    c <= 0x08
        || c == 0x0b
        || c == 0x0c
        || (0x0e..=0x1f).contains(&c)
        || c == 0x7f
        || c == 0xfffe
        || c == 0xffff
}

/// `RE_INVALID_XML.test(text)`.
pub fn has_invalid_xml(text: &str) -> bool {
    text.chars().any(invalid_xml)
}

/// Ports `esc`: escapes text for XML and drops invalid XML characters and DEL.
pub fn esc(text: &str) -> String {
    if !text
        .chars()
        .any(|c| matches!(c, '&' | '<' | '>' | '"') || invalid_xml(c))
    {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if invalid_xml(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// Ports `escText`: escapes text that is only ever read as text (a
/// tooltip, an accessible name, a `data-id`), with entity codes turned into
/// their characters first.
pub fn esc_text(text: &str) -> String {
    esc(&decode_entities(text))
}

/// `FRACTION[i]`: the two decimals of a coordinate, by hundredths.
fn push_fraction(out: &mut String, i: u64) {
    if i == 0 {
        return;
    }
    if i.is_multiple_of(10) {
        out.push('.');
        out.push((b'0' + (i / 10) as u8) as char);
    } else {
        out.push('.');
        out.push((b'0' + (i / 10) as u8) as char);
        out.push((b'0' + (i % 10) as u8) as char);
    }
}

/// Ports `num` (`f()` in the build Obsidian ships): a coordinate rounded to
/// hundredths, with trailing zeros dropped.
pub fn num(value: f64) -> String {
    let n = jround(value * 100.0);
    if n == 0.0 {
        return "0".to_string();
    }
    if n > 0.0 {
        if n < 1e11 {
            let q = (n / 100.0).floor();
            let mut out = format!("{}", q as u64);
            push_fraction(&mut out, (n - q * 100.0) as u64);
            return out;
        }
    } else if n > -1e11 {
        let m = -n;
        let q = (m / 100.0).floor();
        let mut out = format!("-{}", q as u64);
        push_fraction(&mut out, (m - q * 100.0) as u64);
        return out;
    }
    js::num_str(n / 100.0)
}

/// Ports `labelSvg`: writes a measured label as SVG text centered on (cx, cy).
pub fn label_svg(label: &Label, cx: f64, cy: f64, attrs: &str, icons: Icons) -> String {
    let lines = &label.lines;
    if lines.is_empty() {
        return String::new();
    }
    let lh = label.line_height;
    let heights = &label.heights;
    let shift = label.size * 0.35;
    let mut top = cy - label.height / 2.0;

    if lines.len() == 1 && lines[0].len() == 1 {
        let only = &lines[0][0];
        if only.icon.is_none() && only.style == 0 {
            return format!(
                "<text{} x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
                attrs,
                num(cx),
                num(top + lh / 2.0 + shift),
                esc(&only.text)
            );
        }
    }

    let shared = shared_style(label);
    let attrs = text_style(attrs, shared);
    let mut body = String::new();
    let mut extra = String::new();
    for (i, line) in lines.iter().enumerate() {
        let h = match heights {
            Some(hs) => hs[i],
            None => lh,
        };
        let mid = top + h / 2.0;
        let y = mid + shift;
        let placed = line.iter().any(|s| s.icon.is_some());
        if placed {
            let mut x = cx - label.widths[i] / 2.0;
            for span in line {
                if let Some(icon) = &span.icon {
                    let side = label.size * 1.1;
                    let inner = match icons {
                        Some(f) => f(icon).unwrap_or_default(),
                        None => String::new(),
                    };
                    extra.push_str(&format!(
                        "<svg class=\"pele-icon\" data-icon=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{}</svg>",
                        esc_text(icon),
                        num(x),
                        num(mid - side / 2.0),
                        num(side),
                        num(side),
                        inner
                    ));
                } else if !span.text.is_empty() {
                    body.push_str(&format!(
                        "<tspan x=\"{}\" y=\"{}\" text-anchor=\"start\"{}>{}</tspan>",
                        num(x),
                        num(y),
                        span_style(span.style & !shared),
                        esc(&span.text)
                    ));
                }
                x += span.width;
            }
        } else {
            for (j, span) in line.iter().enumerate() {
                let pos = if j == 0 {
                    format!(" x=\"{}\" y=\"{}\"", num(cx), num(y))
                } else {
                    String::new()
                };
                body.push_str(&format!(
                    "<tspan{}{}>{}</tspan>",
                    pos,
                    span_style(span.style & !shared),
                    esc(&span.text)
                ));
            }
            if line.is_empty() {
                body.push_str(&format!(
                    "<tspan x=\"{}\" y=\"{}\"> </tspan>",
                    num(cx),
                    num(y)
                ));
            }
        }
        top += h;
    }
    format!(
        "<text{} text-anchor=\"middle\" xml:space=\"preserve\">{}</text>{}",
        attrs, body, extra
    )
}

const BOLD: &str = " font-weight=\"var(--_w)\"";
const ITALIC: &str = " font-style=\"italic\"";
const UNDERLINE: &str = " text-decoration=\"underline\"";
const MONO: &str = " font-family=\"var(--_fm)\"";
const SUB: &str = " baseline-shift=\"sub\" font-size=\"smaller\"";
const SUP: &str = " baseline-shift=\"super\" font-size=\"smaller\"";

/// Ports `spanStyle`.
pub fn span_style(s: u32) -> String {
    let mut out = String::new();
    if s & style::BOLD != 0 {
        out.push_str(BOLD);
    }
    if s & style::ITALIC != 0 {
        out.push_str(ITALIC);
    }
    if s & style::MONO != 0 {
        out.push_str(MONO);
    }
    if s & style::UNDERLINE != 0 {
        out.push_str(UNDERLINE);
    }
    if s & style::SUB != 0 {
        out.push_str(SUB);
    } else if s & style::SUP != 0 {
        out.push_str(SUP);
    }
    out
}

/// Ports `sharedStyle`: the emphasis every span of a label has. It is
/// written once, on the text element, where a CSS rule for the label
/// reaches it.
pub fn shared_style(label: &Label) -> u32 {
    let mut shared = style::BOLD | style::ITALIC | style::MONO;
    let mut any = false;
    for line in &label.lines {
        for span in line {
            if span.icon.is_some() || span.text.is_empty() {
                continue;
            }
            shared &= span.style;
            any = true;
        }
    }
    if any {
        shared
    } else {
        0
    }
}

/// Ports `textStyle`: adds that emphasis to a text element's attributes,
/// unless they give a weight of their own.
pub fn text_style(attrs: &str, shared: u32) -> String {
    let mut out = attrs.to_string();
    if shared & style::BOLD != 0 && !attrs.contains(" font-weight=\"") {
        out.push_str(BOLD);
    }
    if shared & style::ITALIC != 0 && !attrs.contains(" font-style=\"") {
        out.push_str(ITALIC);
    }
    if shared & style::MONO != 0 && !attrs.contains(" font-family=\"") {
        out.push_str(MONO);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn num_formats_as_pele() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(-0.001), "0");
        assert_eq!(num(1.005), "1");
        assert_eq!(num(12.5), "12.5");
        assert_eq!(num(-3.456), "-3.46");
        assert_eq!(num(100.0), "100");
        assert_eq!(num(0.07), "0.07");
        assert_eq!(num(2e9), "2000000000");
        assert_eq!(num(f64::NAN), "NaN");
        assert_eq!(esc("a<b>&\"\u{1}c"), "a&lt;b&gt;&amp;&quot;c");
    }
}
