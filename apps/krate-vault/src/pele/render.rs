//! The entry Vault calls: Mermaid text to the SVG Obsidian 1.15 draws
//! for it, through the port of Pele's `render` ([`crate::pele::core`]).

use alloc::string::String;

pub use crate::pele::core::{render as render_with, LinkInfo, RenderOptions, RenderResult};
pub use crate::pele::errors::{PeleError, PeleErrorCode};
pub use crate::pele::text::measurer::{style, Measure, MetricsMeasurer, METRICS};

/// Renders `src` with `measure`, the options otherwise Pele's defaults.
pub fn render(src: &str, measure: &dyn Measure) -> Result<String, PeleError> {
    let options = RenderOptions {
        measurer: Some(measure),
        ..RenderOptions::default()
    };
    render_with(src, &options).map(|r| r.svg)
}

/// The options Obsidian 1.15 passes Pele for a ```mermaid block
/// (`useMaxWidth: false` for flowcharts, sequence and ER diagrams, so they
/// draw at their own size), with Vault's measurer and font.
pub fn obsidian_options<'a>(
    measure: &'a dyn Measure,
    id_prefix: &str,
    font_family: &str,
) -> RenderOptions<'a> {
    use crate::json::Value;
    let fixed = || {
        let mut o = Value::obj();
        o.set("useMaxWidth", Value::Bool(false));
        o
    };
    let mut theme = Value::obj();
    theme.set("fontFamily", Value::s("var(--font-mermaid)"));
    let mut config = Value::obj();
    config.set("startOnLoad", Value::Bool(false));
    config.set("securityLevel", Value::s("strict"));
    config.set("themeVariables", theme);
    config.set("flowchart", fixed());
    config.set("sequence", fixed());
    config.set("er", fixed());
    RenderOptions {
        measurer: Some(measure),
        id_prefix: Some(id_prefix.into()),
        font_family: Some(font_family.into()),
        config: Some(config),
        ..RenderOptions::default()
    }
}
