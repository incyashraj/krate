//! Ports svg/title.ts: a diagram's title, drawn as text whatever it holds.

use alloc::format;
use alloc::string::String;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::builder::{label_svg, num};
use crate::pele::js::jmax;
use crate::pele::text::label::{layout_label, Label};
use crate::pele::text::measurer::{style, Measure};

/// Ports `titleLabel`: Mermaid draws diagram titles as text, whatever they
/// hold (no math).
pub fn title_label(
    title: Option<&str>,
    measurer: &dyn Measure,
    size: f64,
    max_width: f64,
) -> Label {
    layout_label(title, false, measurer, size, max_width, style::BOLD, None)
}

/// Ports `interface Titled`.
#[derive(Debug, Clone, PartialEq)]
pub struct Titled {
    pub width: f64,
    pub height: f64,
    pub content: String,
}

/// Ports `withTitle`: puts a diagram title above content that was laid
/// out without one.
#[allow(clippy::too_many_arguments)]
pub fn with_title(
    title: Option<&str>,
    measurer: &dyn Measure,
    size: f64,
    pad: f64,
    width: f64,
    height: f64,
    content: String,
) -> Titled {
    let label = title_label(title, measurer, size, 4000.0);
    if label.height == 0.0 {
        return Titled {
            width,
            height,
            content,
        };
    }
    let shift = label.height + 12.0;
    let full = jmax(width, (label.width + 2.0 * pad).ceil());
    Titled {
        width: full,
        height: (height + shift).ceil(),
        content: label_svg(
            &label,
            full / 2.0,
            pad + label.height / 2.0,
            " class=\"pele-title\" font-weight=\"var(--_tw)\"",
            None,
        ) + &format!(
            "<g transform=\"translate({},{})\">{}</g>",
            num((full - width) / 2.0),
            num(shift),
            content
        ),
    }
}
