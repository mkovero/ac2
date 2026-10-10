//! The valid-resolution marker: where a curve has more columns than its analysis resolves.
//!
//! The daemon says which ranges of a curve's grid are finer than the analysis behind them
//! ([`ac2_proto::model::Unresolved`], computed in `ac2_core::resolution`). There the curve
//! is no less correct, but neighbouring columns share one estimate: on a live transfer
//! curve the columns without an FFT bin of their own are left out (the line shows gaps),
//! on a sweep the log grid interpolates the linear bins. The scene shades those ranges
//! faintly, edges them with a dashed line and names them on the cursor's frequency line.

use ac2_proto::model::Unresolved;

use crate::axis::Mapping;
use crate::canvas::Canvas;
use crate::format;
use crate::primitives::{FillRect, Polyline, Rect};
use crate::theme::Theme;

/// What the curve does where its grid is finer than its analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Columns without an estimate of their own are left out (a live transfer curve).
    Gaps,
    /// Columns between estimates are interpolated (a sweep's response and harmonics).
    Interpolated,
}

impl Fill {
    fn text(self) -> &'static str {
        match self {
            Fill::Gaps => "gaps",
            Fill::Interpolated => "interpolated",
        }
    }
}

/// A curve's unresolved ranges and what its columns do there.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub unresolved: &'a Unresolved,
    pub fill: Fill,
}

/// One marked range.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Whether the range runs from the curve's lowest column (drawn without a lower edge).
    pub from_bottom: bool,
    /// The whole statement: `below 68 Hz: coarser than 1/48 oct (gaps)`.
    pub text: String,
    /// What the cursor's frequency line adds inside the range: `coarser than 1/48 oct (gaps)`.
    pub note: String,
}

/// The marks of `src` on a curve whose lowest column is at `first_hz` (a range starting
/// at or below it is said as `below …`). `what` names the curve when the pane holds more
/// than one kind (`harmonics`).
pub fn marks(src: Source<'_>, first_hz: f64, what: Option<&str>) -> Vec<Mark> {
    let res = format::resolution(src.unresolved.resolution);
    let note = match what {
        Some(w) => format!("{w} coarser than {res} ({})", src.fill.text()),
        None => format!("coarser than {res} ({})", src.fill.text()),
    };
    src.unresolved
        .ranges
        .iter()
        .filter(|r| r.hi.0 > r.lo.0)
        .map(|r| {
            let from_bottom = r.lo.0 <= first_hz;
            let span = if from_bottom {
                format!("below {}", format::freq_rough(r.hi.0))
            } else {
                format!(
                    "{}–{}",
                    format::freq_rough(r.lo.0),
                    format::freq_rough(r.hi.0)
                )
            };
            Mark {
                lo_hz: r.lo.0,
                hi_hz: r.hi.0,
                from_bottom,
                text: format!("{span}: {note}"),
                note: note.clone(),
            }
        })
        .collect()
}

/// The mark covering `hz`, if any.
pub fn at(marks: &[Mark], hz: f64) -> Option<&Mark> {
    marks.iter().find(|m| hz >= m.lo_hz && hz <= m.hi_hz)
}

/// Shade and edges of `marks` in `plot`, under the grid and the curves.
pub(crate) fn draw(c: &mut Canvas, plot: Rect, xm: &Mapping, marks: &[Mark], theme: &Theme) {
    for m in marks {
        let x0 = xm.to_px(m.lo_hz).max(plot.x);
        let x1 = xm.to_px(m.hi_hz).min(plot.right());
        if !(x0.is_finite() && x1.is_finite()) || x1 <= x0 {
            continue;
        }
        c.base.rects.push(FillRect {
            rect: Rect::new(x0, plot.y, x1 - x0, plot.h),
            color: theme.unresolved_shade,
            clip: Some(plot),
        });
        let lo_edge = (!m.from_bottom).then_some(xm.to_px(m.lo_hz));
        for x in lo_edge.into_iter().chain([xm.to_px(m.hi_hz)]) {
            // An edge on the plot's border would only thicken the frame.
            if x > plot.x + 0.5 && x < plot.right() - 0.5 {
                c.base.polylines.push(Polyline {
                    points: vec![[x, plot.y], [x, plot.bottom()]],
                    alpha: vec![],
                    stroke: theme.unresolved_edge,
                    clip: Some(plot),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{FreqRange, Resolution};
    use ac2_proto::units::Hz;

    fn u(resolution: Resolution, ranges: &[(f64, f64)]) -> Unresolved {
        Unresolved {
            resolution,
            ranges: ranges
                .iter()
                .map(|&(lo, hi)| FreqRange {
                    lo: Hz(lo),
                    hi: Hz(hi),
                })
                .collect(),
        }
    }

    #[test]
    fn marks_say_where_and_how_coarse() {
        let un = u(
            Resolution::NinetySixth,
            &[(31.1, 135.4), (203.0, 406.0), (1625.0, 3250.0)],
        );
        let m = marks(
            Source {
                unresolved: &un,
                fill: Fill::Gaps,
            },
            31.3,
            None,
        );
        let texts: Vec<&str> = m.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "below 140 Hz: coarser than 1/96 oct (gaps)",
                "200 Hz–410 Hz: coarser than 1/96 oct (gaps)",
                "1.6 kHz–3.3 kHz: coarser than 1/96 oct (gaps)",
            ]
        );
        assert!(m[0].from_bottom && !m[1].from_bottom);
        assert_eq!(
            at(&m, 300.0).map(|m| m.note.as_str()),
            Some("coarser than 1/96 oct (gaps)")
        );
        assert!(at(&m, 500.0).is_none());

        let h = u(Resolution::FortyEighth, &[(19.9, 67.6)]);
        let m = marks(
            Source {
                unresolved: &h,
                fill: Fill::Interpolated,
            },
            20.0,
            Some("harmonics"),
        );
        assert_eq!(
            m[0].text,
            "below 68 Hz: harmonics coarser than 1/48 oct (interpolated)"
        );
    }
}
