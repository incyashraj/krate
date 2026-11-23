//! Ports diagrams/common/icon.ts.

use alloc::format;
use alloc::string::String;

use crate::pele::svg::builder::{esc_text, num};
use crate::pele::text::label::{icon_markup, Icons};

/// Ports `iconSvg`: a square for a named icon, in the form labels use.
/// The host's resolver supplies the drawing, if any.
pub fn icon_svg(name: &str, x: f64, y: f64, side: f64, icons: Icons) -> String {
    format!(
        "<svg class=\"pele-icon\" data-icon=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{}</svg>",
        esc_text(name),
        num(x),
        num(y),
        num(side),
        num(side),
        icon_markup(icons, name)
    )
}
