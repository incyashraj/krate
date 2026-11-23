//! A web viewer's tab (src/webviewer.rs): its header -- back, forward,
//! reload, the address field, reader view and more, where the notes app
//! puts them -- and the page, the system's web view placed over the tab
//! (krate:ui web-page). A page is taken away when its tab is no longer
//! drawn, and put out of the way while a menu or a dialog is open over it,
//! as the system's view would otherwise cover them.

use super::look::{self, fill, icon, r, round, text, Icon};
use super::{Ui, Zone};
use crate::app::{Hit, View};
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;
use krate::ui::web_page;

/// Why a page was not shown, in plain words.
fn why(e: &krate::ui::types::UiError, url: &str) -> alloc::string::String {
    use krate::ui::types::UiError;
    alloc::string::String::from(match e {
        UiError::PermissionDenied if !url.to_ascii_lowercase().starts_with("https://") => {
            "Vault opens only secure (https) pages."
        }
        UiError::PermissionDenied => "Krate has not let Vault open pages on this site.",
        UiError::Unsupported(_) => {
            "This computer cannot show a web page inside an app yet (K-1368)."
        }
        _ => "The page could not be shown.",
    })
}

impl Ui {
    /// The header of a web viewer's tab.
    pub(super) fn draw_web_header(&mut self, pane: usize, vh: Rect, focused: bool) {
        let t = self.t();
        let k = self.sz.scale;
        let (shown, editing, can_back) = {
            let (tabs, act) = self.app.pane_tabs(pane);
            match tabs.get(act).and_then(|tb| tb.view.as_ref()) {
                Some(View::Web(w)) => (
                    alloc::string::String::from(w.shown()),
                    w.field.is_some(),
                    w.hist.len() > 1,
                ),
                _ => return,
            }
        };
        let cy = vh.y + vh.h * 0.5;
        // The notes app rules a web viewer's header off from its page.
        fill(self.c, r(vh.x, vh.bottom() - 1.0, vh.w, 1.0), t.border);
        for (j, (ic, can, label, hit)) in [
            (
                Icon::ArrowLeft,
                can_back,
                "Navigate back",
                Some(Hit::WebBack),
            ),
            (Icon::ArrowRight, false, "Navigate forward", None),
        ]
        .into_iter()
        .enumerate()
        {
            let cx = vh.x + 26.0 * k + j as f32 * 28.0 * k;
            match hit.filter(|_| focused && can) {
                Some(h) => self.icon_button(ic, cx, cy, label, h, false),
                None => icon(self.c, ic, cx, cy, k, if can { t.muted } else { t.faint }),
            }
        }
        if focused {
            self.icon_button(
                Icon::RotateCw,
                vh.x + 90.0 * k,
                cy,
                "Reload",
                Hit::WebReload,
                false,
            );
        } else {
            icon(self.c, Icon::RotateCw, vh.x + 90.0 * k, cy, k, t.faint);
        }
        // The address field fills the header between them.
        let field = r(
            vh.x + 122.0 * k,
            vh.y + 4.0 * k,
            (vh.w - 208.0 * k).max(60.0),
            30.0 * k,
        );
        round(self.c, field, 8.0 * k, t.field);
        look::outline(
            self.c,
            field,
            8.0 * k,
            1.0,
            if editing { t.accent } else { t.border },
        );
        let base = field.y + field.h * 0.5 + self.sz.ui * 0.36;
        let tx = field.x + 8.0 * k;
        if let Some(inp) = self.app.pane_view(pane).and_then(|v| match v {
            View::Web(w) => w.field.clone(),
            _ => None,
        }) {
            self.draw_input(
                &inp,
                tx,
                base,
                self.sz.ui,
                t.text,
                field.right() - 8.0 * k,
                true,
            );
        } else {
            look::fit(
                self.c,
                &shown,
                tx,
                base,
                self.sz.ui,
                400,
                t.text,
                field.w - 16.0 * k,
            );
        }
        self.said.field(field, "Address", &shown);
        self.hit(field, Zone::App(Hit::WebAddress));
        if focused {
            self.icon_button(
                Icon::Glasses,
                vh.right() - 54.0 * k,
                cy,
                "Reader view",
                Hit::WebReader,
                false,
            );
            self.icon_button(
                Icon::More,
                vh.right() - 26.0 * k,
                cy,
                "More options",
                Hit::Command(crate::app::Cmd::TogglePin),
                false,
            );
        } else {
            icon(self.c, Icon::Glasses, vh.right() - 54.0 * k, cy, k, t.faint);
            icon(self.c, Icon::More, vh.right() - 26.0 * k, cy, k, t.faint);
        }
    }

    /// The page of a web viewer's tab.
    pub(super) fn draw_web(&mut self, pane: usize, a: Rect, focused: bool) {
        let t = self.t();
        let k = self.sz.scale;
        fill(self.c, a, t.bg);
        let covered = self.app.overlay.is_some();
        let (win, frame) = (self.win, self.frames);
        let Some(View::Web(w)) = self.app.pane_view_mut(pane) else {
            return;
        };
        w.seen = frame;
        let blank = w.url == crate::webviewer::BLANK;
        // Open the page, or move it where the tab is now; out of the way
        // while something is drawn over it.
        let (x, y, ww, hh) = if covered {
            (a.x, a.y, 0.0, 0.0)
        } else {
            (a.x, a.y, a.w, a.h)
        };
        match w.page {
            Some((_, page)) if blank => {
                web_page::close(page);
                w.page = None;
            }
            None if !blank && w.go => {
                w.go = false;
                w.loading = true;
                match web_page::open(win, &w.url, x, y, ww, hh) {
                    Ok(page) => {
                        w.page = Some((win, page));
                        w.problem.clear();
                    }
                    Err(e) => {
                        w.loading = false;
                        w.problem = why(&e, &w.url);
                    }
                }
            }
            Some((_, page)) => {
                if w.back {
                    w.back = false;
                    web_page::back(page);
                    w.hist.pop();
                } else if w.go {
                    w.go = false;
                    if let Err(e) = web_page::go(page, &w.url) {
                        w.problem = why(&e, &w.url);
                    }
                }
                let _ = web_page::place(page, x, y, ww, hh);
                if let Ok(s) = web_page::state(page) {
                    if !s.address.is_empty() && s.address != w.url {
                        w.url = s.address.clone();
                    }
                    if w.hist.last() != Some(&w.url) {
                        w.hist.push(w.url.clone());
                    }
                    w.title = crate::webviewer::title_for(&w.url, &s.title);
                    w.loading = s.loading;
                    if !s.problem.is_empty() {
                        w.problem = s.problem;
                    }
                }
            }
            None => {}
        }
        let problem = w.problem.clone();
        let _ = focused;
        // The notes app's own words when a page cannot be shown.
        if !problem.is_empty() {
            let cx = a.x + a.w * 0.5;
            let h1 = "Unable to load site.";
            let p = "Make sure you have the correct address.";
            let s1 = self.sz.ui * 1.6;
            let w1 = look::width(self.c, h1, s1, 700);
            text(self.c, h1, cx - w1 * 0.5, a.y + 80.0 * k, s1, 700, t.text);
            let w2 = look::width(self.c, p, self.sz.ui, 400);
            text(
                self.c,
                p,
                cx - w2 * 0.5,
                a.y + 112.0 * k,
                self.sz.ui,
                400,
                t.muted,
            );
            let w3 = look::width(self.c, &problem, self.sz.ui, 400);
            text(
                self.c,
                &problem,
                cx - w3 * 0.5,
                a.y + 136.0 * k,
                self.sz.ui,
                400,
                t.faint,
            );
            self.said
                .heading(r(cx - w1 * 0.5, a.y + 60.0 * k, w1, s1), h1);
            self.said
                .text(r(cx - w2 * 0.5, a.y + 96.0 * k, w2, 20.0), p);
            self.said
                .text(r(cx - w3 * 0.5, a.y + 120.0 * k, w3, 20.0), &problem);
        }
    }

    /// Whether a page is loading (its title and address are read again).
    pub(super) fn web_loading(&self) -> bool {
        self.app
            .all_tabs()
            .any(|tb| matches!(&tb.view, Some(View::Web(w)) if w.page.is_some() && w.loading))
    }

    /// Take away the pages of this window whose tabs were not drawn.
    pub(super) fn sweep_web(&mut self) {
        let (win, frame) = (self.win, self.frames);
        for tb in self.app.all_tabs_mut() {
            if let Some(View::Web(w)) = tb.view.as_mut() {
                if let Some((pw, page)) = w.page {
                    if pw == win && w.seen != frame {
                        web_page::close(page);
                        w.page = None;
                        w.go = true;
                    }
                }
            }
        }
    }
}

impl Ui {
    /// Workspaces' "Manage workspace layouts" dialog, measured from the
    /// notes app's: a name to save the layout as with Save, a line, then
    /// each saved layout with when it was saved, Load and delete.
    pub(super) fn draw_layouts(&mut self, input: &crate::app::Input, w: f32, h: f32) {
        let t = self.t();
        let k = self.sz.scale;
        let rows = self.app.workspace_list.clone();
        let row_h = 43.0 * k;
        let cw = (560.0 * k).min(w - 24.0);
        let ch =
            116.0 * k + rows.len() as f32 * row_h + if rows.is_empty() { 0.0 } else { 16.0 * k };
        let x = (w - cw) * 0.5;
        let y = ((h - ch) * 0.5).max(12.0);
        let card = r(x, y, cw, ch);
        round(self.c, card, 24.0 * k, t.bg);
        look::outline(self.c, card, 24.0 * k, 1.0, t.border_hover);
        self.hit(card, Zone::App(Hit::ListBox));
        let pad = 17.0 * k;
        let title = "Manage workspace layouts";
        text(
            self.c,
            title,
            x + pad,
            y + 17.0 * k + 15.0 * k,
            15.0 * k,
            600,
            t.text,
        );
        self.said
            .heading(r(x + pad, y + 17.0 * k, cw - 2.0 * pad, 20.0 * k), title);
        let close = (x + cw - 29.0 * k, y + 27.0 * k);
        self.icon_button(Icon::Close, close.0, close.1, "Close", Hit::Scrim, false);
        // The name and Save.
        let bw = 53.0 * k;
        let fb = r(
            x + pad,
            y + 55.0 * k,
            cw - 2.0 * pad - bw - 8.0 * k,
            30.0 * k,
        );
        round(self.c, fb, 8.0 * k, t.field);
        look::outline(self.c, fb, 8.0 * k, 1.0, t.border_focus);
        let base = fb.y + fb.h * 0.5 + 13.0 * k * 0.36;
        if input.text.is_empty() {
            text(
                self.c,
                "Save current workspace layout as...",
                fb.x + 9.0 * k,
                base,
                13.0 * k,
                400,
                t.faint,
            );
        }
        self.draw_input(
            input,
            fb.x + 9.0 * k,
            base,
            13.0 * k,
            t.text,
            fb.right() - 8.0 * k,
            true,
        );
        self.said
            .field(fb, "Save current workspace layout as...", &input.text);
        let save = r(fb.right() + 8.0 * k, fb.y, bw, 30.0 * k);
        self.small_button(save, "Save", Hit::AskOk);
        if rows.is_empty() {
            return;
        }
        fill(
            self.c,
            r(x + pad, y + 101.0 * k, cw - 2.0 * pad, 1.0),
            t.border,
        );
        for (i, (name, when, on)) in rows.iter().enumerate() {
            let ry = y + 114.0 * k + i as f32 * row_h;
            text(self.c, name, x + pad, ry + 15.0 * k, 15.0 * k, 400, t.text);
            if *on {
                let nw = look::width(self.c, name, 15.0 * k, 400);
                let label = "Currently active";
                let fw = look::width(self.c, label, 12.0 * k, 400) + 12.0 * k;
                let fr = r(x + pad + nw + 8.0 * k, ry + 1.0 * k, fw, 18.0 * k);
                round(self.c, fr, 9.0 * k, look::alpha(t.accent, 0.2));
                text(
                    self.c,
                    label,
                    fr.x + 6.0 * k,
                    fr.y + 9.0 * k + 12.0 * k * 0.36,
                    12.0 * k,
                    400,
                    t.accent,
                );
            }
            text(self.c, when, x + pad, ry + 32.0 * k, 12.0 * k, 400, t.faint);
            self.said.text(r(x + pad, ry, cw * 0.6, 34.0 * k), name);
            let lb = r(x + cw - pad - 86.0 * k, ry + 3.0 * k, 54.0 * k, 30.0 * k);
            self.small_button(lb, "Load", Hit::LayoutLoad(name.clone()));
            self.icon_button(
                Icon::Close,
                x + cw - pad - 12.0 * k,
                ry + 18.0 * k,
                "Delete layout",
                Hit::LayoutDelete(name.clone()),
                false,
            );
        }
    }

    /// A plain button of the notes app's dialogs (`--interactive-normal`).
    fn small_button(&mut self, rc: Rect, label: &str, hit: Hit) {
        let t = self.t();
        let k = self.sz.scale;
        let hov = self.hovered(&Zone::App(hit.clone()));
        round(
            self.c,
            rc,
            8.0 * k,
            if hov { t.border_hover } else { t.button },
        );
        let lw = look::width(self.c, label, 13.0 * k, 400);
        text(
            self.c,
            label,
            rc.x + (rc.w - lw) * 0.5,
            rc.y + rc.h * 0.5 + 13.0 * k * 0.36,
            13.0 * k,
            400,
            t.text,
        );
        self.button(rc, label, hit);
    }
}
