//! Where a Programme pane's parts sit (docs/design/programme-meter.md §6.3): in the Meters
//! view the bars with the correlation strip under them and the loudness plate docked beside
//! or above them at the size its room allows; in the Goniometer view the square filling the
//! pane with the correlation chip under it.

use crate::canvas::text_width;
use crate::gonio;
use crate::primitives::Rect;

use super::loudness_plate;

/// Height of the correlation chip and of the strip it sits in, logical pixels.
pub const CHIP_H: f32 = 28.0;
/// A bar's column keeps its readouts whole (`max −11.8 dBTP`) at this width.
pub const BAR_MIN_W: f32 = 96.0;
/// The smallest square side in which a goniometer's figure still reads as a shape; below
/// it the Goniometer view shows its correlation chip alone.
pub const GONIO_MIN_W: f32 = 120.0;
/// What a plate docked above leaves the bars at least: a bar of 60 px plus up to five text
/// rows and the label.
pub const BAR_MIN_H: f32 = 160.0;
/// The widest label of the LUFS scale over the whole target range (−36 … −5 LUFS): the
/// target's tick, its word and a two-digit number.
pub const LUFS_SCALE_TEMPLATE: &str = "target −41";
/// The widest correlation value the chip shows ([`super::Correlation::chip`]).
pub const CHIP_VALUE_TEMPLATE: &str = "right quiet";
/// The largest text scale a docked plate grows to (the plates' own limit).
pub const FIT_SCALE_MAX: f32 = 3.0;
/// The steps a docked plate's fitted scale is floored to, so a pixel of resize does not
/// flicker its text.
pub const FIT_SCALE_STEP: f32 = 0.05;

/// Which numbers a scale column carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleKind {
    /// The 60 dB span under full scale, every peak bar's.
    Dbfs,
    /// The EBU +9 span around the target, a loudness bar's.
    Lufs,
}

/// A scale column's width at the plot font `small`: one number at every **T** step, so
/// the bars never move when the numbers go, and the target's word fits in the LUFS one.
pub fn scale_w(kind: ScaleKind, small: f32) -> f32 {
    match kind {
        ScaleKind::Dbfs => 3.0 * small,
        ScaleKind::Lufs => text_width(LUFS_SCALE_TEMPLATE, small) + 4.0,
    }
}

/// The correlation chip's least width: its word at `small` and its widest value at `font`
/// with the chip's margins, so the chip is as wide whatever it reads.
pub fn chip_min_w(font: f32, small: f32) -> f32 {
    text_width("correlation", small) + text_width(CHIP_VALUE_TEMPLATE, font) + 24.0
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
    /// The goniometer's square (the Goniometer view, room for a figure).
    pub gonio: Option<Rect>,
    /// The correlation chip (a pair): a strip under the bars, or the chip under the square.
    pub correlation: Option<Rect>,
    /// The bars with their scale columns; empty in the Goniometer view.
    pub bars: Rect,
    /// The docked loudness plate, when it has room.
    pub plate: Option<Docked>,
}

/// What `bars` bars with scale columns `cols_w` wide in all need across.
pub fn bars_min(bars: usize, cols_w: f32) -> f32 {
    cols_w + bars.max(1) as f32 * BAR_MIN_W
}

/// The Meters view without a plate: the bars on the whole rect, and a pair's correlation
/// strip [`CHIP_H`] high under them, as wide as they are.
pub fn layout(rect: Rect, has_pair: bool) -> ProgrammeLayout {
    let (bars, strip) = strip_off(rect, has_pair);
    under(bars, has_pair, strip)
}

/// `rect` less a pair's strip at its foot, and the strip's height.
fn strip_off(rect: Rect, has_pair: bool) -> (Rect, f32) {
    let strip = if has_pair {
        CHIP_H.min(rect.h / 2.0)
    } else {
        0.0
    };
    (Rect::new(rect.x, rect.y, rect.w, rect.h - strip), strip)
}

fn under(bars: Rect, has_pair: bool, strip: f32) -> ProgrammeLayout {
    ProgrammeLayout {
        gonio: None,
        correlation: has_pair.then(|| Rect::new(bars.x, bars.bottom(), bars.w, strip)),
        bars,
        plate: None,
    }
}

/// The largest text scale, floored to [`FIT_SCALE_STEP`] and at most [`FIT_SCALE_MAX`], at
/// which the full plate at the plot font `font` fits in `w` × `h`; `None` when not even
/// 1× does.
pub fn fit_scale(w: f32, h: f32, font: f32) -> Option<f32> {
    let fits = |k: u32| {
        let (pw, ph) = loudness_plate::size(font, k as f32 * FIT_SCALE_STEP, 0);
        pw <= w && ph <= h
    };
    let (lo, hi) = (
        (1.0 / FIT_SCALE_STEP).round() as u32,
        (FIT_SCALE_MAX / FIT_SCALE_STEP).round() as u32,
    );
    if !fits(lo) {
        return None;
    }
    // The plate grows with its scale, so the fitting steps are one run from 1× up.
    let (mut ok, mut bad) = (lo, hi + 1);
    while bad - ok > 1 {
        let mid = (ok + bad) / 2;
        if fits(mid) {
            ok = mid;
        } else {
            bad = mid;
        }
    }
    Some(ok as f32 * FIT_SCALE_STEP)
}

/// The Meters view with its plate docked: beside the bars at the pane's right edge, at the
/// largest scale that leaves the bars `need_w` ([`fit_scale`]) when that is at least 1×;
/// else above them at 1×, then at 1× with the slots dropped one step at a time, retrying
/// beside and above; else no plate. A pair's strip is taken from the bars' height first.
/// Which step lands depends on the pane's size only, never on values.
pub fn dock(rect: Rect, has_pair: bool, need_w: f32, font: f32) -> ProgrammeLayout {
    let (room, strip) = strip_off(rect, has_pair);
    let beside = |w: f32, h: f32, s: f32, dropped: u8| {
        let plate = Docked {
            rect: Rect::new(room.right() - w, room.y, w, h),
            dropped,
            scale: s,
        };
        ProgrammeLayout {
            plate: Some(plate),
            ..under(
                Rect::new(room.x, room.y, room.w - w, room.h),
                has_pair,
                strip,
            )
        }
    };
    if let Some(s) = fit_scale(room.w - need_w, room.h, font) {
        let (w, h) = loudness_plate::size(font, s, 0);
        return beside(w, h, s, 0);
    }
    let place = |dropped: u8| {
        let (w, h) = loudness_plate::size(font, 1.0, dropped);
        if h <= room.h && room.w - w >= need_w {
            Some(beside(w, h, 1.0, dropped))
        } else if w <= room.w && room.h - h >= BAR_MIN_H {
            Some(ProgrammeLayout {
                plate: Some(Docked {
                    rect: Rect::new(room.right() - w, room.y, w, h),
                    dropped,
                    scale: 1.0,
                }),
                ..under(
                    Rect::new(room.x, room.y + h, room.w, room.h - h),
                    has_pair,
                    strip,
                )
            })
        } else {
            None
        }
    };
    (0..=loudness_plate::MAX_DROPPED)
        .find_map(place)
        .unwrap_or_else(|| layout(rect, has_pair))
}

/// The Goniometer view: the largest square centred in `rect` less the chip's height, the
/// chip right under it, centred on it, as wide as the square or `chip_w`, whichever is
/// wider (never wider than the pane); a square under [`GONIO_MIN_W`] is no figure, and
/// the chip stands alone in the pane's centre.
pub fn goniometer(rect: Rect, chip_w: f32) -> ProgrammeLayout {
    let field = Rect::new(rect.x, rect.y, rect.w, (rect.h - CHIP_H).max(0.0));
    let sq = gonio::square(field);
    let none = Rect::new(rect.x, rect.y, 0.0, 0.0);
    if sq.w < GONIO_MIN_W {
        let w = chip_w.min(rect.w);
        return ProgrammeLayout {
            gonio: None,
            correlation: Some(Rect::new(
                rect.x + (rect.w - w) / 2.0,
                rect.y + (rect.h - CHIP_H) / 2.0,
                w,
                CHIP_H.min(rect.h),
            )),
            bars: none,
            plate: None,
        };
    }
    let w = sq.w.max(chip_w).min(rect.w);
    let x = (sq.x + (sq.w - w) / 2.0).clamp(rect.x, rect.right() - w);
    ProgrammeLayout {
        gonio: Some(sq),
        correlation: Some(Rect::new(x, sq.bottom(), w, CHIP_H)),
        bars: none,
        plate: None,
    }
}
