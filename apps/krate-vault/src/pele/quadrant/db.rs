//! Ports diagrams/quadrant/db.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::{self, is_space, trim, trim_end, trim_start};

/// Ports `interface QuadrantText`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadrantText {
    pub text: String,
    pub markdown: bool,
}

/// Ports `interface PointStyle`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PointStyle {
    pub radius: Option<f64>,
    pub color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_width: Option<String>,
}

/// Ports `interface QuadrantPoint`.
#[derive(Debug, Clone, PartialEq)]
pub struct QuadrantPoint {
    pub x: f64,
    pub y: f64,
    pub text: QuadrantText,
    pub class_name: String,
    pub style: PointStyle,
}

fn invalid(style: &str, value: &str, kind: &str) -> PeleError {
    PeleError::of(
        format!(
            "value for {} {} is invalid, please use a valid {}",
            style, value, kind
        ),
        PeleErrorCode::Semantic,
        "quadrantChart",
    )
}

/// Ports `keyValue`: the first two parts of Mermaid's
/// `style.trim().split(/\s*:\s*/)`, found in one pass.
fn key_value(style: &str) -> (String, String) {
    let s = trim(style);
    let Some(colon) = s.find(':') else {
        return (s.into(), "undefined".into());
    };
    let rest = trim_start(&s[colon + 1..]);
    let value = match rest.find(':') {
        None => rest,
        Some(next) => trim_end(&rest[..next]),
    };
    (trim_end(&s[..colon]).into(), value.into())
}

/// `/^\d+$/`.
fn is_number(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())
}

/// `/^#?([\dA-Fa-f]{6}|[\dA-Fa-f]{3})$/`.
fn is_hex(v: &str) -> bool {
    let h = v.strip_prefix('#').unwrap_or(v);
    (h.len() == 6 || h.len() == 3) && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `/^\d+px$/`.
fn is_pixels(v: &str) -> bool {
    v.strip_suffix("px").is_some_and(is_number)
}

/// Ports `parseStyles`.
pub fn parse_styles(styles: &[String]) -> Result<PointStyle, PeleError> {
    let mut out = PointStyle::default();
    for style in styles {
        let (key, value) = key_value(style);
        match key.as_str() {
            "radius" => {
                if !is_number(&value) {
                    return Err(invalid(&key, &value, "number"));
                }
                out.radius = Some(js::parse_int(&value, 10));
            }
            "color" => {
                if !is_hex(&value) {
                    return Err(invalid(&key, &value, "hex code"));
                }
                out.color = Some(value);
            }
            "stroke-color" => {
                if !is_hex(&value) {
                    return Err(invalid(&key, &value, "hex code"));
                }
                out.stroke_color = Some(value);
            }
            "stroke-width" => {
                if !is_pixels(&value) {
                    return Err(invalid(&key, &value, "number of pixels (eg. 10px)"));
                }
                out.stroke_width = Some(value);
            }
            _ => {
                return Err(PeleError::of(
                    format!("style named {} is not supported.", key),
                    PeleErrorCode::Semantic,
                    "quadrantChart",
                ))
            }
        }
    }
    Ok(out)
}

fn trimmed(label: &QuadrantText) -> QuadrantText {
    QuadrantText {
        text: trim(&label.text).into(),
        markdown: label.markdown,
    }
}

/// Ports `class QuadrantDb`. `classes` keeps insertion order, as a Map does.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadrantDb {
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub quadrants: [Option<QuadrantText>; 4],
    pub x_axis_left: Option<QuadrantText>,
    pub x_axis_right: Option<QuadrantText>,
    pub y_axis_bottom: Option<QuadrantText>,
    pub y_axis_top: Option<QuadrantText>,
    pub points: Vec<QuadrantPoint>,
    pub classes: Vec<(String, PointStyle)>,
}

/// `.replace(/\n\s+/g, '\n')`.
pub fn tighten_lines(text: &str) -> String {
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

impl QuadrantDb {
    /// `setQuadrant1Text` .. `setQuadrant4Text`.
    pub fn set_quadrant_text(&mut self, k: usize, label: &QuadrantText) {
        self.quadrants[k] = Some(trimmed(label));
    }

    pub fn set_x_axis_left_text(&mut self, label: &QuadrantText) {
        self.x_axis_left = Some(trimmed(label));
    }

    pub fn set_x_axis_right_text(&mut self, label: &QuadrantText) {
        self.x_axis_right = Some(trimmed(label));
    }

    pub fn set_y_axis_bottom_text(&mut self, label: &QuadrantText) {
        self.y_axis_bottom = Some(trimmed(label));
    }

    pub fn set_y_axis_top_text(&mut self, label: &QuadrantText) {
        self.y_axis_top = Some(trimmed(label));
    }

    /// Ports `addPoint`.
    pub fn add_point(
        &mut self,
        label: &QuadrantText,
        class_name: &str,
        x: &str,
        y: &str,
        styles: &[String],
    ) -> Result<(), PeleError> {
        let style = parse_styles(styles)?;
        self.points.push(QuadrantPoint {
            x: js::number(x),
            y: js::number(y),
            text: trimmed(label),
            class_name: class_name.into(),
            style,
        });
        Ok(())
    }

    /// Ports `addClass`.
    pub fn add_class(&mut self, class_name: &str, styles: &[String]) -> Result<(), PeleError> {
        let style = parse_styles(styles)?;
        match self.classes.iter_mut().find(|(k, _)| k == class_name) {
            Some(entry) => entry.1 = style,
            None => self.classes.push((class_name.into(), style)),
        }
        Ok(())
    }

    pub fn set_diagram_title(&mut self, text: &str) {
        self.title = Some(text.into());
    }

    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(trim_start(text).into());
    }

    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(tighten_lines(text));
    }
}
