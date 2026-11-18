//! The harmonics plate: what a live spectrum reads of the generator's sine — the
//! fundamental, each harmonic re it, THD and THD+N — and the marks of those harmonics on
//! the curve. Orders are named and coloured, and ratios written, as the sweep's distortion
//! view has them ([`crate::distortion`]).

use super::*;
use crate::distortion::{Reading, order_color, order_name, percent_text, reading_text};
use crate::plate::{Metrics, PlateBox, PlateKind, PlatePlace};
use crate::view::DistortionUnit;
use ac2_proto::frame::{HarmonicLevel, HarmonicReading, HarmonicUnreadable, SpecHarmonics};

/// The lowest ratio the plate writes, dB re the fundamental. A sample path of 24-bit
/// mantissas rounds near −144 dB re full scale and the longest FFT spreads that over
/// some 50 dB more of bins, so nothing measured lies below about −200 dB: a ratio under
/// it (down to the wire's `HARMONIC_DB_MIN`, a power of exactly zero) only says the
/// arithmetic found nothing, and reads as below this limit.
pub const RATIO_DB_MIN: f64 = -200.0;

/// THD+N reads as the window's while the off-bin sine's own leakage is within this of
/// it, dB: 10 dB under the total the leakage adds 0.4 dB to it; nearer, the figure is more
/// the window's than the device's.
pub const LEAKAGE_MARGIN_DB: f64 = 10.0;

/// One live curve's reading of the sine as listed and marked.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumHarmonics {
    /// The curve.
    pub name: String,
    /// Its rows on the plate, each with the order it is about (0: none, drawn as text).
    pub rows: Vec<HarmonicRow>,
    /// The fundamental and each resolved harmonic where it is on the curve; empty at
    /// *traces only* and without a level axis.
    pub marks: Vec<HarmonicMark>,
}

/// A row of the plate.
#[derive(Clone, Debug, PartialEq)]
pub struct HarmonicRow {
    pub text: String,
    /// The order a row of one read harmonic is about (its mark's colour); `None` for
    /// every other row.
    pub order: Option<u8>,
}

/// A harmonic's mark on the curve.
#[derive(Clone, Debug, PartialEq)]
pub struct HarmonicMark {
    pub order: u8,
    pub freq_hz: f64,
    /// The point of the curve it sits on, logical pixels.
    pub at: [f32; 2],
    /// `H2`.
    pub label: String,
}

fn row(text: String) -> HarmonicRow {
    HarmonicRow { text, order: None }
}

/// `sine 999.02 Hz · on bin`: the frequency fine enough to tell a sine snapped to a bin
/// from the one asked for.
pub fn sine_line(h: &SpecHarmonics) -> String {
    format!(
        "sine {} · {}",
        format::freq_exact(h.f0.0),
        if h.on_bin { "on bin" } else { "between bins" }
    )
}

/// Why a spectrum has no reading, in words.
pub fn unreadable_text(r: HarmonicUnreadable) -> &'static str {
    match r {
        HarmonicUnreadable::RectangularOffBin => {
            "not read: a rectangular window needs the sine on a bin"
        }
        HarmonicUnreadable::TooLow => "not read: the sine is too low for this FFT length",
        HarmonicUnreadable::OutOfBand => "not read: the sine is above this spectrum's range",
        HarmonicUnreadable::NoFundamental => "not read: no sine in this input",
    }
}

/// A ratio re the fundamental as a reading: a level, or a limit when `limit` or below
/// what can be measured at all ([`RATIO_DB_MIN`]).
fn ratio(db: f64, limit: bool) -> Reading {
    if limit || db < RATIO_DB_MIN {
        Reading::BelowFloor(db.max(RATIO_DB_MIN))
    } else {
        Reading::Level(db)
    }
}

fn ratio_text(db: f64, limit: bool) -> String {
    reading_text(ratio(db, limit), DistortionUnit::Db)
}

/// `−95.1 dB · 0.00176 %`; a limit in dB alone, as a percentage of a limit would read as
/// a measured share.
fn ratio_both(db: f64) -> String {
    match ratio(db, false) {
        Reading::Level(v) => format!("{} · {}", ratio_text(v, false), percent_text(v)),
        _ => ratio_text(db, false),
    }
}

/// `H4`, `H4–H10`, `H4, H6–H10`: orders (ascending) with each run of neighbours folded.
pub fn orders_text(orders: &[u8]) -> String {
    let mut runs: Vec<(u8, u8)> = Vec::new();
    for &k in orders {
        match runs.last_mut() {
            Some((_, hi)) if u16::from(*hi) + 1 == u16::from(k) => *hi = k,
            _ => runs.push((k, k)),
        }
    }
    runs.iter()
        .map(|&(lo, hi)| {
            if lo == hi {
                order_name(lo)
            } else {
                format!("{}–{}", order_name(lo), order_name(hi))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What an unresolved harmonic is below: the larger of its level and its floor, as the
/// daemon's distortion limit counts it.
fn limit_of(h: &HarmonicLevel) -> f64 {
    h.level.0.max(h.floor.0)
}

/// The plate's rows for one curve's reading `h`, its fundamental in `scale`.
pub fn rows(h: &SpecHarmonics, scale: LevelScale) -> Vec<HarmonicRow> {
    let mut out = vec![row(sine_line(h))];
    let (fundamental, harmonics, thd, thd_bound, thd_n, leakage) = match &h.reading {
        HarmonicReading::Unreadable { reason } => {
            out.push(row(unreadable_text(*reason).to_string()));
            return out;
        }
        HarmonicReading::Read {
            fundamental,
            harmonics,
            thd,
            thd_bound,
            thd_n,
            leakage,
        } => (*fundamental, harmonics, *thd, *thd_bound, *thd_n, *leakage),
    };
    // The fundamental as the spectrum's cursor reads a tone there.
    out.push(HarmonicRow {
        text: format!(
            "{}  {} {}",
            order_name(1),
            format::level(fundamental),
            value_unit(scale, Quantity::Tone)
        ),
        order: Some(1),
    });
    for k in harmonics.iter().filter(|k| k.resolved) {
        out.push(HarmonicRow {
            text: format!("{}  {}", order_name(k.order), ratio_text(k.level.0, false)),
            order: Some(k.order),
        });
    }
    // Each unresolved harmonic is below its own floor; one row for all of them is true
    // only at the highest of those.
    let unresolved: Vec<&HarmonicLevel> = harmonics.iter().filter(|k| !k.resolved).collect();
    if let Some(limit) = unresolved.iter().map(|k| limit_of(k)).reduce(f64::max) {
        let orders: Vec<u8> = unresolved.iter().map(|k| k.order).collect();
        out.push(row(format!(
            "{}  {}",
            orders_text(&orders),
            ratio_text(limit, true)
        )));
    }
    out.push(row(match thd {
        Some(thd) => format!("THD  {}", ratio_both(thd.0)),
        None => format!("THD  {}", ratio_text(thd_bound.0, true)),
    }));
    out.push(row(match leakage {
        // The window's leakage is in the figure: the device's own is below it, by how
        // much the spectrum cannot say.
        Some(l) if thd_n.0 - l.0 <= LEAKAGE_MARGIN_DB => format!(
            "THD+N  {} · window leakage (sine between bins)",
            ratio_text(thd_n.0, true)
        ),
        _ => format!("THD+N  {}", ratio_both(thd_n.0)),
    }));
    out
}

/// Each live curve's reading of the sine, in `traces` order, with its marks while
/// `marks` (T) draws marks at all and there is a level axis `ym` to put them on.
pub(crate) fn harmonics_of(
    traces: &[SpectrumTrace<'_>],
    xm: &crate::axis::Mapping,
    ym: Option<&crate::axis::Mapping>,
    plot: Rect,
    marks: bool,
) -> Vec<SpectrumHarmonics> {
    traces
        .iter()
        .filter_map(|t| {
            let h = t.harmonics?;
            let marks = match (ym, &h.reading) {
                (Some(ym), HarmonicReading::Read { harmonics, .. }) if marks => std::iter::once(1)
                    .chain(harmonics.iter().filter(|k| k.resolved).map(|k| k.order))
                    .filter_map(|order| mark_at(t, h.f0.0, order, xm, ym, plot))
                    .collect(),
                _ => Vec::new(),
            };
            let name = if t.offset_db != 0.0 {
                offset_note(&t.name, t.offset_db)
            } else {
                t.name.clone()
            };
            Some(SpectrumHarmonics {
                name,
                rows: rows(h, t.scale),
                marks,
            })
        })
        .collect()
}

/// The mark of order `order` of a sine at `f0` on curve `t`: on the curve's own point
/// there (the column holding `order`·`f0`), so it sits on what is drawn whatever the
/// column shows of the bins under it; `None` off the plot.
fn mark_at(
    t: &SpectrumTrace<'_>,
    f0: f64,
    order: u8,
    xm: &crate::axis::Mapping,
    ym: &crate::axis::Mapping,
    plot: Rect,
) -> Option<HarmonicMark> {
    let hz = f0 * f64::from(order);
    let i = crate::grid::nearest_column(t.freqs, hz)?;
    let (x, y) = (xm.to_px(t.freqs[i]), ym.to_px(t.value(i)));
    let inside = x.is_finite()
        && y.is_finite()
        && x >= plot.x
        && x <= plot.right()
        && y >= plot.y
        && y <= plot.bottom();
    inside.then(|| HarmonicMark {
        order,
        freq_hz: hz,
        at: [x, y],
        label: order_name(order),
    })
}

/// Draws the marks of `h` (a diamond on the curve, the order's name over it, both in the
/// order's colour), dimmed like the curve while `stale`. `raised`: the curve's tracked
/// peaks are marked too, whose ranks sit where the name would, so the name goes a line up.
pub(crate) fn draw_marks(
    c: &mut Canvas,
    h: &SpectrumHarmonics,
    plot: Rect,
    stale: bool,
    raised: bool,
    theme: &Theme,
) {
    let font = theme.small_font_size;
    let alpha = if stale { theme.stale_alpha } else { 1.0 };
    for m in &h.marks {
        let color = order_color(theme, m.order).with_alpha(alpha);
        let [x, y] = m.at;
        let r = MARK_HALF;
        c.overlay.polylines.push(Polyline::clipped(
            vec![[x - r, y], [x, y - r], [x + r, y], [x, y + r], [x - r, y]],
            Stroke::solid(color, 1.5),
            plot,
        ));
        let lift = if raised { font * 1.25 } else { 0.0 };
        let mut l = label(
            m.label.clone(),
            [x, y - r - 2.0 - lift],
            anchor(HAlign::Center, VAlign::Bottom),
            font,
            color,
        );
        l.clip = Some(plot);
        c.overlay.labels.push(l);
    }
}

/// The plate's text over `listed`: a block per curve, the curve's name heading its block
/// when the blocks could be read against one another's or its levels are spread by an
/// offset (as the peak list heads its rows). Each line with the order it is about.
pub(crate) fn list_lines(listed: &[(&SpectrumTrace<'_>, &SpectrumHarmonics)]) -> Vec<HarmonicRow> {
    let several = listed.len() > 1;
    let mut lines = Vec::new();
    for (t, h) in listed {
        if several || t.offset_db != 0.0 {
            lines.push(row(h.name.clone()));
        }
        lines.extend(h.rows.iter().cloned());
    }
    lines
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// Draws the harmonics plate with `lines`. Placed by the scene, it stands in the plot's
/// bottom-left corner, `foot` above the plot's bottom edge (the labels drawn along it):
/// harmonics lie above the fundamental, so the left is where the plate covers none of the
/// marks it names. It goes over each of the plates in `avoid` it would cover (a legend,
/// cursor values or peak list dragged there); with no room left over them it stands beside
/// them instead, to their right. Dragged, it sits where it was dropped.
pub(crate) fn harmonics_plate(
    c: &mut Canvas,
    lines: &[HarmonicRow],
    plot: Rect,
    foot: f32,
    avoid: &[PlateBox],
    view: &ViewState,
    theme: &Theme,
) -> PlateBox {
    let place = &view.plates.harmonics;
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    let (w, h) = Metrics::new(theme, place.scale()).size(&texts);
    let s = crate::plate::fitted(place.scale(), plot, w, h);
    let m = Metrics::new(theme, s);
    let (w, h) = m.size(&texts);
    let gap = 0.5 * theme.small_font_size;
    let rect = match place.at {
        Some(at) => PlatePlace::rect_at(at, plot, w, h),
        None => {
            let inset = crate::legend::INSET;
            let x = plot.x + inset;
            // A plot too low for the plate and the labels under it keeps the plate inside.
            let y = (plot.bottom() - inset - foot - h).max(plot.y + inset);
            let mut r = Rect::new(x, y, w, h);
            // Each plate in the way pushes it up at most once.
            for _ in avoid {
                match avoid.iter().find(|b| overlaps(r, b.rect)) {
                    Some(b) => r.y = b.rect.y - gap - h,
                    None => break,
                }
            }
            if r.y < plot.y + inset && r.y < y {
                let column = avoid
                    .iter()
                    .filter(|b| b.rect.x < x + w)
                    .map(|b| b.rect.right())
                    .fold(x, f32::max);
                r = Rect::new((column + gap).min(plot.right() - inset - w).max(x), y, w, h);
            }
            r
        }
    };
    let colored: Vec<(String, Color)> = lines
        .iter()
        .map(|l| {
            let color = l.order.map_or(theme.text, |k| order_color(theme, k));
            (l.text.clone(), color)
        })
        .collect();
    let hover = view.plate_hover.filter(|h| h.kind == PlateKind::Harmonics);
    crate::plate::draw_lines(
        &mut c.overlay,
        rect,
        &colored,
        HAlign::Left,
        m,
        true,
        hover,
        plot,
        theme,
    );
    PlateBox::new(PlateKind::Harmonics, plot, rect, s)
}

/// The harmonics of `traces` drawn: their marks, and their plate `foot` above the plot's
/// bottom edge clear of `avoid`, unless *traces only* (T) hides marks and lists. Each curve's reading, the
/// plate's text and its box.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    c: &mut Canvas,
    traces: &[SpectrumTrace<'_>],
    xm: &crate::axis::Mapping,
    ym: Option<&crate::axis::Mapping>,
    plot: Rect,
    foot: f32,
    avoid: &[PlateBox],
    view: &ViewState,
    theme: &Theme,
) -> (Vec<SpectrumHarmonics>, Option<String>, Option<PlateBox>) {
    let shown = view.chrome.peak_marks();
    let harmonics = harmonics_of(traces, xm, ym, plot, shown);
    let listed: Vec<(&SpectrumTrace<'_>, &SpectrumHarmonics)> = traces
        .iter()
        .filter(|t| t.harmonics.is_some())
        .zip(&harmonics)
        .collect();
    if !shown || listed.is_empty() {
        return (harmonics, None, None);
    }
    for (t, h) in &listed {
        draw_marks(c, h, plot, t.is_stale(), t.tracked.is_some(), theme);
    }
    let lines = list_lines(&listed);
    let b = harmonics_plate(c, &lines, plot, foot, avoid, view, theme);
    let text = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    (harmonics, Some(text), Some(b))
}
