//! Ports core.ts and types.ts: Pele's `render` -- the text through
//! preprocessing, detection and the diagram type's parser and renderer to
//! the finished SVG -- and the options it takes.
//!
//! Pele registers every diagram type; the port has the flowchart so far,
//! and reports any other type as not supported, as Pele does for a type
//! that is not registered.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::json::Value;
use crate::pele::detect::{detect_diagram, Detected};
use crate::pele::errors::{input_offset, syntax_error_in, Edit, PeleError, PeleErrorCode};
use crate::pele::flowchart;
use crate::pele::js;
use crate::pele::preprocess::{
    cleanup_comments_with, encode_entities, encode_entities_with, is_record, normalize_line_breaks,
    preprocess, preprocess_with, Config,
};
use crate::pele::svg::palette::palette;
use crate::pele::text::entities::{decode_entities, written_codes};
use crate::pele::text::label::Icons;
use crate::pele::text::measurer::Measure;

/// Ports `interface RenderOptions`. Math (`math`), `monospace` parts of
/// other diagram types and `now` (gantt) are not ported.
#[derive(Clone, Default)]
pub struct RenderOptions<'a> {
    /// The text measurer; Pele's fallback width table when None.
    pub measurer: Option<&'a dyn Measure>,
    /// The fonts labels are measured in, and drawn in where the page sets no --pele-font or --pele-font-mono.
    pub font_family: Option<String>,
    pub font_family_mono: Option<String>,
    pub font_size: Option<f64>,
    pub id_prefix: Option<String>,
    pub responsive: Option<bool>,
    /// The width the host has for the diagram, in pixels.
    pub max_width: Option<f64>,
    pub auto_direction: Option<bool>,
    pub direction_breakpoint: Option<f64>,
    pub padding: Option<f64>,
    /// Source length limit, in UTF-16 code units (50,000 by default).
    pub limit: Option<usize>,
    /// Output length limit, in UTF-16 code units (4,000,000 by default).
    pub output_limit: Option<usize>,
    /// The most edges a flowchart may have (5,000 by default).
    pub max_edges: Option<usize>,
    pub link_schemes: Option<Vec<String>>,
    pub image_schemes: Option<Vec<String>>,
    pub links: Option<bool>,
    pub images: Option<bool>,
    /// Link `rel` attribute. Defaults to `noopener`; an empty string omits it.
    pub link_rel: Option<String>,
    pub icons: Icons<'a>,
    /// Mermaid config, merged under the diagram's own front matter and directives.
    pub config: Option<Value>,
}

/// Ports `interface LinkInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkInfo {
    pub id: String,
    pub href: String,
    pub internal: bool,
}

/// Ports `interface Rendered`.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub svg: String,
    pub width: f64,
    pub height: f64,
    pub links: Vec<LinkInfo>,
}

/// Ports `interface RenderResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderResult {
    pub kind: &'static str,
    pub mermaid_type: &'static str,
    pub svg: String,
    pub width: f64,
    pub height: f64,
    pub links: Vec<LinkInfo>,
}

/// Mermaid's default maxTextSize.
const DEFAULT_LIMIT: usize = 50000;
const DEFAULT_OUTPUT_LIMIT: usize = 4_000_000;
const DEFAULT_MAX_EDGES: usize = 5000;

/// A parsed diagram (`interface Parsed`).
pub enum Model {
    Flowchart(flowchart::db::FlowDb),
}

/// Ports `tagErrors`: names the diagram type in an error, and writes
/// hidden entity codes back as the author wrote them.
fn tag(found: &Detected, mut error: PeleError) -> PeleError {
    if error.kind.is_none() {
        error.kind = Some(found.kind);
    }
    if error.mermaid_type.is_none() {
        error.mermaid_type = Some(found.mermaid_type);
    }
    error.message = written_codes(&error.message);
    error.snippet = written_codes(&error.snippet);
    error
}

/// Ports `inSource` and `locatingIn`: a syntax error located in the parsed
/// text, located again in the source as the author wrote it, through every
/// edit preprocessing made.
fn relocate(error: PeleError, text: &str, parsed: &str) -> PeleError {
    let Some((at, lexical, detail)) = error.at.as_deref().cloned() else {
        return error;
    };
    let mut steps: Vec<Vec<Edit>> = Vec::new();
    let Ok(pre) = preprocess_with(text, Some(&mut steps)) else {
        return error;
    };
    let cleaned = cleanup_comments_with(&pre.with_comments, Some(&mut steps));
    encode_entities_with(&cleaned, Some(&mut steps));
    // Match the line break appended for parsing. Edits count UTF-16 units, as Pele's do.
    let source = format!("{}\n", normalize_line_breaks(text));
    let find = |offset: usize| {
        let units = input_offset(&steps, js::units_at(parsed, offset));
        (source.clone(), js::byte_at(&source, units))
    };
    let kind = error.kind.unwrap_or_default();
    let mut e = syntax_error_in(kind, parsed, at, &detail, lexical, Some(&find));
    e.kind = error.kind;
    e.mermaid_type = error.mermaid_type;
    e.at = None;
    e
}

/// Ports `parseSource`.
fn parse_source(
    text: &str,
    limit: usize,
    max_edges: usize,
    extra: Option<&Value>,
) -> Result<(Detected, Model, Config), PeleError> {
    if js::len16(text) > limit {
        return Err(PeleError::new(
            format!(
                "Diagram source is longer than the limit of {} characters.",
                limit
            ),
            PeleErrorCode::Limit,
        ));
    }
    let pre = preprocess(text)?;
    let Some(found) = detect_diagram(&pre.text) else {
        return Err(PeleError::new(
            "No diagram type detected for the given text.",
            PeleErrorCode::UnsupportedDiagram,
        ));
    };
    if found.kind != "flowchart" {
        let mut e = PeleError::new(
            format!("Diagram type \"{}\" is not supported yet.", found.kind),
            PeleErrorCode::UnsupportedDiagram,
        );
        e.kind = Some(found.kind);
        e.mermaid_type = Some(found.mermaid_type);
        return Err(e);
    }
    // { ...extra, ...pre.config }
    let config = match extra {
        Some(Value::Obj(e)) => {
            let mut merged = Value::Obj(e.clone());
            if let Value::Obj(own) = &pre.config {
                for (k, v) in own {
                    merged.set(k, v.clone());
                }
            }
            merged
        }
        _ => pre.config.clone(),
    };
    let parsed = format!("{}\n", encode_entities(&pre.text));
    let model = flowchart::parse(&parsed, &config, pre.title.as_deref(), max_edges)
        .map_err(|e| tag(&found, relocate(e, text, &parsed)))?;
    Ok((found, Model::Flowchart(model), config))
}

/// Ports `detectType`.
pub fn detect_type(text: &str) -> Option<&'static str> {
    preprocess(text)
        .ok()
        .and_then(|p| detect_diagram(&p.text))
        .map(|d| d.kind)
}

/// Ports `supports`.
pub fn supports(text: &str) -> bool {
    detect_type(text) == Some("flowchart")
}

/// Ports `render`: the SVG for a diagram's text.
pub fn render(source: &str, options: &RenderOptions) -> Result<RenderResult, PeleError> {
    let (found, model, config) = parse_source(
        source,
        options.limit.unwrap_or(DEFAULT_LIMIT),
        options.max_edges.unwrap_or(DEFAULT_MAX_EDGES),
        options.config.as_ref(),
    )?;
    let section = config.get(found.kind);
    let fixed =
        section.is_some_and(|s| is_record(s) && s.get("useMaxWidth") == Some(&Value::Bool(false)));
    let responsive = options.responsive.unwrap_or(!fixed);
    // The host's icon resolver sees names as the author meant them, with entity codes turned into their characters.
    let resolve = options.icons;
    let wrapped =
        move |name: &str| -> Option<String> { resolve.and_then(|f| f(&decode_entities(name))) };
    let mut opts = options.clone();
    opts.responsive = Some(responsive);
    opts.icons = if resolve.is_some() {
        Some(&wrapped)
    } else {
        None
    };
    let pal = palette(found.kind, &config);
    let mut rendered = match &model {
        Model::Flowchart(db) => {
            flowchart::render::render_flowchart(db, &config, &opts, &pal.style, None)
                .map_err(|e| tag(&found, e))?
        }
    };
    let output_limit = options.output_limit.unwrap_or(DEFAULT_OUTPUT_LIMIT);
    if js::len16(&rendered.svg) > output_limit {
        let mut e = PeleError::new(
            format!(
                "Diagram output is longer than the limit of {} characters.",
                output_limit
            ),
            PeleErrorCode::Limit,
        );
        e.kind = Some(found.kind);
        e.mermaid_type = Some(found.mermaid_type);
        return Err(e);
    }
    for link in rendered.links.iter_mut() {
        link.id = decode_entities(&link.id);
    }
    Ok(RenderResult {
        kind: found.kind,
        mermaid_type: found.mermaid_type,
        svg: rendered.svg,
        width: rendered.width,
        height: rendered.height,
        links: rendered.links,
    })
}
