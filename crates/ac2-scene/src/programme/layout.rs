//! Where a Programme pane's parts sit: the goniometer with its correlation chip, the bars,
//! and the loudness plate's dock (docs/design/programme-meter.md §6.2 *Where it sits*).

use crate::canvas::text_width;
use crate::primitives::Rect;

use super::loudness_plate;

/// Height of the correlation chip strip, logical pixels.
pub const CHIP_H: f32 = 28.0;
/// Width of the chip when it stands alone beside the bars (short pane).
pub const CHIP_W: f32 = 96.0;
/// Below this height the goniometer has no room (it needs a square of at least ~120 px
/// above the chip) and the pane shows bars plus the chip only.
pub const SHORT_PANE_H: f32 = 150.0;
/// A bar's column keeps its readouts whole (`max −11.8 dBTP`) at this width.
pub const BAR_MIN_W: f32 = 96.0;
/// The narrowest goniometer worth drawing beside the bars when they need the room.
pub const GONIO_MIN_W: f32 = 120.0;
/// What a plate docked above leaves the bars at least: a bar of 60 px plus up to five text
/// rows and the label.
pub const BAR_MIN_H: f32 = 160.0;
/// The widest label of the LUFS scale over the whole target range (−36 … −5 LUFS): the
/// target's tick, its word and a two-digit number.
pub const LUFS_SCALE_TEMPLATE: &str = "target −41";

/// Which numbers the pane's scale column carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleKind {
    /// The 60 dB span under full scale, every peak bar's.
    Dbfs,
    /// The EBU +9 span around the target, a loudness bar's.
    Lufs,
}

/// The scale column's width at the plot font `small`: one number at every **T** step, so
/// the bars never move when the numbers go, and the target's word fits in the LUFS one.
pub fn scale_w(kind: ScaleKind, small: f32) -> f32 {
    match kind {
        ScaleKind::Dbfs => 3.0 * small,
        ScaleKind::Lufs => text_width(LUFS_SCALE_TEMPLATE, small) + 4.0,
    }
}

/// A docked loudness plate: where, how many steps of the drop order it took
/// ([`loudness_plate::Slot`]), and the text scale it is drawn at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Docked {
    pub rect: Rect,
    pub dropped: u8,
    pub scale: f32,
}

/// Where the parts go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgrammeLayout {
    /// The goniometer's rect (a pair, tall enough); the figure is [`crate::gonio::square`]
    /// of it.
    pub gonio: Option<Rect>,
    /// The correlation chip (a pair).
    pub correlation: Option<Rect>,
    pub bars: Rect,
    /// The docked loudness plate, when it has room.
    pub plate: Option<Docked>,
}

fn bars_min(bars: usize, scale_w: f32) -> f32 {
    scale_w + bars.max(1) as f32 * BAR_MIN_W
}

/// Bars on the right, the goniometer with the correlation chip under it on the left, split
/// 1:1 (the MSD main screen) while that leaves the bars [`BAR_MIN_W`] each, else the
/// goniometer gives way down to [`GONIO_MIN_W`]; bars alone without a pair; on a pane
/// shorter than [`SHORT_PANE_H`] the chip stands [`CHIP_W`] wide on the left and the bars
/// take the rest. The pure fit of `bars` bars and a scale column `scale_w` wide in `rect`:
/// nothing here reserves room for the plate ([`dock`] does).
pub fn layout(rect: Rect, has_pair: bool, bars: usize, scale_w: f32) -> ProgrammeLayout {
    if !has_pair {
        return ProgrammeLayout {
            gonio: None,
            correlation: None,
            bars: rect,
            plate: None,
        };
    }
    if rect.h < SHORT_PANE_H {
        let w = CHIP_W.min(rect.w / 2.0);
        return ProgrammeLayout {
            gonio: None,
            correlation: Some(Rect::new(rect.x, rect.y, w, rect.h)),
            bars: Rect::new(rect.x + w, rect.y, rect.w - w, rect.h),
            plate: None,
        };
    }
    let half = rect.w / 2.0;
    let gonio_w = (rect.w - bars_min(bars, scale_w)).clamp(GONIO_MIN_W.min(half), half);
    ProgrammeLayout {
        gonio: Some(Rect::new(rect.x, rect.y, gonio_w, rect.h - CHIP_H)),
        correlation: Some(Rect::new(rect.x, rect.bottom() - CHIP_H, gonio_w, CHIP_H)),
        bars: Rect::new(rect.x + gonio_w, rect.y, rect.w - gonio_w, rect.h),
        plate: None,
    }
}

/// The dock ladder for a plate at the plot font `font` and the operator's `scale`: beside
/// the bars at the pane's right edge, else above them at the top right; at `scale`, then at
/// 1× (the stored scale is kept, so a larger pane brings it back); then at 1× with the
/// slots dropped one step at a time, retrying both places; else no plate. The rest of
/// `rect` is laid out by [`layout`]. Which step lands depends on the pane's size and the
/// scale only, never on values.
pub fn dock(
    rect: Rect,
    has_pair: bool,
    bars: usize,
    scale_w: f32,
    font: f32,
    scale: f32,
) -> ProgrammeLayout {
    let beside_need = bars_min(bars, scale_w)
        + match (has_pair, rect.h < SHORT_PANE_H) {
            (false, _) => 0.0,
            (true, true) => CHIP_W,
            (true, false) => GONIO_MIN_W,
        };
    let place = |s: f32, dropped: u8| {
        let (w, h) = loudness_plate::size(font, s, dropped);
        let docked = Docked {
            rect: Rect::new(rect.right() - w, rect.y, w, h),
            dropped,
            scale: s,
        };
        let rest = if h <= rect.h && rect.w - w >= beside_need {
            Rect::new(rect.x, rect.y, rect.w - w, rect.h)
        } else if w <= rect.w && rect.h - h >= BAR_MIN_H {
            Rect::new(rect.x, rect.y + h, rect.w, rect.h - h)
        } else {
            return None;
        };
        Some(ProgrammeLayout {
            plate: Some(docked),
            ..layout(rest, has_pair, bars, scale_w)
        })
    };
    let mut steps = vec![(scale, 0)];
    if scale != 1.0 {
        steps.push((1.0, 0));
    }
    steps.extend((1..=loudness_plate::MAX_DROPPED).map(|d| (1.0, d)));
    steps
        .into_iter()
        .find_map(|(s, d)| place(s, d))
        .unwrap_or_else(|| layout(rect, has_pair, bars, scale_w))
}
