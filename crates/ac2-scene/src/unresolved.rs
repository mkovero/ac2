//! The valid-resolution marker: where a curve has more columns than its analysis resolves.
//!
//! The daemon says which ranges of a curve's grid are finer than the analysis behind them
//! ([`ac2_proto::model::Unresolved`], computed in `ac2_core::resolution`). There the curve
//! is no less correct, but neighbouring columns share one estimate: on a live transfer
//! curve the columns without an FFT bin of their own are passed over (the line joins the
//! bins, each drawn as a point), on a sweep the log grid interpolates the linear bins. The
//! scene shades those ranges faintly, edges them with a dashed line and names them on the
//! cursor's frequency line.

use ac2_proto::frame::ValidityMask;
use ac2_proto::model::{FreqRange, Unresolved};

use crate::axis::Mapping;
use crate::canvas::Canvas;
use crate::format;
use crate::primitives::{FillRect, Polyline, Rect};
use crate::theme::Theme;

/// What the curve does where its grid is finer than its analysis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fill<'a> {
    /// Columns without an estimate of their own ([`ValidityMask::THINNED`] alone) are passed
    /// over: the estimates are joined by a line and each is drawn as a point (a live
    /// transfer curve). The curve's columns come along because the estimates in a range,
    /// one bin apart, are what say how wide its bins are.
    Joined {
        freqs: &'a [f64],
        validity: &'a [ValidityMask],
    },
    /// Columns between estimates are interpolated (a sweep's response and harmonics).
    Interpolated,
    /// Neighbouring columns repeat the bin they lie in (a multi-resolution spectrum).
    Steps,
}

impl Fill<'_> {
    fn text(self) -> &'static str {
        match self {
            Fill::Joined { .. } => "gaps",
            Fill::Interpolated => "interpolated",
            Fill::Steps => "steps",
        }
    }
}

/// A curve's unresolved ranges and what its columns do there.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub unresolved: &'a Unresolved,
    pub fill: Fill<'a>,
}

/// The width of the bins serving `lo..=hi` of a joined curve, from the columns there that
/// carry an estimate. Below the edge a column is narrower than a bin, so it holds at most
/// one, and every bin lands in some column: consecutive estimates are exactly one bin apart.
/// Each is drawn at its column's centre, up to half a column off its bin, and those offsets
/// do not average out pairwise (columns widen along the range, so the spacings skew); the
/// span from the first estimate to the last over the bins between them leaves only the two
/// end columns' offsets, spread over every bin in the range. `None` with fewer than two
/// estimates, or with no thinned column: then the columns are no narrower than the bins and
/// their spacing says nothing of how wide a bin is.
pub fn bin_width(freqs: &[f64], validity: &[ValidityMask], lo: f64, hi: f64) -> Option<f64> {
    let inside = || {
        freqs
            .iter()
            .zip(validity)
            .filter(move |(f, _)| **f >= lo && **f <= hi)
    };
    let thinned = |v: &ValidityMask| v.0 & ValidityMask::THINNED.0 != 0;
    if !inside().any(|(_, v)| thinned(v)) {
        return None;
    }
    let mut own = inside().filter(|(_, v)| !thinned(v)).map(|(f, _)| *f);
    let first = own.next()?;
    let (n, last) = own.fold((0usize, first), |(n, _), f| (n + 1, f));
    (n > 0 && last > first).then(|| (last - first) / n as f64)
}

/// One marked range.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeMark {
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Whether the range runs from the curve's lowest column (drawn without a lower edge).
    pub from_bottom: bool,
    /// The whole statement: `below 68 Hz: 1 Hz bins`, `below 68 Hz: coarser than 1/48 oct
    /// (interpolated)`.
    pub text: String,
    /// What the cursor's frequency line adds inside the range: `1 Hz bins`.
    pub note: String,
}

/// The marks of `src` on a curve whose lowest column is at `first_hz` (a range starting
/// at or below it is said as `below …`). `what` names the curve when the pane holds more
/// than one kind (`harmonics`).
pub fn marks(src: Source<'_>, first_hz: f64, what: Option<&str>) -> Vec<RangeMark> {
    let res = format::resolution(src.unresolved.resolution);
    let coarser = match what {
        Some(w) => format!("{w} coarser than {res} ({})", src.fill.text()),
        None => format!("coarser than {res} ({})", src.fill.text()),
    };
    src.unresolved
        .ranges
        .iter()
        .filter(|r| r.hi.0 > r.lo.0)
        .map(|r| {
            let bins = match src.fill {
                Fill::Joined { freqs, validity } => bin_width(freqs, validity, r.lo.0, r.hi.0),
                Fill::Interpolated | Fill::Steps => None,
            };
            let note = match (bins, what) {
                (Some(b), Some(w)) => format!("{w} {} bins", format::bin_width(b)),
                (Some(b), None) => format!("{} bins", format::bin_width(b)),
                (None, _) => coarser.clone(),
            };
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
                note,
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

/// The mark of a curve whose averages are still filling over `range`, on columns from
/// `first_hz` to `last_hz`: the longer windows of the low analysis stages take longer to
/// reach their depth, so the bottom of a curve can still be moving when the rest has
/// settled.
pub fn settling_marks(range: FreqRange, first_hz: f64, last_hz: f64) -> Vec<RangeMark> {
    const NOTE: &str = "still averaging";
    let (lo, hi) = (range.lo.0.max(first_hz), range.hi.0.min(last_hz));
    if hi <= lo {
        return Vec::new();
    }
    let from_bottom = lo <= first_hz;
    let text = match (from_bottom, hi >= last_hz) {
        (true, true) => NOTE.to_owned(),
        (true, false) => format!("below {}: {NOTE}", format::freq_rough(hi)),
        (false, true) => format!("above {}: {NOTE}", format::freq_rough(lo)),
        (false, false) => format!(
            "{} – {}: {NOTE}",
            format::freq_rough(lo),
            format::freq_rough(hi)
        ),
    };
    vec![RangeMark {
        lo_hz: lo,
        hi_hz: hi,
        from_bottom,
        text,
        note: NOTE.to_owned(),
    }]
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
        // A curve with no estimates to measure its bins by keeps the resolution's words.
        let m = marks(
            Source {
                unresolved: &un,
                fill: Fill::Joined {
                    freqs: &[],
                    validity: &[],
                },
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
mod bin_tests {
    use super::*;
    use ac2_proto::model::Resolution;
    use ac2_proto::units::Hz;

    /// A 1/48-octave grid from 7.8 Hz whose columns hold the bins `k·df` (at most one each
    /// below the edge): a column with a bin is valid, the rest thinned, as the daemon sends.
    fn grid(df: f64) -> (Vec<f64>, Vec<ValidityMask>) {
        let freqs: Vec<f64> = (0..200)
            .map(|i| 7.8 * 2f64.powf(f64::from(i) / 48.0))
            .collect();
        let validity = freqs
            .iter()
            .map(|f| {
                let (lo, hi) = (f * 2f64.powf(-1.0 / 96.0), f * 2f64.powf(1.0 / 96.0));
                let has_bin = (lo / df).ceil() * df < hi;
                if has_bin {
                    ValidityMask::NONE
                } else {
                    ValidityMask::THINNED
                }
            })
            .collect();
        (freqs, validity)
    }

    fn mark(df: f64) -> RangeMark {
        let (freqs, validity) = grid(df);
        // The range ends where a column grows as wide as a bin.
        let edge = df / (2f64.powf(1.0 / 48.0) - 1.0);
        let un = Unresolved {
            resolution: Resolution::FortyEighth,
            ranges: vec![FreqRange {
                lo: Hz(freqs[0]),
                hi: Hz(edge),
            }],
        };
        let m = marks(
            Source {
                unresolved: &un,
                fill: Fill::Joined {
                    freqs: &freqs,
                    validity: &validity,
                },
            },
            freqs[0],
            None,
        );
        assert_eq!(m.len(), 1);
        m[0].clone()
    }

    #[test]
    fn a_joined_range_says_its_bin_width() {
        // 48 kHz / 49152 and / 196608: the bins of the standard and long low stages.
        let m = mark(48_000.0 / 49_152.0);
        assert_eq!(m.text, "below 67 Hz: 1 Hz bins");
        assert_eq!(m.note, "1 Hz bins");
        let m = mark(48_000.0 / 196_608.0);
        assert_eq!(m.text, "below 17 Hz: 0.24 Hz bins");
        assert_eq!(m.note, "0.24 Hz bins");
    }

    #[test]
    fn a_range_with_one_estimate_keeps_the_resolution() {
        let freqs = [20.0, 20.3, 20.6];
        let validity = [
            ValidityMask::THINNED,
            ValidityMask::NONE,
            ValidityMask::THINNED,
        ];
        let un = Unresolved {
            resolution: Resolution::FortyEighth,
            ranges: vec![FreqRange {
                lo: Hz(20.0),
                hi: Hz(21.0),
            }],
        };
        let m = marks(
            Source {
                unresolved: &un,
                fill: Fill::Joined {
                    freqs: &freqs,
                    validity: &validity,
                },
            },
            20.0,
            None,
        );
        assert_eq!(m[0].note, "coarser than 1/48 oct (gaps)");
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
    fn a_curve_still_averaging_says_where() {
        let text = |lo, hi| {
            let m = settling_marks(span(lo, hi), 7.8, 21_600.0);
            assert_eq!(m.len(), 1);
            assert_eq!(m[0].note, "still averaging");
            m[0].text.clone()
        };
        assert_eq!(text(7.8, 90.0), "below 90 Hz: still averaging");
        assert_eq!(text(7.8, 21_600.0), "still averaging");
        assert_eq!(text(68.0, 350.0), "68 Hz – 350 Hz: still averaging");
        assert_eq!(text(2000.0, 21_600.0), "above 2 kHz: still averaging");
        // Outside the columns there is nothing to mark.
        assert!(settling_marks(span(1.0, 5.0), 7.8, 21_600.0).is_empty());
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
