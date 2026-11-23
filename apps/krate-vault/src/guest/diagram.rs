//! A ```mermaid block drawn as Obsidian 1.15 draws it: Pele's own SVG
//! (src/pele/, ported) with Obsidian's theme put into its CSS variables,
//! then read by the app's SVG reader into a picture and its text.

use super::look::{self, Rgba, Theme};
use alloc::string::String;

/// Pele's text measurer, backed by the canvas: the same font and sizes
/// the picture's text is drawn with.
pub struct CanvasMeasure {
    pub c: u64,
}

impl crate::pele::render::Measure for CanvasMeasure {
    fn width(&self, text: &str, size: f64, style: u32) -> f64 {
        use crate::pele::render::style as st;
        let size = if style & (st::SUB | st::SUP) != 0 {
            size / 1.2
        } else {
            size
        } as f32;
        let gs = krate::gfx::types::TextStyle {
            weight: if style & st::BOLD != 0 { 700 } else { 400 },
            italic: style & st::ITALIC != 0,
            letter_spacing: 0.0,
            family: if style & st::MONO != 0 {
                krate::gfx::types::FontFamily::Mono
            } else {
                krate::gfx::types::FontFamily::Sans
            },
        };
        f64::from(look::width_styled(self.c, text, size, gs))
    }
}

fn css(c: Rgba) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    if c.a >= 0.999 {
        alloc::format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
    } else {
        alloc::format!("rgba({},{},{},{:.3})", b(c.r), b(c.g), b(c.b), c.a)
    }
}

/// The value Obsidian's stylesheet gives each `--pele-*` variable (its
/// `body, .mermaid` rule: `--pele-surface: var(--mermaid-surface)` and so
/// on, down to the theme's own colours).
pub fn obsidian_var(name: &str, t: &Theme) -> Option<String> {
    let series_dark = [
        0x027AFF, 0xE9973F, 0x44CF6E, 0xFB464C, 0x53DFDD, 0xE0DE71, 0xA882FF, 0xFA99CD,
    ];
    let series_light = [
        0x086DDD, 0xEC7500, 0x08B94E, 0xE93147, 0x00BFBC, 0xE0AC00, 0x7852EE, 0xD53984,
    ];
    Some(match name {
        "bg" => css(t.bg),
        "fg" | "line" => css(t.text),
        "muted" => css(t.muted),
        "surface" => css(t.side),
        "surface-alt" => css(t.code_bg),
        "border" => css(t.border_hover),
        "accent" => css(t.accent),
        "critical" | "error" => css(t.danger),
        "font" => String::from("sans-serif"),
        "font-mono" => String::from("monospace"),
        "radius" => String::from("4px"),
        "title-weight" | "heading-weight" | "bold-weight" => String::from("600"),
        _ => {
            let n: usize = name.strip_prefix("series-")?.parse().ok()?;
            let pal = if t.dark { series_dark } else { series_light };
            css(look::hex(*pal.get(n.checked_sub(1)?)?))
        }
    })
}

/// Pele's SVG in Obsidian's colours for `t`.
pub fn themed(svg: &str, t: &Theme) -> String {
    crate::pele::obsidian::themed(svg, &|n| obsidian_var(n, t))
}
