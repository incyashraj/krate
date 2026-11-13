//! Krate's own theme for the wall, light and dark, plain and high contrast:
//! the same colours the phone's home is drawn in (apps/krate-home/src/kit.rs,
//! from docs/landing/kit/site.css). The wall is its own crate, so it carries
//! the part of that theme it draws with.

use krate::gfx::types as gfx;
use krate::layout::Look;

pub(crate) const fn rgba(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

pub(crate) const fn hex(v: u32) -> gfx::Color {
    rgba(
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
        1.0,
    )
}

pub(crate) const WHITE: gfx::Color = rgba(1.0, 1.0, 1.0, 1.0);

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    /// Behind the sheet when there is no picture of the screen before it.
    pub backdrop: gfx::Color,
    /// How strongly the app's own colour washes in from the top.
    pub wash: f32,
    /// How much darker the picture of the screen before it is drawn.
    pub dim: f32,
    /// Behind a notice, what an app printed, and the home list.
    pub page: gfx::Color,
    /// The sheet and every card.
    pub card: gfx::Color,
    /// A hairline round a card, where its shadow alone does not show it.
    pub edge: gfx::Color,
    pub shadow: gfx::Color,
    /// The grouped rows inside the sheet's details.
    pub group: gfx::Color,
    /// A quiet fill: the close circle, the "+3" tile.
    pub soft: gfx::Color,
    pub soft_press: gfx::Color,
    /// A row held down.
    pub row_press: gfx::Color,
    pub ink: gfx::Color,
    pub ink2: gfx::Color,
    pub ink3: gfx::Color,
    /// Between rows.
    pub line: gfx::Color,
    /// Round the outlined button.
    pub stroke: gfx::Color,
    pub accent: gfx::Color,
    pub accent_press: gfx::Color,
    /// The one black (in dark mode, white) button.
    pub btn: gfx::Color,
    pub btn_press: gfx::Color,
    pub btn_ink: gfx::Color,
    pub switch_off: gfx::Color,
    pub switch_on: gfx::Color,
    pub knob_shadow: gfx::Color,
    /// A request's tile once it is switched off.
    pub denied: gfx::Color,
    /// The sheet's grabber.
    pub grabber: gfx::Color,
}

pub(crate) fn theme(look: &Look) -> Theme {
    let contrast = look.high_contrast;
    let pick = |plain: u32, strong: u32| if contrast { hex(strong) } else { hex(plain) };
    if look.dark {
        Theme {
            backdrop: hex(0x08080a),
            wash: 0.20,
            dim: 0.50,
            page: hex(0x08080a),
            card: hex(0x18181b),
            edge: rgba(1.0, 1.0, 1.0, if contrast { 0.32 } else { 0.07 }),
            shadow: rgba(0.0, 0.0, 0.0, 0.55),
            group: hex(0x232327),
            soft: hex(0x2a2a2f),
            soft_press: hex(0x3a3a40),
            row_press: rgba(1.0, 1.0, 1.0, 0.07),
            ink: hex(0xf4f4f5),
            ink2: pick(0xa1a1aa, 0xe4e4e7),
            ink3: pick(0x7c7c85, 0xc4c4cc),
            line: rgba(1.0, 1.0, 1.0, if contrast { 0.35 } else { 0.09 }),
            stroke: rgba(1.0, 1.0, 1.0, if contrast { 0.55 } else { 0.18 }),
            accent: pick(0x6f94ff, 0x9db5ff),
            accent_press: pick(0x4f6fd0, 0x7f9be6),
            btn: hex(0xf4f4f5),
            btn_press: hex(0xd4d4d8),
            btn_ink: hex(0x08080a),
            switch_off: pick(0x3a3a3f, 0x71717a),
            switch_on: hex(0x30d158),
            knob_shadow: rgba(0.0, 0.0, 0.0, 0.25),
            denied: pick(0x4a4a51, 0x71717a),
            grabber: pick(0x3f3f46, 0x71717a),
        }
    } else {
        Theme {
            backdrop: hex(0xebebf0),
            wash: 0.26,
            dim: 0.22,
            page: hex(0xf5f5f7),
            card: WHITE,
            edge: rgba(
                24.0 / 255.0,
                24.0 / 255.0,
                27.0 / 255.0,
                if contrast { 0.35 } else { 0.0 },
            ),
            shadow: rgba(0.0, 0.0, 0.0, 0.16),
            group: hex(0xf5f5f7),
            soft: hex(0xf0f0f3),
            soft_press: hex(0xdedee3),
            row_press: rgba(24.0 / 255.0, 24.0 / 255.0, 27.0 / 255.0, 0.06),
            ink: hex(0x1c1c1e),
            ink2: pick(0x52525b, 0x27272a),
            ink3: pick(0x8a8a92, 0x3f3f46),
            line: rgba(0.0, 0.0, 0.0, if contrast { 0.35 } else { 0.09 }),
            stroke: pick(0xdcdce1, 0x6b6b73),
            accent: pick(0x3d6df0, 0x2450d0),
            accent_press: pick(0x2a4fb8, 0x1a3a9c),
            btn: hex(0x0d0d0d),
            btn_press: hex(0x3a3a40),
            btn_ink: WHITE,
            switch_off: pick(0xe4e4e9, 0xa1a1aa),
            switch_on: pick(0x34c759, 0x248a3d),
            knob_shadow: rgba(0.0, 0.0, 0.0, 0.12),
            denied: pick(0xbcbcc4, 0x8a8a92),
            grabber: pick(0xd4d4d9, 0x8a8a92),
        }
    }
}

/// The person's look, with the text size held where the wall's layouts
/// still fit a phone: the largest accessibility sizes go past it, and a
/// sheet whose Open button has left the screen helps nobody.
pub(crate) fn look_now() -> Look {
    let mut look = Look::now();
    look.text_scale = if look.text_scale.is_finite() {
        look.text_scale.clamp(0.85, 2.2)
    } else {
        1.0
    };
    look
}
