//! The valid-resolution marker: where a curve has more columns than its analysis resolves.
//!
//! The daemon says which ranges of a curve's grid are finer than the analysis behind them
//! (computed in `ac2_core::resolution`). There the curve is no less correct, but its
//! columns are narrower than the bins behind them: on a live transfer curve columns
//! without an estimate are passed over, the line joins the estimates, each drawn as a
//! point, and the daemon states how wide the serving bins are ([`BinRange`]); on a sweep
//! the log grid interpolates the linear bins. The scene shades those ranges faintly, edges them with a
//! dashed line and names them on the cursor's frequency line.

use ac2_proto::model::{BinRange, FreqRange, Unresolved};

use crate::axis::Mapping;
use crate::canvas::Canvas;
use crate::format;
use crate::primitives::{FillRect, Polyline, Rect};
use crate::theme::Theme;

/// A curve's unresolved ranges and what its columns do there.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    /// A live transfer curve's ranges, each with the width of the bins serving it. Columns
    /// without a bin of their own ([`ac2_proto::frame::ValidityMask::THINNED`] alone) are
    /// passed over: the estimates are joined by a line and each is drawn as a point.
    Joined(&'a [BinRange]),
    /// Columns between estimates are interpolated (a sweep's response and harmonics, an
    /// imported impulse response).
    Interpolated(&'a Unresolved),
    /// Neighbouring columns repeat the bin they lie in (a multi-resolution spectrum).
    Steps(&'a Unresolved),
}

impl Source<'_> {
    /// The ranges, Hz, as `(lo, hi)`.
    pub fn ranges(&self) -> Vec<(f64, f64)> {
        match self {
            Source::Joined(r) => r.iter().map(|b| (b.lo_hz.0, b.hi_hz.0)).collect(),
            Source::Interpolated(u) | Source::Steps(u) => {
                u.ranges.iter().map(|r| (r.lo_hz.0, r.hi_hz.0)).collect()
            }
        }
    }
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
    let said = |s: String| match what {
        Some(w) => format!("{w} {s}"),
        None => s,
    };
    let ranges: Vec<(f64, f64, String)> = match src {
        Source::Joined(r) => r
            .iter()
            .map(|b| {
                let note = said(format!("{} bins", format::bin_width(b.bin_hz.0)));
                (b.lo_hz.0, b.hi_hz.0, note)
            })
            .collect(),
        Source::Interpolated(u) | Source::Steps(u) => {
            let how = if matches!(src, Source::Steps(_)) {
                "steps"
            } else {
                "interpolated"
            };
            let res = format::resolution(u.resolution);
            let note = said(format!("coarser than {res} ({how})"));
            u.ranges
                .iter()
                .map(|r| (r.lo_hz.0, r.hi_hz.0, note.clone()))
                .collect()
        }
    };
    ranges
        .into_iter()
        .filter(|(lo, hi, _)| hi > lo)
        .map(|(lo, hi, note)| {
            let from_bottom = lo <= first_hz;
            let span = if from_bottom {
                format!("below {}", format::freq_rough(hi))
            } else {
                format!("{}–{}", format::freq_rough(lo), format::freq_rough(hi))
            };
            RangeMark {
                lo_hz: lo,
                hi_hz: hi,
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
    let (lo, hi) = (range.lo_hz.0, range.hi_hz.0);
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
    let (lo, hi) = (range.lo_hz.0.max(first_hz), range.hi_hz.0.min(last_hz));
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
                    lo_hz: Hz(lo),
                    hi_hz: Hz(hi),
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
        let m = marks(Source::Interpolated(&un), 31.3, None);
        let texts: Vec<&str> = m.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "below 140 Hz: coarser than 1/96 oct (interpolated)",
                "200 Hz–410 Hz: coarser than 1/96 oct (interpolated)",
                "1.6 kHz–3.3 kHz: coarser than 1/96 oct (interpolated)",
            ]
        );
        assert!(m[0].from_bottom && !m[1].from_bottom);
        assert_eq!(
            at(&m, 300.0).map(|m| m.note.as_str()),
            Some("coarser than 1/96 oct (interpolated)")
        );
        assert!(at(&m, 500.0).is_none());

        let h = u(Resolution::FortyEighth, &[(19.9, 67.6)]);
        let m = marks(Source::Interpolated(&h), 20.0, Some("harmonics"));
        assert_eq!(
            m[0].text,
            "below 68 Hz: harmonics coarser than 1/48 oct (interpolated)"
        );
    }
}

#[cfg(test)]
mod bin_tests {
    use super::*;
    use ac2_proto::units::Hz;

    fn bins(r: &[(f64, f64, f64)]) -> Vec<BinRange> {
        r.iter()
            .map(|&(lo, hi, bin)| BinRange {
                lo_hz: Hz(lo),
                hi_hz: Hz(hi),
                bin_hz: Hz(bin),
            })
            .collect()
    }

    #[test]
    fn each_joined_range_says_its_bin_width() {
        // 48 kHz, 1/96: the deepest stage's bins at the bottom, the shallower stages' above
        // each crossover.
        let r = bins(&[
            (7.8, 135.3, 48_000.0 / 49_152.0),
            (203.0, 406.0, 12_000.0 / 4096.0),
            (811.0, 1623.0, 48_000.0 / 4096.0),
        ]);
        let m = marks(Source::Joined(&r), 7.8, None);
        let texts: Vec<&str> = m.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "below 140 Hz: 1 Hz bins",
                "200 Hz–410 Hz: 2.9 Hz bins",
                "810 Hz–1.6 kHz: 12 Hz bins",
            ]
        );
        assert_eq!(at(&m, 300.0).map(|m| m.note.as_str()), Some("2.9 Hz bins"));
        // The long LF window's deepest stage.
        let r = bins(&[(7.8, 16.9, 48_000.0 / 196_608.0)]);
        let m = marks(Source::Joined(&r), 7.8, None);
        assert_eq!(m[0].text, "below 17 Hz: 0.24 Hz bins");
        assert_eq!(m[0].note, "0.24 Hz bins");
    }

    #[test]
    fn a_range_at_the_top_of_the_columns_is_named_by_its_edges() {
        let r = bins(&[(16_000.0, 21_600.0, 46.875)]);
        let m = marks(Source::Joined(&r), 7.8, None);
        assert_eq!(m[0].text, "16 kHz–22 kHz: 47 Hz bins");
        assert!(!m[0].from_bottom);
    }
}

#[cfg(test)]
mod mic_tests {
    use super::*;
    use ac2_proto::units::Hz;

    fn span(lo: f64, hi: f64) -> FreqRange {
        FreqRange {
            lo_hz: Hz(lo),
            hi_hz: Hz(hi),
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
                note: "1 Hz bins".into(),
            },
        );
        assert_eq!(
            note_at(&marks, 12.0).as_deref(),
            Some("1 Hz bins · outside the mic curve")
        );
        assert_eq!(note_at(&marks, 40.0).as_deref(), Some("1 Hz bins"));
        assert_eq!(note_at(&marks, 1_000.0), None);
    }
}
