//! Ports svg/palette.ts: the --pele-series-* colours a diagram's Mermaid
//! theme variables set. Pele keeps the palette of the render in progress in
//! a module variable (`withPalette`); the port passes the [`Palette`] value
//! to what reads it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::builder::esc;
use super::ink::{ink, plain_color};
use crate::json::Value;
use crate::pele::js;

/// One entry of `PALETTES`: `first` is the number of the variable for the
/// first series.
struct Entry {
    kind: &'static str,
    prefix: &'static str,
    first: usize,
    repeat: bool,
}

const PALETTES: [Entry; 7] = [
    Entry {
        kind: "pie",
        prefix: "pie",
        first: 1,
        repeat: false,
    },
    Entry {
        kind: "gitGraph",
        prefix: "git",
        first: 0,
        repeat: false,
    },
    Entry {
        kind: "timeline",
        prefix: "cScale",
        first: 0,
        repeat: false,
    },
    Entry {
        kind: "radar",
        prefix: "cScale",
        first: 0,
        repeat: false,
    },
    Entry {
        kind: "treemap",
        prefix: "cScale",
        first: 0,
        repeat: false,
    },
    Entry {
        kind: "mindmap",
        prefix: "cScale",
        first: 1,
        repeat: false,
    },
    Entry {
        kind: "venn",
        prefix: "venn",
        first: 1,
        repeat: true,
    },
];

const SLOTS: usize = 8;

const FUNCTIONS: [&str; 13] = [
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "hwb",
    "lab",
    "lch",
    "oklab",
    "oklch",
    "color",
    "color-mix",
    "light-dark",
    "var",
];

/// `RE_CHARS.test(color)`: /^[\w\s#%.,+\-/()]{1,200}$/.
fn safe_chars(color: &str) -> bool {
    let n = js::len16(color);
    (1..=200).contains(&n)
        && color
            .chars()
            .all(|c| js::is_word(c) || js::is_space(c) || "#%.,+-/()".contains(c))
}

/// Ports `safeColor`: a colour that can't load a resource, end its
/// declaration or read the page's own variables.
fn safe_color(value: Option<&Value>) -> Option<String> {
    let Some(Value::Str(value)) = value else {
        return None;
    };
    let color = js::trim(value);
    if !safe_chars(color) {
        return None;
    }
    if !color.contains('(') {
        return if plain_color(color) {
            Some(color.to_string())
        } else {
            None
        };
    }
    let mut depth: i32 = 0;
    for c in color.chars() {
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
            if depth < 0 {
                return None;
            }
        }
    }
    if depth != 0 {
        return None;
    }
    // RE_FUNCTION = /([\w-]*)\s*\(/g: the name before each '('.
    for (k, c) in color.char_indices() {
        if c != '(' {
            continue;
        }
        let before = &color[..k];
        let trimmed = before.trim_end_matches(js::is_space);
        let start = trimmed
            .char_indices()
            .rev()
            .take_while(|(_, c)| js::is_word(*c) || *c == '-')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(trimmed.len());
        let name = trimmed[start..].to_lowercase();
        if !FUNCTIONS.contains(&name.as_str()) {
            return None;
        }
    }
    // RE_VAR = /var\(\s*([\w-]*)/gi
    let lower = color.to_ascii_lowercase();
    let mut from = 0;
    while let Some(k) = lower[from..].find("var(") {
        let mut p = from + k + 4;
        while let Some(c) = color[p..].chars().next() {
            if !js::is_space(c) {
                break;
            }
            p += c.len_utf8();
        }
        let e = color[p..]
            .char_indices()
            .find(|(_, c)| !(js::is_word(*c) || *c == '-'))
            .map(|(i, _)| p + i)
            .unwrap_or(color.len());
        if !color[p..e].starts_with("--pele-") {
            return None;
        }
        from = e;
    }
    Some(color.to_string())
}

/// Ports `splitList`: splits at the commas between colours, not those
/// inside rgb() and other functions.
fn split_list(value: &str) -> Vec<Value> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in value.char_indices() {
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
        } else if c == ',' && depth == 0 {
            parts.push(&value[start..i]);
            start = i + 1;
        }
    }
    parts.push(&value[start..]);
    parts
        .into_iter()
        .map(|p| Value::Str(js::trim(p).to_string()))
        .collect()
}

/// Ports `entries`.
fn entries(kind: &str, vars: &Value) -> Option<(Vec<Value>, bool)> {
    if kind == "xychart" {
        return match vars.get("xyChart").and_then(|x| x.get("plotColorPalette")) {
            Some(Value::Str(list)) => Some((split_list(list), true)),
            _ => None,
        };
    }
    let palette = PALETTES.iter().find(|p| p.kind == kind)?;
    let mut list: Vec<Value> = (0..SLOTS)
        .map(|i| {
            vars.get(&format!("{}{}", palette.prefix, palette.first + i))
                .cloned()
                .unwrap_or(Value::Null)
        })
        .collect();
    if palette.repeat {
        list.retain(|v| !matches!(v, Value::Null) && *v != Value::Str(String::new()));
    }
    Some((list, palette.repeat))
}

/// Ports `interface Palette`: the style declarations, and label colours
/// for text on each series where the contrast of its colour is known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Palette {
    pub style: String,
    pub inks: Vec<Option<&'static str>>,
}

/// Ports `palette`: the --pele-series-* declarations for the palette a
/// diagram's theme variables set.
pub fn palette(kind: &str, config: &Value) -> Palette {
    let vars = match config.get("themeVariables") {
        Some(v @ Value::Obj(_)) => v.clone(),
        _ => Value::obj(),
    };
    let Some((list, repeat)) = entries(kind, &vars) else {
        return Palette::default();
    };
    if list.is_empty() {
        return Palette::default();
    }
    let mut style = String::new();
    let mut inks: Vec<Option<&'static str>> = Vec::new();
    for i in 0..SLOTS {
        if !repeat && i >= list.len() {
            break;
        }
        let Some(color) = safe_color(list.get(i % list.len())) else {
            continue;
        };
        style.push_str(&format!(";--pele-series-{}:{}", i + 1, color));
        if inks.len() <= i {
            inks.resize(i + 1, None);
        }
        inks[i] = ink(&format!(";fill:{}", color), "");
    }
    if style.is_empty() {
        Palette::default()
    } else {
        Palette {
            style: esc(&style),
            inks,
        }
    }
}

impl Palette {
    /// Ports `seriesInkFill`: a fill for a label on the series colour, when
    /// the palette sets one that needs it.
    pub fn series_ink_fill(&self, index: i64) -> String {
        let k = ((index % SLOTS as i64) + SLOTS as i64) % SLOTS as i64;
        match self.inks.get(k as usize).copied().flatten() {
            Some(c) => format!(" fill=\"{}\"", c),
            None => String::new(),
        }
    }
}
