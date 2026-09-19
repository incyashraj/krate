//! Player -- a music player whose music is real and whose bars are honest.
//!
//! The left rail lists eight tracks with generated artwork, the centre holds
//! the album art, the transport and a progress bar, and along the bottom runs
//! a 56-band spectrum. Nothing on screen is a picture of something else:
//!
//! - The music is synthesised inside the sandbox: a four-chord loop with a
//!   pad, a bass, an arpeggio and a small drum kit, rendered once per track
//!   into a PCM buffer. Each track transposes and re-tempos the same song.
//! - That buffer is what `audio.playback` plays, streamed a little ahead of
//!   the clock with `write`, and it is also what the spectrum reads: every
//!   bar is a Goertzel filter run over the window of samples the speakers
//!   are playing at that instant. On a host without audio the same buffer
//!   still drives the bars, and the status strip says so.
//! - The spectrum is one RGBA buffer blitted per frame rather than fifty-six
//!   rectangles, because every canvas call crosses the sandbox boundary and
//!   the crossing is what a frame budget is spent on (K-227).
//!
//! Time comes from the runtime clock, so the player is deterministic per
//! wall-clock instant and a headless shoot at any offset lands mid-song.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;
extern crate krate as _krate_runtime;

use alloc::vec;
use alloc::vec::Vec;

#[allow(warnings)]
mod bindings;

use bindings::krate::audio::{playback, types as audio};
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The design space; the host scales it to any window, centred, in proportion.
const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

/// Sample rate of everything: the synth, the stream, the spectrum.
const SR: u32 = 22_050;
const SRF: f32 = 22_050.0;

/// How far ahead of the play head the stream is kept filled. Small enough
/// that a seek is heard almost at once, large enough that a slow frame does
/// not let the ring run dry.
const LEAD_SAMPLES: u64 = (SR / 6) as u64;

/// Spectrum geometry: bands, analysis window, the frequency span.
const BANDS: usize = 56;
const WINDOW: usize = 1024;
const F_LO: f32 = 40.0;
const F_HI: f32 = 8_000.0;

/// Frames the quick path draws before reporting. Synthetic time, so every
/// frame costs fuel and buys nothing after the first few.
const QUICK_FRAMES: u32 = 8;

// ---- palette ---------------------------------------------------------------
// The same ground as Query and Trace: near-black, one saturated blue carrying
// meaning, everything else in greys the eye can rest on.

const BG: gfx::Color = rgb(0.055, 0.063, 0.078);
const RAIL: gfx::Color = rgb(0.075, 0.086, 0.106);
const PANE: gfx::Color = rgb(0.086, 0.098, 0.122);
const LINE: gfx::Color = rgb(0.141, 0.161, 0.196);
const LINE_SOFT: gfx::Color = rgb(0.110, 0.126, 0.157);

const INK: gfx::Color = rgb(0.902, 0.925, 0.957);
const INK_DIM: gfx::Color = rgb(0.573, 0.616, 0.686);
const INK_QUIET: gfx::Color = rgb(0.373, 0.412, 0.478);

/// Krate blue, #3b75ff.
const ACCENT: gfx::Color = rgb(0.231, 0.459, 1.0);
const SEL_WASH: gfx::Color = rgb(0.118, 0.165, 0.271);
const GREEN: gfx::Color = rgb(0.310, 0.827, 0.549);

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

const fn rgba(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

// ---- the album ---------------------------------------------------------------

struct Track {
    title: &'static str,
    artist: &'static str,
    /// Length in seconds. The song underneath loops; this is how long the
    /// track runs before the player moves on.
    seconds: u32,
    /// Semitones the whole song is transposed by.
    transpose: i32,
    bpm: f32,
    /// Which arpeggio shape the track uses.
    arp: usize,
    /// Two hues and a style for the generated artwork.
    art: (f32, f32, u32),
}

const TRACKS: [Track; 8] = [
    Track {
        title: "Nightfall",
        artist: "Vela Marin",
        seconds: 222,
        transpose: 0,
        bpm: 96.0,
        arp: 0,
        art: (0.62, 0.78, 0),
    },
    Track {
        title: "Low Tide",
        artist: "Ossian Reyes",
        seconds: 245,
        transpose: -2,
        bpm: 88.0,
        arp: 1,
        art: (0.52, 0.42, 1),
    },
    Track {
        title: "Glass Meridian",
        artist: "Hollow Atlas",
        seconds: 198,
        transpose: 3,
        bpm: 104.0,
        arp: 2,
        art: (0.58, 0.95, 2),
    },
    Track {
        title: "Slow Orbit",
        artist: "Vela Marin",
        seconds: 311,
        transpose: -4,
        bpm: 80.0,
        arp: 1,
        art: (0.72, 0.05, 0),
    },
    Track {
        title: "Paper Lanterns",
        artist: "Juno Halvorsen",
        seconds: 177,
        transpose: 5,
        bpm: 100.0,
        arp: 0,
        art: (0.08, 0.60, 2),
    },
    Track {
        title: "Signal Fade",
        artist: "Ossian Reyes",
        seconds: 266,
        transpose: 1,
        bpm: 92.0,
        arp: 2,
        art: (0.55, 0.68, 1),
    },
    Track {
        title: "Blue Hour",
        artist: "Ida Nakashima",
        seconds: 229,
        transpose: -3,
        bpm: 84.0,
        arp: 0,
        art: (0.60, 0.50, 0),
    },
    Track {
        title: "Afterlight",
        artist: "Hollow Atlas",
        seconds: 273,
        transpose: 2,
        bpm: 98.0,
        arp: 2,
        art: (0.90, 0.62, 1),
    },
];

const ALBUM: &str = "Nightfall Sessions";

/// The song, as MIDI notes. Four bars: Dm7, Bbmaj7, Fmaj7, Cadd9 -- the
/// progression every late-night record is built on. Every track transposes it.
const CHORDS: [[i32; 4]; 4] = [
    [62, 65, 69, 72],
    [58, 62, 65, 69],
    [65, 69, 72, 76],
    [60, 64, 67, 74],
];
const BASS: [i32; 4] = [38, 34, 41, 36];
/// Sixteenth-note arpeggio shapes, as indices into the chord.
const ARPS: [[usize; 8]; 3] = [
    [0, 1, 2, 3, 3, 2, 1, 0],
    [0, 2, 1, 3, 0, 2, 1, 3],
    [0, 3, 1, 2, 3, 0, 2, 1],
];

// ---- tiny drawing + text helpers -------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) {
    let _ = canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        c,
    );
}

fn disc(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) {
    let _ = canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c);
}

fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: all,
        top_right: all,
        bottom_right: all,
        bottom_left: all,
    }
}

fn rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) {
    let _ = canvas2d::fill_round_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        radii(r),
        c,
    );
}

fn style(weight: u16, spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
    }
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

fn text_styled(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color, st: gfx::TextStyle) {
    let _ = canvas2d::draw_text_styled(canvas, s, gfx::Point { x, y }, size, c, st);
}

fn width_of(canvas: u64, s: &str, size: f32) -> f32 {
    canvas2d::measure_text(canvas, s, size)
        .map(|m| m.width)
        .unwrap_or(0.0)
}

fn width_styled(canvas: u64, s: &str, size: f32, st: gfx::TextStyle) -> f32 {
    canvas2d::measure_text_styled(canvas, s, size, st)
        .map(|m| m.width)
        .unwrap_or(0.0)
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - width_of(canvas, s, size), y, size, c);
}

fn text_centred(
    canvas: u64,
    s: &str,
    cx: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
    st: gfx::TextStyle,
) {
    let w = width_styled(canvas, s, size, st);
    text_styled(canvas, s, cx - w * 0.5, y, size, c, st);
}

/// "m:ss" into `buf`.
fn fmt_time(buf: &mut [u8; 8], secs: u32) -> &str {
    let m = secs / 60;
    let s = secs % 60;
    let mut n = 0usize;
    let mut push = |b: u8, buf: &mut [u8; 8]| {
        if let Some(slot) = buf.get_mut(n) {
            *slot = b;
            n += 1;
        }
    };
    if m >= 10 {
        push(b'0' + (m / 10) as u8, buf);
    }
    push(b'0' + (m % 10) as u8, buf);
    push(b':', buf);
    push(b'0' + (s / 10) as u8, buf);
    push(b'0' + (s % 10) as u8, buf);
    core::str::from_utf8(buf.get(..n).unwrap_or(b"0:00")).unwrap_or("0:00")
}

/// A plain integer into `buf`.
fn fmt_int(buf: &mut [u8; 12], value: u32) -> &str {
    let mut tmp = [0u8; 12];
    let mut n = 0usize;
    let mut v = value;
    loop {
        if let Some(slot) = tmp.get_mut(n) {
            *slot = b'0' + (v % 10) as u8;
            n += 1;
        }
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for i in 0..n {
        if let (Some(d), Some(s)) = (buf.get_mut(i), tmp.get(n - 1 - i)) {
            *d = *s;
        }
    }
    core::str::from_utf8(buf.get(..n).unwrap_or(b"0")).unwrap_or("0")
}

fn out_kv(key: &str, value: &str) {
    let stdout = stdio::stdout();
    let _ = stdout.write(key.as_bytes());
    let _ = stdout.write(b":");
    let _ = stdout.write(value.as_bytes());
    let _ = stdout.write(b"\n");
    let _ = stdout.flush();
}

fn out_num(key: &str, value: u32) {
    let mut buf = [0u8; 12];
    let s = fmt_int(&mut buf, value);
    out_kv(key, s);
}

// ---- deterministic noise -----------------------------------------------------

fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x = ((x >> ((x >> 28).wrapping_add(4))) ^ x).wrapping_mul(277_803_737);
    x = (x >> 22) ^ x;
    (x & 0xFFFF) as f32 / 65_535.0
}

/// xorshift, for the drum noise. Seeded per render so a track is the same
/// every time it is opened.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x & 0xFFFF) as f32 / 32_767.5 - 1.0
    }
}

// ---- the synthesiser -----------------------------------------------------------

const SINE_N: usize = 2048;

fn build_sine() -> Vec<f32> {
    let mut table = vec![0.0f32; SINE_N];
    for i in 0..SINE_N {
        if let Some(slot) = table.get_mut(i) {
            *slot = libm::sinf(i as f32 / SINE_N as f32 * core::f32::consts::TAU);
        }
    }
    table
}

/// Table sine of a phase in turns; `k` picks a harmonic without a second
/// accumulator, because the index mask wraps for free.
#[inline(always)]
fn sn(table: &[f32], phase: f32, k: f32) -> f32 {
    let idx = (phase * k * SINE_N as f32) as u32 as usize & (SINE_N - 1);
    table.get(idx).copied().unwrap_or(0.0)
}

fn midi_hz(note: i32) -> f32 {
    440.0 * libm::exp2f((note - 69) as f32 / 12.0)
}

/// Per-sample decay factor for an envelope that falls at `rate` per second.
fn decay(rate: f32) -> f32 {
    libm::expf(-rate / SRF)
}

/// Samples in one loop of the song at this tempo: sixteen beats.
fn loop_len(track: &Track) -> usize {
    (16.0 * 60.0 / track.bpm * SRF) as usize
}

/// Render the whole loop for one track as mono 16-bit PCM.
///
/// Per-sample work is table lookups, multiplies and adds only: every envelope
/// is a running product, the kick's pitch sweep is its own envelope, and the
/// frequencies are fixed per sixteenth-note step. That is what keeps a
/// quarter-million samples inside a headless fuel budget.
fn render_song(track: &Track, sine: &[f32]) -> Vec<i16> {
    let total = loop_len(track);
    let mut out = vec![0i16; total];

    let step_len = total / 64; // 64 sixteenths in four bars
    let bar_len = (step_len * 16) as f32;
    let arp_shape = ARPS.get(track.arp).copied().unwrap_or(ARPS[0]);

    // Voices.
    let mut pad_ph = [[0.0f32; 3]; 4];
    let mut pad_hz = [0.0f32; 4];
    let mut bass_ph = 0.0f32;
    let mut bass_hz = 55.0f32;
    let mut bass_env = 0.0f32;
    let mut arp_ph = 0.0f32;
    let mut arp_hz: f32;
    let mut arp_env: f32;
    let mut arp_gain: f32;
    let mut kick_ph = 0.0f32;
    let mut kick_env = 0.0f32;
    let mut kick_sweep = 0.0f32;
    let mut snare_env = 0.0f32;
    let mut snare_tone = 0.0f32;
    let mut snare_ph = 0.0f32;
    let mut hat_env = 0.0f32;
    let mut hat_rate = decay(60.0);
    let mut noise = Noise(0x9E37_79B9 ^ (track.transpose as u32).wrapping_mul(2654435761));
    let mut last_noise = 0.0f32;

    let bass_decay = decay(3.2);
    let arp_decay = decay(7.5);
    let kick_decay = decay(15.0);
    let kick_sweep_decay = decay(26.0);
    let snare_decay = decay(19.0);
    let snare_tone_decay = decay(34.0);
    let trem_hz = 0.5f32;
    let mut trem_ph = 0.0f32;

    let mut write = 0usize;
    for step in 0..64usize {
        let bar = step / 16;
        let in_bar = step % 16;
        let chord = CHORDS.get(bar).copied().unwrap_or(CHORDS[0]);

        if in_bar == 0 {
            for k in 0..4 {
                pad_hz[k] = midi_hz(chord[k] + track.transpose);
            }
            bass_hz = midi_hz(BASS.get(bar).copied().unwrap_or(38) + track.transpose);
        }
        // Bass: root on 1 and 3, a pickup on the "and" of 2 and of 4.
        if in_bar == 0 || in_bar == 6 || in_bar == 8 || in_bar == 14 {
            bass_env = if in_bar == 0 || in_bar == 8 { 1.0 } else { 0.7 };
        }
        // Arpeggio: every sixteenth, an octave up, accented on the beat.
        let idx = arp_shape.get(step % 8).copied().unwrap_or(0);
        arp_hz = midi_hz(chord.get(idx).copied().unwrap_or(69) + 12 + track.transpose);
        arp_env = 1.0;
        arp_gain = if in_bar % 4 == 0 {
            1.25
        } else if in_bar % 2 == 0 {
            0.9
        } else {
            0.7
        };
        // Drums.
        if in_bar == 0 || in_bar == 8 || in_bar == 11 {
            kick_env = 1.0;
            kick_sweep = 1.0;
        }
        if in_bar == 4 || in_bar == 12 {
            snare_env = 1.0;
            snare_tone = 1.0;
        }
        if in_bar % 2 == 0 {
            hat_env = if in_bar == 14 { 0.9 } else { 0.55 };
            hat_rate = if in_bar == 14 {
                decay(18.0)
            } else {
                decay(70.0)
            };
        }

        let step_start = write;
        for i in 0..step_len {
            let tb = (in_bar * step_len + i) as f32 / bar_len;
            // Pad swells into each bar and lets go before the next.
            let swell = (tb / 0.18).min(1.0) * ((1.0 - tb) / 0.12).min(1.0);
            trem_ph += trem_hz / SRF;
            if trem_ph >= 1.0 {
                trem_ph -= 1.0;
            }
            let trem = 0.85 + 0.15 * sn(sine, trem_ph, 1.0);

            let mut pad = 0.0f32;
            for k in 0..4 {
                let hz = pad_hz[k];
                let ph = &mut pad_ph[k];
                ph[0] += hz / SRF;
                ph[1] += hz * 1.0045 / SRF;
                ph[2] += hz * 0.9955 / SRF;
                for p in ph.iter_mut() {
                    if *p >= 1.0 {
                        *p -= 1.0;
                    }
                }
                pad += sn(sine, ph[0], 1.0)
                    + 0.55 * sn(sine, ph[1], 1.0)
                    + 0.55 * sn(sine, ph[2], 1.0)
                    + 0.22 * sn(sine, ph[0], 2.0)
                    + 0.08 * sn(sine, ph[0], 3.0);
            }
            pad *= 0.040 * swell * trem;

            bass_ph += bass_hz / SRF;
            if bass_ph >= 1.0 {
                bass_ph -= 1.0;
            }
            bass_env *= bass_decay;
            let bass = (sn(sine, bass_ph, 1.0)
                + 0.45 * sn(sine, bass_ph, 2.0) * bass_env
                + 0.18 * sn(sine, bass_ph, 3.0) * bass_env)
                * bass_env
                * 0.26;

            arp_ph += arp_hz / SRF;
            if arp_ph >= 1.0 {
                arp_ph -= 1.0;
            }
            arp_env *= arp_decay;
            let arp = (sn(sine, arp_ph, 1.0) + 0.35 * sn(sine, arp_ph, 3.0) * arp_env)
                * arp_env
                * 0.075
                * arp_gain;

            kick_env *= kick_decay;
            kick_sweep *= kick_sweep_decay;
            let kick_hz = 44.0 + 96.0 * kick_sweep;
            kick_ph += kick_hz / SRF;
            if kick_ph >= 1.0 {
                kick_ph -= 1.0;
            }
            let kick = sn(sine, kick_ph, 1.0) * kick_env * 0.58;

            let n = noise.next();
            let hp = (n - last_noise) * 0.5;
            last_noise = n;
            snare_env *= snare_decay;
            snare_tone *= snare_tone_decay;
            snare_ph += 185.0 / SRF;
            if snare_ph >= 1.0 {
                snare_ph -= 1.0;
            }
            let snare = n * snare_env * 0.22 + sn(sine, snare_ph, 1.0) * snare_tone * 0.18;
            hat_env *= hat_rate;
            let hat = hp * hat_env * 0.30;

            let mut mix = pad + bass + arp + kick + snare + hat;
            // Soft knee above 0.8 so a kick under a chord rounds off rather
            // than cracks.
            let mag = if mix < 0.0 { -mix } else { mix };
            if mag > 0.8 {
                let over = mag - 0.8;
                let soft = 0.8 + over / (1.0 + over * 4.0);
                mix = if mix < 0.0 { -soft } else { soft };
            }
            if let Some(slot) = out.get_mut(step_start + i) {
                *slot = (mix * 32_000.0) as i16;
            }
        }
        write += step_len;
    }
    out
}

// ---- the spectrum ---------------------------------------------------------------

struct Analyser {
    hann: Vec<f32>,
    coeff: [f32; BANDS],
    /// Scratch for the windowed samples of one analysis frame.
    frame: Vec<f32>,
}

impl Analyser {
    fn new() -> Self {
        let mut hann = vec![0.0f32; WINDOW];
        for i in 0..WINDOW {
            if let Some(slot) = hann.get_mut(i) {
                *slot = 0.5 - 0.5 * libm::cosf(core::f32::consts::TAU * i as f32 / WINDOW as f32);
            }
        }
        let mut coeff = [0.0f32; BANDS];
        for b in 0..BANDS {
            let hz = band_hz(b);
            coeff[b] = 2.0 * libm::cosf(core::f32::consts::TAU * hz / SRF);
        }
        Analyser {
            hann,
            coeff,
            frame: vec![0.0f32; WINDOW],
        }
    }

    /// Band levels, 0..1, for the window of the song ending at `end` (a
    /// sample index into the loop, wrapped).
    fn levels(&mut self, song: &[i16], end: usize, out: &mut [f32; BANDS]) {
        let len = song.len().max(1);
        let start = (end + len - WINDOW % len) % len;
        for i in 0..WINDOW {
            let s = song.get((start + i) % len).copied().unwrap_or(0) as f32 / 32_768.0;
            let w = self.hann.get(i).copied().unwrap_or(0.0);
            if let Some(slot) = self.frame.get_mut(i) {
                *slot = s * w;
            }
        }
        for b in 0..BANDS {
            let c = self.coeff[b];
            let mut s1 = 0.0f32;
            let mut s2 = 0.0f32;
            for x in self.frame.iter() {
                let s0 = *x + c * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            let power = s1 * s1 + s2 * s2 - c * s1 * s2;
            // A full-scale sine under a Hann window peaks at N/4.
            let mag = libm::sqrtf(power.max(0.0)) / (WINDOW as f32 * 0.25);
            let db = 20.0 * libm::log10f(mag + 1.0e-5);
            // -56 dB is the floor, 0 dB the top; a tilt lifts the highs the
            // way every player's analyser does, so hats register.
            let f = b as f32 / (BANDS - 1) as f32;
            let tilt = 0.30 * f * libm::sqrtf(f);
            out[b] = ((db + 56.0) / 56.0 + tilt).clamp(0.0, 1.0);
        }
    }
}

fn band_hz(b: usize) -> f32 {
    F_LO * libm::powf(F_HI / F_LO, b as f32 / (BANDS - 1) as f32)
}

/// The bars as drawn: each is the loudest of the last few windows, weighted
/// so a bar falls away over a third of a second, and a peak cap that holds
/// a little longer. Computed fresh from the clock every frame, so two hosts
/// at the same instant draw the same bars.
const TAPS: [(u32, f32); 6] = [
    (0, 1.0),
    (33, 0.92),
    (66, 0.84),
    (133, 0.72),
    (200, 0.60),
    (300, 0.50),
];

fn spectrum_at(
    analyser: &mut Analyser,
    song: &[i16],
    pos: u64,
    bars: &mut [f32; BANDS],
    caps: &mut [f32; BANDS],
) {
    let len = song.len().max(1) as u64;
    let mut tap = [0.0f32; BANDS];
    for b in 0..BANDS {
        bars[b] = 0.0;
        caps[b] = 0.0;
    }
    for &(ms, weight) in TAPS.iter() {
        let back = (ms as u64 * SR as u64) / 1000;
        let end = (pos + len * 64 - back) % len;
        analyser.levels(song, end as usize, &mut tap);
        for b in 0..BANDS {
            let v = tap[b];
            if v * weight > bars[b] {
                bars[b] = v * weight;
            }
            if v > caps[b] {
                caps[b] = v;
            }
        }
    }
}

/// Paint the bars into an RGBA buffer, `scale` device pixels per logical one.
fn paint_spectrum(
    buf: &mut [u8],
    w: usize,
    h: usize,
    bars: &[f32; BANDS],
    caps: &[f32; BANDS],
    live: bool,
) {
    for px in buf.iter_mut() {
        *px = 0;
    }
    let pitch = w as f32 / BANDS as f32;
    let bar_w = (pitch * 0.62).max(2.0);
    for b in 0..BANDS {
        let x0 = (b as f32 * pitch + (pitch - bar_w) * 0.5) as usize;
        let x1 = (x0 as f32 + bar_w) as usize;
        // The slot every bar lives in: a faint column that shows where a
        // silent band would be, so the row reads as an instrument and not as
        // scattered rectangles.
        for y in 0..h {
            for x in x0..x1.min(w) {
                let i = (y * w + x) * 4;
                if let Some(p) = buf.get_mut(i..i + 4) {
                    p[0] = 255;
                    p[1] = 255;
                    p[2] = 255;
                    p[3] = 10;
                }
            }
        }
        let level = if live { bars[b] } else { bars[b] * 0.35 };
        let bar_h = ((level * h as f32) as usize).max(2).min(h);
        let top = h - bar_h;
        for y in top..h {
            // Blue at the base rising to a paler blue at the tip.
            let f = 1.0 - (y - top) as f32 / bar_h.max(1) as f32;
            let r = 59.0 + (150.0 - 59.0) * f;
            let g = 117.0 + (196.0 - 117.0) * f;
            let bl = 255.0;
            for x in x0..x1.min(w) {
                let i = (y * w + x) * 4;
                if let Some(p) = buf.get_mut(i..i + 4) {
                    p[0] = r as u8;
                    p[1] = g as u8;
                    p[2] = bl as u8;
                    p[3] = 255;
                }
            }
        }
        // Peak cap: two device rows of ink hovering where the band last hit.
        // Only where it has somewhere to hover: a cap resting on the floor of
        // a silent band is a dashed line across the panel, not information.
        let cap_level = if live { caps[b] } else { caps[b] * 0.35 };
        let cap_h = ((cap_level * h as f32) as usize).min(h);
        let cap_y = h.saturating_sub(cap_h + 4);
        let cap_rows = if cap_h > bar_h + 6 && cap_level > 0.12 {
            2
        } else {
            0
        };
        for y in cap_y..(cap_y + cap_rows).min(h) {
            for x in x0..x1.min(w) {
                let i = (y * w + x) * 4;
                if let Some(p) = buf.get_mut(i..i + 4) {
                    p[0] = 230;
                    p[1] = 236;
                    p[2] = 244;
                    p[3] = 190;
                }
            }
        }
    }
}

// ---- artwork ----------------------------------------------------------------------

/// A hue in 0..1 to RGB, pastel-dark, the way record covers are printed.
fn hue_rgb(h: f32, sat: f32, val: f32) -> (f32, f32, f32) {
    let h6 = (h - libm::floorf(h)) * 6.0;
    let i = h6 as i32;
    let f = h6 - i as f32;
    let p = val * (1.0 - sat);
    let q = val * (1.0 - sat * f);
    let t = val * (1.0 - sat * (1.0 - f));
    match i {
        0 => (val, t, p),
        1 => (q, val, p),
        2 => (p, val, t),
        3 => (p, q, val),
        4 => (t, p, val),
        _ => (val, p, q),
    }
}

/// Generate one cover: a diagonal two-hue gradient, a large soft light, and a
/// geometric figure chosen by style -- rings, bands or a rising disc.
fn render_art(track: &Track, seed: u32, size: usize, buf: &mut [u8]) {
    let (h1, h2, kind) = track.art;
    let a = hue_rgb(h1, 0.62, 0.30);
    let b = hue_rgb(h2, 0.70, 0.62);
    let light = hue_rgb(h2 + 0.08, 0.35, 1.0);
    let lx = 0.3 + hash01(seed * 7 + 1) * 0.4;
    let ly = 0.25 + hash01(seed * 7 + 2) * 0.35;
    let cx = 0.5;
    let cy = 0.56;

    for y in 0..size {
        let v = y as f32 / size as f32;
        for x in 0..size {
            let u = x as f32 / size as f32;
            let g = (u * 0.6 + v * 0.4).clamp(0.0, 1.0);
            let mut r = a.0 + (b.0 - a.0) * g;
            let mut gg = a.1 + (b.1 - a.1) * g;
            let mut bb = a.2 + (b.2 - a.2) * g;

            // A soft light source, additive.
            let dx = u - lx;
            let dy = v - ly;
            let glow = 0.55 / (1.0 + (dx * dx + dy * dy) * 14.0) - 0.04;
            let glow = glow.max(0.0);
            r += light.0 * glow;
            gg += light.1 * glow;
            bb += light.2 * glow;

            // The figure.
            let ddx = u - cx;
            let ddy = v - cy;
            let d = libm::sqrtf(ddx * ddx + ddy * ddy);
            let figure = match kind {
                0 => {
                    // Concentric rings, thinning outward.
                    let ring = d * 9.0;
                    let frac = ring - libm::floorf(ring);
                    let edge = (0.5 - (frac - 0.5).abs()) * 2.0;
                    if d < 0.42 {
                        ((edge - 0.55) * 6.0).clamp(0.0, 1.0) * 0.55
                    } else {
                        0.0
                    }
                }
                1 => {
                    // Diagonal bands.
                    let s = (u - v) * 5.5 + 0.5;
                    let frac = s - libm::floorf(s);
                    if frac < 0.32 && d < 0.46 {
                        0.42
                    } else {
                        0.0
                    }
                }
                _ => {
                    // A disc sitting on a horizon.
                    let horizon = 0.62;
                    let disc = ((0.30 - d) * 40.0).clamp(0.0, 1.0);
                    if v > horizon {
                        disc * 0.15
                    } else {
                        disc * 0.75
                    }
                }
            };
            r = r * (1.0 - figure) + light.0 * figure * 0.85;
            gg = gg * (1.0 - figure) + light.1 * figure * 0.85;
            bb = bb * (1.0 - figure) + light.2 * figure * 0.85;

            // Vignette and a little grain so it reads as print, not vector.
            let vig = 1.0 - ((u - 0.5) * (u - 0.5) + (v - 0.5) * (v - 0.5)) * 0.9;
            let grain = (hash01(
                (x as u32)
                    .wrapping_mul(1973)
                    .wrapping_add((y as u32).wrapping_mul(9277))
                    .wrapping_add(seed),
            ) - 0.5)
                * 0.045;
            r = (r * vig + grain).clamp(0.0, 1.0);
            gg = (gg * vig + grain).clamp(0.0, 1.0);
            bb = (bb * vig + grain).clamp(0.0, 1.0);

            let i = (y * size + x) * 4;
            if let Some(p) = buf.get_mut(i..i + 4) {
                p[0] = (r * 255.0) as u8;
                p[1] = (gg * 255.0) as u8;
                p[2] = (bb * 255.0) as u8;
                p[3] = 255;
            }
        }
    }
}

// ---- icons ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Play,
    Pause,
    Prev,
    Next,
    Speaker,
}

const ICON_PX: usize = 64;

fn inside_tri(px: f32, py: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let s1 = (b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0);
    let s2 = (c.0 - b.0) * (py - b.1) - (c.1 - b.1) * (px - b.0);
    let s3 = (a.0 - c.0) * (py - c.1) - (a.1 - c.1) * (px - c.0);
    (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0)
}

fn inside_box(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    px >= x0 && px <= x1 && py >= y0 && py <= y1
}

/// Whether a point in the unit square is inside the icon's shape.
fn icon_hit(icon: Icon, x: f32, y: f32) -> bool {
    match icon {
        Icon::Play => inside_tri(x, y, (0.30, 0.18), (0.30, 0.82), (0.84, 0.50)),
        Icon::Pause => {
            inside_box(x, y, 0.26, 0.20, 0.44, 0.80) || inside_box(x, y, 0.56, 0.20, 0.74, 0.80)
        }
        Icon::Prev => {
            inside_box(x, y, 0.16, 0.22, 0.26, 0.78)
                || inside_tri(x, y, (0.84, 0.22), (0.84, 0.78), (0.32, 0.50))
        }
        Icon::Next => {
            inside_box(x, y, 0.74, 0.22, 0.84, 0.78)
                || inside_tri(x, y, (0.16, 0.22), (0.16, 0.78), (0.68, 0.50))
        }
        Icon::Speaker => {
            inside_box(x, y, 0.14, 0.40, 0.32, 0.60)
                || inside_tri(x, y, (0.30, 0.50), (0.56, 0.22), (0.56, 0.78))
                || {
                    // Two sound arcs, as thin rings clipped to the right half.
                    let dx = x - 0.50;
                    let dy = y - 0.50;
                    let d = libm::sqrtf(dx * dx + dy * dy);
                    x > 0.60
                        && ((d - 0.22).abs() < 0.035 || (d - 0.34).abs() < 0.035)
                        && dy.abs() < d * 0.75
                }
        }
    }
}

/// Rasterise an icon with 4x4 supersampling so the edges are smooth at any
/// size it is blitted to.
fn render_icon(icon: Icon, buf: &mut [u8]) {
    for y in 0..ICON_PX {
        for x in 0..ICON_PX {
            let mut cover = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = (x as f32 + (sx as f32 + 0.5) / 4.0) / ICON_PX as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / 4.0) / ICON_PX as f32;
                    if icon_hit(icon, px, py) {
                        cover += 1;
                    }
                }
            }
            let i = (y * ICON_PX + x) * 4;
            if let Some(p) = buf.get_mut(i..i + 4) {
                p[0] = 255;
                p[1] = 255;
                p[2] = 255;
                p[3] = (cover * 255 / 16) as u8;
            }
        }
    }
}

/// Tint a white icon buffer into `out` with the given colour.
fn tint_icon(src: &[u8], out: &mut [u8], c: gfx::Color) {
    for i in 0..(ICON_PX * ICON_PX) {
        let at = i * 4;
        if let (Some(s), Some(d)) = (src.get(at..at + 4), out.get_mut(at..at + 4)) {
            d[0] = (c.r * 255.0) as u8;
            d[1] = (c.g * 255.0) as u8;
            d[2] = (c.b * 255.0) as u8;
            d[3] = ((s[3] as f32) * c.a) as u8;
        }
    }
}

// ---- layout -------------------------------------------------------------------------

const RAIL_W: f32 = 320.0;
const TITLEBAR_H: f32 = 44.0;
const STATUS_H: f32 = 34.0;
const STATUS_Y: f32 = HEIGHT - STATUS_H;

const ROW_TOP: f32 = TITLEBAR_H + 50.0;
const ROW_H: f32 = 62.0;

const CENTRE_X: f32 = RAIL_W + (WIDTH - RAIL_W) * 0.5;
const ART: f32 = 250.0;
const ART_Y: f32 = TITLEBAR_H + 26.0;
const TITLE_Y: f32 = ART_Y + ART + 42.0;
const ARTIST_Y: f32 = TITLE_Y + 24.0;

const BAR_X: f32 = RAIL_W + 100.0;
const BAR_W: f32 = WIDTH - RAIL_W - 200.0;
const BAR_Y: f32 = ARTIST_Y + 30.0;

const TRANSPORT_Y: f32 = BAR_Y + 62.0;
const PLAY_R: f32 = 27.0;
const SKIP_R: f32 = 18.0;
const SKIP_DX: f32 = 88.0;

const VOL_X: f32 = WIDTH - 60.0 - 130.0;
const VOL_W: f32 = 130.0;

const SPEC_X: f32 = RAIL_W + 24.0;
const SPEC_W: f32 = WIDTH - RAIL_W - 48.0;
const SPEC_Y: f32 = TRANSPORT_Y + 48.0;
const SPEC_H: f32 = STATUS_Y - SPEC_Y - 20.0;
/// The bars inside the panel.
const BARS_X: f32 = SPEC_X + 20.0;
const BARS_W: f32 = SPEC_W - 40.0;
const BARS_Y: f32 = SPEC_Y + 40.0;
const BARS_H: f32 = SPEC_H - 56.0;
/// Device pixels per logical pixel in the spectrum buffer: crisp on HiDPI.
const SPEC_SCALE: usize = 2;
const SPEC_PW: usize = BARS_W as usize * SPEC_SCALE;
const SPEC_PH: usize = BARS_H as usize * SPEC_SCALE;

const ART_PX: usize = 500;
const THUMB_PX: usize = 80;

// ---- state ---------------------------------------------------------------------------

struct Player {
    track: usize,
    playing: bool,
    /// Position within the track, in samples at SR, at `anchor`.
    anchor_pos: u64,
    /// Clock reading (nanos) when `anchor_pos` was true. Only meaningful
    /// while playing.
    anchor_nanos: u64,
    volume: f32,
    song: Vec<i16>,
    /// Samples of the current track handed to the stream so far, in the
    /// track's own timeline. Streaming resumes from here.
    written: u64,
    stream: Option<u64>,
    audio_live: bool,
}

impl Player {
    fn position(&self, now: u64) -> u64 {
        if self.playing {
            let elapsed =
                now.saturating_sub(self.anchor_nanos) as u128 * SR as u128 / 1_000_000_000;
            self.anchor_pos + elapsed as u64
        } else {
            self.anchor_pos
        }
    }

    fn track_len(&self) -> u64 {
        TRACKS
            .get(self.track)
            .map(|t| t.seconds as u64 * SR as u64)
            .unwrap_or(1)
    }

    fn set_track(&mut self, index: usize, now: u64, sine: &[f32]) {
        self.track = index % TRACKS.len();
        if let Some(track) = TRACKS.get(self.track) {
            self.song = render_song(track, sine);
        }
        self.anchor_pos = 0;
        self.anchor_nanos = now;
        self.written = 0;
    }

    fn seek(&mut self, pos: u64, now: u64) {
        self.anchor_pos = pos.min(self.track_len().saturating_sub(1));
        self.anchor_nanos = now;
        self.written = self.anchor_pos;
    }

    fn set_playing(&mut self, playing: bool, now: u64) {
        if playing == self.playing {
            return;
        }
        let pos = self.position(now);
        self.anchor_pos = pos;
        self.anchor_nanos = now;
        self.playing = playing;
        if let Some(stream) = self.stream {
            if playing {
                let _ = playback::start(stream);
            } else {
                let _ = playback::stop(stream);
            }
        }
    }

    /// Advance to the next track when the current one runs out, keeping the
    /// clock continuous.
    fn tick(&mut self, now: u64, sine: &[f32]) {
        if self.playing && self.position(now) >= self.track_len() {
            let next = (self.track + 1) % TRACKS.len();
            self.set_track(next, now, sine);
        }
    }

    /// Keep the stream filled up to LEAD ahead of the play head, from the same
    /// buffer the spectrum reads. Backpressure from the host is honoured by
    /// stopping when it accepts less than offered.
    fn stream_audio(&mut self, now: u64, chunk: &mut Vec<u8>) {
        let Some(stream) = self.stream else { return };
        if !self.playing || self.song.is_empty() {
            return;
        }
        let pos = self.position(now);
        // Fell behind (a stalled frame): skip ahead rather than play old audio.
        if self.written < pos {
            self.written = pos;
        }
        let target = pos + LEAD_SAMPLES;
        let len = self.song.len() as u64;
        let gain = self.volume * self.volume;
        while self.written < target {
            let at = (self.written % len) as usize;
            let want = ((target - self.written) as usize)
                .min(self.song.len() - at)
                .min(4096);
            chunk.clear();
            for i in 0..want {
                let s = self.song.get(at + i).copied().unwrap_or(0) as f32 * gain;
                let v = (s as i32).clamp(-32_768, 32_767) as i16;
                chunk.extend_from_slice(&v.to_le_bytes());
            }
            match playback::write(stream, chunk) {
                Ok(accepted) => {
                    let frames = (accepted / 2) as u64;
                    self.written += frames;
                    if frames < want as u64 {
                        break;
                    }
                }
                Err(_) => {
                    self.audio_live = false;
                    break;
                }
            }
        }
    }
}

struct Icons {
    play: Vec<u8>,
    pause: Vec<u8>,
    prev: Vec<u8>,
    next: Vec<u8>,
    speaker: Vec<u8>,
    scratch: Vec<u8>,
}

impl Icons {
    fn new() -> Self {
        let n = ICON_PX * ICON_PX * 4;
        let mut icons = Icons {
            play: vec![0u8; n],
            pause: vec![0u8; n],
            prev: vec![0u8; n],
            next: vec![0u8; n],
            speaker: vec![0u8; n],
            scratch: vec![0u8; n],
        };
        render_icon(Icon::Play, &mut icons.play);
        render_icon(Icon::Pause, &mut icons.pause);
        render_icon(Icon::Prev, &mut icons.prev);
        render_icon(Icon::Next, &mut icons.next);
        render_icon(Icon::Speaker, &mut icons.speaker);
        icons
    }

    fn draw(&mut self, canvas: u64, icon: Icon, cx: f32, cy: f32, size: f32, c: gfx::Color) {
        let src = match icon {
            Icon::Play => &self.play,
            Icon::Pause => &self.pause,
            Icon::Prev => &self.prev,
            Icon::Next => &self.next,
            Icon::Speaker => &self.speaker,
        };
        tint_icon(src, &mut self.scratch, c);
        let _ = canvas2d::draw_pixels(
            canvas,
            gfx::Rect {
                x: cx - size * 0.5,
                y: cy - size * 0.5,
                width: size,
                height: size,
            },
            ICON_PX as u32,
            ICON_PX as u32,
            &self.scratch,
        );
    }
}

struct Frame {
    spectrum: Vec<u8>,
    art: Vec<u8>,
    art_track: usize,
    thumbs: Vec<Vec<u8>>,
    bars: [f32; BANDS],
    caps: [f32; BANDS],
    chunk: Vec<u8>,
}

// ---- drawing -------------------------------------------------------------------------

fn draw(
    canvas: u64,
    player: &Player,
    frame: &mut Frame,
    icons: &mut Icons,
    now: u64,
    t: f32,
) -> Result<(), gfx::GfxError> {
    fill(canvas, 0.0, 0.0, WIDTH, HEIGHT, BG);
    draw_titlebar(canvas, player);
    draw_rail(canvas, player, frame, t);
    draw_centre(canvas, player, frame, icons, now);
    draw_spectrum(canvas, player, frame);
    draw_status(canvas, player, now);
    canvas2d::present(canvas)
}

fn draw_titlebar(canvas: u64, player: &Player) {
    fill(canvas, 0.0, 0.0, WIDTH, TITLEBAR_H, RAIL);
    fill(canvas, 0.0, TITLEBAR_H - 1.0, WIDTH, 1.0, LINE);

    // The mark: a blue tile with a play wedge cut from the ground colour.
    rounded(canvas, 20.0, 13.0, 18.0, 18.0, 5.0, ACCENT);
    fill(canvas, 26.0, 18.0, 2.0, 8.0, BG);
    fill(canvas, 29.0, 20.0, 2.0, 4.0, BG);
    fill(canvas, 32.0, 21.5, 1.5, 1.0, BG);
    text(canvas, "Player", 48.0, 28.0, 15.0, INK);

    // The album chip, centred, with the live dot the way Query shows its
    // connection: the thing you must never be wrong about, in the middle.
    let chip_w = width_of(canvas, ALBUM, 12.5) + 40.0;
    let chip_x = (WIDTH - chip_w) * 0.5;
    rounded(canvas, chip_x, 10.0, chip_w, 24.0, 6.0, PANE);
    disc(
        canvas,
        chip_x + 15.0,
        22.0,
        3.5,
        if player.playing { GREEN } else { INK_QUIET },
    );
    text(canvas, ALBUM, chip_x + 26.0, 26.5, 12.5, INK_DIM);

    let fmt = "22.05 kHz \u{b7} 16-bit";
    text_right(canvas, fmt, WIDTH - 20.0, 26.5, 11.5, INK_QUIET);
}

fn draw_rail(canvas: u64, player: &Player, frame: &Frame, t: f32) {
    fill(canvas, 0.0, TITLEBAR_H, RAIL_W, HEIGHT - TITLEBAR_H, RAIL);
    fill(
        canvas,
        RAIL_W - 1.0,
        TITLEBAR_H,
        1.0,
        HEIGHT - TITLEBAR_H,
        LINE,
    );

    text(canvas, "TRACKS", 20.0, TITLEBAR_H + 34.0, 10.5, INK_QUIET);
    text_right(
        canvas,
        "8",
        RAIL_W - 20.0,
        TITLEBAR_H + 34.0,
        10.5,
        INK_QUIET,
    );

    for i in 0..TRACKS.len() {
        let Some(track) = TRACKS.get(i) else { break };
        let y = ROW_TOP + i as f32 * ROW_H;
        let selected = i == player.track;
        if selected {
            rounded(canvas, 8.0, y, RAIL_W - 20.0, ROW_H - 6.0, 8.0, SEL_WASH);
            rounded(canvas, 0.0, y + 12.0, 3.0, ROW_H - 30.0, 1.5, ACCENT);
        }

        // Thumbnail: the same generator as the cover, at a quarter of the size.
        if let Some(thumb) = frame.thumbs.get(i) {
            let _ = canvas2d::draw_pixels_round(
                canvas,
                gfx::Rect {
                    x: 20.0,
                    y: y + 8.0,
                    width: 40.0,
                    height: 40.0,
                },
                radii(6.0),
                THUMB_PX as u32,
                THUMB_PX as u32,
                thumb,
            );
        }

        text_styled(
            canvas,
            track.title,
            72.0,
            y + 24.0,
            13.5,
            if selected { INK } else { INK_DIM },
            style(if selected { 600 } else { 500 }, 0.0),
        );
        text(canvas, track.artist, 72.0, y + 42.0, 11.5, INK_QUIET);

        let mut buf = [0u8; 8];
        let dur = fmt_time(&mut buf, track.seconds);
        text_right(
            canvas,
            dur,
            RAIL_W - 20.0,
            y + 25.0,
            11.5,
            if selected { INK_DIM } else { INK_QUIET },
        );

        // The playing mark: three bars fed by the spectrum's low, mid and high
        // bands, so the rail moves with the music too.
        if selected && player.playing {
            let base = y + 46.0;
            let heights = [
                frame.bars.get(6).copied().unwrap_or(0.3),
                frame.bars.get(24).copied().unwrap_or(0.5),
                frame.bars.get(44).copied().unwrap_or(0.4),
            ];
            for k in 0..3 {
                let h = 3.0 + heights[k] * 9.0 + 1.5 * libm::sinf(t * 7.0 + k as f32);
                fill(
                    canvas,
                    RAIL_W - 42.0 + k as f32 * 5.0,
                    base - h,
                    3.0,
                    h,
                    ACCENT,
                );
            }
        }
    }

    // A queue summary at the foot of the rail.
    let total: u32 = TRACKS.iter().map(|t| t.seconds).sum();
    let mut buf = [0u8; 8];
    let total_s = fmt_time(&mut buf, total);
    let y = STATUS_Y - 18.0;
    text(canvas, "8 tracks", 20.0, y, 11.0, INK_QUIET);
    text_right(canvas, total_s, RAIL_W - 20.0, y, 11.0, INK_QUIET);
}

fn draw_centre(canvas: u64, player: &Player, frame: &mut Frame, icons: &mut Icons, now: u64) {
    let Some(track) = TRACKS.get(player.track) else {
        return;
    };

    // The cover, regenerated only when the track changes.
    if frame.art_track != player.track {
        render_art(track, player.track as u32 + 11, ART_PX, &mut frame.art);
        frame.art_track = player.track;
    }
    let art_x = CENTRE_X - ART * 0.5;
    let _ = canvas2d::drop_shadow_round_rect(
        canvas,
        gfx::Rect {
            x: art_x,
            y: ART_Y + 10.0,
            width: ART,
            height: ART,
        },
        radii(14.0),
        28.0,
        rgba(0.0, 0.0, 0.0, 0.55),
    );
    let _ = canvas2d::draw_pixels_round(
        canvas,
        gfx::Rect {
            x: art_x,
            y: ART_Y,
            width: ART,
            height: ART,
        },
        radii(14.0),
        ART_PX as u32,
        ART_PX as u32,
        &frame.art,
    );

    text_centred(
        canvas,
        track.title,
        CENTRE_X,
        TITLE_Y,
        22.0,
        INK,
        style(600, -0.3),
    );
    let by = style(400, 0.0);
    let artist_w = width_styled(canvas, track.artist, 13.5, by);
    let sep = "  \u{b7}  ";
    let sep_w = width_styled(canvas, sep, 13.5, by);
    let album_w = width_styled(canvas, ALBUM, 13.5, by);
    let start = CENTRE_X - (artist_w + sep_w + album_w) * 0.5;
    text_styled(canvas, track.artist, start, ARTIST_Y, 13.5, INK_DIM, by);
    text_styled(canvas, sep, start + artist_w, ARTIST_Y, 13.5, INK_QUIET, by);
    text_styled(
        canvas,
        ALBUM,
        start + artist_w + sep_w,
        ARTIST_Y,
        13.5,
        INK_QUIET,
        by,
    );

    // Progress.
    let pos = player.position(now).min(player.track_len());
    let frac = pos as f32 / player.track_len().max(1) as f32;
    rounded(canvas, BAR_X, BAR_Y, BAR_W, 4.0, 2.0, LINE);
    rounded(
        canvas,
        BAR_X,
        BAR_Y,
        (BAR_W * frac).max(4.0),
        4.0,
        2.0,
        ACCENT,
    );
    disc(canvas, BAR_X + BAR_W * frac, BAR_Y + 2.0, 6.0, INK);

    let elapsed_s = (pos / SR as u64) as u32;
    let remaining_s = track.seconds.saturating_sub(elapsed_s);
    let mut b1 = [0u8; 8];
    let mut b2 = [0u8; 8];
    let elapsed = fmt_time(&mut b1, elapsed_s);
    let remaining = fmt_time(&mut b2, remaining_s);
    text(canvas, elapsed, BAR_X, BAR_Y + 24.0, 11.5, INK_QUIET);
    let rem_w = width_of(canvas, remaining, 11.5);
    let dash_w = width_of(canvas, "-", 11.5);
    text(
        canvas,
        "-",
        BAR_X + BAR_W - rem_w - dash_w,
        BAR_Y + 24.0,
        11.5,
        INK_QUIET,
    );
    text(
        canvas,
        remaining,
        BAR_X + BAR_W - rem_w,
        BAR_Y + 24.0,
        11.5,
        INK_QUIET,
    );

    // Transport.
    disc(canvas, CENTRE_X - SKIP_DX, TRANSPORT_Y, SKIP_R, PANE);
    icons.draw(
        canvas,
        Icon::Prev,
        CENTRE_X - SKIP_DX,
        TRANSPORT_Y,
        22.0,
        INK_DIM,
    );
    disc(canvas, CENTRE_X + SKIP_DX, TRANSPORT_Y, SKIP_R, PANE);
    icons.draw(
        canvas,
        Icon::Next,
        CENTRE_X + SKIP_DX,
        TRANSPORT_Y,
        22.0,
        INK_DIM,
    );

    let _ = canvas2d::radial_gradient(
        canvas,
        gfx::Point {
            x: CENTRE_X,
            y: TRANSPORT_Y + 4.0,
        },
        PLAY_R * 1.9,
        rgba(0.231, 0.459, 1.0, 0.28),
        rgba(0.231, 0.459, 1.0, 0.0),
    );
    disc(canvas, CENTRE_X, TRANSPORT_Y, PLAY_R, ACCENT);
    if player.playing {
        icons.draw(canvas, Icon::Pause, CENTRE_X, TRANSPORT_Y, 30.0, INK);
    } else {
        icons.draw(canvas, Icon::Play, CENTRE_X + 2.0, TRANSPORT_Y, 30.0, INK);
    }

    // Track counter on the left, volume on the right, balancing the row.
    let mut nb = [0u8; 12];
    let n = fmt_int(&mut nb, player.track as u32 + 1);
    let label_x = RAIL_W + 60.0;
    text(canvas, "TRACK", label_x, TRANSPORT_Y + 4.0, 10.5, INK_QUIET);
    let w = width_of(canvas, "TRACK ", 10.5);
    text_styled(
        canvas,
        n,
        label_x + w,
        TRANSPORT_Y + 4.0,
        12.0,
        INK_DIM,
        style(600, 0.0),
    );
    let nw = width_styled(canvas, n, 12.0, style(600, 0.0));
    text(
        canvas,
        " OF 8",
        label_x + w + nw,
        TRANSPORT_Y + 4.0,
        10.5,
        INK_QUIET,
    );

    icons.draw(
        canvas,
        Icon::Speaker,
        VOL_X - 20.0,
        TRANSPORT_Y,
        18.0,
        INK_QUIET,
    );
    rounded(canvas, VOL_X, TRANSPORT_Y - 2.0, VOL_W, 4.0, 2.0, LINE);
    rounded(
        canvas,
        VOL_X,
        TRANSPORT_Y - 2.0,
        (VOL_W * player.volume).max(4.0),
        4.0,
        2.0,
        INK_DIM,
    );
    disc(canvas, VOL_X + VOL_W * player.volume, TRANSPORT_Y, 5.0, INK);
}

fn draw_spectrum(canvas: u64, player: &Player, frame: &mut Frame) {
    rounded(canvas, SPEC_X, SPEC_Y, SPEC_W, SPEC_H, 10.0, PANE);
    let _ = canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect {
            x: SPEC_X,
            y: SPEC_Y,
            width: SPEC_W,
            height: SPEC_H,
        },
        radii(10.0),
        1.0,
        LINE_SOFT,
    );
    text(
        canvas,
        "SPECTRUM",
        SPEC_X + 20.0,
        SPEC_Y + 24.0,
        10.5,
        INK_QUIET,
    );
    text_right(
        canvas,
        "56 bands \u{b7} 40 Hz \u{2013} 8 kHz",
        SPEC_X + SPEC_W - 20.0,
        SPEC_Y + 24.0,
        10.5,
        INK_QUIET,
    );

    paint_spectrum(
        &mut frame.spectrum,
        SPEC_PW,
        SPEC_PH,
        &frame.bars,
        &frame.caps,
        player.playing,
    );
    let _ = canvas2d::draw_pixels(
        canvas,
        gfx::Rect {
            x: BARS_X,
            y: BARS_Y,
            width: BARS_W,
            height: BARS_H,
        },
        SPEC_PW as u32,
        SPEC_PH as u32,
        &frame.spectrum,
    );
}

fn draw_status(canvas: u64, player: &Player, now: u64) {
    fill(canvas, 0.0, STATUS_Y, WIDTH, STATUS_H, RAIL);
    fill(canvas, 0.0, STATUS_Y, WIDTH, 1.0, LINE);
    let base = STATUS_Y + 22.0;
    let mut pen = 20.0;

    let state = if player.playing { "playing" } else { "paused" };
    text(canvas, state, pen, base, 11.5, INK_DIM);
    pen += width_of(canvas, state, 11.5);
    let sep = "  \u{b7}  ";
    text(canvas, sep, pen, base, 11.5, INK_QUIET);
    pen += width_of(canvas, sep, 11.5);
    if let Some(track) = TRACKS.get(player.track) {
        text(canvas, track.title, pen, base, 11.5, INK_QUIET);
        pen += width_of(canvas, track.title, 11.5);
    }
    text(canvas, sep, pen, base, 11.5, INK_QUIET);
    pen += width_of(canvas, sep, 11.5);
    // Honest about where the sound is going. A host that withheld the
    // speakers still gets the spectrum, and the strip says which it is.
    let audio = if player.audio_live {
        "audio: live"
    } else {
        "audio: unavailable \u{b7} spectrum from the synthesised buffer"
    };
    text(
        canvas,
        audio,
        pen,
        base,
        11.5,
        if player.audio_live {
            INK_QUIET
        } else {
            INK_DIM
        },
    );

    let pos = player.position(now);
    let mut buf = [0u8; 12];
    let bpm = TRACKS.get(player.track).map(|t| t.bpm as u32).unwrap_or(0);
    let bpm_s = fmt_int(&mut buf, bpm);
    let mut pen_r = WIDTH - 20.0;
    let tail = " bpm \u{b7} mono \u{b7} PCM";
    pen_r -= width_of(canvas, tail, 11.5);
    text(canvas, tail, pen_r, base, 11.5, INK_QUIET);
    pen_r -= width_of(canvas, bpm_s, 11.5);
    text(canvas, bpm_s, pen_r, base, 11.5, INK_DIM);
    let _ = pos;
}

// ---- hit testing --------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Track(usize),
    Play,
    Prev,
    Next,
    Seek(f32),
    Volume(f32),
    None,
}

fn hit_test(x: f32, y: f32) -> Hit {
    if x < RAIL_W {
        if y >= ROW_TOP {
            let i = ((y - ROW_TOP) / ROW_H) as usize;
            if i < TRACKS.len() && x >= 8.0 && x <= RAIL_W - 12.0 {
                return Hit::Track(i);
            }
        }
        return Hit::None;
    }
    let within = |cx: f32, cy: f32, r: f32| (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r;
    if within(CENTRE_X, TRANSPORT_Y, PLAY_R + 6.0) {
        return Hit::Play;
    }
    if within(CENTRE_X - SKIP_DX, TRANSPORT_Y, SKIP_R + 6.0) {
        return Hit::Prev;
    }
    if within(CENTRE_X + SKIP_DX, TRANSPORT_Y, SKIP_R + 6.0) {
        return Hit::Next;
    }
    if y >= BAR_Y - 12.0 && y <= BAR_Y + 16.0 && x >= BAR_X - 8.0 && x <= BAR_X + BAR_W + 8.0 {
        return Hit::Seek(((x - BAR_X) / BAR_W).clamp(0.0, 1.0));
    }
    if y >= TRANSPORT_Y - 14.0
        && y <= TRANSPORT_Y + 14.0
        && x >= VOL_X - 8.0
        && x <= VOL_X + VOL_W + 8.0
    {
        return Hit::Volume(((x - VOL_X) / VOL_W).clamp(0.0, 1.0));
    }
    Hit::None
}

fn apply(hit: Hit, player: &mut Player, now: u64, sine: &[f32]) {
    match hit {
        Hit::Track(i) => {
            player.set_track(i, now, sine);
            player.set_playing(true, now);
        }
        Hit::Play => {
            let playing = player.playing;
            player.set_playing(!playing, now);
        }
        Hit::Prev => {
            // Back to the start if the track is under way, else the previous one.
            if player.position(now) > SR as u64 * 3 {
                player.seek(0, now);
            } else {
                let prev = (player.track + TRACKS.len() - 1) % TRACKS.len();
                player.set_track(prev, now, sine);
            }
        }
        Hit::Next => {
            let next = (player.track + 1) % TRACKS.len();
            player.set_track(next, now, sine);
        }
        Hit::Seek(f) => {
            let len = player.track_len();
            player.seek((len as f32 * f) as u64, now);
        }
        Hit::Volume(v) => player.volume = v,
        Hit::None => {}
    }
}

// ---- widget scaffolding ------------------------------------------------------------

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
            grow: 1.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ---- app --------------------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|b| *b == b'\n')
            .any(|arg| arg == b"quick");

        // The speakers. Permission withheld is a refusal (exit 5); any other
        // failure -- no device, a host without audio -- means the player runs
        // silent and says so on its face.
        let mut audio_live = false;
        let stream = match playback::open(audio::StreamConfig {
            sample_rate: SR,
            channels: 1,
            format: audio::SampleFormat::PcmS16,
            buffer_frames: 1024,
        }) {
            Ok(stream) => {
                audio_live = playback::start(stream).is_ok();
                Some(stream)
            }
            Err(audio::AudioError::PermissionDenied) => {
                out_kv("refused", "audio.playback");
                return 5;
            }
            Err(_) => None,
        };

        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Player", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(
                win,
                &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
            )
            .is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            let _ = window::close(win);
            return 33;
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let sine = build_sine();
        let mut analyser = Analyser::new();
        let mut icons = Icons::new();

        let mut thumbs = Vec::new();
        for i in 0..TRACKS.len() {
            let mut thumb = vec![0u8; THUMB_PX * THUMB_PX * 4];
            if let Some(track) = TRACKS.get(i) {
                render_art(track, i as u32 + 11, THUMB_PX, &mut thumb);
            }
            thumbs.push(thumb);
        }
        let mut frame = Frame {
            spectrum: vec![0u8; SPEC_PW * SPEC_PH * 4],
            art: vec![0u8; ART_PX * ART_PX * 4],
            art_track: usize::MAX,
            thumbs,
            bars: [0.0; BANDS],
            caps: [0.0; BANDS],
            chunk: Vec::new(),
        };

        let started = clock::monotonic_nanos();
        let mut player = Player {
            track: 0,
            playing: false,
            anchor_pos: 0,
            anchor_nanos: started,
            volume: 0.72,
            song: Vec::new(),
            written: 0,
            stream,
            audio_live,
        };
        player.set_track(0, started, &sine);
        // Pick up where a listener would have left off: a minute and a half
        // into the opener, so the progress bar has something to show.
        player.seek(SR as u64 * 84, started);
        player.set_playing(true, started);

        const FRAME_NANOS: u64 = 1_000_000_000 / 60;
        let mut next_frame = clock::monotonic_nanos();
        let mut frames: u32 = 0;
        let mut quick_actions = 0u32;

        loop {
            let now = if quick {
                started + frames as u64 * 33_333_333
            } else {
                clock::monotonic_nanos()
            };
            let t = now.saturating_sub(started) as f32 / 1_000_000_000.0;

            player.tick(now, &sine);
            player.stream_audio(now, &mut frame.chunk);
            let pos = player.position(now);
            let song_pos = pos % player.song.len().max(1) as u64;
            spectrum_at(
                &mut analyser,
                &player.song,
                song_pos,
                &mut frame.bars,
                &mut frame.caps,
            );

            if draw(canvas, &player, &mut frame, &mut icons, now, t).is_err() {
                out_kv("draw", "failed");
                let _ = window::close(win);
                return 1;
            }
            frames += 1;

            if quick {
                // Operate the controls the way a person would, at fixed
                // moments: skip, pause, resume, seek.
                match frames {
                    2 => {
                        apply(Hit::Next, &mut player, now, &sine);
                        quick_actions += 1;
                    }
                    4 => {
                        apply(Hit::Play, &mut player, now, &sine);
                        quick_actions += 1;
                    }
                    5 => {
                        apply(Hit::Play, &mut player, now, &sine);
                        quick_actions += 1;
                    }
                    6 => {
                        apply(Hit::Seek(0.5), &mut player, now, &sine);
                        quick_actions += 1;
                    }
                    _ => {}
                }
                let _ = events::poll();
                if frames >= QUICK_FRAMES {
                    break;
                }
                continue;
            }

            next_frame = next_frame.saturating_add(FRAME_NANOS);
            let after_draw = clock::monotonic_nanos();
            if next_frame < after_draw {
                next_frame = after_draw;
            }

            let mut closing = false;
            loop {
                let now = clock::monotonic_nanos();
                let remaining = next_frame.saturating_sub(now);
                // Paused: nothing moves, so sleep long and redraw on input.
                if remaining == 0 && player.playing {
                    break;
                }
                let millis = if player.playing {
                    (remaining / 1_000_000) as u32
                } else {
                    250
                };
                match events::wait(Some(millis.max(1))) {
                    Some(types::Event::CloseRequested(_)) => {
                        closing = true;
                        break;
                    }
                    Some(types::Event::Pointer(p)) if p.pressed => {
                        apply(
                            hit_test(p.x, p.y),
                            &mut player,
                            clock::monotonic_nanos(),
                            &sine,
                        );
                        break;
                    }
                    Some(types::Event::Key(k)) if k.pressed => {
                        let name = k.key.as_str();
                        let now = clock::monotonic_nanos();
                        match name {
                            " " | "Space" => apply(Hit::Play, &mut player, now, &sine),
                            "ArrowRight" => apply(Hit::Next, &mut player, now, &sine),
                            "ArrowLeft" => apply(Hit::Prev, &mut player, now, &sine),
                            _ => {}
                        }
                        break;
                    }
                    Some(types::Event::Resized(_)) => break,
                    Some(_) => {}
                    None => {
                        if !player.playing {
                            break;
                        }
                    }
                }
            }
            if closing {
                break;
            }
        }

        if let Some(stream) = player.stream {
            let _ = playback::stop(stream);
        }
        let _ = window::close(win);

        // The position, read from the player itself at the moment of printing.
        let now = if quick {
            started + frames as u64 * 33_333_333
        } else {
            clock::monotonic_nanos()
        };
        out_num("tracks", TRACKS.len() as u32);
        out_num("track", player.track as u32 + 1);
        out_kv(
            "title",
            TRACKS.get(player.track).map(|t| t.title).unwrap_or(""),
        );
        out_kv("playing", if player.playing { "true" } else { "false" });
        out_num("position_s", (player.position(now) / SR as u64) as u32);
        out_num("loop_samples", player.song.len() as u32);
        out_num("bands", BANDS as u32);
        let mut peak = 0.0f32;
        for b in frame.bars.iter() {
            if *b > peak {
                peak = *b;
            }
        }
        out_num("peak_percent", (peak * 100.0) as u32);
        out_num("frames", frames);
        out_num("actions", quick_actions);
        out_kv(
            "audio",
            if player.audio_live {
                "live"
            } else {
                "unavailable"
            },
        );
        0
    }
}

bindings::export!(Component with_types_in bindings);
