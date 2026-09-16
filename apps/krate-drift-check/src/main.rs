//! Drive Drift's physics with simulated input and check the car responds.
//!
//! `apps/krate-drift` is a `cdylib` for wasm, so `cargo test` cannot run
//! inside it. Its logic modules -- `mathx`, `track`, `car` -- import nothing
//! from the SDK, so this crate includes THE REAL FILES by path and runs them
//! natively. Not copies: a copied harness passes happily while the game it is
//! supposed to be checking changes underneath it.
//!
//! Why this exists: `check-app` can build an app, run it, and photograph it,
//! and it still cannot tell you whether pressing right turns the car right.
//! It reported exactly that -- "responds to a press: not checked". Every bug
//! below was found by asking that question in code.

// Fields and helpers the GAME uses for drawing are unused here, which is
// expected: this crate exercises the physics, not the renderer.
#![allow(dead_code)]

// The game is `#![no_std]` and its modules say `use alloc::vec::Vec`. A
// native binary has std, but `alloc` still has to be named explicitly.
extern crate alloc;

#[path = "../../krate-drift/src/mathx.rs"]
mod mathx;
#[path = "../../krate-drift/src/track.rs"]
mod track;
#[path = "../../krate-drift/src/car.rs"]
mod car;

use car::{drive_ai, Car, Surface};
use mathx::{abs, sqrt_approx};

/// One frame at 60 Hz, the rate the game is paced to.
const DT: f32 = 1.0 / 60.0;

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    sqrt_approx((b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1))
}

fn main() {
    let t = track::build(7, 360);
    let mut failures = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!("{} {name}: {detail}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };

    // Throttle. Steering is held on the racing line rather than at zero: the
    // circuit curves, so a car driven perfectly straight is off the road
    // within two seconds and the grass then hides whether the throttle worked.
    let mut c = Car::new(&t, 0, 6);
    let start = (c.x, c.z);
    for _ in 0..120 {
        let (_, steer) = drive_ai(&c, &t, 1.0);
        c.step(&t, 1.0, steer, DT, 0.0);
    }
    let travelled = dist(start, (c.x, c.z));
    check(
        "throttle builds speed",
        c.speed > 30.0 && travelled > 40.0,
        format!("2 s of throttle: speed {:.1}, travelled {travelled:.1}", c.speed),
    );

    // Steering, both ways, from the same state.
    let mut left = Car::new(&t, 0, 6);
    let mut right = Car::new(&t, 0, 6);
    for _ in 0..90 {
        left.step(&t, 1.0, 0.0, DT, 0.0);
        right.step(&t, 1.0, 0.0, DT, 0.0);
    }
    let straight = left.heading;
    for _ in 0..60 {
        left.step(&t, 1.0, -1.0, DT, 0.0);
        right.step(&t, 1.0, 1.0, DT, 0.0);
    }
    check(
        "steering turns both ways",
        left.heading < straight && right.heading > straight,
        format!(
            "from {straight:+.3}: left {:+.3}, right {:+.3}",
            left.heading, right.heading
        ),
    );

    // Braking stops the car, holds it stopped, and reverse needs a new press.
    let mut b = Car::new(&t, 0, 6);
    for _ in 0..180 {
        let (_, steer) = drive_ai(&b, &t, 1.0);
        b.step(&t, 1.0, steer, DT, 0.0);
    }
    let quick = b.speed;
    for _ in 0..60 {
        let (_, steer) = drive_ai(&b, &t, 1.0);
        b.step(&t, -1.0, steer, DT, 0.0);
    }
    let stopped = b.speed;
    for _ in 0..60 {
        b.step(&t, -1.0, 0.0, DT, 0.0);
    }
    let held = b.speed;
    b.step(&t, 0.0, 0.0, DT, 0.0);
    for _ in 0..60 {
        b.step(&t, -1.0, 0.0, DT, 0.0);
    }
    check(
        "braking stops without reversing",
        stopped < quick - 10.0 && stopped >= 0.0 && held >= 0.0,
        format!("{quick:.1} -> {stopped:.1}, still {held:.1} while held"),
    );
    check(
        "reverse needs a second press",
        b.speed < -2.0,
        format!("after releasing and pressing again: {:.1}", b.speed),
    );

    // Grass costs you. Both cars steer along the line; only one is held off
    // the road, so the surface is the only difference between them.
    let mut on = Car::new(&t, 0, 6);
    for _ in 0..240 {
        let (_, steer) = drive_ai(&on, &t, 1.0);
        on.step(&t, 1.0, steer, DT, 0.0);
    }
    let mut off = Car::new(&t, 0, 6);
    for _ in 0..240 {
        let (_, steer) = drive_ai(&off, &t, 1.0);
        let w = t.locate(off.x, off.z, off.node);
        let node = t.nodes[w.node];
        if abs(w.offset) < 13.0 {
            off.x += -node.dir_z * 0.35;
            off.z += node.dir_x * 0.35;
        }
        off.step(&t, 1.0, steer, DT, 0.0);
    }
    check(
        "grass is slower than road",
        off.speed < on.speed && off.surface == Surface::Grass,
        format!("road {:.1} vs grass {:.1}", on.speed, off.speed),
    );

    // Two cars overlapping are pushed apart and exchange some speed.
    let mut fast = Car::new(&t, 0, 6);
    let mut slow = Car::new(&t, 1, 6);
    slow.x = fast.x + 1.0;
    slow.z = fast.z;
    fast.speed = 40.0;
    slow.speed = 10.0;
    let before = dist((fast.x, fast.z), (slow.x, slow.z));
    Car::collide(&mut fast, &mut slow);
    let after = dist((fast.x, fast.z), (slow.x, slow.z));
    check(
        "cars collide and separate",
        after > before,
        format!("gap {before:.2} -> {after:.2}, speeds {:.1}/{:.1}", fast.speed, slow.speed),
    );

    // The whole thing: can a car actually get round, repeatedly, on the road?
    let mut p = Car::new(&t, 0, 6);
    let mut off_track = 0;
    let frames = 60 * 120;
    for _ in 0..frames {
        let (throttle, steer) = drive_ai(&p, &t, 0.95);
        p.step(&t, throttle, steer, DT, 0.0);
        if p.surface == Surface::Grass {
            off_track += 1;
        }
    }
    let off_pct = off_track as f32 / frames as f32 * 100.0;
    check(
        "a driven car completes laps",
        p.lap >= 2 && off_pct < 25.0,
        format!("two minutes: {} laps, {off_pct:.0}% off-track", p.lap),
    );

    // A whole race, to the finish: does everyone actually complete three laps,
    // does the field finish in a sensible order, and does the player's own car
    // reach the end? This is the results screen checked by its data -- the
    // screen itself cannot be photographed, because `--shoot` closes the
    // window after capturing and the app correctly exits on the close.
    let mut field: Vec<Car> = (0..6).map(|i| Car::new(&t, i, 6)).collect();
    let mut order: Vec<usize> = Vec::new();
    let mut race_t = 0.0_f32;
    for _ in 0..(60 * 400) {
        race_t += DT;
        for i in 0..field.len() {
            let skill = if i == 0 { 0.93 } else { 0.88 + i as f32 * 0.018 };
            let (throttle, steer) = drive_ai(&field[i], &t, skill.min(1.0));
            if !field[i].finished {
                field[i].step(&t, throttle, steer, DT, race_t);
            }
        }
        for i in 0..field.len() {
            for j in (i + 1)..field.len() {
                let (a, b) = field.split_at_mut(j);
                Car::collide(&mut a[i], &mut b[0]);
            }
        }
        for i in 0..field.len() {
            if field[i].finished && !order.contains(&i) {
                order.push(i);
            }
        }
        if order.len() == field.len() {
            break;
        }
    }
    let all_done = order.len() == field.len();
    let player_place = order.iter().position(|&i| i == 0).map(|p| p + 1);
    let winner_time = order.first().map(|&i| field[i].finish_time).unwrap_or(0.0);
    check(
        "a full race finishes for every car",
        all_done && winner_time > 60.0,
        format!(
            "{}/6 finished, winner {winner_time:.1} s, player {}",
            order.len(),
            player_place.map_or("did not finish".to_string(), |p| format!("P{p}"))
        ),
    );
    // The order must be by finishing time, which is what the results plate
    // draws top to bottom.
    let times: Vec<f32> = order.iter().map(|&i| field[i].finish_time).collect();
    check(
        "the finishing order is by time",
        times.windows(2).all(|w| w[0] <= w[1]),
        format!("{:?}", times.iter().map(|t| (t * 10.0) as i32 as f32 / 10.0).collect::<Vec<_>>()),
    );

    if failures > 0 {
        println!("\n{failures} check(s) failed");
        std::process::exit(1);
    }
    println!("\nevery input check passed");
}
