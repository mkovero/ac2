//! Where a multi-resolution spectrum's windows change.
//!
//! A multi-resolution spectrum serves each frequency from the windows its frame names
//! ([`ac2_proto::frame::SpecBands`]): the longest alone below the first band's `sole`
//! frequency, the shortest alone above the last, two neighbouring windows blended between.
//! The columns therefore follow a change faster, and update more often, the higher they
//! are. The scene marks each band's `sole` frequency with a faint dashed line (the
//! valid-resolution marker's style and toggle) labelled with that window's length, and the
//! cursor's frequency line names the windows behind the column under it: a curve that
//! moves at the top while it holds still at the bottom then reads as what it is.

use ac2_proto::frame::SpecBands;

use crate::axis::Mapping;
use crate::canvas::{Canvas, anchor, label, text_width};
use crate::primitives::{HAlign, Polyline, Rect, VAlign};
use crate::theme::Theme;

/// One band's line.
#[derive(Clone, Debug, PartialEq)]
pub struct Stitch {
    /// Where the band alone serves, Hz.
    pub hz: f64,
    /// Its window: `683 ms`.
    pub label: String,
}

/// A window length: `683 ms`, `21 ms`, `1.37 s`.
pub fn window(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{:.0} ms", seconds * 1000.0)
    } else {
        format!("{seconds:.2} s")
    }
}

/// The lines of `bands`, lowest first.
pub fn stitches(bands: &SpecBands) -> Vec<Stitch> {
    bands
        .bands
        .iter()
        .map(|b| Stitch {
            hz: b.sole.0,
            label: window(b.window.0),
        })
        .collect()
}

/// The windows behind a column at `hz`: `683 ms window` where one band serves,
/// `85 + 43 ms windows` in a blend.
pub fn windows_at(bands: &SpecBands, hz: f64) -> Option<String> {
    let b = &bands.bands;
    let (first, last) = (b.first()?, b.last()?);
    if hz <= first.sole.0 {
        return Some(format!("{} window", window(first.window.0)));
    }
    if hz >= last.sole.0 {
        return Some(format!("{} window", window(last.window.0)));
    }
    let i = b.windows(2).position(|w| hz < w[1].sole.0)?;
    let ms = |s: f64| format!("{:.0}", s * 1000.0);
    Some(format!(
        "{} + {} ms windows",
        ms(b[i].window.0),
        ms(b[i + 1].window.0)
    ))
}

/// The lines in `plot`, labelled at its foot when `labels` (a label that would run into
/// the one before is left out; the line stays).
pub(crate) fn draw(
    c: &mut Canvas,
    plot: Rect,
    xm: &Mapping,
    stitches: &[Stitch],
    labels: bool,
    theme: &Theme,
) {
    let font = theme.small_font_size;
    let mut free_from = plot.x;
    for s in stitches {
        let x = xm.to_px(s.hz);
        if !(x.is_finite() && x > plot.x + 0.5 && x < plot.right() - 0.5) {
            continue;
        }
        c.base.polylines.push(Polyline {
            points: vec![[x, plot.y], [x, plot.bottom()]],
            alpha: vec![],
            stroke: theme.unresolved_edge,
            clip: Some(plot),
        });
        let w = text_width(&s.label, font);
        if labels && x + 3.0 >= free_from && x + 3.0 + w <= plot.right() - 2.0 {
            c.base.labels.push(label(
                s.label.clone(),
                [x + 3.0, plot.bottom() - 3.0],
                anchor(HAlign::Left, VAlign::Bottom),
                font,
                theme.text_dim,
            ));
            free_from = x + 3.0 + w + 6.0;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ac2_proto::frame::SpecBand;
    use ac2_proto::model::{FreqRange, Resolution, Unresolved};
    use ac2_proto::units::{Hz, Seconds};

    /// The bands of a multi-resolution spectrum at 96 kHz.
    pub(crate) fn bands_96k() -> SpecBands {
        let fs = 96_000.0;
        let kappa = 1.0 / (2f64.powf(1.0 / 96.0) - 2f64.powf(-1.0 / 96.0));
        SpecBands {
            bands: [65_536u32, 32_768, 16_384, 8192, 4096, 2048]
                .iter()
                .zip([8192u32, 4096, 2048, 2048, 2048, 1024])
                .map(|(&n, hop)| SpecBand {
                    window: Seconds(f64::from(n) / fs),
                    hop: Seconds(f64::from(hop) / fs),
                    sole: Hz(2.0 * kappa * fs / f64::from(n)),
                    age: Some(Seconds(0.0)),
                })
                .collect(),
            unresolved: Unresolved {
                resolution: Resolution::FortyEighth,
                ranges: vec![FreqRange {
                    lo: Hz(7.8125),
                    hi: Hz(101.5),
                }],
            },
        }
    }

    #[test]
    fn a_line_per_band_named_by_its_window() {
        let s = stitches(&bands_96k());
        let labels: Vec<&str> = s.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            ["683 ms", "341 ms", "171 ms", "85 ms", "43 ms", "21 ms"]
        );
        assert!((s[0].hz - 202.9).abs() < 0.1 && (s[5].hz - 6492.0).abs() < 2.0);
    }

    #[test]
    fn the_cursor_names_the_windows_behind_a_column() {
        let b = bands_96k();
        assert_eq!(windows_at(&b, 50.0).as_deref(), Some("683 ms window"));
        assert_eq!(
            windows_at(&b, 1000.0).as_deref(),
            Some("171 + 85 ms windows")
        );
        assert_eq!(windows_at(&b, 12_000.0).as_deref(), Some("21 ms window"));
    }
}
