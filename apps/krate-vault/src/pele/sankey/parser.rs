//! Ports diagrams/sankey/parser.ts: preparing the rows and parsing them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::SankeyDb;
use super::lexer::*;
use crate::pele::errors::{input_offset, syntax_error, Edit, PeleError};
use crate::pele::js::{self, parse_float, trim};
use crate::pele::text::entities::decode_entities;

/// Ports `isBlank`.
fn is_blank(c: u32) -> bool {
    if c == 10 || c == 13 {
        return false;
    }
    if c < 128 {
        return c == 32 || c == 9 || c == 11 || c == 12;
    }
    c == 0xa0
        || c == 0x1680
        || (0x2000..=0x200a).contains(&c)
        || c == 0x2028
        || c == 0x2029
        || c == 0x202f
        || c == 0x205f
        || c == 0x3000
        || c == 0xfeff
}

/// Ports `trimRows`.
fn trim_rows(text: &[u16], edits: &mut Vec<Edit>) -> Vec<u16> {
    let mut end = text.len();
    while end > 0 && is_blank(text[end - 1] as u32) {
        end -= 1;
    }
    let mut out: Vec<u16> = Vec::new();
    let mut kept = 0;
    let mut quoted = false;
    let mut i = 0;
    while i < end {
        let c = text[i] as u32;
        if quoted {
            if c == 34 {
                if cu(text, i + 1) == 34 {
                    i += 1;
                } else {
                    quoted = false;
                }
            }
            i += 1;
        } else if c == 34 {
            quoted = true;
            i += 1;
        } else if is_blank(c) {
            let mut j = i + 1;
            while is_blank(cu(text, j)) {
                j += 1;
            }
            let p = if i == 0 { u32::MAX } else { cu(text, i - 1) };
            let q = cu(text, j);
            // Preserve blank first fields, which Mermaid accepts as empty names.
            if i == 0 || q == 10 || q == 13 || ((p == 10 || p == 13) && q != 44) {
                out.extend_from_slice(&text[kept..i]);
                edits.push((i, j, 0));
                kept = j;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    if end < text.len() {
        edits.push((end, text.len(), 0));
    }
    out.extend_from_slice(&text[kept..end.max(kept)]);
    out
}

fn is_space16(c: u16) -> bool {
    char::from_u32(c as u32).is_some_and(js::is_space)
}

/// Ports `prepareTextForParsing`: unlike Mermaid's sankeyUtils, also trims
/// row edges outside quoted fields.
pub fn prepare_text_for_parsing(text: &[u16], steps: Option<&mut Vec<Vec<Edit>>>) -> Vec<u16> {
    let mut edits: Vec<Edit> = Vec::new();
    let rows = trim_rows(text, &mut edits);
    // rows.replace(/[\n\r]+/g, '\n'), with its edits.
    let mut collapsed: Vec<u16> = Vec::with_capacity(rows.len());
    let mut breaks: Vec<Edit> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if rows[i] == 10 || rows[i] == 13 {
            let mut j = i + 1;
            while j < rows.len() && (rows[j] == 10 || rows[j] == 13) {
                j += 1;
            }
            collapsed.push(10);
            breaks.push((i, j, 1));
            i = j;
        } else {
            collapsed.push(rows[i]);
            i += 1;
        }
    }
    let mut s = 0;
    while s < collapsed.len() && is_space16(collapsed[s]) {
        s += 1;
    }
    let mut e = collapsed.len();
    while e > s && is_space16(collapsed[e - 1]) {
        e -= 1;
    }
    let trimmed = collapsed[s..e].to_vec();
    if let Some(steps) = steps {
        let lead = s;
        steps.push(edits);
        steps.push(breaks);
        steps.push(alloc::vec![
            (0, lead, 0),
            (lead + trimmed.len(), collapsed.len(), 0)
        ]);
    }
    trimmed
}

/// `RE_INVALID_XML.test(text)`.
fn invalid_xml(text: &str) -> bool {
    text.chars().any(|c| {
        let c = c as u32;
        c <= 0x08
            || c == 0x0b
            || c == 0x0c
            || (0x0e..=0x1f).contains(&c)
            || c == 0xfffe
            || c == 0xffff
    })
}

/// Ports `parseSankey`: errors are reported in `source`, the text before
/// prepareTextForParsing.
pub fn parse_sankey(src: &[u16], db: &mut SankeyDb, source: &str) -> Result<(), PeleError> {
    let toks = tokenize(src);
    let types = &toks.types;
    let texts = &toks.texts;
    let starts = &toks.starts;
    let mut i = 0usize;

    let error = |at: usize, detail: &str| -> PeleError {
        let mut steps: Vec<Vec<Edit>> = Vec::new();
        let units: Vec<u16> = source.encode_utf16().collect();
        if units != src {
            prepare_text_for_parsing(&units, Some(&mut steps));
        }
        // Map EOF before the trimmed trailing blanks.
        let offset = if at > 0 && at >= src.len() {
            input_offset(&steps, src.len() - 1) + 1
        } else {
            input_offset(&steps, at)
        };
        syntax_error("sankey", source, js::byte_at(source, offset), detail, false)
    };

    let fail = |i: usize, expected: &str| -> PeleError {
        let t = types[i];
        let got = if t == STUCK {
            // `TOKEN_NAMES[types[-1]]` is undefined.
            if i == 0 {
                "undefined"
            } else {
                TOKEN_NAMES[types[i - 1] as usize]
            }
        } else {
            TOKEN_NAMES[t as usize]
        };
        error(starts[i], &format!("Expecting {}, got '{}'", expected, got))
    };

    let expect = |i: &mut usize, ty: i32, also: &str| -> Result<(), PeleError> {
        if types[*i] != ty {
            return Err(fail(*i, &format!("'{}'{}", TOKEN_NAMES[ty as usize], also)));
        }
        *i += 1;
        Ok(())
    };

    let field = |i: &mut usize| -> Result<String, PeleError> {
        if types[*i] == NON_ESCAPED_TEXT {
            *i += 1;
            return Ok(texts[*i - 1].clone());
        }
        if types[*i] != DQUOTE {
            return Err(fail(*i, "'DQUOTE', 'NON_ESCAPED_TEXT'"));
        }
        *i += 1;
        if types[*i] != ESCAPED_TEXT {
            return Err(fail(*i, "'ESCAPED_TEXT'"));
        }
        let text = texts[*i].clone();
        *i += 1;
        expect(i, DQUOTE, "")?;
        Ok(text)
    };

    let name = |i: &mut usize| -> Result<String, PeleError> {
        let start = starts[*i];
        let text = trim(&field(i)?).replace("\"\"", "\"");
        // Entity codes can encode characters excluded by the lexer.
        if invalid_xml(&decode_entities(&text)) {
            return Err(error(start, "Invalid XML character in node name"));
        }
        Ok(text)
    };

    expect(&mut i, SANKEY, "")?;
    expect(&mut i, NEWLINE, "")?;
    loop {
        let from = name(&mut i)?;
        expect(&mut i, COMMA, ", 'NEWLINE', 'EOF'")?;
        let to = name(&mut i)?;
        expect(&mut i, COMMA, ", 'NEWLINE', 'EOF'")?;
        let start = starts[i];
        // Keep Mermaid's numeric prefixes, but reject non-finite values.
        let value = parse_float(&field(&mut i)?);
        if !value.is_finite() {
            return Err(error(start, "Expecting a number"));
        }
        let source = db.find_or_create_node(&from);
        let target = db.find_or_create_node(&to);
        db.add_link(source, target, value);
        if types[i] != NEWLINE {
            break;
        }
        i += 1;
    }
    if types[i] == EOF {
        i += 1;
    }
    if types[i] != END {
        return Err(fail(i, "'NEWLINE', 'EOF'"));
    }
    Ok(())
}
