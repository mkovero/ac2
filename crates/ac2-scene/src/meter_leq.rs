//! The SPL pane's meter + Leq view: the meter's held number on top, centred, in about a
//! third of the pane, and the Leq windows under it as the Leq view draws them, under the
//! Leq view's one caption (meter name and unit, the run, the calibration). Composed from the
//! two views' own builders ([`crate::spl`], [`crate::leq`]): the same number with the same
//! hold, the same columns or tiles.
//!
//! The meter's own statistics, heading and calibration stay in the meter view (G): next to
//! the windows they would read as more windows.

use crate::banner::Status;
use crate::canvas::Canvas;
use crate::leq::{LeqScene, LeqView, leq_scene_under};
use crate::primitives::{Rect, Viewport};
use crate::spl::{SplReadout, draw_number_block, draw_number_line, number_block_size};
use crate::theme::Theme;

/// Share of the height under the caption the meter takes when it is a block.
const METER_SHARE: f32 = 1.0 / 3.0;
/// The block needs its number at least this many caption ems high, else it is one line.
const BLOCK_MIN_EM: f32 = 4.0;
/// Height under the caption below which the meter gives way to the windows.
const LINE_MIN_AREA: f32 = 120.0;
/// Share of the height under the caption the meter takes when it is one line.
const LINE_SHARE: f32 = 0.2;
/// Between the meter and the windows.
const GAP: f32 = 10.0;

/// How the meter is drawn above the windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterForm {
    /// The number, its name and unit under it, the live bar.
    Block,
    /// The number with its name and unit after it, on one line (a short pane).
    Line,
    /// Left out: the pane is too short for both, and the windows judge the limits.
    Hidden,
}

/// The meter part as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterHead {
    pub form: MeterForm,
    /// Where the meter is drawn (zero height when hidden).
    pub region: Rect,
    /// The number's type size (0 when hidden).
    pub size: f32,
    /// The live level bar (block only).
    pub bar: Option<Rect>,
}

/// The meter + Leq view as drawn: `leq.scene` is the whole picture.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterLeqScene {
    pub leq: LeqScene,
    pub meter: MeterHead,
}

/// The form the meter takes in `area` (the pane under the caption) and its region there.
fn meter_region(value: &str, area: Rect, theme: &Theme) -> (MeterForm, Rect) {
    let block = Rect::new(area.x, area.y, area.w, area.h * METER_SHARE);
    if number_block_size(value, block, theme.font_size)
        .is_some_and(|s| s >= BLOCK_MIN_EM * theme.font_size)
    {
        return (MeterForm::Block, block);
    }
    if area.h >= LINE_MIN_AREA {
        return (
            MeterForm::Line,
            Rect::new(area.x, area.y, area.w, area.h * LINE_SHARE),
        );
    }
    (MeterForm::Hidden, Rect::new(area.x, area.y, area.w, 0.0))
}

/// Lays the pane out in `size`: the banner strip and the Leq view's caption across the
/// top, the meter's number (`meter`, the held reading) centred under them in about a third
/// of what is left — one line on a short pane, nothing on a very short one, where the
/// windows matter more: they judge the limits — and the windows below.
pub fn meter_leq_scene(
    meter: &SplReadout,
    v: &LeqView<'_>,
    status: &Status,
    theme: &Theme,
    size: Viewport,
) -> MeterLeqScene {
    let (leq, meter) = leq_scene_under(v, status, theme, size, |c: &mut Canvas, area| {
        let (form, region) = meter_region(&meter.value, area, theme);
        let (size, bar) = match form {
            MeterForm::Block => {
                let b = draw_number_block(c, meter, region, theme);
                (b.size, b.bar)
            }
            MeterForm::Line => (draw_number_line(c, meter, region, theme), None),
            MeterForm::Hidden => return (area, hidden(region)),
        };
        let below = region.bottom() + GAP;
        let rest = Rect::new(area.x, below, area.w, (area.bottom() - below).max(1.0));
        let head = MeterHead {
            form,
            region,
            size,
            bar,
        };
        (rest, head)
    });
    MeterLeqScene { leq, meter }
}

fn hidden(region: Rect) -> MeterHead {
    MeterHead {
        form: MeterForm::Hidden,
        region,
        size: 0.0,
        bar: None,
    }
}

#[cfg(test)]
mod tests;
