//! A base's toolbar menus drawn as the notes app draws them: a card under
//! the button with a title, rows of dropdowns and the actions at its foot.
//! What they do is in src/basemenu.rs.

use super::look::{self, fill, fit, icon, r, round, text, width, Icon};
use super::{Ui, Zone};
use crate::app::{Hit, View};
use crate::basefile::BaseDoc;
use crate::basemenu::{
    dir_labels, group_conj, operators, parse_cond, prop_label, BaseUi, PType, Panel, FILE_PROPS,
};
use crate::json::Value;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::canvas2d;
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

/// A property type's mark, as the notes app marks properties.
fn type_icon(id: &str, t: PType) -> Icon {
    if id.starts_with("file.") && id != "file.file" {
        return Icon::Info;
    }
    match t {
        PType::Number => Icon::Binary,
        PType::Date => Icon::Calendar,
        PType::Checkbox => Icon::SquareCheck,
        PType::List => Icon::List,
        PType::File => Icon::File,
        PType::Text => Icon::Text,
    }
}

const ROW: f32 = 36.0;

impl Ui {
    /// A dropdown: an icon, its label and the up-down chevrons, in a box.
    fn combo(&mut self, b: Rect, ic: Option<Icon>, label: &str, faint: bool, hit: BaseUi) {
        let t = self.t();
        let k = self.sz.scale;
        let z = super::Zone::App(Hit::BaseUi(hit.clone()));
        if self.hovered(&z) {
            round(self.c, b, 4.0 * k, t.hover);
        }
        let cy = b.y + b.h * 0.5;
        let mut x = b.x + 9.0 * k;
        if let Some(ic) = ic {
            icon(self.c, ic, x + 7.0 * k, cy, k * 0.9, t.muted);
            x += 20.0 * k;
        }
        fit(
            self.c,
            label,
            x,
            cy + self.sz.ui * 0.36,
            self.sz.ui,
            400,
            if faint { t.faint } else { t.text },
            b.right() - x - 22.0 * k,
        );
        icon(
            self.c,
            Icon::ChevronsUpDown,
            b.right() - 12.0 * k,
            cy,
            k * 0.8,
            t.muted,
        );
        self.button(b, label, Hit::BaseUi(hit));
    }

    fn pop_icon(&mut self, cx: f32, cy: f32, ic: Icon, label: &str, hit: BaseUi) {
        let t = self.t();
        let k = self.sz.scale;
        let b = r(cx - 12.0 * k, cy - 12.0 * k, 24.0 * k, 24.0 * k);
        let z = super::Zone::App(Hit::BaseUi(hit.clone()));
        let hot = self.hovered(&z);
        if hot {
            round(self.c, b, 4.0 * k, t.hover);
        }
        icon(self.c, ic, cx, cy, k, if hot { t.text } else { t.muted });
        self.button(b, label, Hit::BaseUi(hit));
    }

    /// "+ Add sort" and the like: an action row at a card's foot.
    fn pop_action(&mut self, x: f32, y: f32, w: f32, label: &str, hit: BaseUi) -> f32 {
        let t = self.t();
        let k = self.sz.scale;
        let lw = width(self.c, label, self.sz.ui, 400);
        let b = r(x, y, (lw + 36.0 * k).min(w), 28.0 * k);
        let z = super::Zone::App(Hit::BaseUi(hit.clone()));
        if self.hovered(&z) {
            round(self.c, b, 4.0 * k, t.hover);
        }
        icon(
            self.c,
            Icon::Plus,
            b.x + 12.0 * k,
            b.y + b.h * 0.5,
            k,
            t.muted,
        );
        text(
            self.c,
            label,
            b.x + 28.0 * k,
            b.y + b.h * 0.5 + self.sz.ui * 0.36,
            self.sz.ui,
            400,
            t.text,
        );
        self.button(b, label, Hit::BaseUi(hit));
        b.w
    }

    /// The card a menu sits on, with its shadow and edge.
    fn pop_card(&mut self, card: Rect) {
        let t = self.t();
        let _ = canvas2d::drop_shadow_round_rect(
            self.c,
            look::g(card),
            look::radii(8.0),
            16.0,
            look::alpha(look::hex(0), 0.4),
        );
        round(self.c, card, 8.0, t.side);
        look::outline(self.c, card, 8.0, 1.0, t.border_hover);
        self.base_pop = Some(card);
        // The card takes presses that hit nothing in it.
        self.hit(card, super::Zone::App(Hit::ListBox));
    }

    fn grip(&mut self, x: f32, cy: f32) {
        let t = self.t();
        let k = self.sz.scale;
        for i in 0..3 {
            for j in 0..2 {
                let _ = canvas2d::fill_circle(
                    self.c,
                    krate::gfx::types::Point {
                        x: x - 2.5 * k + j as f32 * 5.0 * k,
                        y: cy - 5.0 * k + i as f32 * 5.0 * k,
                    },
                    1.3 * k,
                    t.faint,
                );
            }
        }
    }

    /// The open toolbar menu of the base in focus, under its button.
    pub(super) fn draw_base_panel(&mut self, area: Rect) {
        let Some(View::Base(b)) = self.app.view() else {
            return;
        };
        let Some(panel) = b.panel else {
            return;
        };
        let Some(&(_, ax, ay)) = self.base_anchors.iter().find(|(p, _, _)| *p == panel) else {
            return;
        };
        let doc = BaseDoc::parse(b.source.as_deref().unwrap_or(""));
        let view = b.view;
        let all_open = b.filter_all_open;
        let draft = b.draft.clone();
        let names = b.base.clone();
        match panel {
            Panel::Sort => self.draw_sort_panel(&doc, view, &names, ax, ay, area),
            Panel::Group => self.draw_group_panel(&doc, view, &names, ax, ay, area),
            Panel::Properties => self.draw_props_panel(&doc, view, &names, ax, ay, area),
            Panel::Filter => self.draw_filter_panel(&doc, view, all_open, draft, ax, ay, area),
        }
    }

    fn header(&mut self, card: Rect, title: &str) {
        let t = self.t();
        let k = self.sz.scale;
        text(
            self.c,
            title,
            card.x + 15.0 * k,
            card.y + 19.0 * k + self.sz.ui * 0.36,
            self.sz.ui,
            600,
            t.text,
        );
        self.said
            .heading(r(card.x, card.y, card.w, 36.0 * k), title);
    }

    fn label_of(&self, base: &crate::bases::Base, id: &str) -> String {
        let shown = base.column_name(id);
        if shown != prop_label(id) && !shown.is_empty() && !id.starts_with("file.") {
            shown
        } else {
            prop_label(id)
        }
    }

    fn draw_sort_panel(
        &mut self,
        doc: &BaseDoc,
        view: usize,
        base: &crate::bases::Base,
        ax: f32,
        ay: f32,
        area: Rect,
    ) {
        let t = self.t();
        let k = self.sz.scale;
        let sort = doc
            .views
            .get(view)
            .and_then(|w| w.sort.clone())
            .unwrap_or_default();
        // Each dropdown as wide as what it says, as the notes app sizes them.
        let ui = self.sz.ui;
        let mut pw = 88.0 * k;
        let mut dw = 67.0 * k;
        for (p, _) in &sort {
            let label = self.label_of(base, p);
            pw = pw.max(width(self.c, &label, ui, 400) + 58.0 * k);
            let (az, za) = dir_labels(self.app.prop_type(p));
            dw = dw.max(width(self.c, az, ui, 400).max(width(self.c, za, ui, 400)) + 36.0 * k);
        }
        let w = (26.0 * k + pw + dw + 40.0 * k).max(220.0 * k);
        let h = 38.0 * k + sort.len() as f32 * ROW * k + 8.0 * k + 40.0 * k;
        let card = r(
            (ax - 10.0 * k).min(area.right() - w - 8.0 * k),
            ay - 2.0 * k,
            w,
            h,
        );
        self.pop_card(card);
        self.header(card, "Sort by");
        let mut y = card.y + 39.0 * k;
        for (i, (p, d)) in sort.iter().enumerate() {
            let cy = y + 15.0 * k;
            self.grip(card.x + 15.0 * k, cy);
            // The grip carries the key up or down the list.
            let row_r = r(card.x, y, card.w, ROW * k);
            self.pop_rows.push((row_r, i));
            self.hit(
                r(card.x + 6.0 * k, y, 20.0 * k, 30.0 * k),
                Zone::PopRow(0, i),
            );
            self.said.button(
                r(card.x + 6.0 * k, y, 20.0 * k, 30.0 * k),
                "Drag to reorder",
            );
            let ty = self.app.prop_type(p);
            let (az, za) = dir_labels(ty);
            let label = self.label_of(base, p);
            let pb = r(card.x + 26.0 * k, y, pw, 30.0 * k);
            let db = r(pb.right(), y, dw, 30.0 * k);
            round(self.c, r(pb.x, y, pb.w + db.w, 30.0 * k), 5.0 * k, t.field);
            look::outline(
                self.c,
                r(pb.x, y, pb.w + db.w, 30.0 * k),
                5.0 * k,
                1.0,
                t.border_hover,
            );
            fill(self.c, r(db.x, y + 6.0 * k, 1.0, 18.0 * k), t.border_hover);
            self.combo(
                pb,
                Some(type_icon(p, ty)),
                &label,
                false,
                BaseUi::SortProp(i),
            );
            self.combo(
                db,
                None,
                if d == "DESC" { za } else { az },
                false,
                BaseUi::SortDir(i),
            );
            self.pop_icon(
                db.right() + 17.0 * k,
                cy,
                Icon::Trash,
                "Remove sort",
                BaseUi::SortRemove(i),
            );
            y += ROW * k;
        }
        fill(self.c, r(card.x, y + 2.0 * k, card.w, 1.0), t.border);
        self.pop_action(
            card.x + 6.0 * k,
            y + 8.0 * k,
            card.w - 12.0 * k,
            "Add sort",
            BaseUi::SortAdd,
        );
    }

    fn draw_group_panel(
        &mut self,
        doc: &BaseDoc,
        view: usize,
        base: &crate::bases::Base,
        ax: f32,
        ay: f32,
        area: Rect,
    ) {
        let t = self.t();
        let k = self.sz.scale;
        let g = doc.views.get(view).and_then(|w| w.group_by.clone());
        let (p, d) = g.clone().unwrap_or_default();
        let ty = self.app.prop_type(&p);
        let (az, za) = dir_labels(ty);
        let label_w = if g.is_some() {
            width(self.c, &self.label_of(base, &p), self.sz.ui, 400) + 58.0 * k
        } else {
            0.0
        };
        let pw = (102.0 * k).max(label_w);
        let dir_w =
            width(self.c, az, self.sz.ui, 400).max(width(self.c, za, self.sz.ui, 400)) + 36.0 * k;
        let dw = (67.0 * k).max(dir_w);
        let w = (12.0 * k + pw + dw + 40.0 * k).max(220.0 * k);
        let h = 38.0 * k + ROW * k + 4.0 * k;
        let card = r(
            (ax - 10.0 * k).min(area.right() - w - 8.0 * k),
            ay - 2.0 * k,
            w,
            h,
        );
        self.pop_card(card);
        self.header(card, "Group by");
        let y = card.y + 39.0 * k;
        let cy = y + 15.0 * k;
        let pb = r(card.x + 12.0 * k, y, pw, 30.0 * k);
        let db = r(pb.right(), y, dw, 30.0 * k);
        round(self.c, r(pb.x, y, pb.w + db.w, 30.0 * k), 5.0 * k, t.field);
        look::outline(
            self.c,
            r(pb.x, y, pb.w + db.w, 30.0 * k),
            5.0 * k,
            1.0,
            t.border_hover,
        );
        fill(self.c, r(db.x, y + 6.0 * k, 1.0, 18.0 * k), t.border_hover);
        if g.is_some() {
            let label = self.label_of(base, &p);
            self.combo(
                pb,
                Some(type_icon(&p, ty)),
                &label,
                false,
                BaseUi::GroupProp,
            );
            self.combo(
                db,
                None,
                if d == "DESC" { za } else { az },
                false,
                BaseUi::GroupDir,
            );
        } else {
            self.combo(pb, None, "Property", true, BaseUi::GroupProp);
            self.combo(db, None, az, true, BaseUi::GroupDir);
        }
        self.pop_icon(
            db.right() + 17.0 * k,
            cy,
            Icon::Trash,
            "Remove group",
            BaseUi::GroupRemove,
        );
    }

    fn draw_props_panel(
        &mut self,
        doc: &BaseDoc,
        view: usize,
        base: &crate::bases::Base,
        ax: f32,
        ay: f32,
        area: Rect,
    ) {
        let t = self.t();
        let k = self.sz.scale;
        let shown: Vec<String> = doc
            .views
            .get(view)
            .and_then(|w| w.order.clone())
            .unwrap_or_default();
        // Shown first, in their order; then the rest.
        let mut list: Vec<(String, bool)> = shown.iter().map(|p| (p.clone(), true)).collect();
        let mut rest: Vec<String> = self.app.base_props();
        rest.retain(|p| !shown.contains(p) && p != "file.file" && p != "note.tags");
        let files: Vec<String> = FILE_PROPS
            .iter()
            .map(|s| String::from(*s))
            .filter(|p| rest.contains(p))
            .collect();
        let mut files = files;
        files.sort();
        rest.retain(|p| !p.starts_with("file."));
        for p in files.into_iter().chain(rest) {
            list.push((p, false));
        }
        // What is typed in "Find or create..." narrows the list.
        let query = match self.app.view() {
            Some(View::Base(b)) => b.prop_query.clone(),
            _ => String::new(),
        };
        let q = crate::text::lower(query.trim());
        if !q.is_empty() {
            list.retain(|(p, _)| {
                crate::text::lower(&self.label_of(base, p)).contains(&q)
                    || crate::text::lower(p).contains(&q)
            });
        }
        let rh = 26.0 * k;
        let w = 220.0 * k;
        let h = (44.0 * k + list.len() as f32 * rh + 8.0 * k).min(area.bottom() - ay - 8.0 * k);
        let card = r(
            (ax - 10.0 * k).min(area.right() - w - 8.0 * k),
            ay - 2.0 * k,
            w,
            h,
        );
        self.pop_card(card);
        // The search field at its head.
        let sb = r(
            card.x + 8.0 * k,
            card.y + 8.0 * k,
            card.w - 16.0 * k,
            28.0 * k,
        );
        icon(
            self.c,
            Icon::Search,
            sb.x + 12.0 * k,
            sb.y + sb.h * 0.5,
            k * 0.85,
            t.muted,
        );
        let qw = text_w(
            self.c,
            if query.is_empty() {
                "Find or create..."
            } else {
                &query
            },
            sb.x + 26.0 * k,
            sb.y + sb.h * 0.5 + self.sz.ui * 0.36,
            self.sz.ui,
            if query.is_empty() { t.faint } else { t.text },
        );
        if self.caret_visible() {
            let cx = sb.x + 26.0 * k + if query.is_empty() { 0.0 } else { qw };
            fill(self.c, r(cx, sb.y + 7.0 * k, 1.0, sb.h - 14.0 * k), t.text);
        }
        self.said.field(sb, "Find or create...", &query);
        // Nothing found: the line that makes a property of that name.
        if list.is_empty() && !query.trim().is_empty() {
            let y = sb.bottom() + 10.0 * k;
            let mut label = String::from("Add \u{201C}");
            label.push_str(query.trim());
            label.push_str("\u{201D} property");
            self.pop_action(
                card.x + 6.0 * k,
                y,
                card.w - 12.0 * k,
                &label,
                BaseUi::CreateProp,
            );
        }
        fill(
            self.c,
            r(card.x, sb.bottom() + 4.0 * k, card.w, 1.0),
            t.border,
        );
        let top = sb.bottom() + 8.0 * k;
        let _ = look::clip(self.c, card.x, top, card.w, card.bottom() - top - 4.0 * k);
        for (i, (p, on)) in list.iter().enumerate() {
            let y = top + i as f32 * rh;
            if y > card.bottom() {
                break;
            }
            let row = r(card.x + 6.0 * k, y, card.w - 12.0 * k, rh);
            let z = Zone::PopRow(1, i);
            if self.hovered(&z) {
                round(self.c, row, 4.0 * k, t.hover);
            }
            let cy = y + rh * 0.5;
            let bx = r(row.x + 6.0 * k, cy - 7.0 * k, 14.0 * k, 14.0 * k);
            if *on {
                round(self.c, bx, 3.0 * k, t.accent);
                icon(
                    self.c,
                    Icon::Check,
                    bx.x + bx.w * 0.5,
                    cy,
                    k * 0.7,
                    t.on_accent,
                );
            } else {
                look::outline(self.c, bx, 3.0 * k, 1.0, t.faint);
            }
            let ty = self.app.prop_type(p);
            let ink = if *on { t.text } else { t.faint };
            icon(
                self.c,
                type_icon(p, ty),
                row.x + 36.0 * k,
                cy,
                k * 0.9,
                if *on { t.muted } else { t.faint },
            );
            let label = self.label_of(base, p);
            fit(
                self.c,
                &label,
                row.x + 50.0 * k,
                cy + self.sz.ui * 0.36,
                self.sz.ui,
                400,
                ink,
                row.w - 80.0 * k,
            );
            // A press toggles the column; a drag of a shown one moves it.
            self.hit(r(row.x, y, row.w - 28.0 * k, rh), z);
            self.said.button(r(row.x, y, row.w - 28.0 * k, rh), &label);
            if *on {
                self.pop_rows.push((r(row.x, y, row.w, rh), i));
            }
            self.pop_props.push(p.clone());
            self.said.toggle(r(row.x, y, 20.0 * k, rh), &label, *on);
            self.pop_icon(
                row.right() - 12.0 * k,
                cy,
                Icon::ChevronRight,
                "Edit property",
                BaseUi::Rename(p.clone()),
            );
        }
        let _ = look::unclip(self.c);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_filter_panel(
        &mut self,
        doc: &BaseDoc,
        view: usize,
        all_open: bool,
        draft: Option<(bool, Vec<usize>, Option<String>)>,
        ax: f32,
        ay: f32,
        area: Rect,
    ) {
        let t = self.t();
        let k = self.sz.scale;
        let w = (470.0 * k).min(area.w - 16.0 * k);
        let global = doc.filters.clone();
        let own = doc.views.get(view).and_then(|v| v.filters.clone());
        let count = |v: &Option<Value>| -> usize {
            match v {
                Some(Value::Obj(kv)) => kv.first().map_or(0, |(_, a)| a.arr().len()),
                Some(Value::Str(_)) => 1,
                _ => 0,
            }
        };
        // Height: worked out by laying out first.
        let gh = if all_open {
            self.filter_height(&global, &draft, true)
        } else {
            0.0
        };
        let vh = self.filter_height(&own, &draft, false);
        let h = 8.0 * k + 34.0 * k + gh + 34.0 * k + vh + 8.0 * k;
        let card = r(
            (ax - 10.0 * k).min(area.right() - w - 8.0 * k),
            ay - 2.0 * k,
            w,
            h.min(area.bottom() - ay),
        );
        self.pop_card(card);
        let mut y = card.y + 8.0 * k;
        for (g, label, open, root) in [
            (true, "All views", all_open, &global),
            (false, "This view", true, &own),
        ] {
            let hb = r(card.x + 6.0 * k, y, card.w - 12.0 * k, 30.0 * k);
            let cy = hb.y + hb.h * 0.5;
            look::chevron(self.c, hb.x + 12.0 * k, cy, open, k, t.muted);
            text(
                self.c,
                label,
                hb.x + 26.0 * k,
                cy + self.sz.ui * 0.36,
                self.sz.ui,
                600,
                t.text,
            );
            let n = count(root);
            if n > 0 {
                let lw = width(self.c, label, self.sz.ui, 600);
                let s = alloc::format!("{n}");
                let fb = r(
                    hb.x + 34.0 * k + lw,
                    cy - 9.0 * k,
                    width(self.c, &s, self.sz.small, 400) + 10.0 * k,
                    18.0 * k,
                );
                round(self.c, fb, 4.0 * k, t.hover);
                text(
                    self.c,
                    &s,
                    fb.x + 5.0 * k,
                    cy + self.sz.small * 0.36,
                    self.sz.small,
                    400,
                    t.muted,
                );
            }
            if g {
                self.button(hb, label, Hit::BaseUi(BaseUi::FilterAll));
            } else {
                self.said.heading(hb, label);
            }
            y += 34.0 * k;
            if open {
                let used = self.draw_filter_group(card, y, g, root, &Vec::new(), &draft);
                y += used;
            }
        }
    }

    fn filter_height(
        &self,
        root: &Option<Value>,
        draft: &Option<(bool, Vec<usize>, Option<String>)>,
        g: bool,
    ) -> f32 {
        let k = self.sz.scale;
        fn rows(v: &Value) -> usize {
            match v {
                Value::Obj(kv) => kv.first().map_or(0, |(_, a)| {
                    a.arr()
                        .iter()
                        .map(|c| match c {
                            Value::Obj(_) => 2 + rows(c),
                            _ => 1,
                        })
                        .sum()
                }),
                Value::Str(_) => 1,
                _ => 0,
            }
        }
        let n =
            root.as_ref().map_or(0, rows) + usize::from(draft.as_ref().is_some_and(|d| d.0 == g));
        (36.0 + n as f32 * ROW + 36.0) * k
    }

    /// One filter group: its conjunction, its rows, its actions. Returns
    /// the height used.
    fn draw_filter_group(
        &mut self,
        card: Rect,
        top: f32,
        g: bool,
        root: &Option<Value>,
        path: &Vec<usize>,
        draft: &Option<(bool, Vec<usize>, Option<String>)>,
    ) -> f32 {
        let t = self.t();
        let k = self.sz.scale;
        let indent = path.len() as f32 * 16.0 * k;
        let x0 = card.x + 12.0 * k + indent;
        let wide = card.right() - 12.0 * k - x0;
        let group_root = match root {
            Some(Value::Obj(kv)) if !kv.is_empty() => Value::Obj(kv.clone()),
            Some(Value::Str(s)) => Value::Obj(alloc::vec![(
                String::from("and"),
                Value::Arr(alloc::vec![Value::Str(s.clone())])
            )]),
            _ => Value::Obj(alloc::vec![(String::from("and"), Value::Arr(Vec::new()))]),
        };
        // The group at `path`.
        let mut node = group_root.clone();
        for i in path {
            let next = match &node {
                Value::Obj(kv) => kv.first().and_then(|(_, a)| a.arr().get(*i).cloned()),
                _ => None,
            };
            match next {
                Some(n) => node = n,
                None => return 0.0,
            }
        }
        let conj = group_conj(&group_root, path);
        let items: Vec<Value> = match &node {
            Value::Obj(kv) => kv
                .first()
                .map(|(_, a)| a.arr().to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let conj_label = match conj.as_str() {
            "or" => "Any of the following are true",
            "not" => "None of the following are true",
            _ => "All the following are true",
        };
        let mut y = top;
        let cw = width(self.c, conj_label, self.sz.ui, 400) + 34.0 * k;
        self.combo(
            r(x0, y, cw, 28.0 * k),
            None,
            conj_label,
            false,
            BaseUi::FilterConj(g, path.clone()),
        );
        if !path.is_empty() {
            let mut p = path.clone();
            let _ = &mut p;
            self.pop_icon(
                x0 + wide - 12.0 * k,
                y + 14.0 * k,
                Icon::Trash,
                "Remove filter group",
                BaseUi::FilterRemove(g, path.clone()),
            );
        }
        y += 36.0 * k;
        let word = if conj == "or" || conj == "not" {
            "or"
        } else {
            "and"
        };
        let mut rows: Vec<(Vec<usize>, Option<Value>)> = items
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let mut p = path.clone();
                p.push(i);
                (p, Some(v.clone()))
            })
            .collect();
        if let Some((dg, dp, _)) = draft {
            if *dg == g && dp.len() == path.len() + 1 && dp[..path.len()] == path[..] {
                rows.push((dp.clone(), None));
            }
        }
        for (ri, (rp, v)) in rows.iter().enumerate() {
            if let Some(Value::Obj(_)) = v {
                // A group inside: framed, then its own rows.
                let inner = self.draw_filter_group(
                    card,
                    y + 4.0 * k,
                    g,
                    &Some(group_root.clone()),
                    rp,
                    draft,
                );
                look::outline(
                    self.c,
                    r(x0 + 4.0 * k, y, wide - 4.0 * k, inner + 8.0 * k),
                    6.0 * k,
                    1.0,
                    t.border,
                );
                y += inner + 16.0 * k;
                continue;
            }
            let cy = y + 15.0 * k;
            self.grip(x0 + 4.0 * k, cy);
            let lead = if ri == 0 { "where" } else { word };
            text(
                self.c,
                lead,
                x0 + 14.0 * k,
                cy + self.sz.ui * 0.36,
                self.sz.ui,
                400,
                t.muted,
            );
            let bx = x0 + 60.0 * k;
            let room = card.right() - 12.0 * k - bx - 56.0 * k;
            let text_of = match v {
                Some(Value::Str(s)) => Some(s.clone()),
                _ => None,
            };
            match text_of.as_deref().map(|s| (s, parse_cond(s))) {
                Some((_, Some(c))) => {
                    let ty = self.app.prop_type(&c.prop);
                    let op_label = operators(ty)
                        .into_iter()
                        .find(|(_, o)| *o == c.op)
                        .map_or(c.op.clone(), |(l, _)| String::from(l));
                    let pw = (room * 0.3).max(70.0 * k);
                    let ow = (room * 0.3).max(70.0 * k);
                    let vw = room - pw - ow;
                    let row_box = r(bx, y, room, 30.0 * k);
                    round(self.c, row_box, 5.0 * k, t.field);
                    look::outline(self.c, row_box, 5.0 * k, 1.0, t.border_hover);
                    fill(
                        self.c,
                        r(bx + pw, y + 6.0 * k, 1.0, 18.0 * k),
                        t.border_hover,
                    );
                    fill(
                        self.c,
                        r(bx + pw + ow, y + 6.0 * k, 1.0, 18.0 * k),
                        t.border_hover,
                    );
                    self.combo(
                        r(bx, y, pw, 30.0 * k),
                        Some(type_icon(&c.prop, ty)),
                        &prop_label(&c.prop),
                        false,
                        BaseUi::FilterProp(g, rp.clone()),
                    );
                    self.combo(
                        r(bx + pw, y, ow, 30.0 * k),
                        None,
                        &op_label,
                        false,
                        BaseUi::FilterOp(g, rp.clone()),
                    );
                    let vb = r(bx + pw + ow, y, vw, 30.0 * k);
                    let needs = !c.op.trim_start_matches('!').eq("isEmpty");
                    if needs {
                        let shown = if ty == PType::Checkbox {
                            String::from(if c.value == "true" {
                                "\u{2611} true"
                            } else {
                                "\u{2610} false"
                            })
                        } else {
                            c.value.clone()
                        };
                        let faint = shown.is_empty();
                        fit(
                            self.c,
                            if faint { "Empty" } else { &shown },
                            vb.x + 9.0 * k,
                            cy + self.sz.ui * 0.36,
                            self.sz.ui,
                            400,
                            if faint { t.faint } else { t.text },
                            vb.w - 14.0 * k,
                        );
                        self.button(vb, "Value", Hit::BaseUi(BaseUi::FilterValue(g, rp.clone())));
                    }
                }
                Some((s, None)) => {
                    // A formula the simple builder cannot show: as written.
                    let row_box = r(bx, y, room, 30.0 * k);
                    round(self.c, row_box, 5.0 * k, t.field);
                    look::outline(self.c, row_box, 5.0 * k, 1.0, t.border_hover);
                    text(
                        self.c,
                        "Formula",
                        bx + 9.0 * k,
                        cy + self.sz.small * 0.36,
                        self.sz.small,
                        400,
                        t.muted,
                    );
                    fit(
                        self.c,
                        s,
                        bx + 64.0 * k,
                        cy + self.sz.ui * 0.36,
                        self.sz.ui,
                        400,
                        t.text,
                        room - 72.0 * k,
                    );
                    self.said.text(row_box, s);
                }
                None => {
                    // The row being filled in.
                    let prop = draft.as_ref().and_then(|d| d.2.clone());
                    let pw = (room * 0.3).max(70.0 * k);
                    let ow = (room * 0.3).max(70.0 * k);
                    let row_box = r(bx, y, room, 30.0 * k);
                    round(self.c, row_box, 5.0 * k, t.field);
                    look::outline(self.c, row_box, 5.0 * k, 1.0, t.border_hover);
                    match prop {
                        Some(p) => {
                            let ty = self.app.prop_type(&p);
                            self.combo(
                                r(bx, y, pw, 30.0 * k),
                                Some(type_icon(&p, ty)),
                                &prop_label(&p),
                                false,
                                BaseUi::FilterProp(g, rp.clone()),
                            );
                            self.combo(
                                r(bx + pw, y, ow, 30.0 * k),
                                None,
                                "Operator",
                                true,
                                BaseUi::FilterOp(g, rp.clone()),
                            );
                        }
                        None => {
                            self.combo(
                                r(bx, y, pw, 30.0 * k),
                                None,
                                "Property",
                                true,
                                BaseUi::FilterProp(g, rp.clone()),
                            );
                        }
                    }
                }
            }
            self.pop_icon(
                card.right() - 40.0 * k,
                cy,
                Icon::Code,
                "Advanced filter",
                BaseUi::FilterFormula(g, rp.clone()),
            );
            self.pop_icon(
                card.right() - 20.0 * k,
                cy,
                Icon::Close,
                "Remove filter",
                BaseUi::FilterRemove(g, rp.clone()),
            );
            y += ROW * k;
        }
        let aw = self.pop_action(
            x0,
            y + 4.0 * k,
            wide,
            "Add filter",
            BaseUi::FilterAdd(g, path.clone()),
        );
        self.pop_action(
            x0 + aw + 8.0 * k,
            y + 4.0 * k,
            wide,
            "Add filter group",
            BaseUi::FilterAddGroup(g, path.clone()),
        );
        y += 36.0 * k;
        y - top
    }
}

/// Text drawn, returning its width.
fn text_w(c: u64, s: &str, x: f32, base: f32, size: f32, col: look::Rgba) -> f32 {
    text(c, s, x, base, size, 400, col);
    width(c, s, size, 400)
}
