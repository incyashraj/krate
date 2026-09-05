//! Finds text drawn on top of other text.
//!
//! Two generated games shipped with a collision that a person spots in the
//! first second: a tic tac toe drew its "New round" button across the word
//! "Draws", and a memory game printed its hint over the bottom row of cards.
//! Both passed every stage of `check-app`, because nothing looked at where
//! the app put things -- only that it built, imported cleanly, and painted.
//!
//! This reads the recorded draw list, so the geometry is the app's own
//! numbers rather than a guess made from pixels. Anti-aliasing, subpixel
//! coverage and colour blending never enter into it.
//!
//! **Only text against text counts.** Text over a panel, a card or a
//! gradient is ordinary design -- that is what backgrounds are for, and
//! flagging it would bury the real defect in noise. Two strings sharing
//! pixels is never intentional, so it is the one pair worth reporting.

use krate_adapter_common::canvas_list::CanvasOp;

/// The host's text measurement: a string and a font size in, a width out.
///
/// Named rather than written inline because clippy is right that the bare
/// `Option<&dyn Fn(&str, f32) -> f32>` is hard to read at a call site, and
/// because there is only ever one thing this can be: the same measurement
/// the app itself used to lay the string out.
type Measure<'a> = dyn Fn(&str, f32) -> f32 + 'a;

/// A box in canvas coordinates: left, top, right, bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box2 {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Box2 {
    fn intersect(&self, other: &Box2) -> Option<Box2> {
        let left = self.left.max(other.left);
        let top = self.top.max(other.top);
        let right = self.right.min(other.right);
        let bottom = self.bottom.min(other.bottom);
        if right > left && bottom > top {
            Some(Box2 {
                left,
                top,
                right,
                bottom,
            })
        } else {
            None
        }
    }

    fn area(&self) -> f32 {
        (self.right - self.left).max(0.0) * (self.bottom - self.top).max(0.0)
    }
}

/// One collision, described the way the report will phrase it.
#[derive(Clone, Debug)]
pub struct Collision {
    pub first: String,
    pub second: String,
    /// Share of the smaller string's box that the two have in common, 0..1.
    pub overlap_fraction: f32,
    pub region: Box2,
}

/// Anything below this share of the smaller box is treated as two strings
/// sitting close together rather than one on top of the other. Descenders
/// and letter spacing routinely put neighbouring lines a pixel or two into
/// each other, and calling that a defect would be wrong.
const MIN_OVERLAP: f32 = 0.18;

/// Fully transparent text is a legitimate way to hide a string -- a fade,
/// a disabled state -- and it cannot obscure anything.
fn is_invisible(color: u32) -> bool {
    (color >> 24) < 8
}

/// The box a string occupies.
///
/// `origin` is a baseline, not a corner. Ascent and descent are taken as
/// fractions of the font size rather than measured through the font, which
/// keeps this free of a font handle.
///
/// The WIDTH has to be measured, though. It used to be `chars * 0.52em`, an
/// average advance, and that is roughly twice too wide for punctuation: a
/// comma is about 0.26em and a colon 0.28em. So every `"value",` in a JSON
/// view reported the comma sitting on top of the string before it, at "100%
/// of the smaller one" -- Probe and Query were both flagged for a dozen of
/// these while their pixels were perfect. An estimator that cannot tell a
/// comma from an `m` cannot be used to accuse an app of overlapping text.
///
/// `measure` is the host's own text measurement, the same one the app calls
/// to lay the string out. When it is unavailable the average is used, which
/// keeps this usable in tests and off the GUI path.
fn text_box(
    origin: (f32, f32),
    font_size: f32,
    letter_spacing: f32,
    text: &str,
    measure: Option<&Measure<'_>>,
) -> Option<Box2> {
    let chars = text.chars().count();
    if chars == 0 || font_size <= 0.0 {
        return None;
    }
    let width = match measure {
        Some(m) => m(text, font_size) + chars as f32 * letter_spacing,
        None => chars as f32 * (font_size * 0.52 + letter_spacing),
    };
    let ascent = font_size * 0.72;
    let descent = font_size * 0.20;
    Some(Box2 {
        left: origin.0,
        top: origin.1 - ascent,
        right: origin.0 + width,
        bottom: origin.1 + descent,
    })
}

/// Every pair of strings in one frame that share enough space to read as a
/// mistake, worst first.
///
/// Uses the average-advance estimate for widths. Prefer [`find_measured`]
/// wherever the host's own text measurement is reachable: the estimate is
/// about twice too wide for punctuation and invents collisions between a
/// value and the comma after it.
pub fn find(ops: &[CanvasOp]) -> Vec<Collision> {
    find_measured(ops, None)
}

/// As [`find`], but with the host's text measurement, so a comma is a comma
/// wide rather than an average character wide.
pub fn find_measured(ops: &[CanvasOp], measure: Option<&Measure<'_>>) -> Vec<Collision> {
    let mut strings: Vec<(Box2, &str)> = Vec::new();
    for op in ops {
        if let CanvasOp::Text {
            origin,
            font_size,
            color,
            letter_spacing,
            text,
            ..
        } = op
        {
            if is_invisible(*color) || text.trim().is_empty() {
                continue;
            }
            if let Some(b) = text_box(*origin, *font_size, *letter_spacing, text, measure) {
                strings.push((b, text.as_str()));
            }
        }
    }

    let mut found = Vec::new();
    for i in 0..strings.len() {
        for j in (i + 1)..strings.len() {
            let (a, a_text) = &strings[i];
            let (b, b_text) = &strings[j];
            let Some(region) = a.intersect(b) else {
                continue;
            };
            let smaller = a.area().min(b.area());
            if smaller <= 0.0 {
                continue;
            }
            let fraction = region.area() / smaller;
            if fraction >= MIN_OVERLAP {
                found.push(Collision {
                    first: (*a_text).to_string(),
                    second: (*b_text).to_string(),
                    overlap_fraction: fraction,
                    region,
                });
            }
        }
    }
    found.sort_by(|x, y| {
        y.overlap_fraction
            .partial_cmp(&x.overlap_fraction)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    found
}

/// One draw call as a single line, for inspecting what a frame contains.
pub fn describe_op(op: &CanvasOp) -> String {
    match op {
        CanvasOp::Clear(c) => format!("clear                                        #{c:08x}"),
        CanvasOp::SetClip(r) => format!("clip      {r:?}"),
        CanvasOp::FillRect { rect, color } => {
            format!(
                "fillrect  {:8.1} {:8.1} {:8.1} {:8.1}  #{color:08x}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::StrokeRect { rect, color, .. } => {
            format!(
                "strokerect{:8.1} {:8.1} {:8.1} {:8.1}  #{color:08x}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::FillRoundRect { rect, color, .. } => {
            format!(
                "fillround {:8.1} {:8.1} {:8.1} {:8.1}  #{color:08x}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::StrokeRoundRect { rect, color, .. } => {
            format!(
                "strkround {:8.1} {:8.1} {:8.1} {:8.1}  #{color:08x}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::DropShadowRoundRect { rect, .. } => {
            format!(
                "shadow    {:8.1} {:8.1} {:8.1} {:8.1}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::LinearGradient { rect, .. } => {
            format!(
                "lgrad     {:8.1} {:8.1} {:8.1} {:8.1}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::LinearGradientStops { rect, .. } => {
            format!(
                "lgradstop {:8.1} {:8.1} {:8.1} {:8.1}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::RadialGradient { center, radius, .. } => {
            format!("rgrad     c{:7.1},{:7.1} r{radius:.1}", center.0, center.1)
        }
        CanvasOp::FillCircle {
            center,
            radius,
            color,
        } => {
            format!(
                "circle    c{:7.1},{:7.1} r{radius:7.1}          #{color:08x}",
                center.0, center.1
            )
        }
        CanvasOp::StrokeCircle { center, radius, .. } => {
            format!("strkcirc  c{:7.1},{:7.1} r{radius:7.1}", center.0, center.1)
        }
        CanvasOp::StrokeArc { center, radius, .. } => {
            format!("arc       c{:7.1},{:7.1} r{radius:7.1}", center.0, center.1)
        }
        CanvasOp::Text {
            origin,
            font_size,
            text,
            ..
        } => {
            format!(
                "text      {:8.1} {:8.1} size{font_size:6.1}          {text:?}",
                origin.0, origin.1
            )
        }
        CanvasOp::Pixels { rect, .. } => {
            format!(
                "pixels    {:8.1} {:8.1} {:8.1} {:8.1}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::PixelsRound { rect, .. } => {
            format!(
                "pixelsrnd {:8.1} {:8.1} {:8.1} {:8.1}",
                rect.0, rect.1, rect.2, rect.3
            )
        }
        CanvasOp::Sprite { center, dst, .. } => {
            format!(
                "sprite    c{:7.1},{:7.1} d{:7.1},{:7.1}",
                center.0, center.1, dst.0, dst.1
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_at(x: f32, y: f32, size: f32, s: &str) -> CanvasOp {
        CanvasOp::Text {
            origin: (x, y),
            font_size: size,
            color: 0xFF_FF_FF_FF,
            weight: 400,
            italic: false,
            letter_spacing: 0.0,
            family: 0,
            text: s.to_string(),
        }
    }

    #[test]
    fn separate_lines_are_not_a_collision() {
        // The ordinary case: a column of labels, one under the next.
        let ops = vec![
            text_at(20.0, 40.0, 14.0, "Name"),
            text_at(20.0, 70.0, 14.0, "Role"),
            text_at(20.0, 100.0, 14.0, "Team"),
        ];
        assert!(find(&ops).is_empty());
    }

    #[test]
    fn tight_line_spacing_is_not_a_collision() {
        // Two lines one after another at a normal leading. Descender of the
        // first and ascender of the second nearly touch; that is typography,
        // not a bug.
        let ops = vec![
            text_at(10.0, 100.0, 20.0, "first line"),
            text_at(10.0, 118.0, 20.0, "second line"),
        ];
        assert!(
            find(&ops).is_empty(),
            "normal leading must not be reported: {:?}",
            find(&ops)
        );
    }

    #[test]
    fn one_string_over_another_is_reported() {
        // The tic tac toe shape: a button label landing on a score label.
        let ops = vec![
            text_at(100.0, 600.0, 13.0, "Draws"),
            text_at(100.0, 598.0, 15.0, "New round"),
        ];
        let hits = find(&ops);
        assert_eq!(hits.len(), 1, "expected one collision, got {hits:?}");
        assert!(hits[0].overlap_fraction > 0.5);
    }

    #[test]
    fn invisible_text_cannot_collide() {
        let mut hidden = text_at(100.0, 600.0, 15.0, "faded out");
        if let CanvasOp::Text { color, .. } = &mut hidden {
            *color = 0x00_FF_FF_FF;
        }
        let ops = vec![text_at(100.0, 600.0, 13.0, "Draws"), hidden];
        assert!(find(&ops).is_empty());
    }

    #[test]
    fn blank_strings_are_ignored() {
        let ops = vec![
            text_at(100.0, 600.0, 13.0, "   "),
            text_at(100.0, 600.0, 15.0, ""),
            text_at(100.0, 600.0, 15.0, "real"),
        ];
        assert!(find(&ops).is_empty());
    }

    #[test]
    fn text_over_a_panel_is_not_reported() {
        // A card with a label on it -- the single most common thing an app
        // draws. Only the text counts, so the rect is irrelevant.
        let ops = vec![
            CanvasOp::FillRoundRect {
                rect: (40.0, 520.0, 380.0, 76.0),
                radii: (18.0, 18.0, 18.0, 18.0),
                color: 0xFF_20_20_20,
            },
            text_at(60.0, 560.0, 14.0, "Balance"),
        ];
        assert!(find(&ops).is_empty());
    }

    #[test]
    fn worst_collision_is_reported_first() {
        let ops = vec![
            text_at(0.0, 100.0, 14.0, "aaaaaaaa"),
            // Slight clip of the one above.
            text_at(38.0, 100.0, 14.0, "bbbbbbbb"),
            // Sits squarely on the first.
            text_at(0.0, 100.0, 14.0, "cccccccc"),
        ];
        let hits = find(&ops);
        assert!(hits.len() >= 2);
        assert!(
            hits[0].overlap_fraction >= hits[1].overlap_fraction,
            "not sorted worst-first: {hits:?}"
        );
        assert!(hits[0].overlap_fraction > 0.9);
    }

    #[test]
    fn columns_side_by_side_are_not_a_collision() {
        // The scoreboard done right: three columns across, well separated.
        let ops = vec![
            text_at(60.0, 560.0, 26.0, "3"),
            text_at(200.0, 560.0, 26.0, "1"),
            text_at(340.0, 560.0, 26.0, "2"),
        ];
        assert!(find(&ops).is_empty());
    }

    #[test]
    fn punctuation_after_a_value_is_not_a_collision() {
        // A syntax-highlighted JSON line, drawn as separate coloured tokens
        // laid end to end -- which is how Probe and Query render a response.
        //
        // With the average-advance estimate this reported `"list"` and `,`
        // sharing "100% of the smaller one": a comma is about 0.26em and the
        // estimate calls every character 0.52em, so the comma's box was twice
        // its real width and reached back over the string before it. Twelve
        // of these were reported against apps whose pixels were perfect.
        //
        // Measured, the tokens abut and nothing overlaps.
        let size = 13.0;
        // Real advances for this face, as fractions of the em.
        let advance = |s: &str, size: f32| -> f32 {
            s.chars()
                .map(|c| match c {
                    ',' => 0.26,
                    ':' => 0.28,
                    ' ' => 0.26,
                    '"' => 0.35,
                    _ => 0.55,
                })
                .sum::<f32>()
                * size
        };
        let mut x = 100.0;
        let mut ops = Vec::new();
        for tok in ["\"object\"", ": ", "\"list\"", ","] {
            ops.push(text_at(x, 300.0, size, tok));
            x += advance(tok, size);
        }
        let measure: &dyn Fn(&str, f32) -> f32 = &advance;
        assert!(
            find_measured(&ops, Some(measure)).is_empty(),
            "tokens laid end to end must not report as overlapping"
        );
    }

    #[test]
    fn a_real_overlap_is_still_caught_when_measuring() {
        // The measurement must not make the check toothless: two strings at
        // the same origin are still one on top of the other.
        let advance = |s: &str, size: f32| s.chars().count() as f32 * size * 0.55;
        let measure: &dyn Fn(&str, f32) -> f32 = &advance;
        let ops = vec![
            text_at(100.0, 300.0, 16.0, "New round"),
            text_at(104.0, 302.0, 16.0, "Draws"),
        ];
        assert!(!find_measured(&ops, Some(measure)).is_empty());
    }
}
