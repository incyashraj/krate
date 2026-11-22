//! Spell check as Obsidian does it by default: the words of the prose are
//! checked by the computer's own spell checker and the unknown ones get a
//! red wavy underline; code, links, web addresses, tags, maths, HTML tags,
//! block ids, footnote labels and front matter keys are never marked.
//!
//! This module decides what of a line is prose. The checker sees the line
//! with everything else blanked to spaces of the same byte length, so the
//! offsets it answers are offsets into the line itself and one call checks
//! a whole line.

use crate::md::{self, Block, Target};
use alloc::string::String;
use alloc::vec::Vec;

/// A choice in the spelling menu, done to the open note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpellAct {
    /// Replace the word at (line, byte, length) with a suggestion.
    Replace {
        line: usize,
        start: usize,
        len: usize,
        with: String,
    },
    /// Add to dictionary: the checker learns it.
    Learn(String),
    /// Ignore: not marked again while Vault is open.
    Ignore(String),
}

/// Do a spelling menu choice: a replacement is an edit; learning and
/// ignoring are left for the guest, which asks the checker.
pub fn act(doc: &mut crate::editor::Doc, a: SpellAct, now: u64) {
    match a {
        SpellAct::Replace {
            line,
            start,
            len,
            with,
        } => {
            let p = crate::buffer::Pos::new(line, start);
            let e = doc.replace(p, crate::buffer::Pos::new(line, start + len), &with, now);
            doc.place(e, false);
        }
        other => doc.spell_pending.push(other),
    }
}

/// The line with everything that is not prose blanked, byte for byte, or
/// `None` when nothing in it is checked.
pub fn checkable(line: &str, block: Block, front_matter: bool, in_math: bool) -> Option<String> {
    if in_math
        || matches!(
            block,
            Block::Code | Block::Fence | Block::Rule | Block::Blank
        )
    {
        return None;
    }
    let b = line.as_bytes();
    let mut keep = alloc::vec![false; b.len()];
    if front_matter {
        // Values are prose; keys, list dashes and the --- lines are not.
        let t = line.trim();
        if t == "---" || t == "..." {
            return None;
        }
        let from = match line.find(':') {
            Some(i) if !line.trim_start().starts_with('-') => i + 1,
            _ => line.find("- ").map_or(0, |i| i + 2),
        };
        for k in keep.iter_mut().skip(from) {
            *k = true;
        }
    } else {
        for seg in md::inline(line, block.content()) {
            let st = seg.style;
            let prose = !st.syntax
                && !st.code
                && !st.math
                && !st.tag
                && !st.blockid
                && !st.kbd
                && (!st.footnote || line.get(..seg.start).is_some_and(|h| h.ends_with("^[")))
                && !matches!(
                    seg.target,
                    Some(Target::Url(..)) | Some(Target::Wiki(..)) | Some(Target::Tag(..))
                );
            // A link's shown text (an alias, a Markdown link's words) is
            // prose; the note name or address it points at is not.
            let alias = st.link
                && !st.syntax
                && match seg.target {
                    Some(Target::Wiki(_, z)) => seg.start >= z,
                    Some(Target::Url(a, _)) => seg.end <= a,
                    _ => false,
                };
            if prose || alias {
                for k in keep.iter_mut().take(seg.end.min(b.len())).skip(seg.start) {
                    *k = true;
                }
            }
        }
    }
    if !keep.iter().any(|k| *k) {
        return None;
    }
    let mut out = String::with_capacity(line.len());
    for (i, ch) in line.char_indices() {
        if keep.get(i).copied().unwrap_or(false) {
            out.push(ch);
        } else {
            for _ in 0..ch.len_utf8() {
                out.push(' ');
            }
        }
    }
    Some(out)
}

/// What the checker answered, kept to whole words of the prose and without
/// the words the person chose to ignore.
pub fn keep_marks(text: &str, marks: &[(usize, usize)], ignored: &[String]) -> Vec<(usize, usize)> {
    marks
        .iter()
        .copied()
        .filter(|(a, n)| {
            let w = text.get(*a..a + n).unwrap_or("");
            !w.trim().is_empty()
                && !w.bytes().all(|c| c.is_ascii_digit())
                && !ignored.iter().any(|i| i.eq_ignore_ascii_case(w))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(line: &str) -> String {
        checkable(line, md::classify(line, false), false, false)
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    }

    #[test]
    fn only_prose_is_checked() {
        assert_eq!(kept("Teh quick brown fox"), "Teh quick brown fox");
        assert_eq!(kept("call `fnuc()` now"), "call now");
        assert_eq!(
            kept("see [[Nmae of note]] and [[Other|an alais]]"),
            "see and an alais"
        );
        assert_eq!(kept("go to https://exmaple.com/pth today"), "go to today");
        assert_eq!(kept("[the wrods](https://x.y/zz)"), "the wrods");
        assert_eq!(
            kept("tagged #prjoect and $x^2 + y$ maths"),
            "tagged and maths"
        );
        assert_eq!(kept("## A hedaing"), "A hedaing");
        assert_eq!(kept("- [ ] a tsak"), "a tsak");
        assert_eq!(kept("a <kbd>Cmmd</kbd> b ^blk1"), "a b");
        assert_eq!(
            kept("note[^lbl] and ^[an inlnie note]"),
            "note and an inlnie note"
        );
        assert_eq!(kept("**bodl** and *itlaic*"), "bodl and itlaic");
        // Offsets stay offsets into the line, multi-byte characters too.
        let l = "café `x` naïve";
        let c = checkable(l, md::classify(l, false), false, false).unwrap();
        assert_eq!(c.len(), l.len());
        assert_eq!(&c[l.find("naïve").unwrap()..], "naïve");
        assert!(checkable("let x = 1;", Block::Code, false, false).is_none());
        assert!(checkable("x^2", Block::Para, false, true).is_none());
        assert!(checkable("```rust", Block::Fence, false, false).is_none());
    }

    #[test]
    fn the_menu_replaces_or_queues() {
        let mut d = crate::editor::Doc::new("n.md", "n", "Teh end");
        act(
            &mut d,
            SpellAct::Replace {
                line: 0,
                start: 0,
                len: 3,
                with: String::from("The"),
            },
            0,
        );
        assert_eq!(d.text(), "The end");
        assert_eq!(d.cur.col, 3);
        act(&mut d, SpellAct::Learn(String::from("Krate")), 0);
        assert_eq!(d.spell_pending, [SpellAct::Learn(String::from("Krate"))]);
    }

    #[test]
    fn front_matter_checks_values_not_keys() {
        let c = checkable("title: Teh title", Block::Para, true, false).unwrap();
        assert_eq!(c.trim(), "Teh title");
        let l = checkable(
            "  - a tgaa",
            Block::Bullet {
                indent: 1,
                marker: 2,
                content: 4,
            },
            true,
            false,
        )
        .unwrap();
        assert_eq!(l.trim(), "a tgaa");
        assert!(checkable("---", Block::Rule, true, false).is_none());
        let marks = keep_marks(
            "Teh Teh 42 mispell",
            &[(0, 3), (4, 3), (8, 2), (11, 7)],
            &[String::from("mispell")],
        );
        assert_eq!(marks, [(0, 3), (4, 3)]);
    }

    #[test]
    fn a_property_value_is_replaced() {
        let mut d =
            crate::editor::Doc::new("n.md", "n", "---\ntitle: Spelling chek\n---\n# Spelling\n");
        d.place(crate::buffer::Pos::new(1, 19), false);
        act(
            &mut d,
            SpellAct::Replace {
                line: 1,
                start: 16,
                len: 4,
                with: String::from("check"),
            },
            0,
        );
        assert_eq!(d.line(1), "title: Spelling check");
        assert_eq!(
            crate::propedit::props(&d)[0].value,
            crate::props::Value::Text(String::from("Spelling check"))
        );
        // A word in the body first, then the value (the order a person did it in).
        let text = "---\ntitle: Spelling chek\n---\n# Spelling\n\nTeh quick brown fox jumpd over the lazy dog.\n";
        let mut d = crate::editor::Doc::new("n.md", "n", text);
        d.place(crate::buffer::Pos::new(5, 3), false);
        act(
            &mut d,
            SpellAct::Replace {
                line: 5,
                start: 0,
                len: 3,
                with: String::from("The"),
            },
            10,
        );
        d.place(crate::buffer::Pos::new(1, 20), false);
        act(
            &mut d,
            SpellAct::Replace {
                line: 1,
                start: 16,
                len: 4,
                with: String::from("check"),
            },
            20,
        );
        assert_eq!(d.line(1), "title: Spelling check");
        assert_eq!(d.line(5), "The quick brown fox jumpd over the lazy dog.");
    }
}
