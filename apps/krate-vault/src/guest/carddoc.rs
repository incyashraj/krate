//! Canvas cards drawn with the note editor, as the notes app draws them: a
//! text card's Markdown rendered, a note card's note rendered as an embed,
//! and the card being edited (double-click) a live editor in place -- the
//! same editor a note's tab has, at the canvas's zoom.

use super::editor::{self as ed, Shown, View as EdView};
use super::look::{r, Sizes};
use super::{Ui, Zone};
use crate::canvas::{Card, CardKind};
use crate::editor::{Doc, Mode};
use alloc::string::String;
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

/// The editor's sizes at a canvas zoom.
fn at_zoom(sz: Sizes, z: f32) -> Sizes {
    Sizes {
        ui: sz.ui * z,
        small: sz.small * z,
        medium: sz.medium * z,
        body: sz.body * z,
        code: sz.code * z,
        row: sz.row * z,
        head: sz.head * z,
        ribbon: sz.ribbon * z,
        tab: sz.tab * z,
        view_head: sz.view_head * z,
        measure: sz.measure * z,
        scale: sz.scale * z,
    }
}

impl Ui {
    /// A text or note card's content, with the note editor. Returns false
    /// when the card is too small to draw text in (the caller draws lines).
    pub(super) fn draw_card_doc(
        &mut self,
        n: &Card,
        rc: Rect,
        a: Rect,
        z: f32,
        focused: bool,
    ) -> bool {
        let t = self.t();
        let sz = at_zoom(self.sz, z);
        if sz.body < 5.0 {
            return false;
        }
        let pad = 16.0 * sz.scale;
        let inner = r(
            rc.x + pad,
            rc.y + pad * 0.5,
            (rc.w - pad * 2.0).max(10.0),
            (rc.h - pad).max(10.0),
        );
        // The card's box within the canvas: the editor clips to it.
        let x0 = inner.x.max(a.x);
        let y0 = inner.y.max(a.y);
        let area = r(
            x0,
            y0,
            (inner.right().min(a.right()) - x0).max(0.0),
            (inner.bottom().min(a.bottom()) - y0).max(0.0),
        );
        if area.w <= 0.0 || area.h <= 0.0 {
            return true;
        }
        let view = EdView {
            area,
            x: inner.x,
            width: inner.w,
            sz,
        };
        let live = matches!(self.app.view(), Some(crate::app::View::Canvas(c))
            if c.editing.as_ref().is_some_and(|(id, _)| *id == n.id))
            && self.app.doc().is_some();
        let hide = self.cache.hide_title;
        self.cache.hide_title = true;
        if live && focused {
            // The card being edited: the tab's own editor, with its caret.
            let caret = self.caret_visible();
            let app = &mut self.app;
            if let Some(doc) = app.doc_mut() {
                doc.top_off = (area.y - inner.y).max(0.0);
            }
            let app = &self.app;
            let resolved = |name: &str| -> bool { app.resolved(name) };
            if let Some(doc) = app.doc() {
                ed::draw(
                    &mut self.cache,
                    self.c,
                    doc,
                    &view,
                    t,
                    &mut self.shown,
                    &resolved,
                    caret,
                    &mut self.said,
                );
            }
            self.view = Some(view);
            self.hit(area, Zone::Editor);
        } else {
            let key = match n.kind {
                CardKind::Text => {
                    let mut s = String::from("t:");
                    s.push_str(&n.id);
                    s
                }
                _ => {
                    let mut s = String::from("f:");
                    s.push_str(n.file.as_deref().unwrap_or(""));
                    s
                }
            };
            let gen = self.app.vault.as_ref().map_or(0, |v| v.generation);
            let text = match n.kind {
                CardKind::Text => Some(n.text.clone().unwrap_or_default()),
                _ => {
                    let fresh = self.card_docs.get(&key).is_some_and(|(g, _, _)| *g == gen);
                    if fresh {
                        None
                    } else {
                        let f = n.file.clone().unwrap_or_default();
                        self.app
                            .vault
                            .as_mut()
                            .and_then(|v| v.fs.read(&f).ok())
                            .map(|t| match crate::yaml::front_matter(&t) {
                                // An embed shows the note without its
                                // properties block.
                                Some((_, line)) => t
                                    .split('\n')
                                    .skip(line)
                                    .collect::<alloc::vec::Vec<_>>()
                                    .join("\n"),
                                None => t,
                            })
                    }
                }
            };
            let stale = match (&text, self.card_docs.get(&key)) {
                (Some(tx), Some((_, old, _))) => old != tx,
                (Some(_), None) => true,
                _ => false,
            };
            if stale {
                let tx = text.unwrap_or_default();
                let mut d = Doc::new(&key, "", &tx);
                d.mode = Mode::Reading;
                self.card_docs.insert(key.clone(), (gen, tx, d));
            }
            if let Some((_, _, d)) = self.card_docs.get_mut(&key) {
                d.top_off = (area.y - inner.y).max(0.0);
            }
            let app = &self.app;
            let resolved = |name: &str| -> bool { app.resolved(name) };
            if let Some((_, _, d)) = self.card_docs.get(&key) {
                let mut other = Shown::default();
                ed::draw(
                    &mut self.cache,
                    self.c,
                    d,
                    &view,
                    t,
                    &mut other,
                    &resolved,
                    false,
                    &mut self.said,
                );
            }
        }
        self.cache.hide_title = hide;
        true
    }
}
