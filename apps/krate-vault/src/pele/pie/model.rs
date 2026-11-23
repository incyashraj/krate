//! Ports diagrams/pie/model.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::parser::PieAst;
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::{is_space, num_str};

/// Ports `interface PieModel`. `sections` keeps insertion order, as a Map does.
#[derive(Debug, Clone, PartialEq)]
pub struct PieModel {
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub show_data: bool,
    pub sections: Vec<(String, f64)>,
}

/// `.replace(/\n\s+/g, '\n')`.
fn tighten_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '\n' {
            while chars.peek().is_some_and(|d| is_space(*d)) {
                chars.next();
            }
        }
    }
    out
}

/// Ports `buildPie`.
pub fn build_pie(ast: PieAst, title: Option<&str>) -> Result<PieModel, PeleError> {
    let mut sections: Vec<(String, f64)> = Vec::new();
    for (label, value) in ast.sections {
        if value < 0.0 {
            return Err(PeleError::of(
                format!(
                    "\"{}\" has invalid value: {}. Negative values are not allowed in pie charts. All slice values must be >= 0.",
                    label,
                    num_str(value)
                ),
                PeleErrorCode::Semantic,
                "pie",
            ));
        }
        if !sections.iter().any(|(l, _)| *l == label) {
            sections.push((label, value));
        }
    }
    Ok(PieModel {
        title: match ast.title.filter(|t| !t.is_empty()) {
            Some(t) => Some(t),
            None => title.map(String::from),
        },
        acc_title: ast
            .acc_title
            .filter(|t| !t.is_empty())
            .map(|t| String::from(crate::pele::js::trim_start(&t))),
        acc_descr: ast
            .acc_descr
            .filter(|t| !t.is_empty())
            .map(|t| tighten_lines(&t)),
        show_data: ast.show_data,
        sections,
    })
}
