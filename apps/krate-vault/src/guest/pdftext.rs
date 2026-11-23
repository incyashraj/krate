//! A PDF's text, as the notes app's PDF viewer lets it be used: drag
//! across words to select them (double-click takes one word), Cmd+C
//! copies them, and Cmd+F finds text in the whole document with every
//! match marked, the current one brought into view, Enter and Shift+Enter
//! (or the arrows) going to the next and previous. Krate gives each page's
//! words and their boxes (`krate::gfx::pdf::page_text`).

use super::look::{self, fill, fit, icon, r, round, text, width, Icon};
use super::media::FileCtl;
use super::{Ui, Zone};
use crate::app::{HostRequest, View};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::pdf;
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

/// A word on a page, its box in points from the page's top-left.
#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Where a word is: its page and its place on the page.
pub type At = (u32, usize);

/// Text selected in a PDF: its file and the two ends.
#[derive(Clone, Debug)]
pub struct Sel {
    pub path: String,
    pub a: At,
    pub b: At,
    /// Something is selected: a drag went past the first word, or a
    /// double press took a word. A plain press selects nothing.
    pub shown: bool,
}

/// The find bar of a PDF tab.
#[derive(Clone, Debug, Default)]
pub struct Find {
    pub pane: usize,
    pub path: String,
    pub query: String,
    /// Each match: its page, first word and how many words.
    pub matches: Vec<(u32, usize, usize)>,
    pub current: usize,
    /// Bring the current match into view on the next frame.
    pub reveal: bool,
}

fn ordered(a: At, b: At) -> (At, At) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

impl Ui {
    /// A page's words, read once.
    pub(super) fn page_words(&mut self, path: &str, page: u32) -> Vec<Word> {
        let Some(d) = self.files.doc_index(path) else {
            return Vec::new();
        };
        if let Some(Some(w)) = self.files.words_of(d, page) {
            return w.clone();
        }
        let words: Vec<Word> = match self.files.doc_handle(d) {
            Some(h) => pdf::page_text(h, page)
                .map(|spans| {
                    spans
                        .into_iter()
                        .map(|s| Word {
                            text: s.text,
                            x: s.x,
                            y: s.y,
                            w: s.width,
                            h: s.height,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        };
        self.files.set_words(d, page, words.clone());
        words
    }

    /// The word under a point on a page drawn at `rc`, `px` pixels a point:
    /// the one it is in, or else the nearest on its line, or the nearest.
    fn word_at(
        &mut self,
        path: &str,
        page: u32,
        rc: Rect,
        px: f32,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        let words = self.page_words(path, page);
        if words.is_empty() {
            return None;
        }
        let (qx, qy) = ((x - rc.x) / px, (y - rc.y) / px);
        let mut best: Option<(f32, usize)> = None;
        for (i, w) in words.iter().enumerate() {
            let dx = if qx < w.x {
                w.x - qx
            } else if qx > w.x + w.w {
                qx - w.x - w.w
            } else {
                0.0
            };
            let dy = if qy < w.y {
                w.y - qy
            } else if qy > w.y + w.h {
                qy - w.y - w.h
            } else {
                0.0
            };
            // A line counts more than a column: rows are read across.
            let d = dx + dy * 4.0;
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// The page and its box under a point, as last drawn in `pane`.
    fn page_under(&self, pane: usize, _x: f32, y: f32) -> Option<(u32, Rect, f32)> {
        self.files
            .drawn
            .iter()
            .filter(|(p, _, _, _)| *p == pane)
            .min_by(|a, b| {
                let d = |rc: &Rect| {
                    if y < rc.y {
                        rc.y - y
                    } else if y > rc.bottom() {
                        y - rc.bottom()
                    } else {
                        0.0
                    }
                };
                d(&a.2)
                    .partial_cmp(&d(&b.2))
                    .unwrap_or(core::cmp::Ordering::Equal)
            })
            .map(|(_, page, rc, px)| (*page, *rc, *px))
    }

    /// A press on the pages: start a selection (or take a word on a
    /// double press).
    pub(super) fn pdf_press(&mut self, pane: usize, path: &str, x: f32, y: f32) {
        let now = krate::time::monotonic_nanos() / 1_000_000;
        let (lt, lx, ly) = self.files.last_press;
        let double = now.saturating_sub(lt) < 400 && (lx - x).abs() < 4.0 && (ly - y).abs() < 4.0;
        self.files.last_press = (now, x, y);
        let Some((page, rc, px)) = self.page_under(pane, x, y) else {
            self.files.sel = None;
            return;
        };
        let Some(i) = self.word_at(path, page, rc, px, x, y) else {
            self.files.sel = None;
            return;
        };
        if double {
            self.files.sel = Some(Sel {
                path: String::from(path),
                a: (page, i),
                b: (page, i),
                shown: true,
            });
            return;
        }
        self.files.sel = Some(Sel {
            path: String::from(path),
            a: (page, i),
            b: (page, i),
            shown: false,
        });
        self.files.selecting = false;
        self.drag = super::Drag::PdfSel(pane);
    }

    /// The pointer moved with the button held: the selection's far end.
    pub(super) fn pdf_drag(&mut self, pane: usize, x: f32, y: f32) {
        let Some(path) = self.files.sel.as_ref().map(|s| s.path.clone()) else {
            return;
        };
        let Some((page, rc, px)) = self.page_under(pane, x, y) else {
            return;
        };
        if let Some(i) = self.word_at(&path, page, rc, px, x, y) {
            if let Some(s) = self.files.sel.as_mut() {
                s.b = (page, i);
                s.shown = s.shown || s.a != s.b;
                self.files.selecting = true;
            }
        }
    }

    /// The selected words as text: a space between words, a line break
    /// where a line ends.
    pub(super) fn pdf_selected(&mut self) -> Option<String> {
        let s = self.files.sel.clone().filter(|s| s.shown)?;
        let (a, b) = ordered(s.a, s.b);
        let mut out = String::new();
        let mut last: Option<(u32, f32)> = None;
        for page in a.0..=b.0 {
            let words = self.page_words(&s.path, page);
            let from = if page == a.0 { a.1 } else { 0 };
            let to = if page == b.0 {
                b.1
            } else {
                words.len().saturating_sub(1)
            };
            for w in words.iter().take(to + 1).skip(from) {
                if let Some((lp, ly)) = last {
                    out.push(if lp != page || (w.y - ly).abs() > w.h * 0.5 {
                        '\n'
                    } else {
                        ' '
                    });
                }
                out.push_str(&w.text);
                last = Some((page, w.y));
            }
        }
        Some(out)
    }

    /// Cmd+C on a PDF: copy the selection.
    pub(super) fn pdf_copy(&mut self) -> bool {
        match self.pdf_selected() {
            Some(t) if !t.is_empty() => {
                self.app.host.push(HostRequest::Copy(t));
                true
            }
            _ => false,
        }
    }

    /// Open the find bar on the PDF in `pane`.
    pub(super) fn pdf_find_open(&mut self, pane: usize, path: &str) {
        let query = self
            .files
            .find
            .as_ref()
            .filter(|f| f.path == path)
            .map(|f| f.query.clone())
            .or_else(|| self.pdf_selected().filter(|s| !s.contains('\n')))
            .unwrap_or_default();
        self.files.find = Some(Find {
            pane,
            path: String::from(path),
            query,
            ..Find::default()
        });
        self.pdf_find_run();
    }

    /// Work the matches out again from the query.
    pub(super) fn pdf_find_run(&mut self) {
        let Some(f) = self.files.find.clone() else {
            return;
        };
        let q: Vec<String> = crate::text::lower(&f.query)
            .split_whitespace()
            .map(String::from)
            .collect();
        let mut matches = Vec::new();
        if !q.is_empty() {
            let pages = self
                .files
                .doc_index(&f.path)
                .map_or(0, |d| self.files.page_count(d));
            for page in 0..pages {
                let words = self.page_words(&f.path, page as u32);
                let lw: Vec<String> = words.iter().map(|w| crate::text::lower(&w.text)).collect();
                let mut i = 0;
                while i < lw.len() {
                    let ok = if q.len() == 1 {
                        lw[i].contains(q[0].as_str())
                    } else {
                        i + q.len() <= lw.len()
                            && q.iter().enumerate().all(|(j, part)| {
                                let w = &lw[i + j];
                                if j == 0 {
                                    w.ends_with(part.as_str())
                                } else if j + 1 == q.len() {
                                    w.starts_with(part.as_str())
                                } else {
                                    w == part
                                }
                            })
                    };
                    if ok {
                        matches.push((page as u32, i, q.len()));
                        i += q.len();
                    } else {
                        i += 1;
                    }
                }
            }
        }
        if let Some(f) = self.files.find.as_mut() {
            f.current = 0;
            f.matches = matches;
            f.reveal = !f.matches.is_empty();
        }
    }

    /// Go to the next (or previous) match.
    pub(super) fn pdf_find_step(&mut self, back: bool) {
        if let Some(f) = self.files.find.as_mut() {
            let n = f.matches.len();
            if n == 0 {
                return;
            }
            f.current = if back {
                (f.current + n - 1) % n
            } else {
                (f.current + 1) % n
            };
            f.reveal = true;
        }
    }

    /// Keys while the find bar is open. True when it took the key.
    pub(super) fn pdf_find_key(&mut self, key: &str, shift: bool) -> bool {
        if self.files.find.is_none() {
            return false;
        }
        match key {
            "Escape" => self.files.find = None,
            "Enter" => self.pdf_find_step(shift),
            "Backspace" => {
                if let Some(f) = self.files.find.as_mut() {
                    f.query.pop();
                }
                self.pdf_find_run();
            }
            _ => return false,
        }
        true
    }

    pub(super) fn pdf_find_text(&mut self, s: &str) -> bool {
        let Some(f) = self.files.find.as_mut() else {
            return false;
        };
        f.query.push_str(s);
        self.pdf_find_run();
        true
    }

    /// The find bar, at the top of a PDF's pages: the query, how many
    /// matches, previous and next, close. Returns its height.
    pub(super) fn draw_pdf_findbar(&mut self, pane: usize, path: &str, at: Rect) -> f32 {
        let Some(f) = self
            .files
            .find
            .clone()
            .filter(|f| f.pane == pane && f.path == path)
        else {
            return 0.0;
        };
        let t = self.t();
        let k = self.sz.scale;
        let h = 40.0 * k;
        let bar = r(at.x, at.y, at.w, h);
        fill(self.c, bar, t.bg);
        fill(self.c, r(bar.x, bar.bottom() - 1.0, bar.w, 1.0), t.border);
        let cy = bar.y + h * 0.5;
        let fw = (260.0 * k).min(bar.w - 200.0 * k).max(120.0 * k);
        let field = r(bar.x + 12.0 * k, cy - 14.0 * k, fw, 28.0 * k);
        round(self.c, field, 4.0 * k, t.field);
        look::outline(self.c, field, 4.0 * k, 1.0, t.border_focus);
        let shown = if f.query.is_empty() {
            "Find in document"
        } else {
            f.query.as_str()
        };
        let qw = fit(
            self.c,
            shown,
            field.x + 10.0 * k,
            cy + self.sz.ui * 0.36,
            self.sz.ui,
            400,
            if f.query.is_empty() { t.faint } else { t.text },
            field.w - 20.0 * k,
        );
        if self.caret_visible() {
            let cx = field.x + 10.0 * k + if f.query.is_empty() { 0.0 } else { qw };
            fill(self.c, r(cx, cy - 7.0 * k, 1.0, 14.0 * k), t.text);
        }
        self.said.field(field, "Find in document", &f.query);
        let mut x = field.right() + 8.0 * k;
        for (ic, label, ctl) in [
            (Icon::ChevronUp, "Previous match", FileCtl::FindPrev),
            (Icon::ChevronDown, "Next match", FileCtl::FindNext),
        ] {
            let b = r(x, cy - 12.0 * k, 26.0 * k, 24.0 * k);
            let z = Zone::File(pane, ctl);
            if self.hover.as_ref() == Some(&z) {
                round(self.c, b, 4.0 * k, t.hover);
            }
            icon(self.c, ic, b.x + 13.0 * k, cy, k, t.muted);
            self.hit(b, z);
            self.said.button(b, label);
            x += 28.0 * k;
        }
        let msg = if f.query.is_empty() {
            String::new()
        } else if f.matches.is_empty() {
            String::from("Phrase not found")
        } else {
            format!("{} of {} matches", f.current + 1, f.matches.len())
        };
        text(
            self.c,
            &msg,
            x + 8.0 * k,
            cy + self.sz.small * 0.36,
            self.sz.small,
            400,
            t.muted,
        );
        self.said.text(
            r(
                x,
                bar.y,
                width(self.c, &msg, self.sz.small, 400) + 16.0 * k,
                h,
            ),
            &msg,
        );
        let close = r(bar.right() - 36.0 * k, cy - 12.0 * k, 26.0 * k, 24.0 * k);
        let z = Zone::File(pane, FileCtl::FindClose);
        if self.hover.as_ref() == Some(&z) {
            round(self.c, close, 4.0 * k, t.hover);
        }
        icon(self.c, Icon::Close, close.x + 13.0 * k, cy, k, t.muted);
        self.hit(close, z);
        self.said.button(close, "Close");
        h
    }

    /// Where the current match is, as a page and a top in points, once,
    /// for the view to scroll to.
    pub(super) fn pdf_find_reveal(&mut self, path: &str) -> Option<(u32, f32)> {
        let f = self
            .files
            .find
            .as_mut()
            .filter(|f| f.path == path && f.reveal)?;
        f.reveal = false;
        let (page, i, _) = *f.matches.get(f.current)?;
        let words = self.page_words(path, page);
        Some((page, words.get(i).map_or(0.0, |w| w.y)))
    }

    /// The selection and the find matches over a page drawn at `rc`.
    pub(super) fn pdf_marks(&mut self, pane: usize, path: &str, page: u32, rc: Rect, px: f32) {
        self.files.drawn.push((pane, page, rc, px));
        let t = self.t();
        let sel = self.files.sel.clone().filter(|s| s.path == path && s.shown);
        let find = self.files.find.clone().filter(|f| f.path == path);
        if sel.is_none() && find.as_ref().is_none_or(|f| f.matches.is_empty()) {
            return;
        }
        let words = self.page_words(path, page);
        let boxr = |w: &Word| r(rc.x + w.x * px, rc.y + w.y * px, w.w * px, w.h * px);
        if let Some(f) = &find {
            for (mi, (p, i, n)) in f.matches.iter().enumerate() {
                if *p != page {
                    continue;
                }
                let current = mi == f.current;
                for w in words.iter().skip(*i).take(*n) {
                    let b = boxr(w);
                    // As the viewer marks them: the current in the accent,
                    // the others in yellow.
                    let c = if current {
                        look::alpha(t.accent, 0.55)
                    } else {
                        look::alpha(look::hex(0xE0DE71), 0.55)
                    };
                    fill(self.c, r(b.x - 1.0, b.y - 1.0, b.w + 2.0, b.h + 2.0), c);
                }
            }
        }
        if let Some(s) = sel {
            let (a, b) = ordered(s.a, s.b);
            if page < a.0 || page > b.0 {
                return;
            }
            let from = if page == a.0 { a.1 } else { 0 };
            let to = if page == b.0 {
                b.1
            } else {
                words.len().saturating_sub(1)
            };
            for (j, w) in words.iter().enumerate().take(to + 1).skip(from) {
                let bx = boxr(w);
                // Joined to the next word on the same line, as a run.
                let reach = words
                    .get(j + 1)
                    .filter(|n| j < to && (n.y - w.y).abs() < w.h * 0.5)
                    .map_or(bx.right(), |n| rc.x + n.x * px);
                fill(
                    self.c,
                    r(bx.x, bx.y, reach - bx.x, bx.h),
                    look::alpha(look::hex(0x8A5CF5), 0.3),
                );
            }
        }
    }

    /// Whether the PDF tab in focus is the file `path`.
    pub(super) fn pdf_in_focus(&self) -> Option<(usize, String)> {
        match self.app.view() {
            Some(View::File(f)) if f.kind == crate::vault::FileKind::Pdf => {
                Some((self.app.pane, f.path.clone()))
            }
            _ => None,
        }
    }
}
