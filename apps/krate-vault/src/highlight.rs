//! Colouring code inside fenced blocks, one line at a time, the way the
//! editor's code blocks show it: keywords, strings, comments, numbers,
//! function names, types and punctuation.
//!
//! A line is coloured from its own text and the fence's language, so a
//! 200,000-line note colours only the lines on screen. Strings and block
//! comments that span lines are coloured on the line they open.

use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
    Operator,
    Punct,
    Property,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lang {
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    line_comment: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// `#` starts a comment only at a word boundary (shell, python, yaml).
    hash_comment: bool,
}

const RUST: Lang = Lang {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ],
    types: &[
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
        "f32", "f64", "bool", "char", "str", "String", "Vec", "Option", "Result", "Box", "Some",
        "None", "Ok", "Err",
    ],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    hash_comment: false,
};

const JS: Lang = Lang {
    keywords: &[
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "null",
        "of",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "yield",
    ],
    types: &[
        "string", "number", "boolean", "any", "unknown", "never", "Array", "Promise", "Map", "Set",
        "Object",
    ],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    hash_comment: false,
};

const PYTHON: Lang = Lang {
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
        "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True",
        "try", "while", "with", "yield", "self",
    ],
    types: &[
        "int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes",
    ],
    line_comment: &[],
    block_comment: None,
    hash_comment: true,
};

const SHELL: Lang = Lang {
    keywords: &[
        "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while", "case", "esac",
        "function", "return", "export", "local", "echo", "cd", "sudo",
    ],
    types: &[],
    line_comment: &[],
    block_comment: None,
    hash_comment: true,
};

const C: Lang = Lang {
    keywords: &[
        "auto",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "extern",
        "false",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "include",
        "namespace",
        "new",
        "nil",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "sizeof",
        "static",
        "struct",
        "switch",
        "template",
        "this",
        "throw",
        "true",
        "try",
        "typedef",
        "union",
        "using",
        "var",
        "virtual",
        "void",
        "while",
        "defer",
        "range",
        "chan",
        "select",
        "final",
        "extends",
        "implements",
        "interface",
        "null",
    ],
    types: &[
        "int", "char", "float", "double", "long", "short", "unsigned", "signed", "bool", "string",
        "byte", "error", "String", "Integer", "size_t",
    ],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    hash_comment: false,
};

const SQL: Lang = Lang {
    keywords: &[
        "select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set",
        "delete", "create", "table", "drop", "alter", "join", "left", "right", "inner", "outer",
        "on", "group", "by", "order", "having", "limit", "as", "null", "is", "in", "distinct",
        "union", "primary", "key", "index",
    ],
    types: &[
        "int", "integer", "text", "varchar", "real", "boolean", "date",
    ],
    line_comment: &["--"],
    block_comment: Some(("/*", "*/")),
    hash_comment: false,
};

const DATA: Lang = Lang {
    keywords: &["true", "false", "null", "yes", "no"],
    types: &[],
    line_comment: &[],
    block_comment: None,
    hash_comment: true,
};

const CSS: Lang = Lang {
    keywords: &["important", "media", "import", "root"],
    types: &[],
    line_comment: &[],
    block_comment: Some(("/*", "*/")),
    hash_comment: false,
};

fn lang(name: &str) -> Option<Lang> {
    Some(match crate::text::lower(name).as_str() {
        "rust" | "rs" => RUST,
        "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" | "mjs" => JS,
        "py" | "python" | "python3" => PYTHON,
        "sh" | "bash" | "zsh" | "shell" | "console" | "fish" => SHELL,
        "c" | "cpp" | "c++" | "h" | "hpp" | "java" | "go" | "golang" | "cs" | "csharp"
        | "kotlin" | "swift" | "dart" | "php" => C,
        "sql" | "sqlite" | "postgres" | "mysql" => SQL,
        "json" | "yaml" | "yml" | "toml" | "ini" => DATA,
        "css" | "scss" | "less" => CSS,
        _ => return None,
    })
}

/// The name a code block's language is shown by in its corner.
pub fn display_name(lang: &str) -> &str {
    match crate::text::lower(lang).as_str() {
        "rust" | "rs" => "Rust",
        "js" | "javascript" | "mjs" => "JavaScript",
        "ts" | "typescript" => "TypeScript",
        "py" | "python" | "python3" => "Python",
        "sh" | "bash" => "Bash",
        "zsh" => "Zsh",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "css" => "CSS",
        "html" => "HTML",
        "sql" => "SQL",
        "go" | "golang" => "Go",
        "java" => "Java",
        "c" => "C",
        "cpp" | "c++" => "C++",
        "swift" => "Swift",
        "md" | "markdown" => "Markdown",
        _ => lang,
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The coloured runs of one line of code in `lang`, as (start, end, kind),
/// covering every byte in order. An unknown language is all plain.
fn push(out: &mut Vec<(usize, usize, Tok)>, a: usize, z: usize, t: Tok) {
    if z <= a {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.2 == t && last.1 == a {
            last.1 = z;
            return;
        }
    }
    out.push((a, z, t));
}

pub fn line(text: &str, lang_name: &str) -> Vec<(usize, usize, Tok)> {
    let mut out: Vec<(usize, usize, Tok)> = Vec::new();
    let Some(l) = lang(lang_name) else {
        push(&mut out, 0, text.len(), Tok::Plain);
        return out;
    };
    let ci = matches!(lang_name, "sql" | "SQL" | "sqlite" | "postgres" | "mysql");
    let data = l == DATA;
    let s = text.as_bytes();
    let mut i = 0usize;
    while i < s.len() {
        let c = s.get(i).copied().unwrap_or(0);
        let rest = text.get(i..).unwrap_or("");
        // Comments.
        if l.line_comment.iter().any(|p| rest.starts_with(p))
            || (l.hash_comment
                && c == b'#'
                && (i == 0 || !is_word(s.get(i - 1).copied().unwrap_or(b' '))))
        {
            push(&mut out, i, s.len(), Tok::Comment);
            break;
        }
        if let Some((open, close)) = l.block_comment {
            if rest.starts_with(open) {
                let end = text
                    .get(i + open.len()..)
                    .and_then(|r| r.find(close))
                    .map_or(s.len(), |e| i + open.len() + e + close.len());
                push(&mut out, i, end, Tok::Comment);
                i = end;
                continue;
            }
        }
        // Strings.
        if matches!(c, b'"' | b'\'' | b'`') {
            let triple = rest.starts_with("\"\"\"") || rest.starts_with("'''");
            let q = if triple { 3 } else { 1 };
            let mut j = i + q;
            let mut end = s.len();
            while j < s.len() {
                if s.get(j) == Some(&b'\\') {
                    j += 2;
                    continue;
                }
                if s.get(j..j + q).is_some_and(|w| w.iter().all(|b| *b == c)) {
                    end = j + q;
                    break;
                }
                j += 1;
            }
            // A Rust lifetime `'a` is not a string.
            if c == b'\'' && l == RUST && end == s.len() {
                push(&mut out, i, i + 1, Tok::Operator);
                i += 1;
                continue;
            }
            // `f"..."` and `r"..."` prefixes are part of the string.
            let prefixed = i > 0
                && matches!(s.get(i - 1), Some(b'f' | b'r' | b'b'))
                && (i < 2 || !is_word(s.get(i - 2).copied().unwrap_or(b' ')));
            let start = if prefixed {
                if let Some(last) = out.last_mut() {
                    if last.1 == i {
                        last.1 = i - 1;
                        if last.1 <= last.0 {
                            out.pop();
                        }
                    }
                }
                i - 1
            } else {
                i
            };
            push(&mut out, start, end, Tok::String);
            i = end;
            continue;
        }
        // Numbers.
        if c.is_ascii_digit() && (i == 0 || !is_word(s.get(i - 1).copied().unwrap_or(b' '))) {
            let mut j = i;
            while j < s.len()
                && (is_word(s.get(j).copied().unwrap_or(0)) || s.get(j) == Some(&b'.'))
            {
                j += 1;
            }
            push(&mut out, i, j, Tok::Number);
            i = j;
            continue;
        }
        // Words: keywords, types, calls, keys.
        if is_word(c) || c >= 0x80 {
            let mut j = i;
            while j < s.len()
                && (is_word(s.get(j).copied().unwrap_or(0)) || s.get(j).is_some_and(|b| *b >= 0x80))
            {
                j += 1;
            }
            let w = text.get(i..j).unwrap_or("");
            let after = text.get(j..).unwrap_or("").trim_start();
            let is_kw = if ci {
                let lw = crate::text::lower(w);
                l.keywords.contains(&lw.as_str())
            } else {
                l.keywords.contains(&w)
            };
            let tok = if data && after.starts_with(':') && i == text.len() - text.trim_start().len()
            {
                Tok::Property
            } else if is_kw {
                Tok::Keyword
            } else if l.types.contains(&w) {
                Tok::Type
            } else if after.starts_with('(') || after.starts_with("!(") {
                Tok::Function
            } else {
                Tok::Plain
            };
            push(&mut out, i, j, tok);
            i = j;
            continue;
        }
        let tok = match c {
            b'=' | b'+' | b'-' | b'*' | b'/' | b'%' | b'<' | b'>' | b'!' | b'&' | b'|' | b'^' => {
                Tok::Operator
            }
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b';' | b'.' | b':' => Tok::Punct,
            _ => Tok::Plain,
        };
        push(&mut out, i, i + 1, tok);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds<'a>(text: &'a str, lang: &str) -> Vec<(&'a str, Tok)> {
        line(text, lang)
            .into_iter()
            .map(|(a, z, t)| (&text[a..z], t))
            .filter(|(s, t)| *t != Tok::Plain || !s.trim().is_empty())
            .collect()
    }

    #[test]
    fn code_lines_are_coloured_by_language() {
        let r = kinds("    let x: u32 = 42; // note", "rust");
        assert!(r.contains(&("let", Tok::Keyword)));
        assert!(r.contains(&("u32", Tok::Type)));
        assert!(r.contains(&("42", Tok::Number)));
        assert!(r.contains(&("// note", Tok::Comment)));
        let call = kinds("println!(\"hello {}\", x);", "rust");
        assert!(call.contains(&("println", Tok::Function)));
        assert!(call.contains(&("\"hello {}\"", Tok::String)));
        let py = kinds("    return f\"hello {name}\"  # comment", "python");
        assert!(py.contains(&("return", Tok::Keyword)));
        assert!(py.contains(&("f\"hello {name}\"", Tok::String)));
        assert!(py.contains(&("# comment", Tok::Comment)));
        let doc = kinds("    \"\"\"Say hello.\"\"\"", "py");
        assert!(doc.contains(&("\"\"\"Say hello.\"\"\"", Tok::String)));
        let js = kinds("const n = [1, 2].map((x) => x * 2);", "js");
        assert!(js.contains(&("const", Tok::Keyword)) && js.contains(&("map", Tok::Function)));
        let sql = kinds("SELECT name FROM t WHERE id = 1", "sql");
        assert!(sql.contains(&("SELECT", Tok::Keyword)) && sql.contains(&("WHERE", Tok::Keyword)));
        let yaml = kinds("title: x # c", "yaml");
        assert!(yaml.contains(&("title", Tok::Property)) && yaml.contains(&("# c", Tok::Comment)));
        let life = kinds("fn f<'a>(x: &'a str)", "rust");
        assert!(life.contains(&("fn", Tok::Keyword)));
        assert_eq!(display_name("py"), "Python");
        // Unknown languages and every byte covered.
        for (t, l) in [
            ("anything at all", "brainfuck"),
            ("é = \"ü\" // 漢", "rust"),
            ("'unterminated", "js"),
        ] {
            let runs = line(t, l);
            let mut at = 0;
            for (a, z, _) in &runs {
                assert_eq!(*a, at);
                at = *z;
            }
            assert_eq!(at, t.len());
        }
    }
}
