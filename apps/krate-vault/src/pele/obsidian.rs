//! Pele's SVG as Obsidian shows it: Obsidian's stylesheet sets Pele's
//! `--pele-*` CSS variables from its theme; the app's SVG reader has no
//! CSS variables, so they are put in place here, and the little CSS Pele
//! writes (`style="rx:…"`) becomes plain attributes.

use alloc::string::String;
use alloc::vec::Vec;

/// Replace every `var(--name[, fallback])` in `s` using `get`, innermost
/// first; a name `get` does not know keeps its fallback.
fn resolve(s: &str, get: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::from(s);
    for _ in 0..8 {
        let Some(start) = out.rfind("var(--") else {
            break;
        };
        // The matching close paren.
        let mut depth = 0;
        let mut end = None;
        for (i, ch) in out[start..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            break;
        };
        let inner = &out[start + 6..end];
        let (name, fallback) = match inner.split_once(',') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner.trim(), None),
        };
        let value = get(name)
            .or_else(|| fallback.map(String::from))
            .unwrap_or_default();
        out.replace_range(start..=end, &value);
    }
    out
}

/// Pele's SVG with Obsidian's theme in place of its CSS variables, and the
/// little CSS it uses (`style="rx:…"`, the root's custom properties)
/// written as plain attributes the SVG reader takes.
pub fn themed(svg: &str, pele_var: &dyn Fn(&str) -> Option<String>) -> String {
    // The root's `--_x:value` properties, themed.
    let mut props: Vec<(String, String)> = Vec::new();
    if let Some(i) = svg.find("style=\"") {
        let body = &svg[i + 7..];
        let body = &body[..body.find('"').unwrap_or(body.len())];
        for decl in body.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                if let Some(name) = k.trim().strip_prefix("--") {
                    let v = resolve(v, &|n| n.strip_prefix("pele-").and_then(pele_var));
                    props.push((String::from(name), v));
                }
            }
        }
    }
    let get = |n: &str| -> Option<String> {
        if let Some(p) = n.strip_prefix("pele-") {
            return pele_var(p);
        }
        props.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone())
    };
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    // Each var() inside an attribute value is resolved where it stands.
    while let Some(i) = rest.find("var(--") {
        out.push_str(&rest[..i]);
        // The whole var(...) including nested ones.
        let mut depth = 0;
        let mut end = rest.len();
        for (k, ch) in rest[i..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + k + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push_str(&resolve(&rest[i..end], &get));
        rest = &rest[end..];
    }
    out.push_str(rest);
    // `style="rx:4px"` on a rect is CSS; the reader wants the attribute.
    let mut fixed = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(i) = rest.find("style=\"rx:") {
        fixed.push_str(&rest[..i]);
        let after = &rest[i + 10..];
        let end = after.find('"').unwrap_or(after.len());
        let v = after[..end].trim().trim_end_matches("px");
        fixed.push_str("rx=\"");
        fixed.push_str(v);
        fixed.push('"');
        rest = &after[(end + 1).min(after.len())..];
    }
    fixed.push_str(rest);
    fixed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsidian_theme_goes_into_peles_variables() {
        let svg = r#"<svg style="--_bg:var(--pele-bg,#fff);--_s:var(--pele-surface,#f3f3f3);--_r:var(--pele-radius,4px);--_x:var(--pele-nothing,#123456)" fill="var(--_bg)"><rect fill="var(--_s)" style="rx:var(--_r)"/><path fill="var(--pele-series-2)"/><g fill="var(--_x)"/></svg>"#;
        let var = |n: &str| -> Option<String> {
            Some(String::from(match n {
                "bg" => "#1c1c1c",
                "surface" => "#282828",
                "radius" => "4px",
                "series-2" => "#e9973f",
                _ => return None,
            }))
        };
        let out = themed(svg, &var);
        assert!(
            out.contains(r##"fill="#1c1c1c"><rect fill="#282828" rx="4"/>"##),
            "{out}"
        );
        assert!(out.contains(r##"<path fill="#e9973f"/>"##), "{out}");
        assert!(
            out.contains(r##"<g fill="#123456"/>"##),
            "a name Obsidian does not set keeps Pele's own: {out}"
        );
        assert!(!out.contains("var("), "{out}");
    }
}
