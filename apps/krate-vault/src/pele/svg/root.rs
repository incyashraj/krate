//! Ports svg/root.ts: the `<svg>` element around a drawn diagram.

use alloc::format;
use alloc::string::String;

use super::builder::{esc, esc_text};
use super::theme::{font_style, RADIUS, ROOT_STYLE};
use crate::pele::core::RenderOptions;
use crate::pele::js;

/// The weights of bold text, each set only in a diagram that has such text (`WEIGHTS`).
const WEIGHTS: [(&str, &str); 3] = [("tw", "title"), ("hw", "heading"), ("w", "bold")];

/// Ports `interface Accessible`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Accessible {
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
}

/// `content.matchAll(/var\(--_(r|tw|hw|w)\)/g)`, stopping at four names.
fn used(content: &str) -> [bool; 4] {
    // r, tw, hw, w
    let mut seen = [false; 4];
    let mut count = 0;
    let mut from = 0;
    while let Some(k) = content[from..].find("var(--_") {
        let s = from + k + 7;
        let rest = &content[s..];
        let hit = if rest.starts_with("r)") {
            Some(0)
        } else if rest.starts_with("tw)") {
            Some(1)
        } else if rest.starts_with("hw)") {
            Some(2)
        } else if rest.starts_with("w)") {
            Some(3)
        } else {
            None
        };
        if let Some(h) = hit {
            if !seen[h] {
                seen[h] = true;
                count += 1;
                if count == 4 {
                    break;
                }
            }
        }
        from = s;
    }
    seen
}

/// Ports `svgDocument`. `palette_style` is what Pele's `paletteStyle()`
/// reads from the palette of the render in progress.
#[allow(clippy::too_many_arguments)]
pub fn svg_document(
    kind: &str,
    width: f64,
    height: f64,
    font_size: f64,
    options: &RenderOptions,
    palette_style: &str,
    acc: &Accessible,
    content: &str,
) -> String {
    let prefix = esc(options.id_prefix.as_deref().unwrap_or("pele"));
    let mut head = String::new();
    let mut aria = String::new();
    if let Some(t) = acc.acc_title.as_deref().filter(|t| !t.is_empty()) {
        head.push_str(&format!(
            "<title id=\"{}-title\">{}</title>",
            prefix,
            esc_text(t)
        ));
        aria.push_str(&format!(" aria-labelledby=\"{}-title\"", prefix));
    }
    if let Some(d) = acc.acc_descr.as_deref().filter(|d| !d.is_empty()) {
        head.push_str(&format!(
            "<desc id=\"{}-desc\">{}</desc>",
            prefix,
            esc_text(d)
        ));
        aria.push_str(&format!(" aria-describedby=\"{}-desc\"", prefix));
    }
    let fit = if options.responsive == Some(false) {
        ""
    } else {
        "max-width:100%;height:auto;"
    };
    let seen = used(content);
    let mut weights = if seen[0] {
        format!(";--_r:{}", RADIUS)
    } else {
        String::new()
    };
    for (alias, name) in WEIGHTS {
        let idx = match alias {
            "tw" => 1,
            "hw" => 2,
            _ => 3,
        };
        if seen[idx] {
            weights.push_str(&format!(";--_{}:var(--pele-{}-weight,bold)", alias, name));
        }
    }
    let w = js::num_str(width);
    let h = js::num_str(height);
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"pele pele-{kind}\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" style=\"{fit}{ROOT_STYLE}{weights}{font}{palette_style}\" fill=\"var(--_fg)\" role=\"graphics-document document\" aria-roledescription=\"{kind}\"{aria}>{head}{content}</svg>",
        font = esc(&font_style(font_size, options.font_family.as_deref(), options.font_family_mono.as_deref())),
    )
}
