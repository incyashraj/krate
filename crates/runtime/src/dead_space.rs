//! Finds a large part of the window where the app drew nothing.
//!
//! An earlier attempt measured this from pixels, as the largest band in from
//! an edge containing no content. It was reverted: every real app scored
//! 3-6%, including one with an obvious dead region, because a full-width
//! header or footer near each edge means no complete row or column is ever
//! empty. See K-099 for the numbers.
//!
//! This works from the recorded draw calls instead, which fixes the part
//! that was actually hard. The background is one op -- a gradient or a rect
//! covering the whole canvas -- so it can be recognised and excluded by its
//! size, rather than guessed at from colour. Everything else is content.
//!
//! The bar is deliberately "nothing was drawn here at all", not "this looks
//! sparse". A deliberately airy layout is a style, not a defect, and a check
//! that argues about taste gets switched off.
//!
//! **This reports an observation, not a verdict.** It cannot tell an editor
//! with room left to type in from an app that stopped drawing halfway down:
//! both put content at the top and nothing below it. Measured across 21
//! apps, one real app (krate-notes, 38.9%) is a false positive for exactly
//! that reason, and the test named after it keeps the limit visible. So the
//! finding is phrased as what was measured and surfaces as a note, never as
//! a failure. K-108 tracks the gap.

use krate_adapter_common::canvas_list::CanvasOp;

/// A rectangle of the window that has nothing in it.
#[derive(Clone, Copy, Debug)]
pub struct DeadRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Share of the whole window this covers, 0..1.
    pub fraction: f32,
}

/// How fine the grid is. 24x24 on a 1080x700 window is cells of about
/// 45x29 -- small enough to see a real gap, coarse enough that ordinary
/// spacing between cards does not register as one.
const GRID: usize = 24;

/// An op covering at least this much of the canvas is the background, not
/// content. A card or a panel is well under half; a background wash is
/// essentially all of it.
const BACKGROUND_COVERAGE: f32 = 0.85;

/// Report a region only when it is at least this share of the window. Below
/// this it is ordinary breathing room around the content.
///
/// Measured across fourteen generated apps and seven hand-written ones: all
/// but two sit at or under 14.6%, so 16% is above the ordinary band rather
/// than a round number picked in advance.
const MIN_REPORTABLE: f32 = 0.16;

fn op_bounds(op: &CanvasOp) -> Option<(f32, f32, f32, f32)> {
    let r = match op {
        // Clear and clip say nothing about where content is.
        CanvasOp::Clear(_) | CanvasOp::SetClip(_) => return None,
        CanvasOp::FillRect { rect, .. }
        | CanvasOp::StrokeRect { rect, .. }
        | CanvasOp::FillRoundRect { rect, .. }
        | CanvasOp::StrokeRoundRect { rect, .. }
        | CanvasOp::LinearGradient { rect, .. }
        | CanvasOp::LinearGradientStops { rect, .. }
        | CanvasOp::Pixels { rect, .. }
        | CanvasOp::PixelsRound { rect, .. } => *rect,
        // A shadow is cast by something else; the something else is the
        // content and is drawn too, so counting the shadow would only
        // spread the footprint.
        CanvasOp::DropShadowRoundRect { .. } => return None,
        CanvasOp::RadialGradient { center, radius, .. }
        | CanvasOp::FillCircle { center, radius, .. }
        | CanvasOp::StrokeCircle { center, radius, .. }
        | CanvasOp::StrokeArc { center, radius, .. } => (
            center.0 - radius,
            center.1 - radius,
            radius * 2.0,
            radius * 2.0,
        ),
        CanvasOp::Text {
            origin,
            font_size,
            letter_spacing,
            text,
            ..
        } => {
            let chars = text.chars().count();
            if chars == 0 || *font_size <= 0.0 {
                return None;
            }
            let w = chars as f32 * (font_size * 0.52 + letter_spacing);
            (
                origin.0,
                origin.1 - font_size * 0.72,
                w,
                font_size * 0.92,
            )
        }
        CanvasOp::Sprite { center, dst, .. } => {
            (center.0 - dst.0 / 2.0, center.1 - dst.1 / 2.0, dst.0, dst.1)
        }
    };
    if r.2 <= 0.0 || r.3 <= 0.0 {
        return None;
    }
    Some(r)
}

/// Mark every grid cell any content op touches.
///
/// The background wash is excluded by size: without that every app looks
/// completely full and the check reports nothing, ever.
fn coverage(ops: &[CanvasOp], width: f32, height: f32) -> [[bool; GRID]; GRID] {
    let canvas_area = width * height;
    let mut covered = [[false; GRID]; GRID];
    for op in ops {
        let Some((x, y, w, h)) = op_bounds(op) else {
            continue;
        };
        if (w * h) / canvas_area >= BACKGROUND_COVERAGE {
            continue;
        }
        let c0 = ((x / width) * GRID as f32).floor().max(0.0) as usize;
        let r0 = ((y / height) * GRID as f32).floor().max(0.0) as usize;
        let c1 = (((x + w) / width) * GRID as f32).ceil().min(GRID as f32) as usize;
        let r1 = (((y + h) / height) * GRID as f32).ceil().min(GRID as f32) as usize;
        for row in covered.iter_mut().take(r1).skip(r0) {
            for cell in row.iter_mut().take(c1).skip(c0) {
                *cell = true;
            }
        }
    }
    covered
}

/// Calibration view: (fraction, content above the region, content left of it).
pub fn measure_detail(ops: &[CanvasOp], width: f32, height: f32) -> (f32, bool, bool) {
    if width <= 0.0 || height <= 0.0 {
        return (0.0, false, false);
    }
    let covered = coverage(ops, width, height);
    let Some((cells, (c0, r0, c1, r1))) = largest_empty_rect(&covered) else {
        return (0.0, false, false);
    };
    let above = (0..r0).any(|r| (c0..c1).any(|c| covered[r][c]));
    let left = (r0..r1).any(|r| (0..c0).any(|c| covered[r][c]));
    (cells as f32 / (GRID * GRID) as f32, above, left)
}

/// The largest empty fraction, whatever its size. For calibration: `find`
/// applies a reporting threshold on top of this.
pub fn measure(ops: &[CanvasOp], width: f32, height: f32) -> f32 {
    measure_detail(ops, width, height).0
}

/// The largest rectangle of the window that no content was drawn into.
///
/// `width` and `height` are the window in logical pixels. Returns `None`
/// when nothing stands out, which is the answer for a well filled app.
pub fn find(ops: &[CanvasOp], width: f32, height: f32) -> Option<DeadRegion> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let covered = coverage(ops, width, height);
    let (cells, rect) = largest_empty_rect(&covered)?;
    let fraction = cells as f32 / (GRID * GRID) as f32;
    if fraction < MIN_REPORTABLE {
        return None;
    }
    let cw = width / GRID as f32;
    let ch = height / GRID as f32;
    Some(DeadRegion {
        x: rect.0 as f32 * cw,
        y: rect.1 as f32 * ch,
        width: (rect.2 - rect.0) as f32 * cw,
        height: (rect.3 - rect.1) as f32 * ch,
        fraction,
    })
}

/// Largest all-empty axis-aligned rectangle, as (area in cells, (c0,r0,c1,r1)).
///
/// The standard largest-rectangle-in-a-histogram sweep, run once per row.
fn largest_empty_rect(covered: &[[bool; GRID]; GRID]) -> Option<(usize, (usize, usize, usize, usize))> {
    let mut heights = [0usize; GRID];
    let mut best: Option<(usize, (usize, usize, usize, usize))> = None;

    for (r, row) in covered.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            heights[c] = if *cell { 0 } else { heights[c] + 1 };
        }
        // Monotonic stack over this row's histogram.
        let mut stack: Vec<usize> = Vec::with_capacity(GRID + 1);
        for c in 0..=GRID {
            let h = if c == GRID { 0 } else { heights[c] };
            while let Some(&top) = stack.last() {
                if heights[top] <= h {
                    break;
                }
                stack.pop();
                let height = heights[top];
                let left = stack.last().map(|&i| i + 1).unwrap_or(0);
                let area = height * (c - left);
                if area > best.map(|(a, _)| a).unwrap_or(0) {
                    best = Some((area, (left, r + 1 - height, c, r + 1)));
                }
            }
            stack.push(c);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> CanvasOp {
        CanvasOp::FillRect {
            rect: (x, y, w, h),
            color: 0xFF_40_40_40,
        }
    }

    #[test]
    fn a_full_window_reports_nothing() {
        // Content edge to edge, drawn as a grid of tiles.
        let mut ops = vec![CanvasOp::Clear(0xFF_00_00_00)];
        for r in 0..10 {
            for c in 0..10 {
                ops.push(rect(c as f32 * 100.0, r as f32 * 70.0, 96.0, 66.0));
            }
        }
        assert!(find(&ops, 1000.0, 700.0).is_none());
    }

    #[test]
    fn the_background_wash_does_not_count_as_content() {
        // One gradient over everything, and a single small card. Almost the
        // whole window is genuinely empty; if the wash counted, this would
        // wrongly look full.
        let ops = vec![
            CanvasOp::LinearGradient {
                rect: (0.0, 0.0, 1000.0, 700.0),
                top: 0xFF_10_10_10,
                bottom: 0xFF_20_20_20,
            },
            rect(20.0, 20.0, 200.0, 80.0),
        ];
        let found = find(&ops, 1000.0, 700.0).expect("a nearly empty window is dead space");
        assert!(
            found.fraction > 0.5,
            "expected most of the window, got {:.2}",
            found.fraction
        );
    }

    #[test]
    fn the_bottom_half_left_empty_is_reported() {
        // The shape of the real defect: content stops partway down.
        let mut ops = vec![CanvasOp::Clear(0xFF_00_00_00)];
        for c in 0..10 {
            ops.push(rect(c as f32 * 100.0, 10.0, 96.0, 300.0));
        }
        let found = find(&ops, 1000.0, 700.0).expect("half a window is dead space");
        assert!(found.y > 250.0, "region should be low down, got y {}", found.y);
        assert!(found.fraction > 0.3);
    }

    #[test]
    fn ordinary_gaps_between_cards_are_not_reported() {
        // Three cards with normal spacing, filling the window. The gaps are
        // real but small; calling them a defect would be wrong.
        let mut ops = vec![CanvasOp::Clear(0xFF_00_00_00)];
        for r in 0..3 {
            for c in 0..3 {
                ops.push(rect(
                    20.0 + c as f32 * 330.0,
                    20.0 + r as f32 * 230.0,
                    300.0,
                    200.0,
                ));
            }
        }
        assert!(find(&ops, 1000.0, 700.0).is_none());
    }

    #[test]
    fn text_counts_as_content() {
        // Text is the only thing this app draws, and it fills the window.
        // A line of 92 characters at size 20 is about 960 of the 1000 wide.
        let line: String = "wrapped body text ".repeat(6);
        let mut ops = vec![CanvasOp::Clear(0xFF_00_00_00)];
        for i in 0..25 {
            ops.push(CanvasOp::Text {
                origin: (10.0, 26.0 + i as f32 * 28.0),
                font_size: 20.0,
                color: 0xFF_FF_FF_FF,
                weight: 400,
                italic: false,
                letter_spacing: 0.0,
                family: 0,
                text: line.clone(),
            });
        }
        assert!(
            find(&ops, 1000.0, 700.0).is_none(),
            "a page of text is not dead space: {:?}",
            find(&ops, 1000.0, 700.0)
        );
    }

    #[test]
    fn a_short_column_of_text_leaves_the_rest_dead() {
        // The counterpart, and the reason the test above had to be rewritten:
        // narrow lines down one side really do leave the other side empty,
        // and that is a finding, not a false positive.
        let mut ops = vec![CanvasOp::Clear(0xFF_00_00_00)];
        for i in 0..24 {
            ops.push(CanvasOp::Text {
                origin: (10.0, 30.0 + i as f32 * 28.0),
                font_size: 20.0,
                color: 0xFF_FF_FF_FF,
                weight: 400,
                italic: false,
                letter_spacing: 0.0,
                family: 0,
                text: "a narrow column".to_string(),
            });
        }
        let found = find(&ops, 1000.0, 700.0).expect("the right of the window is empty");
        // The lines are about 156 wide, so the empty part starts just past
        // them and runs to the right edge.
        assert!(
            found.x >= 150.0 && found.x + found.width >= 950.0,
            "expected the empty right-hand side, got x {} width {}",
            found.x,
            found.width
        );
        assert!(found.fraction > 0.5);
    }

    #[test]
    fn an_empty_frame_is_all_dead_space() {
        let ops = vec![CanvasOp::Clear(0xFF_00_00_00)];
        let found = find(&ops, 1000.0, 700.0).expect("an empty frame is entirely dead");
        assert!(found.fraction > 0.9);
    }

    /// The known limit, kept as a test so it cannot be forgotten.
    ///
    /// This is krate-notes reduced: a sidebar, a short note at the top of an
    /// editor pane, and the rest of the pane empty because nobody has typed
    /// there yet. The measure reports it, and reporting it is wrong -- that
    /// space is where you type.
    ///
    /// It is not fixable from the draw list. An editor with room left in it
    /// and an app that stopped drawing halfway down produce the same ops:
    /// content at the top, nothing below. krate-notes draws its editor on a
    /// canvas, so there is no widget kind and no container rect to find --
    /// its only large rect is the sidebar, which does not contain the empty
    /// region. Both candidate rules were tried against all 21 measured apps
    /// and neither separated them.
    ///
    /// Two tried and rejected:
    ///   - "content above the region" also suppressed
    ///     `the_bottom_half_left_empty_is_reported`, a real defect.
    ///   - "the region sits inside a drawn container" finds no container,
    ///     because the editor pane is bare background with text on it.
    ///
    /// This is why the finding is worded as an observation and not a
    /// verdict, and why it is a note rather than a failure. See K-108.
    #[test]
    fn an_editor_with_room_to_type_is_reported_and_should_not_be() {
        let mut ops = vec![CanvasOp::Clear(0xFF_00_00_00)];
        ops.push(rect(0.0, 0.0, 280.0, 640.0));
        for i in 0..3 {
            ops.push(CanvasOp::Text {
                origin: (320.0, 60.0 + i as f32 * 40.0),
                font_size: 22.0,
                color: 0xFF_FF_FF_FF,
                weight: 400,
                italic: false,
                letter_spacing: 0.0,
                family: 0,
                text: "a line of the note that was typed here".to_string(),
            });
        }
        assert!(
            find(&ops, 900.0, 640.0).is_some(),
            "documents the false positive; if this starts passing, the \
             limit was fixed and K-108 can close"
        );
    }

    #[test]
    fn a_strip_the_app_never_used_is_reported() {
        // req-34, reduced: a table occupying the left two thirds, full
        // height, and nothing at all to the right of it. Nothing is above
        // the empty strip, so it is space the app did not use rather than a
        // pane with room in it.
        let mut ops = vec![CanvasOp::Clear(0xFF_00_00_00)];
        for r in 0..15 {
            ops.push(rect(20.0, 20.0 + r as f32 * 40.0, 740.0, 36.0));
        }
        let found = find(&ops, 1200.0, 620.0).expect("an unused strip is a finding");
        assert!(
            found.x > 700.0,
            "expected the strip on the right, got x {}",
            found.x
        );
    }

    #[test]
    fn a_zero_sized_window_is_not_measured() {
        assert!(find(&[CanvasOp::Clear(0)], 0.0, 0.0).is_none());
    }
}
