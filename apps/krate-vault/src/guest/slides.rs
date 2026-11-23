//! Slides (a core plugin): the note shown one slide at a time across the
//! whole screen, cut at each `---` line, with the arrows at the corner and
//! the progress line at the foot, as the notes app presents it.

use super::look::{self, fill, icon, r, round, text, width, Icon};
use super::{Ui, Zone};
use crate::app::Hit;
use alloc::string::String;
use alloc::vec::Vec;
#[allow(unused_imports)]
use krate::prelude::*;
use krate::ui::window;

impl Ui {
    /// Put the window full screen when a presentation starts, and back
    /// when it ends.
    pub(super) fn follow_slides(&mut self) {
        let on = self.app.slides.is_some();
        if on != self.slides_on {
            self.slides_on = on;
            let _ = window::set_state(
                self.win,
                if on {
                    window::WindowState::Fullscreen
                } else {
                    window::WindowState::Normal
                },
            );
        }
    }

    pub(super) fn draw_slides(&mut self, w: f32, h: f32) {
        let Some((src, i)) = self.app.slides.clone() else {
            return;
        };
        let t = self.t();
        let parts = crate::workspace::slides_of(&src);
        let n = parts.len();
        let i = i.min(n.saturating_sub(1));
        let body = parts.get(i).cloned().unwrap_or_default();
        let _ = krate::gfx::canvas2d::clear(self.c, t.bg);
        // Laid out as the presentation lays a slide out: everything centred
        // across and down, headings in capitals and large, white text.
        let scale = (w / 1024.0).min(h / 800.0);
        let base = 40.0 * scale;
        let white = look::hex(0xFFFFFF);
        let room = w * 0.84;
        let mut lines: Vec<(String, f32, u16, bool)> = Vec::new();
        for raw in body.split('\n') {
            let l = raw.trim_end();
            let (size, weight, txt, bullet) = if let Some(x) = l.strip_prefix("# ") {
                (base * 2.5, 700, x.to_uppercase(), false)
            } else if let Some(x) = l.strip_prefix("## ") {
                (base * 1.6, 700, x.to_uppercase(), false)
            } else if let Some(x) = l.strip_prefix("### ") {
                (base * 1.3, 700, x.to_uppercase(), false)
            } else if let Some(x) = l
                .trim_start()
                .strip_prefix("- ")
                .or_else(|| l.trim_start().strip_prefix("* "))
            {
                (base, 400, String::from(x), true)
            } else if l.trim().is_empty() {
                lines.push((String::new(), base * 0.5, 400, false));
                continue;
            } else {
                (base, 400, String::from(l), false)
            };
            let plain = crate::md::plain(&txt, 0);
            for wl in self.wrap(&plain, size, room) {
                lines.push((wl, size, weight, bullet));
            }
        }
        let total: f32 = lines.iter().map(|(_, s, _, _)| s * 1.25).sum();
        let mut y = ((h - total) * 0.5).max(16.0);
        // A list is a block centred as a whole, its lines starting together.
        let list_w = lines
            .iter()
            .filter(|l| l.3)
            .map(|(s, size, wt, _)| width(self.c, s, *size, *wt) + base)
            .fold(0.0f32, f32::max);
        for (s, size, wt, bullet) in &lines {
            y += size * 1.25;
            if s.is_empty() {
                continue;
            }
            if *bullet {
                let x = (w - list_w) * 0.5;
                let _ = krate::gfx::canvas2d::fill_circle(
                    self.c,
                    krate::gfx::types::Point {
                        x: x + base * 0.3,
                        y: y - size * 0.32,
                    },
                    size * 0.09,
                    white,
                );
                text(self.c, s, x + base, y, *size, *wt, white);
            } else {
                let lw = width(self.c, s, *size, *wt);
                text(self.c, s, (w - lw) * 0.5, y, *size, *wt, white);
            }
            self.said.text(r(0.0, y - size, w, size * 1.25), s);
        }
        // The presentation's arrows at the corner, in its blue.
        let blue = look::hex(0x42AFFA);
        for (on, label, x, y0) in [
            (false, "Previous slide", w - 62.0, h - 36.0),
            (true, "Next slide", w - 30.0, h - 36.0),
        ] {
            let can = if on { i + 1 < n } else { i > 0 };
            if !can {
                continue;
            }
            let b = r(x - 14.0, y0 - 16.0, 28.0, 32.0);
            let (dx, c) = (if on { 1.0 } else { -1.0 }, blue);
            look::stroke(self.c, x - 5.0 * dx, y0 - 10.0, x + 5.0 * dx, y0, 3.0, c);
            look::stroke(self.c, x + 5.0 * dx, y0, x - 5.0 * dx, y0 + 10.0, 3.0, c);
            let _ = Zone::App(Hit::Slide(on));
            self.button(b, label, Hit::Slide(on));
        }
        let _ = (Icon::Close, round, fill, icon);
    }
}
