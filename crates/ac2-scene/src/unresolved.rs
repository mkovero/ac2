//! The valid-resolution marker: where a curve has more columns than its analysis resolves.
//!
//! The daemon says which ranges of a curve's grid are finer than the analysis behind them
//! ([`ac2_proto::model::Unresolved`], computed in `ac2_core::resolution`). There the curve
//! is no less correct, but neighbouring columns share one estimate: on a live transfer
//! curve the columns without an FFT bin of their own are left out (the line shows gaps),
//! on a sweep the log grid interpolates the linear bins. The scene shades those ranges
//! faintly, edges them with a dashed line and names them on the cursor's frequency line.

use ac2_proto::model::{FreqRange, Unresolved};

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
    /// Neighbouring columns repeat the bin they lie in (a multi-resolution spectrum).
    Steps,
}

impl Fill {
    fn text(self) -> &'static str {
        match self {
            Fill::Gaps => "gaps",
            Fill::Interpolated => "interpolated",
            Fill::Steps => "steps",
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
pub struct RangeMark {
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
pub fn marks(src: Source<'_>, first_hz: f64, what: Option<&str>) -> Vec<RangeMark> {
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
            RangeMark {
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
pub fn at(marks: &[RangeMark], hz: f64) -> Option<&RangeMark> {
    marks.iter().find(|m| hz >= m.lo_hz && hz <= m.hi_hz)
}

/// The marks of a curve corrected with a mic curve whose points span `range`, on columns
/// from `first_hz` to `last_hz`: outside the span the curve is held at its end value, so
/// the columns there show the mic's own response uncorrected.
pub fn mic_marks(range: FreqRange, first_hz: f64, last_hz: f64) -> Vec<RangeMark> {
    const NOTE: &str = "outside the mic curve";
    let (lo, hi) = (range.lo.0, range.hi.0);
    let below = (first_hz < lo).then(|| RangeMark {
        lo_hz: first_hz,
        hi_hz: lo,
        from_bottom: true,
        text: format!("below {}: {NOTE}", format::freq_rough(lo)),
        note: NOTE.to_owned(),
    });
    let above = (last_hz > hi).then(|| RangeMark {
        lo_hz: hi,
        hi_hz: last_hz,
        from_bottom: false,
        text: format!("above {}: {NOTE}", format::freq_rough(hi)),
        note: NOTE.to_owned(),
    });
    below.into_iter().chain(above).collect()
}

/// What the marks covering `hz` say, joined; `None` outside every mark.
pub fn note_at(marks: &[RangeMark], hz: f64) -> Option<String> {
    let notes: Vec<&str> = marks
        .iter()
        .filter(|m| hz >= m.lo_hz && hz <= m.hi_hz)
        .map(|m| m.note.as_str())
        .collect();
    (!notes.is_empty()).then(|| notes.join(" · "))
}

/// Shade and edges of `marks` in `plot`, under the grid and the curves.
pub(crate) fn draw(c: &mut Canvas, plot: Rect, xm: &Mapping, marks: &[RangeMark], theme: &Theme) {
    for m in marks {
        let x0 = xm.to_px(m.lo_hz).max(plot.x);
        let x1 = xm.to_px(m.hi_hz).min(plot.right());
        if !(x0.is_finite() && x1.is_finite()) || x1 <= x0 {
            continue;
        }
        c.base.rects.push(FillRect::clipped(
            Rect::new(x0, plot.y, x1 - x0, plot.h),
            theme.unresolved_shade,
            plot,
        ));
        let lo_edge = (!m.from_bottom).then_some(xm.to_px(m.lo_hz));
        for x in lo_edge.into_iter().chain([xm.to_px(m.hi_hz)]) {
            // An edge on the plot's border would only thicken the frame.
            if x > plot.x + 0.5 && x < plot.right() - 0.5 {
                c.base.polylines.push(Polyline::clipped(
                    vec![[x, plot.y], [x, plot.bottom()]],
                    theme.unresolved_edge,
                    plot,
                ));
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

#[cfg(test)]
mod mic_tests {
    use super::*;
    use ac2_proto::units::Hz;

    fn span(lo: f64, hi: f64) -> FreqRange {
        FreqRange {
            lo: Hz(lo),
            hi: Hz(hi),
        }
    }

    #[test]
    fn a_curve_shorter_than_the_columns_is_marked_at_both_ends() {
        let m = mic_marks(span(20.0, 20_000.0), 7.8, 256_000.0);
        let texts: Vec<&str> = m.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "below 20 Hz: outside the mic curve",
                "above 20 kHz: outside the mic curve"
            ]
        );
        assert!(m[0].from_bottom && !m[1].from_bottom);
        assert_eq!((m[0].lo_hz, m[0].hi_hz), (7.8, 20.0));
        assert_eq!((m[1].lo_hz, m[1].hi_hz), (20_000.0, 256_000.0));
    }

    #[test]
    fn a_curve_covering_the_columns_marks_nothing() {
        assert!(mic_marks(span(5.0, 300_000.0), 7.8, 256_000.0).is_empty());
    }

    #[test]
    fn the_cursor_says_every_mark_it_is_in() {
        let mut marks = mic_marks(span(20.0, 20_000.0), 7.8, 256_000.0);
        marks.insert(
            0,
            RangeMark {
                lo_hz: 7.8,
                hi_hz: 68.0,
                from_bottom: true,
                text: String::new(),
                note: "coarser than 1/48 oct (gaps)".into(),
            },
        );
        assert_eq!(
            note_at(&marks, 12.0).as_deref(),
            Some("coarser than 1/48 oct (gaps) · outside the mic curve")
        );
        assert_eq!(
            note_at(&marks, 40.0).as_deref(),
            Some("coarser than 1/48 oct (gaps)")
        );
        assert_eq!(note_at(&marks, 1_000.0), None);
    }
}
