//! Ports util/url.ts: the checks a click target, an image and an internal
//! link go through before they are written into the drawing.
//!
//! `new URL(...).toString()`, which Pele uses to normalise an http(s)
//! link, is ported for what diagrams hold: the scheme and host lowered, a
//! default port dropped, an empty path written as `/`, dot segments
//! resolved, and spaces, quotes, angle brackets and non-ASCII characters
//! percent-encoded where the URL Standard encodes them. Hosts that need
//! IDNA (punycode) are kept as written.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use crate::pele::core::{LinkInfo, RenderOptions};
use crate::pele::js;
use crate::pele::svg::builder::{esc, invalid_xml};
use crate::pele::text::entities::decode_entities;

const BLANK: &str = "about:blank";

/// `RE_CTRL`: /[\u0000-\u001F\u007F-\u009F -‍﻿\uD800-\uDFFF￾￿]/u.
fn is_ctrl(c: char) -> bool {
    let c = c as u32;
    c <= 0x1f
        || (0x7f..=0x9f).contains(&c)
        || (0x2000..=0x200d).contains(&c)
        || c == 0xfeff
        || c == 0xfffe
        || c == 0xffff
}

/// Ports `decode`: `decodeURIComponent`, or the text as it was where that throws.
fn decode(uri: &str) -> String {
    let b = uri.as_bytes();
    let mut bytes: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |k: usize| b.get(k).and_then(|c| (*c as char).to_digit(16));
            match (hex(i + 1), hex(i + 2)) {
                (Some(h), Some(l)) => {
                    bytes.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => return uri.to_string(),
            }
        }
        bytes.push(b[i]);
        i += 1;
    }
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => uri.to_string(),
    }
}

/// `/&#(\w+)(^\w|;)?/g`, each match replaced by `String.fromCharCode(dec)`.
fn replace_html_entities(s: &str) -> (String, bool) {
    let b = s.as_bytes();
    let mut out = String::new();
    let mut last = 0;
    let mut found = false;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'&' && b.get(i + 1) == Some(&b'#') {
            let st = i + 2;
            let mut e = st;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            if e > st {
                found = true;
                out.push_str(&s[last..i]);
                let n = js::number(&s[st..e]);
                // ToUint16.
                let code = if n.is_finite() {
                    (n.trunc() as i64).rem_euclid(65536) as u32
                } else {
                    0
                };
                match char::from_u32(code) {
                    Some(c) => out.push(c),
                    None => out.push('\u{fffd}'),
                }
                let mut end = e;
                if b.get(end) == Some(&b';') {
                    end += 1;
                }
                last = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    (out, found)
}

/// `/&(newline|tab);/gi`.
fn find_ctrl_entity(s: &str) -> Option<(usize, usize)> {
    let lower = s.to_ascii_lowercase();
    let a = lower.find("&newline;").map(|k| (k, k + 9));
    let t = lower.find("&tab;").map(|k| (k, k + 5));
    match (a, t) {
        (Some(x), Some(y)) => Some(if x.0 < y.0 { x } else { y }),
        (x, y) => x.or(y),
    }
}

fn remove_ctrl_entities(s: &str) -> String {
    let mut out = s.to_string();
    while let Some((a, b)) = find_ctrl_entity(&out) {
        // Matches do not overlap and the removed text cannot form a new one
        // across the gap that the regex would not also have seen; strip in order.
        out = format!("{}{}", &out[..a], &out[b..]);
    }
    out
}

/// `/(\\|%5[cC])((%(6[eE]|72|74))|[nrt])/g`: one match at `i`, its length.
fn ws_escape_at(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    let lead = if b.get(i) == Some(&b'\\') {
        1
    } else if b.get(i) == Some(&b'%')
        && b.get(i + 1) == Some(&b'5')
        && matches!(b.get(i + 2), Some(b'c') | Some(b'C'))
    {
        3
    } else {
        return None;
    };
    let j = i + lead;
    if b.get(j) == Some(&b'%') {
        let two = s.get(j + 1..j + 3)?;
        if matches!(two, "6e" | "6E" | "72" | "74") {
            return Some(lead + 3);
        }
    }
    if matches!(b.get(j), Some(b'n') | Some(b'r') | Some(b't')) {
        return Some(lead + 1);
    }
    None
}

fn remove_ws_escapes(s: &str) -> (String, bool) {
    let mut out = String::new();
    let mut found = false;
    let mut i = 0;
    while i < s.len() {
        if let Some(n) = ws_escape_at(s, i) {
            found = true;
            i += n;
            continue;
        }
        let c = s[i..].chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    (out, found)
}

/// Ports `unwrap`: removes the control characters, entity codes and
/// escapes that could hide a scheme, and with `percent`, percent escapes
/// too. None for a URL wrapped too many times to be a real one.
fn unwrap(url: &str, percent: bool) -> Option<String> {
    let mut out = if percent {
        decode(js::trim(url))
    } else {
        js::trim(url).to_string()
    };
    let mut rounds = 0;
    loop {
        rounds += 1;
        if rounds > 32 {
            return None;
        }
        let a: String = out.chars().filter(|c| !is_ctrl(*c)).collect();
        let (b, _) = replace_html_entities(&a);
        let c = remove_ctrl_entities(&b);
        let d: String = c.chars().filter(|c| !is_ctrl(*c)).collect();
        let (e, _) = remove_ws_escapes(&d);
        out = js::trim(&e).to_string();
        if percent {
            out = decode(&out);
        }
        let more = out.chars().any(is_ctrl)
            || replace_html_entities(&out).1
            || find_ctrl_entity(&out).is_some()
            || remove_ws_escapes(&out).1;
        if !more {
            break;
        }
    }
    Some(out)
}

/// `decoded.trimStart().match(RE_SCHEME)?.[0]`: RE_SCHEME = /^.+(:|&colon;)/gim.
fn scheme_of(text: &str) -> Option<&str> {
    let mut start = 0;
    for (i, c) in text
        .char_indices()
        .chain(core::iter::once((text.len(), '\n')))
    {
        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            let line = &text[start..i];
            let mut best: Option<usize> = None;
            for (k, ch) in line.char_indices() {
                if k == 0 {
                    continue;
                }
                if ch == ':' {
                    best = Some(k + 1);
                } else if line[k..].starts_with("&colon;") {
                    best = Some(k + 7);
                }
            }
            if let Some(e) = best {
                return Some(&text[start..start + e]);
            }
            start = i + c.len_utf8();
        }
    }
    None
}

/// `RE_INVALID_PROTOCOL.test(protocol)`: /^([^\w]*)(javascript|data|vbscript)/im.
fn invalid_protocol(protocol: &str) -> bool {
    protocol
        .split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .any(|line| {
            let rest = line.trim_start_matches(|c: char| !js::is_word(c));
            let lower = rest.to_ascii_lowercase();
            lower.starts_with("javascript")
                || lower.starts_with("data")
                || lower.starts_with("vbscript")
        })
}

/// Percent-encodes what the URL Standard's set for a URL part encodes.
fn encode_part(s: &str, extra: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let cu = c as u32;
        if cu <= 0x20 || cu >= 0x7f || extra.contains(c) {
            let mut buf = [0u8; 4];
            for byte in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{:02X}", byte));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `new URL(normalized)` then `toString()` with the protocol and hostname
/// lowered, for an http or https URL. None where the URL parser throws.
fn normalise_http(url: &str) -> Option<String> {
    let colon = url.find(':')?;
    let scheme = url[..colon].to_ascii_lowercase();
    let rest = url[colon + 1..].trim_start_matches('/');
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let tail = &rest[end..];
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(k) => (Some(&authority[..k]), &authority[k + 1..]),
        None => (None, authority),
    };
    let (host, port) = if hostport.starts_with('[') {
        match hostport.find(']') {
            Some(k) => (&hostport[..k + 1], hostport[k + 1..].strip_prefix(':')),
            None => return None,
        }
    } else {
        match hostport.rfind(':') {
            Some(k) => (&hostport[..k], Some(&hostport[k + 1..])),
            None => (hostport, None),
        }
    };
    if host.is_empty() || host.contains([' ', '<', '>', '^', '|', '%', '\\']) {
        return None;
    }
    let host = host.to_lowercase();
    let port = match port {
        None | Some("") => None,
        Some(p) => {
            if !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: u32 = p.parse().ok()?;
            if n > 65535 {
                return None;
            }
            let default = if scheme == "http" { 80 } else { 443 };
            if n == default {
                None
            } else {
                Some(format!("{}", n))
            }
        }
    };
    let (path, after) = match tail.find(['?', '#']) {
        Some(k) => (&tail[..k], &tail[k..]),
        None => (tail, ""),
    };
    let (query, fragment) = match after.find('#') {
        Some(k) => (&after[..k], Some(&after[k + 1..])),
        None => (after, None),
    };
    // Dot segments.
    let mut segs: Vec<&str> = Vec::new();
    let parts: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path[1..].split('/').collect()
    };
    let n = parts.len();
    for (k, seg) in parts.iter().enumerate() {
        let lower = seg.to_ascii_lowercase();
        let dot = lower == "." || lower == "%2e";
        let dots = lower == ".." || lower == ".%2e" || lower == "%2e." || lower == "%2e%2e";
        if dots {
            segs.pop();
            if k == n - 1 {
                segs.push("");
            }
        } else if dot {
            if k == n - 1 {
                segs.push("");
            }
        } else {
            segs.push(seg);
        }
    }
    let mut out = format!("{}://", scheme);
    if let Some(u) = userinfo {
        out.push_str(&encode_part(u, "\"#<>?`{}/:;=@[\\]^|"));
        out.push('@');
    }
    out.push_str(&host);
    if let Some(p) = port {
        out.push(':');
        out.push_str(&p);
    }
    out.push('/');
    out.push_str(&encode_part(&segs.join("/"), "\"#<>?`{}"));
    if !query.is_empty() {
        out.push_str(&encode_part(query, "\"#<>'"));
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(&encode_part(f, "\"<>`"));
    }
    Some(out)
}

/// Ports `sanitizeUrl`: refuses what @braintree/sanitize-url refuses, the
/// check Mermaid applies to click targets, keeping percent escapes.
pub fn sanitize_url(url: &str) -> String {
    if url.is_empty() {
        return BLANK.to_string();
    }
    let (Some(decoded), Some(kept)) = (unwrap(url, true), unwrap(url, false)) else {
        return BLANK.to_string();
    };
    if decoded.is_empty() || kept.is_empty() {
        return BLANK.to_string();
    }
    if decoded.starts_with(['.', '/']) {
        return kept;
    }
    let Some(scheme) = scheme_of(js::trim_start(&decoded)) else {
        return kept;
    };
    let protocol = js::trim(&scheme.to_lowercase()).to_string();
    if invalid_protocol(&protocol) {
        return BLANK.to_string();
    }
    let normalized = js::trim_start(&kept).replace('\\', "/");
    if protocol == "mailto:" || protocol.contains("://") {
        return normalized;
    }
    if protocol == "http:" || protocol == "https:" {
        return normalise_http(&normalized).unwrap_or_else(|| BLANK.to_string());
    }
    normalized
}

/// Ports `safeUrl`: a URL as written in a diagram, entity codes read first.
pub fn safe_url(url: &str) -> String {
    sanitize_url(&decode_entities(url))
}

const LINK_SCHEMES: [&str; 4] = ["http", "https", "mailto", "tel"];
const IMAGE_SCHEMES: [&str; 2] = ["http", "https"];

/// `RE_SCHEME_NAME.exec(url)?.[1]`: /^([a-z][a-z0-9+.-]*):/i.
fn scheme_name(url: &str) -> Option<&str> {
    let b = url.as_bytes();
    if !b.first()?.is_ascii_alphabetic() {
        return None;
    }
    let mut k = 1;
    while k < b.len() && (b[k].is_ascii_alphanumeric() || matches!(b[k], b'+' | b'.' | b'-')) {
        k += 1;
    }
    if b.get(k) == Some(&b':') {
        Some(&url[..k])
    } else {
        None
    }
}

/// Ports `allowed`.
fn allowed(url: &str, schemes: &[&str]) -> bool {
    let b = url.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'/' | b'\\') && matches!(b[1], b'/' | b'\\') {
        return false;
    }
    match scheme_name(url) {
        None => true,
        Some(s) => schemes.contains(&s.to_ascii_lowercase().as_str()),
    }
}

/// Ports `allow`.
fn allow(url: &str, schemes: &[&str]) -> Option<String> {
    let decoded = unwrap(url, true)?;
    if allowed(url, schemes) && allowed(&decoded, schemes) {
        Some(url.to_string())
    } else {
        None
    }
}

fn schemes_of<'a>(given: &'a Option<Vec<String>>, fallback: &'a [&'a str]) -> Vec<&'a str> {
    match given {
        Some(list) => list.iter().map(|s| s.as_str()).collect(),
        None => fallback.to_vec(),
    }
}

/// Ports `linkUrl`.
pub fn link_url(url: &str, options: &RenderOptions) -> Option<String> {
    if options.links == Some(false) {
        return None;
    }
    allow(url, &schemes_of(&options.link_schemes, &LINK_SCHEMES))
}

/// Ports `imageUrl`.
pub fn image_url(url: &str, options: &RenderOptions) -> Option<String> {
    if options.images == Some(false) {
        return None;
    }
    allow(url, &schemes_of(&options.image_schemes, &IMAGE_SCHEMES))
}

/// Ports `internalLink`: the href keeps the original name, so sanitising
/// alone is not enough.
pub fn internal_link(name: &str, options: &RenderOptions) -> bool {
    let url = sanitize_url(name);
    if url == BLANK || link_url(&url, options).is_none() {
        return false;
    }
    // Match character removal by esc() and the browser.
    let seen: String = name
        .chars()
        .filter(|c| !invalid_xml(*c) && !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let seen = seen.trim_start_matches(|c: char| (c as u32) <= 0x20);
    link_url(seen, options).is_some()
}

/// Ports `linked`.
pub fn linked(
    body: &str,
    url: &str,
    id: &str,
    links: &mut Vec<LinkInfo>,
    options: &RenderOptions,
    attrs: &str,
) -> String {
    let Some(href) = link_url(url, options) else {
        return body.to_string();
    };
    let out = format!(
        "<a href=\"{}\"{}{}>{}</a>",
        esc(&href),
        attrs,
        rel_attr(options),
        body
    );
    links.push(LinkInfo {
        id: id.to_string(),
        href,
        internal: false,
    });
    out
}

/// Ports `relAttr`.
pub fn rel_attr(options: &RenderOptions) -> String {
    let rel = options.link_rel.as_deref().unwrap_or("noopener");
    if rel.is_empty() {
        String::new()
    } else {
        format!(" rel=\"{}\"", esc(rel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize_url("javascript:alert(1)"), BLANK);
        assert_eq!(sanitize_url("jav&#97;script:alert(1)"), BLANK);
        assert_eq!(sanitize_url("https://Example.com"), "https://example.com/");
        assert_eq!(
            sanitize_url("https://example.com/a b?q=1#x"),
            "https://example.com/a%20b?q=1#x"
        );
        assert_eq!(sanitize_url("./note"), "./note");
        assert_eq!(sanitize_url("mailto:a@b.c"), "mailto:a@b.c");
        assert_eq!(sanitize_url("Note name"), "Note name");
    }
}
