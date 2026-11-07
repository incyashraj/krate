//! How one frame lands on one screen: the smallest text a person has to
//! read, and the text that runs off the side of the screen (IC-911).
//!
//! The fit rule in `usability` reads what an app declares -- a fixed design
//! size -- and so passes an app that asks its real size and then draws a
//! desktop layout into a phone anyway. On 2026-10-10 the hub's Music app,
//! rendered at 390x844, kept its 1280-wide sidebar and cut its cards off at
//! the right; Markdown Editor and Chess were cut off at both sides. This
//! reads the frame the app actually drew on that screen, in the app's own
//! numbers (the recorded draw list), so it judges the layout and not the
//! declaration.
//!
//! Vertical overflow is not a fault: content below the screen is what
//! scrolling is for. Text crossing the left or right edge of the screen is,
//! because nothing in an app scrolls sideways by accident.

use krate_adapter_common::canvas_list::CanvasOp;
use serde::{Deserialize, Serialize};

/// The host's text measurement: a string and a size in, a width out.
type Measure<'a> = dyn Fn(&str, f32) -> f32 + 'a;

/// What one frame looks like on one screen.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScreenFacts {
    /// The screen, in points.
    pub screen: (f32, f32),
    /// How much the app's drawing is scaled to reach the screen: 1 for an
    /// app that lays out from its real size, less for a fixed design shrunk
    /// to fit.
    pub scale: f32,
    /// The smallest readable text on the screen, in points as it lands, and
    /// the text itself.
    pub smallest_text: Option<(f32, String)>,
    /// Text cut at the left or right edge of what is visible (the screen, or
    /// the app's own clip): the text, and how many points of it are cut.
    pub off_edge: Vec<(String, f32)>,
    /// How many strings were drawn on the screen.
    pub texts: usize,
    /// Visible text drawn over other visible text: the two strings. A
    /// sidebar's button drawn across its own list did this on a turned phone
    /// (an AI-made music player, 2026-10-10).
    #[serde(default)]
    pub overlaps: Vec<(String, String)>,
}

/// Two strings sharing at least this much of the smaller one's box are one
/// on top of the other, not neighbours whose descenders touch.
pub const MIN_OVERLAP: f32 = 0.4;

/// A copy of the same text at or under this opacity (0-255) beneath or above
/// it is a shadow or a glow, not a second label.
pub const SHADOW_ALPHA: u8 = 160;

/// Fully transparent text cannot be read, so it cannot be too small.
fn is_invisible(color: u32) -> bool {
    (color >> 24) < 8
}

/// A string worth reading: at least two letters or digits. A lone glyph is
/// usually an icon (a chevron, a dot, a "x" on a close button), sized by
/// its box rather than as text.
fn is_reading(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() >= 2
}

/// The box a shape op fills, for the ones that are content: panels, cards,
/// pictures. Strokes and circles are left out: an outline or a dot reaching
/// past the edge says less than the box it belongs to.
fn shape_rect(op: &CanvasOp) -> Option<(f32, f32, f32, f32)> {
    match op {
        CanvasOp::FillRect { rect, .. }
        | CanvasOp::FillRoundRect { rect, .. }
        | CanvasOp::LinearGradient { rect, .. }
        | CanvasOp::LinearGradientStops { rect, .. }
        | CanvasOp::Pixels { rect, .. }
        | CanvasOp::PixelsRound { rect, .. } => Some(*rect),
        _ => None,
    }
}

/// Measure one frame. `canvas` is the space the ops are drawn in (the
/// design size when the app set one); `screen` is the screen it is shown
/// on, in points.
pub fn measure(
    ops: &[CanvasOp],
    canvas: (f32, f32),
    screen: (f32, f32),
    measure: Option<&Measure<'_>>,
) -> ScreenFacts {
    let (cw, ch) = (canvas.0.max(1.0), canvas.1.max(1.0));
    let k = (screen.0 / cw).min(screen.1 / ch);
    let mut facts = ScreenFacts {
        screen,
        scale: k,
        ..ScreenFacts::default()
    };
    // What is visible is the screen, narrowed by the app's clip when one is
    // set. Anything wholly outside that band is hidden (scrolled away, or
    // the next card of a list not yet reached); anything partly inside it
    // is cut, and a person sees half a word or half a card. Music's content
    // pane, clipped to the right part of a phone, cut "Featuring Marlow..."
    // at the screen edge; a weather app laid out for 1280 points cut its
    // day cards there and hid five of them.
    let band = |clip: Option<(f32, f32, f32, f32)>| {
        let (mut bl, mut bt, mut br, mut bb) = (0.0f32, 0.0f32, cw, ch);
        if let Some((cx, cy, cwid, chgt)) = clip {
            bl = bl.max(cx);
            bt = bt.max(cy);
            br = br.min(cx + cwid);
            bb = bb.min(cy + chgt);
        }
        (bl, bt, br, bb)
    };
    let mut clip: Option<(f32, f32, f32, f32)> = None;
    // Visible strings, for naming a cut box by what it holds, and the box
    // each one takes for finding text drawn over text.
    let mut shown: Vec<((f32, f32), String)> = Vec::new();
    // Each text's visible box, the text, and its opacity (0-255).
    type TextBox = ((f32, f32, f32, f32), String, u8);
    let mut boxes: Vec<TextBox> = Vec::new();
    let mut cut_boxes: Vec<((f32, f32, f32, f32), f32)> = Vec::new();
    for op in ops {
        if let CanvasOp::SetClip(rect) = op {
            clip = *rect;
            continue;
        }
        let (bl, bt, br, bb) = band(clip);
        if let Some((x, y, w, h)) = shape_rect(op) {
            let (left, top, right, bottom) = (x, y, x + w, y + h);
            // Lines and hairlines are not content; a box that starts at the
            // visible left edge is a background reaching across, however
            // far past the right it runs.
            if w < 8.0 || h < 8.0 || left <= bl + 2.0 {
                continue;
            }
            if bottom <= bt || top >= bb || right <= bl || left >= br {
                continue;
            }
            let past = right - br;
            if past > 2.0 {
                cut_boxes.push(((left, top, right, bottom), past));
            }
            continue;
        }
        if let CanvasOp::Text {
            origin,
            font_size,
            color,
            letter_spacing,
            text,
            ..
        } = op
        {
            if is_invisible(*color) || !is_reading(text) || *font_size <= 0.0 {
                continue;
            }
            let chars = text.chars().count() as f32;
            let width = match measure {
                Some(m) => m(text, *font_size) + chars * letter_spacing,
                None => chars * (font_size * 0.52 + letter_spacing),
            };
            let (left, right) = (origin.0, origin.0 + width);
            let (top, bottom) = (origin.1 - font_size * 0.72, origin.1 + font_size * 0.20);
            if bottom <= bt || top >= bb || right <= bl || left >= br {
                continue;
            }
            facts.texts += 1;
            shown.push((*origin, text.clone()));
            boxes.push((
                (left.max(bl), top.max(bt), right.min(br), bottom.min(bb)),
                text.clone(),
                (*color >> 24) as u8,
            ));
            let on_screen = font_size * k;
            if facts
                .smallest_text
                .as_ref()
                .is_none_or(|(size, _)| on_screen < *size)
            {
                facts.smallest_text = Some((on_screen, text.clone()));
            }
            let past = (bl - left).max(right - br);
            if past > 2.0 {
                facts.off_edge.push((text.clone(), past * k));
            }
        }
    }
    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
            let ((al, at, ar, ab), a, alpha_a) = &boxes[i];
            let ((bl2, bt2, br2, bb2), b, alpha_b) = &boxes[j];
            // The same words drawn twice, one copy faint, is a drop shadow:
            // a design, not one label over another. Krate Keyvault's count
            // read as "13 is drawn over 13" once it reached two digits.
            if a == b && (*alpha_a).min(*alpha_b) < SHADOW_ALPHA {
                continue;
            }
            let w = ar.min(*br2) - al.max(*bl2);
            let h = ab.min(*bb2) - at.max(*bt2);
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let smaller = ((ar - al) * (ab - at)).min((br2 - bl2) * (bb2 - bt2));
            if smaller > 0.0 && w * h / smaller >= MIN_OVERLAP {
                facts.overlaps.push((a.clone(), b.clone()));
            }
        }
    }
    for ((left, top, right, bottom), past) in cut_boxes {
        let held = shown
            .iter()
            .find(|((x, y), _)| *x >= left && *x <= right && *y >= top && *y <= bottom);
        let name = match held {
            Some((_, text)) => format!("the box holding {text}"),
            None => format!("a {:.0}x{:.0} box", (right - left) * k, (bottom - top) * k),
        };
        facts.off_edge.push((name, past * k));
    }
    facts
        .off_edge
        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
    facts
}

/// The screens an app made once is checked on. An app that lays itself
/// out from its real size must work on a phone held either way, as the
/// phone's own apps do. An app with a fixed design (a game drawn at one
/// size) is judged on the phone the way the Player turns it: landscape for
/// a wide design, upright otherwise.
pub fn screens(asked: (f32, f32), fixed: bool) -> Vec<(&'static str, f32, f32)> {
    let upright = ("a phone", 390.0, 844.0);
    let turned = ("a phone turned sideways", 844.0, 390.0);
    let mut out = match (fixed, asked.0 > asked.1) {
        (false, _) => vec![upright, turned],
        (true, true) => vec![turned],
        (true, false) => vec![upright],
    };
    out.push(("a tablet", 820.0, 1180.0));
    out.push(("a desktop window", 1280.0, 800.0));
    out
}

/// Text smaller than this on the screen cannot be read without zooming.
/// Apple's smallest text style (caption 2) is 11 points; 9 leaves room for
/// a deliberate fine print and still catches a desktop table shrunk onto a
/// phone (Bigtable's rows landed at about 4 points).
pub const MIN_TEXT_POINTS: f32 = 9.0;

/// A finding a person and an AI author can act on, or None when the frame
/// fits the screen.
pub fn fault(name: &str, facts: &ScreenFacts) -> Option<String> {
    let (w, h) = facts.screen;
    let mut found = Vec::new();
    if let Some((size, text)) = &facts.smallest_text {
        if *size < MIN_TEXT_POINTS {
            found.push(format!(
                "text lands at {size:.1} points (\"{}\"), too small to read",
                short(text)
            ));
        }
    }
    if let Some((text, past)) = facts.off_edge.first() {
        let more = facts.off_edge.len() - 1;
        found.push(format!(
            "{} is cut {past:.0} points at the side of the screen{}",
            if text.starts_with("the box holding ")
                || text.starts_with("a ") && text.ends_with(" box")
            {
                short_box(text)
            } else {
                format!("\"{}\"", short(text))
            },
            if more > 0 {
                format!(" (and {more} more)")
            } else {
                String::new()
            }
        ));
    }
    if let Some((a, b)) = facts.overlaps.first() {
        let more = facts.overlaps.len() - 1;
        found.push(format!(
            "\"{}\" is drawn over \"{}\"{}",
            short(a),
            short(b),
            if more > 0 {
                format!(" (and {more} more)")
            } else {
                String::new()
            }
        ));
    }
    if found.is_empty() {
        return None;
    }
    Some(format!("on {name} ({w:.0}x{h:.0}): {}", found.join("; ")))
}

/// A cut box's name, with the text it holds quoted and shortened.
fn short_box(name: &str) -> String {
    match name.strip_prefix("the box holding ") {
        Some(text) => format!("the box holding \"{}\"", short(text)),
        None => name.to_string(),
    }
}

fn short(text: &str) -> String {
    let t: String = text.chars().take(32).collect();
    if text.chars().count() > 32 {
        format!("{t}...")
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(x: f32, y: f32, size: f32, s: &str) -> CanvasOp {
        CanvasOp::Text {
            origin: (x, y),
            font_size: size,
            color: 0xff00_0000,
            letter_spacing: 0.0,
            weight: 400,
            italic: false,
            family: Default::default(),
            text: s.to_string(),
        }
    }

    #[test]
    fn a_desktop_sidebar_on_a_phone_runs_off_the_side() {
        // A 1280-wide layout drawn into a 390-wide canvas: the right column
        // starts past the screen.
        let ops = vec![
            text(20.0, 40.0, 15.0, "Library"),
            text(300.0, 80.0, 15.0, "Recently Played"),
        ];
        let facts = measure(&ops, (390.0, 844.0), (390.0, 844.0), None);
        assert_eq!(facts.texts, 2);
        assert_eq!(facts.off_edge.len(), 1, "{facts:?}");
        assert!(fault("a phone", &facts)
            .unwrap()
            .contains("Recently Played"));
    }

    #[test]
    fn a_fixed_desktop_table_shrunk_onto_a_phone_is_too_small() {
        // A 1280x800 design on a 390-wide phone: 13-point rows land near 4.
        let ops = vec![text(10.0, 30.0, 13.0, "account_id")];
        let facts = measure(&ops, (1280.0, 800.0), (390.0, 844.0), None);
        let (size, _) = facts.smallest_text.clone().unwrap();
        assert!(size < 4.5, "{size}");
        assert!(fault("a phone", &facts).unwrap().contains("too small"));
    }

    #[test]
    fn a_layout_that_fits_has_no_fault() {
        let ops = vec![
            text(16.0, 40.0, 17.0, "Weather"),
            text(16.0, 2000.0, 15.0, "below, scrolled"),
        ];
        let facts = measure(&ops, (390.0, 844.0), (390.0, 844.0), None);
        assert_eq!(
            facts.texts, 1,
            "text below the screen is scrolled away, not shown"
        );
        assert!(fault("a phone", &facts).is_none(), "{facts:?}");
    }

    #[test]
    fn text_cut_by_the_apps_own_pane_counts_and_text_wholly_hidden_does_not() {
        // A content pane clipped to the right part of the screen, laid out
        // for a desktop: one title is cut at its edge, the next is wholly
        // past it.
        let ops = vec![
            CanvasOp::SetClip(Some((150.0, 72.0, 240.0, 772.0))),
            text(322.0, 176.0, 12.5, "Featuring Marlow"),
            text(462.0, 176.0, 12.5, "Featuring Kamau"),
            CanvasOp::SetClip(None),
        ];
        let facts = measure(&ops, (390.0, 844.0), (390.0, 844.0), None);
        assert_eq!(facts.texts, 1, "{facts:?}");
        assert_eq!(facts.off_edge.len(), 1, "{facts:?}");
        assert_eq!(facts.off_edge[0].0, "Featuring Marlow");
    }

    #[test]
    fn a_row_of_cards_laid_out_for_a_desktop_is_cut_at_the_side() {
        // A full-width background (starts at the left edge, runs past the
        // right) is fine; the second card is cut; the third is hidden.
        let ops = vec![
            CanvasOp::FillRect {
                rect: (0.0, 0.0, 1280.0, 844.0),
                color: 0xff22_4488,
            },
            CanvasOp::FillRoundRect {
                rect: (24.0, 180.0, 140.0, 100.0),
                radii: Default::default(),
                color: 0xff33_5599,
            },
            text(36.0, 200.0, 13.0, "Mon"),
            CanvasOp::FillRoundRect {
                rect: (176.0, 180.0, 300.0, 100.0),
                radii: Default::default(),
                color: 0xff33_5599,
            },
            text(188.0, 200.0, 13.0, "Tue"),
            CanvasOp::FillRoundRect {
                rect: (500.0, 180.0, 140.0, 100.0),
                radii: Default::default(),
                color: 0xff33_5599,
            },
        ];
        let facts = measure(&ops, (390.0, 844.0), (390.0, 844.0), None);
        assert_eq!(facts.off_edge.len(), 1, "{facts:?}");
        assert_eq!(facts.off_edge[0].0, "the box holding Tue");
        let fault = fault("a phone", &facts).unwrap();
        assert!(fault.contains("the box holding \"Tue\" is cut"), "{fault}");
    }

    #[test]
    fn text_drawn_over_text_is_a_fault_and_neighbours_are_not() {
        // A sidebar button's label drawn across a list row's, and two rows
        // whose descenders brush.
        let ops = vec![
            text(20.0, 120.0, 16.0, "Choose Folder"),
            text(24.0, 122.0, 16.0, "Now Playing"),
            text(20.0, 200.0, 16.0, "Albums"),
            text(20.0, 219.0, 16.0, "Artists"),
        ];
        let facts = measure(&ops, (844.0, 390.0), (844.0, 390.0), None);
        assert_eq!(facts.overlaps.len(), 1, "{facts:?}");
        let fault = fault("a phone turned sideways", &facts).unwrap();
        assert!(
            fault.contains("\"Choose Folder\" is drawn over \"Now Playing\""),
            "{fault}"
        );
    }

    #[test]
    fn a_drop_shadow_is_not_text_drawn_over_text() {
        // Keyvault draws its count twice: a 35%-black copy 2,3 px off, then
        // the number. Two strangers in the same place are still a fault.
        let shadow = CanvasOp::Text {
            origin: (102.0, 203.0),
            font_size: 72.0,
            color: 0x5900_0000,
            letter_spacing: 0.0,
            weight: 400,
            italic: false,
            family: Default::default(),
            text: "13".to_string(),
        };
        let ops = vec![shadow, text(100.0, 200.0, 72.0, "13")];
        let facts = measure(&ops, (390.0, 844.0), (390.0, 844.0), None);
        assert!(facts.overlaps.is_empty(), "{facts:?}");
        let ops = vec![
            text(100.0, 200.0, 72.0, "13"),
            text(102.0, 203.0, 72.0, "13"),
        ];
        let facts = measure(&ops, (390.0, 844.0), (390.0, 844.0), None);
        assert_eq!(
            facts.overlaps.len(),
            1,
            "two solid copies still overlap: {facts:?}"
        );
    }

    #[test]
    fn text_clipped_away_cannot_overlap() {
        let ops = vec![
            text(20.0, 120.0, 16.0, "Visible row"),
            CanvasOp::SetClip(Some((0.0, 200.0, 390.0, 300.0))),
            text(20.0, 121.0, 16.0, "Scrolled away"),
            CanvasOp::SetClip(None),
        ];
        let facts = measure(&ops, (390.0, 844.0), (390.0, 844.0), None);
        assert!(facts.overlaps.is_empty(), "{facts:?}");
    }

    #[test]
    fn icons_and_hidden_text_are_not_judged_as_reading() {
        let ops = vec![text(10.0, 20.0, 6.0, "x"), {
            let mut hidden = text(10.0, 40.0, 5.0, "faded out");
            if let CanvasOp::Text { color, .. } = &mut hidden {
                *color = 0;
            }
            hidden
        }];
        let facts = measure(&ops, (390.0, 844.0), (390.0, 844.0), None);
        assert_eq!(facts.texts, 0);
        assert!(facts.smallest_text.is_none());
    }

    #[test]
    fn an_adaptive_app_is_judged_on_a_phone_held_both_ways() {
        let both = screens((1280.0, 800.0), false);
        assert_eq!(both[0], ("a phone", 390.0, 844.0));
        assert_eq!(both[1], ("a phone turned sideways", 844.0, 390.0));
        assert_eq!(both.len(), 4);
    }

    #[test]
    fn a_fixed_design_is_judged_the_way_the_player_turns_it() {
        assert_eq!(
            screens((768.0, 720.0), true)[0],
            ("a phone turned sideways", 844.0, 390.0)
        );
        assert_eq!(screens((480.0, 640.0), true)[0], ("a phone", 390.0, 844.0));
        assert_eq!(screens((768.0, 720.0), true).len(), 3);
    }
}
