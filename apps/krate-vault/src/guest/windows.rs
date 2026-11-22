//! The windows of their own (src/popout.rs), as real windows: each has its
//! own canvas, its own tab bar and what its tab shows, and the menus and
//! lists opened from it. The drawing and event code is the main window's:
//! while one of these windows is drawn, or its event is handled, its state
//! (canvas, hit boxes, what is hovered, the note's layout) is swapped into
//! the [`Ui`] and swapped back after.

use super::editor::{LayCache, Shown, View as EdView};
use super::look::r;
use super::{views, widget, Drag, Ui, Zone, CANVAS_ID, LIGHTS, ROOT_ID};
use crate::panes::Divider;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::canvas2d;
use krate::layout::{Rect, Said};
#[allow(unused_imports)]
use krate::prelude::*;
use krate::ui::{tree, types, window};

/// A window of its own: the floating group's id and the window's state.
pub struct WinState {
    pub id: u64,
    win: u64,
    c: u64,
    hits: Vec<(Rect, Zone)>,
    said: Said,
    w: f32,
    h: f32,
    pane_areas: Vec<(usize, Rect, Rect)>,
    dividers: Vec<Divider>,
    tab_rects: Vec<(usize, usize, Rect)>,
    view_areas: Vec<(usize, Rect)>,
    tree_area: Option<Rect>,
    search_area: Option<Rect>,
    right_area: Option<Rect>,
    list_area: Option<Rect>,
    settings_area: Option<Rect>,
    hover: Option<Zone>,
    hover_point: Option<(f32, f32)>,
    drag: Drag,
    caret_xy: Option<(f32, f32)>,
    title: String,
    graph_img: views::GraphImage,
    cache: LayCache,
    shown: Shown,
    view: Option<EdView>,
}

/// Make a window for a floating group: its tree and canvas.
fn open_window(w: f32, h: f32) -> Option<(u64, u64)> {
    let win = window::create(
        "Vault",
        types::WindowSize {
            width: w.max(200.0) as u32,
            height: h.max(150.0) as u32,
        },
    )
    .ok()?;
    let _ = window::set_full_bleed(win, true);
    let _ = window::show(win);
    if tree::set_root(win, &widget(ROOT_ID, None, types::WidgetKind::Stack)).is_err()
        || tree::upsert_node(
            win,
            &widget(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        )
        .is_err()
    {
        let _ = window::close(win);
        return None;
    }
    match canvas2d::bind(win, CANVAS_ID) {
        Ok(c) => Some((win, c)),
        Err(_) => {
            let _ = window::close(win);
            None
        }
    }
}

impl Ui {
    /// Swap window `i`'s state with what the [`Ui`] holds: in, then out.
    pub(super) fn swap_window(&mut self, i: usize) {
        let e = &mut self.extra[i];
        core::mem::swap(&mut self.win, &mut e.win);
        core::mem::swap(&mut self.c, &mut e.c);
        core::mem::swap(&mut self.hits, &mut e.hits);
        core::mem::swap(&mut self.said, &mut e.said);
        core::mem::swap(&mut self.w, &mut e.w);
        core::mem::swap(&mut self.h, &mut e.h);
        core::mem::swap(&mut self.pane_areas, &mut e.pane_areas);
        core::mem::swap(&mut self.dividers, &mut e.dividers);
        core::mem::swap(&mut self.tab_rects, &mut e.tab_rects);
        core::mem::swap(&mut self.view_areas, &mut e.view_areas);
        core::mem::swap(&mut self.tree_area, &mut e.tree_area);
        core::mem::swap(&mut self.search_area, &mut e.search_area);
        core::mem::swap(&mut self.right_area, &mut e.right_area);
        core::mem::swap(&mut self.list_area, &mut e.list_area);
        core::mem::swap(&mut self.settings_area, &mut e.settings_area);
        core::mem::swap(&mut self.hover, &mut e.hover);
        core::mem::swap(&mut self.hover_point, &mut e.hover_point);
        core::mem::swap(&mut self.drag, &mut e.drag);
        core::mem::swap(&mut self.caret_xy, &mut e.caret_xy);
        core::mem::swap(&mut self.title, &mut e.title);
        core::mem::swap(&mut self.graph_img, &mut e.graph_img);
        core::mem::swap(&mut self.cache, &mut e.cache);
        core::mem::swap(&mut self.shown, &mut e.shown);
        core::mem::swap(&mut self.view, &mut e.view);
        self.in_window = if self.in_window == 0 { e.id } else { 0 };
    }

    /// The window of its own an event came from, by its place in `extra`.
    pub(super) fn window_index(&self, win: u64) -> Option<usize> {
        if win == self.win {
            return None;
        }
        self.extra.iter().position(|e| e.win == win)
    }

    /// Open a window for each new floating group; close the windows whose
    /// group is gone.
    pub(super) fn sync_windows(&mut self) -> bool {
        let mut changed = false;
        let ids: Vec<u64> = self.app.floating.iter().map(|f| f.id).collect();
        let mut i = 0;
        while i < self.extra.len() {
            if ids.contains(&self.extra[i].id) {
                i += 1;
            } else {
                let e = self.extra.remove(i);
                let _ = window::close(e.win);
                if self.overlay_in == e.id {
                    self.overlay_in = 0;
                    self.app.overlay = None;
                }
                changed = true;
            }
        }
        let new: Vec<(u64, f32, f32)> = self
            .app
            .floating
            .iter()
            .filter(|f| !self.extra.iter().any(|e| e.id == f.id))
            .map(|f| (f.id, f.w, f.h))
            .collect();
        for (id, w, h) in new {
            let Some((win, c)) = open_window(w, h) else {
                // No window: the group goes, as if it were closed.
                self.app.close_floating(id);
                continue;
            };
            self.extra.push(WinState {
                id,
                win,
                c,
                hits: Vec::new(),
                said: Said::new(),
                w,
                h,
                pane_areas: Vec::new(),
                dividers: Vec::new(),
                tab_rects: Vec::new(),
                view_areas: Vec::new(),
                tree_area: None,
                search_area: None,
                right_area: None,
                list_area: None,
                settings_area: None,
                hover: None,
                hover_point: None,
                drag: Drag::None,
                caret_xy: None,
                title: String::new(),
                graph_img: views::GraphImage::default(),
                cache: LayCache::default(),
                shown: Shown::default(),
                view: None,
            });
            changed = true;
        }
        changed
    }

    /// Draw every window of its own.
    pub(super) fn draw_windows(&mut self) {
        for i in 0..self.extra.len() {
            self.swap_window(i);
            self.draw_window();
            self.swap_window(i);
        }
    }

    /// Draw the window swapped in: its group across the whole window, the
    /// tab bar leaving room for the system's window buttons.
    pub(super) fn draw_window(&mut self) {
        let id = self.in_window;
        let Some(f) = self.app.floating.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let size = canvas2d::canvas_size(self.c).unwrap_or(krate::gfx::types::Size {
            width: f.w,
            height: f.h,
        });
        let (w, h) = (size.width.max(200.0), size.height.max(150.0));
        self.w = w;
        self.h = h;
        self.app.floating_moved(id, f.x, f.y, w, h);
        let t = self.t();
        self.hits.clear();
        self.said = Said::new();
        self.caret_xy = None;
        self.pane_areas.clear();
        self.dividers.clear();
        self.tab_rects.clear();
        self.view_areas.clear();
        if self.app.pane != f.pane {
            self.view = None;
            self.shown.area = None;
        }
        let _ = canvas2d::clear(self.c, t.side);
        self.draw_pane(f.pane, r(0.0, 0.0, w, h), LIGHTS, 0.0);
        if self.app.pane == f.pane {
            self.draw_editor_extras(w, h);
        }
        if self.overlay_in == id {
            self.draw_overlay(w, h);
        }
        let _ = canvas2d::present(self.c);
        self.said.send(self.win);
        // The window's title is its tab's, as the notes app titles it.
        let mut want = String::new();
        let (tabs, act) = self.app.pane_tabs(f.pane);
        if let Some(tb) = tabs.get(act) {
            want.push_str(tb.title());
            want.push_str(" - ");
        }
        if let Some(v) = &self.app.vault {
            want.push_str(&v.name);
            want.push_str(" - ");
        }
        want.push_str("Vault");
        if want != self.title {
            let _ = window::set_title(self.win, &want);
            self.title = want;
        }
    }
}
