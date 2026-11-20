//! A small XML pull reader, just enough for the parts of a workbook.
//!
//! Spreadsheet XML is machine-written and plain: elements, attributes,
//! text, the five named entities and numeric character references. This
//! reader yields start tags, end tags and text in document order, skips
//! the declaration, comments and a DOCTYPE, and never panics on bad input
//! -- a damaged file ends the walk early instead. Names keep their prefix;
//! callers compare the local part with [`local`], because writers differ in
//! which prefix they bind (`x:c` and `c` are the same element).

use alloc::string::String;

pub(crate) enum Event<'a> {
    /// `<name attrs>` or `<name attrs/>` (`empty` is true for the latter).
    Start {
        name: &'a str,
        attrs: &'a str,
        empty: bool,
    },
    End {
        name: &'a str,
    },
    /// Raw character data, entities still escaped.
    Text(&'a str),
    /// The inside of a CDATA section, taken literally.
    CData(&'a str),
}

pub(crate) struct Reader<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        Reader { src, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or("")
    }

    /// Skip past `end`, or to the end of the input when it never comes.
    fn skip_past(&mut self, end: &str) {
        match self.rest().find(end) {
            Some(at) => self.pos += at + end.len(),
            None => self.pos = self.src.len(),
        }
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        loop {
            let rest = self.rest();
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let len = rest.find('<').unwrap_or(rest.len());
                self.pos += len;
                return Some(Event::Text(rest.get(..len).unwrap_or("")));
            }
            if rest.starts_with("<?") {
                self.skip_past("?>");
                continue;
            }
            if rest.starts_with("<!--") {
                self.skip_past("-->");
                continue;
            }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let len = body.find("]]>").unwrap_or(body.len());
                self.pos += "<![CDATA[".len() + len + "]]>".len().min(body.len() - len);
                return Some(Event::CData(body.get(..len).unwrap_or("")));
            }
            if rest.starts_with("<!") {
                self.skip_past(">");
                continue;
            }
            if let Some(body) = rest.strip_prefix("</") {
                let len = body.find('>').unwrap_or(body.len());
                self.pos += 2 + len + 1;
                if self.pos > self.src.len() {
                    self.pos = self.src.len();
                }
                return Some(Event::End {
                    name: body.get(..len).unwrap_or("").trim(),
                });
            }
            // A start tag. A '>' may sit inside a quoted attribute value,
            // so the end is the first '>' outside quotes.
            let bytes = rest.as_bytes();
            let mut quote = 0u8;
            let mut end = None;
            let mut i = 1;
            while let Some(&b) = bytes.get(i) {
                if quote != 0 {
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'>' {
                    end = Some(i);
                    break;
                }
                i += 1;
            }
            let Some(end) = end else {
                self.pos = self.src.len();
                return None;
            };
            self.pos += end + 1;
            let inner = rest.get(1..end).unwrap_or("");
            let (inner, empty) = match inner.strip_suffix('/') {
                Some(s) => (s, true),
                None => (inner, false),
            };
            let name_len = inner
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(inner.len());
            return Some(Event::Start {
                name: inner.get(..name_len).unwrap_or(""),
                attrs: inner.get(name_len..).unwrap_or(""),
                empty,
            });
        }
    }
}

/// The part of a name after its prefix: `x:row` -> `row`.
pub(crate) fn local(name: &str) -> &str {
    match name.rfind(':') {
        Some(at) => name.get(at + 1..).unwrap_or(""),
        None => name,
    }
}

/// The raw (still escaped) value of the attribute whose local name is
/// `key`. Namespace declarations are never matched.
pub(crate) fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = attrs;
    loop {
        rest = rest.trim_start();
        let eq = rest.find('=')?;
        let name = rest.get(..eq)?.trim();
        let after = rest.get(eq + 1..)?.trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let body = after.get(1..)?;
        let close = body.find(quote)?;
        let value = body.get(..close)?;
        if !name.starts_with("xmlns") && local(name) == key {
            return Some(value);
        }
        rest = body.get(close + 1..)?;
    }
}

/// Like [`attr`] with the entities resolved.
pub(crate) fn attr_text(attrs: &str, key: &str) -> Option<String> {
    attr(attrs, key).map(unescape)
}

/// Resolve `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;` and `&#...;`.
/// Anything else that starts with `&` is kept as written.
pub(crate) fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    push_unescaped(&mut out, raw);
    out
}

pub(crate) fn push_unescaped(out: &mut String, raw: &str) {
    // XML reads a literal CR LF, or a lone CR, as one LF (XML 1.0, 2.11);
    // only an escaped `&#13;` is a real carriage return. Excel writes
    // literal CR LF inside cells saved on Windows.
    if raw.contains('\r') {
        let normalised = raw.replace("\r\n", "\n").replace('\r', "\n");
        return push_unescaped(out, &normalised);
    }
    if !raw.contains('&') {
        out.push_str(raw);
        return;
    }
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or(""));
        let tail = rest.get(at..).unwrap_or("");
        let semi = tail.find(';').filter(|&s| s <= 12);
        let resolved = semi.and_then(|semi| {
            let entity = tail.get(1..semi)?;
            let ch = match entity {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ => {
                    let num = entity.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse::<u32>().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((ch, semi + 1))
        });
        match resolved {
            Some((ch, used)) => {
                out.push(ch);
                rest = tail.get(used..).unwrap_or("");
            }
            None => {
                out.push('&');
                rest = tail.get(1..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
}

/// Append `text` escaped for an element body or a double-quoted attribute.
/// Characters XML 1.0 cannot carry at all (most C0 controls) are dropped.
pub(crate) fn push_escaped(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' => out.push(ch),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn walks_tags_text_and_skips_the_rest() {
        let doc = "\u{feff}<?xml version=\"1.0\"?><!-- c --><a x='1>2' y=\"q\"><b/>t&amp;u<![CDATA[<raw>]]></a>";
        let mut seen = Vec::new();
        for ev in Reader::new(doc) {
            seen.push(match ev {
                Event::Start { name, attrs, empty } => {
                    alloc::format!("S {name} {} {empty}", attr(attrs, "x").unwrap_or("-"))
                }
                Event::End { name } => alloc::format!("E {name}"),
                Event::Text(t) => alloc::format!("T {}", unescape(t)),
                Event::CData(t) => alloc::format!("C {t}"),
            });
        }
        assert_eq!(
            seen,
            ["S a 1>2 false", "S b - true", "T t&u", "C <raw>", "E a"]
        );
    }

    #[test]
    fn attributes_match_by_local_name_and_skip_namespaces() {
        let attrs = r#" xmlns:r="urn:r" name="One &amp; Two" r:id="rId3""#;
        assert_eq!(attr(attrs, "id"), Some("rId3"));
        assert_eq!(attr_text(attrs, "name").as_deref(), Some("One & Two"));
        assert_eq!(attr(attrs, "r"), None);
    }

    #[test]
    fn entities_resolve_and_unknown_ones_survive() {
        assert_eq!(unescape("&#x41;&#66;&lt;&bogus;&"), "AB<&bogus;&");
        assert_eq!(unescape("a\r\nb\rc&#13;"), "a\nb\nc\r");
    }

    #[test]
    fn truncated_input_ends_without_panicking() {
        for doc in ["<a", "<a x='", "</a", "<![CDATA[x", "<!-- x", "<?x"] {
            let _ = Reader::new(doc).count();
        }
    }
}
