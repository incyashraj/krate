//! The port against Pele itself: every flowchart in the corpus rendered
//! here and compared with the SVG Pele's own build produced for it under
//! node, with the fallback measurer and `{idPrefix: 'x'}`
//! (evidence/benchmarks/obsidian-vs-krate/pele-oracle, made by
//! pele-oracle/capture.js). Byte for byte first; where that fails, the
//! same text with every number within 0.01, and the first difference
//! printed.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use std::println;

use super::core::{render, RenderOptions};
use super::text::measurer::METRICS;
use crate::json;

macro_rules! case {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!(
                "../../../../evidence/benchmarks/obsidian-vs-krate/pele-corpus/",
                $name,
                ".mmd"
            )),
            include_str!(concat!(
                "../../../../evidence/benchmarks/obsidian-vs-krate/pele-oracle/",
                $name,
                ".json"
            )),
        )
    };
}

const CORPUS: [(&str, &str, &str); 8] = [
    case!("flow-back-edge"),
    case!("flow-bt"),
    case!("flow-edges"),
    case!("flow-multiline"),
    case!("flow-rl-nested"),
    case!("flow-shapes"),
    case!("flow-subgraphs"),
    case!("flow-wide"),
];

/// Extra cases, made by running Pele's build (`r2.js`) on each source.
const EXTRA: &str = include_str!("oracle_extra.json");

fn number_at(s: &[u8], i: usize) -> Option<(f64, usize)> {
    let mut j = i;
    if j < s.len() && s[j] == b'-' {
        j += 1;
    }
    let d = j;
    while j < s.len() && (s[j].is_ascii_digit() || s[j] == b'.') {
        j += 1;
    }
    if j == d || !s[d..j].iter().any(|c| c.is_ascii_digit()) {
        return None;
    }
    core::str::from_utf8(&s[i..j])
        .ok()?
        .parse()
        .ok()
        .map(|v| (v, j))
}

/// Where `got` and `want` first differ, beyond numbers within 0.01.
fn numeric_diff(got: &str, want: &str) -> Option<String> {
    let (a, b) = (got.as_bytes(), want.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let starts = |s: &[u8], k: usize| {
            s[k].is_ascii_digit() || (s[k] == b'-' && k + 1 < s.len() && s[k + 1].is_ascii_digit())
        };
        if starts(a, i) && starts(b, j) {
            if let (Some((x, ni)), Some((y, nj))) = (number_at(a, i), number_at(b, j)) {
                if (x - y).abs() > 0.01 {
                    return Some(context(got, want, i, j));
                }
                i = ni;
                j = nj;
                continue;
            }
        }
        if a[i] != b[j] {
            return Some(context(got, want, i, j));
        }
        i += 1;
        j += 1;
    }
    if i < a.len() || j < b.len() {
        return Some(context(got, want, i.min(a.len()), j.min(b.len())));
    }
    None
}

fn context(got: &str, want: &str, i: usize, j: usize) -> String {
    let cut = |s: &str, k: usize| {
        let mut lo = k.saturating_sub(120);
        while !s.is_char_boundary(lo) {
            lo -= 1;
        }
        let mut hi = (k + 120).min(s.len());
        while !s.is_char_boundary(hi) {
            hi += 1;
        }
        String::from(&s[lo..hi])
    };
    format!(
        "\n  got:  ...{}...\n  want: ...{}...",
        cut(got, i),
        cut(want, j)
    )
}

/// The options a case gives `r2.js`, over `{idPrefix: 'x'}`.
fn options_of(opts: Option<&json::Value>) -> RenderOptions<'static> {
    let mut o = RenderOptions {
        measurer: Some(&METRICS),
        id_prefix: Some("x".into()),
        ..RenderOptions::default()
    };
    if let Some(v) = opts {
        let s = |k: &str| v.get_str(k).map(String::from);
        o.id_prefix = s("idPrefix").or(o.id_prefix);
        o.font_family = s("fontFamily");
        o.font_family_mono = s("fontFamilyMono");
        o.font_size = v.get_num("fontSize");
        o.padding = v.get_num("padding");
        o.max_width = v.get_num("maxWidth");
        o.responsive = v.get_bool("responsive");
        o.config = v.get("config").cloned();
        o.now = v.get_num("now");
        if let Some(json::Value::Arr(parts)) = v.get("monospace") {
            o.monospace = Some(
                parts
                    .iter()
                    .filter_map(|p| p.str().map(String::from))
                    .collect(),
            );
        }
        if let Some(json::Value::Obj(map)) = v.get("iconsMap") {
            // The host's resolver: `name => map[name]`.
            let map: Vec<(String, String)> = map
                .iter()
                .filter_map(|(k, v)| v.str().map(|s| (k.clone(), String::from(s))))
                .collect();
            let resolver: &'static dyn Fn(&str) -> Option<String> =
                std::boxed::Box::leak(std::boxed::Box::new(move |name: &str| {
                    map.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
                }));
            o.icons = Some(resolver);
        }
    }
    o
}

/// Ok(true) identical, Ok(false) numbers within 0.01, Err the difference.
/// A case Pele refused wants the same error message.
fn check(
    name: &str,
    src: &str,
    want: Result<&str, &str>,
    opts: Option<&json::Value>,
) -> Result<bool, String> {
    let options = options_of(opts);
    let got = match (render(src, &options), want) {
        (Ok(r), Ok(_)) => r.svg,
        (Err(e), Err(w)) if e.message == w => return Ok(true),
        (Err(e), Err(w)) => {
            return Err(format!(
                "{}: error differs\n  got:  {}\n  want: {}",
                name, e.message, w
            ))
        }
        (Err(e), Ok(_)) => return Err(format!("{}: render failed: {}", name, e.message)),
        (Ok(_), Err(w)) => return Err(format!("{}: rendered where Pele refused: {}", name, w)),
    };
    let want = want.unwrap_or_default();
    if got == want {
        return Ok(true);
    }
    match numeric_diff(&got, want) {
        None => Ok(false),
        Some(d) => Err(format!("{}: differs{}", name, d)),
    }
}

/// Runs every case in a list made by Pele's build, outside the repository:
/// `PELE_CASES=/path/to/cases.json cargo test pele::oracle -- --ignored`.
#[test]
#[ignore]
fn cases_file_matches_pele() {
    let Ok(path) = std::env::var("PELE_CASES") else {
        return;
    };
    let text = std::fs::read_to_string(&path).expect("PELE_CASES file");
    let Some(json::Value::Arr(cases)) = json::parse(&text) else {
        panic!("{} is not a JSON array", path)
    };
    run(cases);
}

#[test]
fn flowcharts_match_pele() {
    let mut cases: Vec<json::Value> = CORPUS
        .iter()
        .map(|(n, s, o)| {
            let mut oracle = json::parse(o).expect("oracle json");
            oracle.set("name", json::Value::s(n));
            oracle.set("src", json::Value::s(s));
            oracle
        })
        .collect();
    match json::parse(EXTRA) {
        Some(json::Value::Arr(extra)) => cases.extend(extra),
        _ => panic!("oracle_extra.json is not a JSON array"),
    }
    run(cases);
}

pub(super) fn run(cases: Vec<json::Value>) {
    let mut failures: Vec<String> = Vec::new();
    let mut close = 0;
    for case in &cases {
        let name = case.get_str("name").unwrap_or("?");
        let src = case.get_str("src").unwrap_or("");
        let want = match case.get_str("error") {
            Some(e) => Err(e),
            None => Ok(case.get_str("svg").expect("svg field")),
        };
        match check(name, src, want, case.get("opts")) {
            Ok(true) => println!("{}: identical", name),
            Ok(false) => {
                close += 1;
                println!("{}: numbers within 0.01", name)
            }
            Err(e) => {
                println!("{}", e);
                failures.push(e);
            }
        }
    }
    println!(
        "{} cases: {} identical, {} within 0.01, {} differ",
        cases.len(),
        cases.len() - close - failures.len(),
        close,
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
