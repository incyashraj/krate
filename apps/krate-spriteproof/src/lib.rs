//! Proof that the sprite pipeline works end to end: read a raw-RGBA asset from
//! the bundle, draw a background sprite, then draw a smaller sprite rotated on
//! top. If this shows a rotated red triangle over a teal field, the whole
//! assets -> draw_sprite chain the sci-fi game needs is real.
//!
//! Assets are raw RGBA (8-byte little-endian w,h header + w*h*4 bytes), so the
//! guest does zero decoding -- it reads bytes and hands them to draw_sprite.

#![no_std]

extern crate alloc;
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::vec::Vec;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::resources::assets;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: u32 = 480;
const HEIGHT: u32 = 480;

struct Component;

/// A decoded raw-RGBA asset: dimensions plus the pixel bytes.
struct Raw {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

/// Read a raw-RGBA asset: little-endian u32 width, u32 height, then the bytes.
fn read_raw(path: &str) -> Option<Raw> {
    let bytes = assets::read(path).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let h = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let need = (w as usize) * (h as usize) * 4;
    let body = bytes.get(8..8 + need)?;
    Some(Raw {
        w,
        h,
        rgba: body.to_vec(),
    })
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        let Some(bg) = read_raw("bg.rgba") else {
            let _ = out.write(b"asset:bg-missing\n");
            return 40;
        };
        let Some(sprite) = read_raw("sprite.rgba") else {
            let _ = out.write(b"asset:sprite-missing\n");
            return 41;
        };
        let _ = out.write(b"assets:loaded\n");

        let Ok(win) = window::create(
            "Sprite proof",
            types::WindowSize {
                width: WIDTH,
                height: HEIGHT,
            },
        ) else {
            return 30;
        };
        let _ = window::show(win);
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas))
                .is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };

        // Background: draw the bg sprite stretched to fill, no rotation.
        let _ = canvas2d::draw_sprite(
            canvas,
            gfx::Point {
                x: WIDTH as f32 * 0.5,
                y: HEIGHT as f32 * 0.5,
            },
            gfx::Size {
                width: WIDTH as f32,
                height: HEIGHT as f32,
            },
            0.0,
            bg.w,
            bg.h,
            &bg.rgba,
        );

        // Foreground: the same sprite drawn several times at different angles,
        // to prove rotation, scaling, and alpha over the background.
        let mut i = 0u32;
        while i < 6 {
            let angle = (i as f32) * 1.0472; // 60 degrees apart
            let cx = 120.0 + (i % 3) as f32 * 120.0;
            let cy = 150.0 + (i / 3) as f32 * 160.0;
            let _ = canvas2d::draw_sprite(
                canvas,
                gfx::Point { x: cx, y: cy },
                gfx::Size {
                    width: 80.0,
                    height: 80.0,
                },
                angle,
                sprite.w,
                sprite.h,
                &sprite.rgba,
            );
            i += 1;
        }

        let _ = canvas2d::present(canvas);
        let _ = out.write(b"spriteproof:ok\n");

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        let rounds = if quick { 20 } else { 400 };
        for _ in 0..rounds {
            match events::wait(Some(50)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                _ => {}
            }
        }
        let _ = window::close(win);
        0
    }
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
