//! Ports diagrams/common/fit-width.ts: drawing a diagram to the width the
//! host has for it.

use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::js::{jmax, jmin};

/// Ports `fitWidth`: draws a chart whose plot can be any width so that
/// the whole diagram fits the width the host has.
pub fn fit_width<E>(
    options: &RenderOptions,
    natural: f64,
    least: f64,
    mut draw: impl FnMut(f64, bool) -> Result<Rendered, E>,
) -> Result<Rendered, E> {
    let mut result = draw(natural, false)?;
    let available = options.max_width;
    let Some(available) = available.filter(|a| *a > 0.0) else {
        return Ok(result);
    };
    if result.width <= available {
        return Ok(result);
    }
    let floor = jmin(least, natural);
    let mut plot = natural;
    result = draw(plot, true)?;
    let mut round = 0;
    while round < 3 && result.width > available && plot > floor {
        plot = jmax(floor, plot - (result.width - available));
        result = draw(plot, true)?;
        round += 1;
    }
    Ok(result)
}

/// Ports `titleRoom`: the width a title may take before it wraps.
pub fn title_room(options: &RenderOptions) -> f64 {
    match options.max_width {
        Some(a) if a > 0.0 => jmax(80.0, a - 2.0 * options.padding.unwrap_or(8.0)),
        _ => 4000.0,
    }
}

/// Ports `capWidth`.
pub fn cap_width(options: &RenderOptions, natural: f64, least: f64, around: f64) -> f64 {
    match options.max_width {
        Some(a) if a > 0.0 => jmax(jmin(least, natural), jmin(natural, a - around)),
        _ => natural,
    }
}

const BREAKPOINT: f64 = 640.0;

/// Ports `turnToFit`: draws a diagram that can run across or down across,
/// and down instead when the host allows it, the width it has is under the
/// breakpoint, across does not fit, and down is narrower.
pub fn turn_to_fit<E>(
    options: &RenderOptions,
    mut draw: impl FnMut(bool) -> Result<Rendered, E>,
) -> Result<Rendered, E> {
    let across = draw(false)?;
    let available = match options.max_width {
        Some(a) if a > 0.0 => a,
        _ => return Ok(across),
    };
    if options.auto_direction == Some(false) || across.width <= available {
        return Ok(across);
    }
    let breakpoint = match options.direction_breakpoint {
        Some(b) if b > 0.0 => b,
        _ => BREAKPOINT,
    };
    if available >= breakpoint {
        return Ok(across);
    }
    let down = draw(true)?;
    Ok(if down.width < across.width {
        down
    } else {
        across
    })
}

/// Ports `runsAcross`.
pub fn runs_across(direction: Option<&str>) -> bool {
    matches!(direction, Some("LR") | Some("RL"))
}

/// Ports `tighten`: draws a diagram with room to spare, and tightly instead
/// when that does not fit the width the host has and tight is narrower.
pub fn tighten<E>(
    options: &RenderOptions,
    mut draw: impl FnMut(bool) -> Result<Rendered, E>,
) -> Result<Rendered, E> {
    let loose = draw(false)?;
    let available = match options.max_width {
        Some(a) if a > 0.0 => a,
        _ => return Ok(loose),
    };
    if loose.width <= available {
        return Ok(loose);
    }
    let tight = draw(true)?;
    Ok(if tight.width < loose.width {
        tight
    } else {
        loose
    })
}
