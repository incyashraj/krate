//! The Markdown that spans lines: tables, callouts, `$$` math, front
//! matter, `%%` comment blocks, and the embeds and footnotes that the
//! renderers draw as one piece.
//!
//! Like [`crate::md`], nothing here builds a document tree. A block is
//! found from the line asked about: tables and callouts by walking to the
//! edge of their run of `|` or `>` lines (runs are short), `$$` math and
//! `%%` comments through [`Marks`], an index of their delimiter lines that
//! an edit updates in place, as [`crate::md::Fences`] does for code.

use alloc::string::String;
use alloc::vec::Vec;

/// How far a table or callout is walked looking for its edge. A run longer
/// than this is drawn line by line, as source.
pub const RUN_LIMIT: usize = 2_000;

// ---- delimiter lines ------------------------------------------------------------------

/// How many times `pat` occurs in `line`, not overlapping.
fn occurrences(line: &str, pat: &str) -> usize {
    line.matches(pat).count()
}

/// A `$$` line that opens or closes a display-math block (one that holds a
/// whole `$$...$$` toggles nothing).
pub fn is_math_mark(line: &str) -> bool {
    occurrences(line, "$$") % 2 == 1
}

/// A `%%` line that opens or closes a comment block.
pub fn is_comment_mark(line: &str) -> bool {
    occurrences(line, "%%") % 2 == 1
}

/// The delimiter lines of one kind of block, kept as the note is edited.
#[derive(Debug, Clone, Default)]
pub struct Marks {
    lines: Vec<usize>,
}

impl Marks {
    pub fn build(count: usize, is_mark: impl Fn(usize) -> bool) -> Marks {
        Marks {
            lines: (0..count).filter(|n| is_mark(*n)).collect(),
        }
    }

    fn before(&self, n: usize) -> usize {
        self.lines.partition_point(|l| *l < n)
    }

    pub fn is_mark(&self, n: usize) -> bool {
        self.lines.binary_search(&n).is_ok()
    }

    /// The block line `n` belongs to, delimiters included: its first and
    /// last line. A delimiter with no partner opens nothing.
    pub fn span(&self, n: usize) -> Option<(usize, usize)> {
        let k = self.before(n);
        if self.is_mark(n) {
            if k.is_multiple_of(2) {
                let close = *self.lines.get(k + 1)?;
                Some((n, close))
            } else {
                Some((*self.lines.get(k - 1)?, n))
            }
        } else if k % 2 == 1 {
            let open = *self.lines.get(k - 1)?;
            let close = *self.lines.get(k)?;
            Some((open, close))
        } else {
            None
        }
    }

    /// Lines `first .. first + old` became `first .. first + new`.
    pub fn edit(&mut self, first: usize, old: usize, new: usize, is_mark: impl Fn(usize) -> bool) {
        let lo = self.before(first);
        let hi = self.before(first + old);
        let delta = new as isize - old as isize;
        let fresh: Vec<usize> = (first..first + new).filter(|n| is_mark(*n)).collect();
        let tail: Vec<usize> = self
            .lines
            .get(hi..)
            .unwrap_or(&[])
            .iter()
            .map(|l| (*l as isize + delta).max(0) as usize)
            .collect();
        self.lines.truncate(lo);
        self.lines.extend(fresh);
        self.lines.extend(tail);
    }

    pub fn count(&self) -> usize {
        self.lines.len()
    }
}

// ---- front matter ---------------------------------------------------------------------

/// The last line of the YAML front matter, when line 0 is `---` and a
/// closing `---` follows within the first `limit` lines.
pub fn front_matter_end<'a>(line: impl Fn(usize) -> &'a str, count: usize) -> Option<usize> {
    if count < 2 || line(0).trim_end() != "---" {
        return None;
    }
    (1..count.min(5_000)).find(|n| {
        let l = line(*n).trim_end();
        l == "---" || l == "..."
    })
}

// ---- tables -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Default,
    Left,
    Center,
    Right,
}

/// The byte ranges of a table row's cells, trimmed: pipes inside code
/// spans, inside `[[link|alias]]` and escaped as `\|` do not split.
pub fn split_row(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut cuts: Vec<usize> = Vec::new();
    let mut i = 0usize;
    let mut in_code = false;
    let mut wiki = 0usize;
    while i < b.len() {
        match b.get(i) {
            Some(b'\\') => {
                i += 2;
                continue;
            }
            Some(b'`') => in_code = !in_code,
            Some(b'[') if !in_code && b.get(i + 1) == Some(&b'[') => {
                wiki += 1;
                i += 2;
                continue;
            }
            Some(b']') if !in_code && wiki > 0 && b.get(i + 1) == Some(&b']') => {
                wiki -= 1;
                i += 2;
                continue;
            }
            Some(b'|') if !in_code && wiki == 0 => cuts.push(i),
            _ => {}
        }
        i += 1;
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    let trimmed_end = line.trim_end().len();
    for (k, cut) in cuts.iter().enumerate() {
        // A leading pipe opens the row; it does not end an empty cell.
        if k == 0 && line.get(..*cut).is_some_and(|h| h.trim().is_empty()) {
            start = cut + 1;
            continue;
        }
        out.push((start, *cut));
        start = cut + 1;
    }
    // A trailing pipe closes the row.
    if start < trimmed_end || cuts.last().is_none_or(|c| *c + 1 < trimmed_end) {
        out.push((start, trimmed_end.max(start)));
    }
    out.into_iter()
        .map(|(a, z)| {
            let cell = line.get(a..z).unwrap_or("");
            let lead = cell.len() - cell.trim_start().len();
            let tail = cell.len() - cell.trim_end().len();
            (a + lead, (z - tail).max(a + lead))
        })
        .collect()
}

/// The delimiter row under a table's header (`| :--- | :---: | ---: |`):
/// each column's alignment.
pub fn delimiter_row(line: &str) -> Option<Vec<Align>> {
    if !line.contains('-') || !line.contains(['|', '-']) {
        return None;
    }
    let cells = split_row(line);
    if cells.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (a, z) in cells {
        let c = line.get(a..z).unwrap_or("").trim();
        let left = c.starts_with(':');
        let right = c.ends_with(':');
        let dashes = c.trim_matches(':');
        if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
            return None;
        }
        out.push(match (left, right) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            _ => Align::Default,
        });
    }
    Some(out)
}

/// Whether a line can be a table row (it holds an unescaped pipe).
pub fn row_like(line: &str) -> bool {
    line.contains('|') && !line.trim().is_empty()
}

// ---- callouts ---------------------------------------------------------------------------

/// `> [!type]+ Title` on the first line of a quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalloutHead {
    /// The type, lower case (`note`, `warning`, or anything a person types).
    pub kind: String,
    /// `Some(true)` for `+` (foldable, open), `Some(false)` for `-`
    /// (folded), `None` when it does not fold.
    pub fold: Option<bool>,
    /// The title's byte range in the line; empty means the type's name.
    pub title: (usize, usize),
}

pub fn callout_head(line: &str) -> Option<CalloutHead> {
    let (body, at) = strip_quote(line)?;
    let rest = body.strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = rest.get(..close)?;
    if kind.is_empty() || kind.contains(' ') {
        return None;
    }
    let after = at + 2 + close + 1;
    let tail = line.get(after..).unwrap_or("");
    let (fold, skip) = match tail.as_bytes().first() {
        Some(b'+') => (Some(true), 1),
        Some(b'-') => (Some(false), 1),
        _ => (None, 0),
    };
    let t0 = after + skip;
    let t = line.get(t0..).unwrap_or("");
    let lead = t.len() - t.trim_start().len();
    let a = t0 + lead;
    let z = line.trim_end().len().max(a);
    Some(CalloutHead {
        kind: kind.to_ascii_lowercase(),
        fold,
        title: (a, z),
    })
}

/// A quote line's text after its `>` (and the one space after it), and the
/// byte it starts at.
pub fn strip_quote(line: &str) -> Option<(&str, usize)> {
    let t = line.trim_start();
    let lead = line.len() - t.len();
    if lead > 3 || !t.starts_with('>') {
        return None;
    }
    let mut at = lead + 1;
    if line.as_bytes().get(at) == Some(&b' ') {
        at += 1;
    }
    Some((line.get(at..).unwrap_or(""), at))
}

/// What a callout type looks like: its icon, its colour (Obsidian's RGB),
/// and the title it gets when none is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalloutIcon {
    Pencil,
    Clipboard,
    Info,
    Check,
    Flame,
    CheckCircle,
    Help,
    Warning,
    Cross,
    Zap,
    Bug,
    List,
    Quote,
}

pub fn callout_style(kind: &str) -> (CalloutIcon, (u8, u8, u8), &'static str) {
    use CalloutIcon::*;
    match kind {
        "abstract" | "summary" | "tldr" => (Clipboard, (0, 191, 188), "Abstract"),
        "info" => (Info, (8, 109, 221), "Info"),
        "todo" => (Check, (8, 109, 221), "Todo"),
        "tip" | "hint" | "important" => (Flame, (0, 191, 188), "Tip"),
        "success" | "check" | "done" => (CheckCircle, (8, 185, 78), "Success"),
        "question" | "help" | "faq" => (Help, (236, 117, 0), "Question"),
        "warning" | "caution" | "attention" => (Warning, (236, 117, 0), "Warning"),
        "failure" | "fail" | "missing" => (Cross, (233, 49, 71), "Failure"),
        "danger" | "error" => (Zap, (233, 49, 71), "Danger"),
        "bug" => (Bug, (233, 49, 71), "Bug"),
        "example" => (List, (120, 82, 238), "Example"),
        "quote" | "cite" => (Quote, (158, 158, 158), "Quote"),
        _ => (Pencil, (8, 109, 221), "Note"),
    }
}

/// The title a callout shows when none is written: its type, capitalised.
pub fn callout_default_title(kind: &str) -> String {
    let (_, _, name) = callout_style(kind);
    let known = !matches!(name, "Note") || kind == "note";
    if known {
        return String::from(name);
    }
    let mut s = String::new();
    for (i, c) in kind.chars().enumerate() {
        if i == 0 {
            s.extend(c.to_uppercase());
        } else {
            s.push(c);
        }
    }
    s
}

// ---- embeds -----------------------------------------------------------------------------

/// `![[target#anchor|size]]` or `![alt](path)` alone on a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embed {
    /// The note or file named (`Note`, `image.png`, `folder/pic.jpg`).
    pub target: String,
    /// `#Heading` or `^block` after the target, without the `#`.
    pub anchor: Option<String>,
    /// `|300` or `|300x200` (pixels) for a picture.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt: String,
}

impl Embed {
    /// Whether the target is a picture by its extension.
    pub fn is_image(&self) -> bool {
        is_image_name(&self.target)
    }
}

pub fn is_image_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp", ".avif",
    ]
    .iter()
    .any(|e| lower.ends_with(e))
}

fn size_of(spec: &str) -> (Option<u32>, Option<u32>) {
    let spec = spec.trim();
    match spec.split_once('x') {
        Some((w, h)) => (w.trim().parse().ok(), h.trim().parse().ok()),
        None => (spec.parse().ok(), None),
    }
}

pub fn embed(line: &str) -> Option<Embed> {
    let t = line.trim();
    if let Some(inner) = t.strip_prefix("![[").and_then(|r| r.strip_suffix("]]")) {
        if inner.is_empty() || inner.contains("]]") {
            return None;
        }
        let (target_part, alias) = match inner.split_once('|') {
            Some((a, b)) => (a, Some(b)),
            None => (inner, None),
        };
        let (target, anchor) = match target_part.find(['#', '^']) {
            Some(i) => {
                let a = target_part.get(i..).unwrap_or("");
                let anchor = if a.starts_with('#') {
                    a.get(1..).unwrap_or("")
                } else {
                    a
                };
                (
                    target_part.get(..i).unwrap_or(""),
                    Some(String::from(anchor)),
                )
            }
            None => (target_part, None),
        };
        let (width, height) = alias.map_or((None, None), size_of);
        let alt = if width.is_none() {
            String::from(alias.unwrap_or(""))
        } else {
            String::new()
        };
        return Some(Embed {
            target: String::from(target.trim()),
            anchor,
            width,
            height,
            alt,
        });
    }
    // ![alt](path) -- a picture by its Markdown name.
    let rest = t.strip_prefix("![")?;
    let mid = rest.find("](")?;
    let path = rest.get(mid + 2..)?.strip_suffix(')')?;
    if path.contains(')') || path.is_empty() {
        return None;
    }
    let alt_full = rest.get(..mid).unwrap_or("");
    let (alt, (width, height)) = match alt_full.rsplit_once('|') {
        Some((a, s)) if size_of(s).0.is_some() => (a, size_of(s)),
        _ => (alt_full, (None, None)),
    };
    let path = path.split_whitespace().next().unwrap_or(path);
    let target = percent_decode(path.trim_matches(['<', '>']));
    Some(Embed {
        target,
        anchor: None,
        width,
        height,
        alt: String::from(alt),
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b.get(i) == Some(&b'%') {
            let hex = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b.get(i).copied().unwrap_or(b' '));
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| String::from(s))
}

// ---- sections of a note, for embeds and links ----------------------------------------------

/// The lines of the section under `heading` (the heading line itself to the
/// line before the next heading at its level or higher), matched the way
/// links match headings: case and inline syntax ignored.
pub fn heading_section(lines: &[&str], heading: &str) -> Option<(usize, usize)> {
    let want = crate::text::lower(heading.trim());
    let mut found: Option<(usize, u8)> = None;
    let mut in_code = false;
    for (n, l) in lines.iter().enumerate() {
        if crate::md::is_fence(l) {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if let crate::md::Block::Heading { level, content } = crate::md::classify(l, false) {
            match found {
                Some((start, lv)) if level <= lv => return Some((start, n - 1)),
                Some(_) => {}
                None => {
                    if crate::text::lower(crate::md::plain(l, content).trim()) == want {
                        found = Some((n, level));
                    }
                }
            }
        }
    }
    found.map(|(s, _)| (s, lines.len().saturating_sub(1)))
}

/// The line holding block id `^id`, and the lines of its block (a
/// paragraph or list item runs back to the blank line before it).
pub fn block_lines(lines: &[&str], id: &str) -> Option<(usize, usize)> {
    let id = id.trim_start_matches('^');
    let mut tag = String::from("^");
    tag.push_str(id);
    let n = lines.iter().position(|l| {
        let t = l.trim_end();
        t.trim_start() == tag
            || (t.ends_with(tag.as_str())
                && t.len() > tag.len()
                && t.as_bytes().get(t.len() - tag.len() - 1) == Some(&b' '))
    })?;
    let line = lines.get(n)?;
    if line.trim() == tag {
        // An id on its own line names the block above it.
        let mut start = n.saturating_sub(1);
        while start > 0 && !lines.get(start - 1).is_none_or(|l| l.trim().is_empty()) {
            start -= 1;
        }
        return Some((start, n.saturating_sub(1)));
    }
    if crate::md::classify(line, false).is_list() {
        return Some((n, n));
    }
    let mut start = n;
    while start > 0 && !lines.get(start - 1).is_none_or(|l| l.trim().is_empty()) {
        start -= 1;
    }
    Some((start, n))
}

/// Every block id in a note, with the text of its line, for `[[note^`
/// completion.
pub fn block_ids(lines: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for l in lines {
        let t = l.trim_end();
        if let Some(i) = t.rfind(" ^") {
            let id = t.get(i + 2..).unwrap_or("");
            if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                out.push((
                    String::from(id),
                    String::from(t.get(..i).unwrap_or("").trim()),
                ));
            }
        }
    }
    out
}

// ---- footnotes --------------------------------------------------------------------------

/// A footnote definition line `[^label]: text`: the label and where the
/// text starts.
pub fn footnote_def(line: &str) -> Option<(&str, usize)> {
    let rest = line.strip_prefix("[^")?;
    let close = rest.find("]:")?;
    let label = rest.get(..close)?;
    if label.is_empty() || label.contains(' ') {
        return None;
    }
    let mut at = 2 + close + 2;
    while line.as_bytes().get(at) == Some(&b' ') {
        at += 1;
    }
    Some((label, at))
}

/// The footnotes of a note in the order reading view numbers them: by first
/// reference. Each is (label or `None` for an inline note, its text, the
/// line of its first reference).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footnotes {
    pub notes: Vec<Footnote>,
    /// (line, byte of the reference, number) for every reference, in order.
    pub refs: Vec<(usize, usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footnote {
    pub label: Option<String>,
    pub text: String,
}

impl Footnotes {
    pub fn collect<'a>(
        count: usize,
        line: impl Fn(usize) -> &'a str,
        in_code: impl Fn(usize) -> bool,
    ) -> Footnotes {
        let mut defs: Vec<(String, String)> = Vec::new();
        let mut out = Footnotes::default();
        for n in 0..count {
            let l = line(n);
            if !l.contains("[^") && !l.contains("^[") || in_code(n) {
                continue;
            }
            if let Some((label, at)) = footnote_def(l) {
                defs.push((String::from(label), String::from(l.get(at..).unwrap_or(""))));
                continue;
            }
            for seg in crate::md::inline(l, 0) {
                if let (Some(crate::md::Target::Footnote(a, z)), false) =
                    (seg.target, seg.style.syntax)
                {
                    let inline = l.get(..a).is_some_and(|h| h.ends_with("^["));
                    let text = l.get(a..z).unwrap_or("");
                    let number = if inline {
                        out.notes.push(Footnote {
                            label: None,
                            text: String::from(text),
                        });
                        out.notes.len()
                    } else {
                        match out
                            .notes
                            .iter()
                            .position(|f| f.label.as_deref() == Some(text))
                        {
                            Some(i) => i + 1,
                            None => {
                                out.notes.push(Footnote {
                                    label: Some(String::from(text)),
                                    text: String::new(),
                                });
                                out.notes.len()
                            }
                        }
                    };
                    out.refs.push((n, a, number));
                }
            }
        }
        for f in out.notes.iter_mut() {
            if let Some(label) = &f.label {
                if let Some((_, t)) = defs.iter().find(|(l, _)| l == label) {
                    f.text = t.clone();
                }
            }
        }
        out
    }

    /// The number a reference at `line`, byte `at` shows.
    pub fn number(&self, line: usize, at: usize) -> Option<usize> {
        let i = self.refs.partition_point(|r| (r.0, r.1) < (line, at));
        self.refs
            .get(i)
            .filter(|r| r.0 == line && r.1 == at)
            .map(|r| r.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_pair_and_follow_edits() {
        let text = ["a", "$$", "x^2", "$$", "b", "$$x$$", "$$", "y"];
        let mut m = Marks::build(text.len(), |n| is_math_mark(text[n]));
        assert_eq!(m.count(), 3, "the one-line block toggles nothing");
        assert_eq!(m.span(2), Some((1, 3)));
        assert_eq!(m.span(1), Some((1, 3)));
        assert_eq!(m.span(3), Some((1, 3)));
        assert_eq!(m.span(4), None);
        assert_eq!(m.span(7), None, "an unclosed $$ opens nothing");
        // Insert a line inside the block: the closing mark shifts.
        let after = ["a", "$$", "x^2", "+1", "$$", "b", "$$x$$", "$$", "y"];
        m.edit(2, 1, 2, |n| is_math_mark(after[n]));
        assert_eq!(m.span(3), Some((1, 4)));
        let rebuilt = Marks::build(after.len(), |n| is_math_mark(after[n]));
        assert_eq!(rebuilt.lines, m.lines);
        assert!(is_comment_mark("%%") && !is_comment_mark("a %%b%% c"));
    }

    #[test]
    fn front_matter_is_found_at_the_top_only() {
        let t = ["---", "title: x", "---", "# body"];
        assert_eq!(front_matter_end(|n| t[n], t.len()), Some(2));
        let u = ["# x", "---", "a", "---"];
        assert_eq!(front_matter_end(|n| u[n], u.len()), None);
        let open = ["---", "title: x"];
        assert_eq!(front_matter_end(|n| open[n], open.len()), None);
    }

    #[test]
    fn table_rows_split_into_cells() {
        let row = "| Name | [[a|b]] | `x|y` | a \\| b |";
        let cells: Vec<&str> = split_row(row).iter().map(|(a, z)| &row[*a..*z]).collect();
        assert_eq!(cells, ["Name", "[[a|b]]", "`x|y`", "a \\| b"]);
        let bare = "a | b";
        let c2: Vec<&str> = split_row(bare).iter().map(|(a, z)| &bare[*a..*z]).collect();
        assert_eq!(c2, ["a", "b"]);
        let empty = "| a |  | c |";
        assert_eq!(split_row(empty).len(), 3);
        assert_eq!(
            delimiter_row("| :--- | :---: | ---: | --- |"),
            Some(alloc::vec![
                Align::Left,
                Align::Center,
                Align::Right,
                Align::Default
            ])
        );
        assert_eq!(delimiter_row("|---|---|").map(|v| v.len()), Some(2));
        assert_eq!(delimiter_row("| a | b |"), None);
        assert_eq!(delimiter_row("---"), Some(alloc::vec![Align::Default]));
    }

    #[test]
    fn callouts_read_their_head() {
        let h = callout_head("> [!warning]- Folded warning").unwrap();
        assert_eq!(h.kind, "warning");
        assert_eq!(h.fold, Some(false));
        let line = "> [!warning]- Folded warning";
        assert_eq!(&line[h.title.0..h.title.1], "Folded warning");
        let n = callout_head("> [!NOTE]").unwrap();
        assert_eq!(
            (n.kind.as_str(), n.fold, n.title.0 == n.title.1),
            ("note", None, true)
        );
        assert_eq!(callout_head("> plain quote"), None);
        assert_eq!(callout_head("[!note] not quoted"), None);
        assert_eq!(callout_style("tldr").2, "Abstract");
        assert_eq!(callout_style("danger").1, (233, 49, 71));
        assert_eq!(callout_default_title("danger"), "Danger");
        assert_eq!(callout_default_title("custom"), "Custom");
        assert_eq!(callout_default_title("note"), "Note");
        assert_eq!(strip_quote("> - item"), Some(("- item", 2)));
    }

    #[test]
    fn embeds_name_notes_pictures_and_sizes() {
        let e = embed("![[sunset.png|300]]").unwrap();
        assert_eq!(
            (e.target.as_str(), e.width, e.is_image()),
            ("sunset.png", Some(300), true)
        );
        let s = embed("  ![[chart.png|150x80]] ").unwrap();
        assert_eq!((s.width, s.height), (Some(150), Some(80)));
        let n = embed("![[Note#Part two]]").unwrap();
        assert_eq!(
            (n.target.as_str(), n.anchor.as_deref()),
            ("Note", Some("Part two"))
        );
        assert!(!n.is_image());
        let b = embed("![[Note^abc123]]").unwrap();
        assert_eq!(b.anchor.as_deref(), Some("^abc123"));
        let m = embed("![A chart](pics/my%20chart.png)").unwrap();
        assert_eq!(
            (m.target.as_str(), m.alt.as_str()),
            ("pics/my chart.png", "A chart")
        );
        let ms = embed("![logo|64](logo.png)").unwrap();
        assert_eq!((ms.alt.as_str(), ms.width), ("logo", Some(64)));
        assert_eq!(embed("text ![[x]] more"), None);
        assert_eq!(embed("[[not an embed]]"), None);
    }

    #[test]
    fn sections_and_blocks_are_found() {
        let note = [
            "# Top",
            "intro",
            "## Part *two*",
            "a",
            "### deeper",
            "b",
            "## Next",
            "c ^tail",
            "",
            "para one",
            "para two ^pp",
            "- item ^li",
            "",
            "quoted",
            "^own",
        ];
        assert_eq!(heading_section(&note, "part two"), Some((2, 5)));
        assert_eq!(heading_section(&note, "Next"), Some((6, 14)));
        assert_eq!(heading_section(&note, "missing"), None);
        assert_eq!(block_lines(&note, "pp"), Some((9, 10)));
        assert_eq!(block_lines(&note, "^li"), Some((11, 11)));
        assert_eq!(block_lines(&note, "own"), Some((13, 13)));
        assert_eq!(block_lines(&note, "nope"), None);
        let ids = block_ids(&note);
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], (String::from("tail"), String::from("c")));
    }

    #[test]
    fn footnotes_number_in_reference_order() {
        let note = [
            "First[^b] then inline^[Quick note.] then[^a] again[^b].",
            "`[^code]` is code",
            "[^a]: Alpha text.",
            "[^b]: Beta text.",
        ];
        let f = Footnotes::collect(note.len(), |n| note[n], |_| false);
        assert_eq!(f.notes.len(), 3);
        assert_eq!(f.notes[0].text, "Beta text.");
        assert_eq!(f.notes[1].label, None);
        assert_eq!(f.notes[1].text, "Quick note.");
        assert_eq!(f.notes[2].text, "Alpha text.");
        let nums: Vec<usize> = f.refs.iter().map(|r| r.2).collect();
        assert_eq!(nums, [1, 2, 3, 1]);
        let (line, at, _) = f.refs[2];
        assert_eq!(f.number(line, at), Some(3));
        assert_eq!(footnote_def("[^a]: Alpha"), Some(("a", 6)));
    }
}
