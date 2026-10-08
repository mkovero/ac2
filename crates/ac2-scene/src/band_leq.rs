//! The band meter of an SPL meter (`docs/design/band-leq.md`): eleven bars, 20 … 200 Hz,
//! each the band's rolling Leq against its limit line, the worst band named with what to do
//! about it, the limit set in force and where the limits come from, and the predicted
//! dwelling LAeq. Read from a stage: the headline says which band eats the neighbour's
//! budget and for how long to back off; the bars show where in the spectrum.
//!
//! Display truth only: the daemon judges (the `band_leq` frame carries every judgement and
//! figure), this module words and colours it, with the Leq windows' vocabulary
//! (`stay ≤ …`, `cooling down in …`) and the alarms' names (`63 Hz band Leq`,
//! `predicted dwelling LAeq`).

use ac2_proto::frame::BandLeqMeta;
use ac2_proto::model::{
    BandLeqPreset, BandLimitPlace, BandPeriod, BandTransferBand, BandTransferSet, LF_BAND_COUNT,
    LeqJudgement, LevelScale,
};

use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, anchor, label};
use crate::format;
use crate::leq::{TileState, clock, column_colors, length, tile_colors};
use crate::primitives::{Color, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport};
use crate::theme::Theme;

/// A band's name as the alarms, the CLI and the app say it: `63 Hz band Leq`.
pub fn band_name(nominal_hz: f64) -> String {
    format!("{} Hz band Leq", band_label(nominal_hz))
}

/// A band's nominal centre as its bar is labelled: `20`, `31.5`, `200`.
pub fn band_label(nominal_hz: f64) -> String {
    format!("{nominal_hz}")
}

/// What the bands are: `band Leq 60 min, unweighted`.
pub fn meter_name(duration_s: f64) -> String {
    format!("band Leq {}, unweighted", length(duration_s))
}

/// Every string and figure of one band's bar.
#[derive(Clone, Debug, PartialEq)]
pub struct BandBar {
    pub nominal_hz: f64,
    /// `63`.
    pub label: String,
    /// `63 Hz band Leq`.
    pub name: String,
    /// `45.2`, `—` before anything was measured.
    pub value: String,
    /// The Leq as shown (rounded to 0.1 dB), NaN before anything was measured.
    pub leq_db: f64,
    /// The limit at the mic when judged.
    pub limit_db: Option<f64>,
    /// `limit 42.0 dB`; none without a limit.
    pub limit: Option<String>,
    pub state: TileState,
    /// `OVER`, `NEAR`, `ON COURSE`, `OK`, `not calibrated`; none without a limit.
    pub state_text: Option<String>,
    pub on_course: bool,
    /// Floored steady level allowed (a ceiling to stay under).
    pub allowed_db: Option<f64>,
    /// `next 1 min: stay ≤ 41.3 dB`, `until full: stay ≤ 40.2 dB`.
    pub headroom: Option<String>,
    pub recover_s: Option<f64>,
    /// `cooling down in 6 min 52 s` when it cannot recover within the horizon.
    pub recover: Option<String>,
    /// Leq − limit, dB (positive: over), when judged and measured.
    pub over_db: Option<f64>,
}

/// The predicted dwelling LAeq as shown.
#[derive(Clone, Debug, PartialEq)]
pub struct PredictedText {
    /// `predicted dwelling LAeq 23.5 dB · at most 27.3 dB · limit 25.0 dB · NEAR`.
    pub line: String,
    pub state: TileState,
}

/// Everything the band view says.
#[derive(Clone, Debug, PartialEq)]
pub struct BandLeqText {
    /// `band Leq 60 min, unweighted`.
    pub name: String,
    /// `dB SPL` or `dBFS`.
    pub unit: String,
    pub bars: Vec<BandBar>,
    /// Index of the worst band, as the daemon picked it.
    pub worst: Option<usize>,
    /// `63 Hz band Leq 3.2 dB over its limit · cooling down in 6 min 52 s`.
    pub headline: String,
    pub headline_state: TileState,
    /// `night limits (22–07)`, `day limits (07–22) · headroom for the night limits from
    /// 22:00`.
    pub period: String,
    /// `limits transferred from the dwelling`, `dwelling limits at the mic (no transfer)`.
    pub limits_from: String,
    /// `§13 correction +5 dB`.
    pub correction: Option<String>,
    /// `so far · 30:00 / 1:00:00` while the windows fill.
    pub filling: Option<String>,
    /// `offline for 10 s`: time in the windows with no audio.
    pub incomplete: Option<String>,
    pub predicted: Option<PredictedText>,
}

fn state_text(state: TileState, on_course: bool) -> Option<String> {
    match state {
        TileState::NoLimit => None,
        TileState::NotCalibrated => Some("not calibrated".into()),
        TileState::Ok => Some("OK".into()),
        TileState::Near if on_course => Some("ON COURSE".into()),
        TileState::Near => Some("NEAR".into()),
        TileState::Over => Some("OVER".into()),
    }
}

fn judged(j: LeqJudgement) -> bool {
    matches!(
        j,
        LeqJudgement::Ok | LeqJudgement::Near | LeqJudgement::Over
    )
}

fn rounded(v: f64) -> f64 {
    if v.is_finite() {
        (v * 10.0).round() / 10.0
    } else {
        f64::NAN
    }
}

/// The period line: the set the windows are judged by and, when the headroom is computed
/// against the other set, which.
pub fn period_text(period: BandPeriod, after_horizon: BandPeriod) -> String {
    let now = match period {
        BandPeriod::Night => "night limits (22–07)",
        BandPeriod::Day => "day limits (07–22)",
    };
    match (period, after_horizon) {
        (BandPeriod::Day, BandPeriod::Night) => {
            format!("{now} · headroom for the night limits from 22:00")
        }
        (BandPeriod::Night, BandPeriod::Day) => format!("{now} · headroom for the day limits"),
        _ => now.to_owned(),
    }
}

/// Where the limits judged at the mic come from.
pub fn limits_from_text(p: BandLimitPlace) -> &'static str {
    match p {
        BandLimitPlace::AtMic => "dwelling limits at the mic (no transfer)",
        BandLimitPlace::Transferred => "limits transferred from the dwelling",
    }
}

/// The band view's text from a `band_leq` frame.
pub fn band_leq_text(m: &BandLeqMeta) -> BandLeqText {
    let unit = match m.scale {
        LevelScale::DbSpl => "dB SPL",
        LevelScale::Dbfs => "dBFS",
    };
    let duration = m.duration.0;
    let elapsed = m.elapsed.0;
    let filling = elapsed < duration;
    // A window filling for at least the horizon has its headroom until it is full.
    let until_full = filling && duration - elapsed >= m.horizon.0;
    let horizon = length(m.horizon.0);
    let bars: Vec<BandBar> = m
        .bands
        .iter()
        .map(|b| {
            let state = TileState::from(b.judgement);
            let is_judged = judged(b.judgement);
            let on_course = is_judged && b.on_course;
            // Floored: a ceiling to stay under.
            let allowed_db = b
                .allowed
                .filter(|a| is_judged && a.is_finite())
                .map(|a| (a * 10.0 + 1e-6).floor() / 10.0);
            let headroom = allowed_db.map(|a| {
                let a = format::level(a);
                if until_full {
                    format!("until full: stay ≤ {a} dB")
                } else {
                    format!("next {horizon}: stay ≤ {a} dB")
                }
            });
            let recover_s = b
                .recover
                .map(|r| r.0)
                .filter(|r| is_judged && r.is_finite());
            let cannot = is_judged && allowed_db.is_none() && state == TileState::Over;
            let recover = match recover_s {
                Some(r) => Some(format!("cooling down in {}", format::duration(r))),
                None if cannot => Some("cooling down".to_owned()),
                None => None,
            };
            let leq_db = rounded(b.leq);
            let limit_db = b.limit.filter(|l| is_judged && l.is_finite());
            BandBar {
                nominal_hz: b.nominal.0,
                label: band_label(b.nominal.0),
                name: band_name(b.nominal.0),
                value: format::level(b.leq),
                leq_db,
                limit_db,
                limit: b
                    .limit
                    .filter(|l| l.is_finite())
                    .map(|l| format!("limit {} dB", format::level(l))),
                state,
                state_text: state_text(state, on_course),
                on_course,
                allowed_db,
                headroom,
                recover_s,
                recover,
                over_db: limit_db
                    .filter(|_| leq_db.is_finite())
                    .map(|l| rounded(leq_db - l)),
            }
        })
        .collect();
    let worst = m.worst.map(usize::from).filter(|&i| i < bars.len());
    let (headline, headline_state) = headline(m, &bars, worst);
    let correction = (m.correction.0 != 0.0)
        .then(|| format!("§13 correction {} dB", format::signed(m.correction.0, 0)));
    let incomplete = (elapsed.min(duration) - m.measured.0 >= 1.0).then(|| {
        format!(
            "offline for {}",
            length((elapsed.min(duration) - m.measured.0).round())
        )
    });
    BandLeqText {
        name: meter_name(duration),
        unit: unit.to_owned(),
        bars,
        worst,
        headline,
        headline_state,
        period: period_text(m.period, m.period_after_horizon),
        limits_from: limits_from_text(m.limits_from).to_owned(),
        correction,
        filling: filling.then(|| format!("so far · {} / {}", clock(elapsed), clock(duration))),
        incomplete,
        predicted: m.predicted.map(|p| predicted_text(&p, m.period)),
    }
}

fn headline(m: &BandLeqMeta, bars: &[BandBar], worst: Option<usize>) -> (String, TileState) {
    if m.scale == LevelScale::Dbfs {
        return (
            "not calibrated: band levels in dBFS, limits not judged".to_owned(),
            TileState::NotCalibrated,
        );
    }
    if bars.iter().all(|b| !b.leq_db.is_finite()) {
        return (
            "waiting for the first second".to_owned(),
            TileState::NoLimit,
        );
    }
    let Some(b) = worst.and_then(|i| bars.get(i)) else {
        let state = m
            .predicted
            .map_or(TileState::NoLimit, |p| p.judgement.into());
        return ("no band limits".to_owned(), state);
    };
    let what = match b.over_db {
        Some(d) if d > 0.0 => format!("{} {} dB over its limit", b.name, format::level(d)),
        Some(d) if d < 0.0 => format!("{} {} dB under its limit", b.name, format::level(-d)),
        Some(_) => format!("{} at its limit", b.name),
        None => b.name.clone(),
    };
    let todo = b.recover.clone().or_else(|| b.headroom.clone());
    let mut line = what;
    if b.on_course {
        line.push_str(" · on course to go over");
    }
    if let Some(t) = todo {
        line.push_str(" · ");
        line.push_str(&t);
    }
    (line, b.state)
}

/// The predicted dwelling LAeq line.
pub fn predicted_text(p: &ac2_proto::model::PredictedLeq, period: BandPeriod) -> PredictedText {
    let mut parts = vec![format!("predicted dwelling LAeq {}", with_db(p.estimate))];
    // The unusable bands at their bound can only raise it: shown when they do.
    if p.at_most.is_finite() && (!p.estimate.is_finite() || p.at_most - p.estimate >= 0.05) {
        parts.push(format!("at most {} dB", format::level(p.at_most)));
    }
    match p.limit {
        Some(l) => parts.push(format!("limit {} dB", format::level(l.0))),
        None => parts.push(match period {
            BandPeriod::Night => "no night limit".to_owned(),
            BandPeriod::Day => "no day limit".to_owned(),
        }),
    }
    let state = TileState::from(p.judgement);
    if let Some(s) = state_text(state, false) {
        parts.push(s);
    }
    PredictedText {
        line: parts.join(" · "),
        state,
    }
}

fn with_db(v: f64) -> String {
    if v.is_finite() {
        format!("{} dB", format::level(v))
    } else {
        format::NO_VALUE.to_owned()
    }
}

/// What a band preset sets, as the dialog and the CLI show it.
pub fn preset_summary(p: BandLeqPreset) -> String {
    let c = p.config(None);
    let mut parts = Vec::new();
    let first = c.night.first().copied().flatten();
    let last = c.night.last().copied().flatten();
    if let (Some(a), Some(b)) = (first, last) {
        parts.push(format!(
            "{} 20 … 200 Hz, night {} … {} dB, day {} dB higher",
            meter_name(c.duration.0),
            a.0,
            b.0,
            BandLeqPreset::FINLAND_545_DAY_OFFSET_DB
        ));
    }
    let mut pred = Vec::new();
    if let Some(d) = c.predicted.day {
        pred.push(format!("day ≤ {} dB", d.0));
    }
    if let Some(n) = c.predicted.night {
        pred.push(format!("night ≤ {} dB", n.0));
    }
    if !pred.is_empty() {
        parts.push(format!("predicted dwelling LAeq {}", pred.join(", ")));
    }
    format!("{}: {}", p.name(), parts.join("; "))
}

/// Where a band preset's figures come from, with the caveats it carries.
pub fn preset_source(p: BandLeqPreset) -> String {
    format!(
        "{} — informational, not legal advice; a prediction from FOH is not a measurement \
         in the dwelling",
        p.source()
    )
}

/// One band of a transfer: `63 Hz 18.2 dB`, `40 Hz ≥ 35.0 dB (under the background)`.
pub fn transfer_band_text(nominal_hz: f64, b: &BandTransferBand) -> String {
    let hz = format!("{} Hz", band_label(nominal_hz));
    match *b {
        BandTransferBand::Unchecked { attenuation } => {
            format!("{hz} {} dB (no background)", format::level(attenuation.0))
        }
        BandTransferBand::Clean { attenuation } => {
            format!("{hz} {} dB", format::level(attenuation.0))
        }
        BandTransferBand::Corrected {
            attenuation,
            margin,
        } => format!(
            "{hz} {} dB (background subtracted, {} dB over it)",
            format::level(attenuation.0),
            format::level(margin.0)
        ),
        BandTransferBand::Unusable { at_least } => format!(
            "{hz} ≥ {} dB (under the background: a bound)",
            format::level(at_least.0)
        ),
        BandTransferBand::Missing => format!("{hz} not measured"),
    }
}

/// The transfer of the limited bands in a line: `transfer 20–200 Hz: 8 clean, 1 corrected,
/// 1 bound, 1 not measured`; `no transfer: limits judged at the mic`.
pub fn transfer_summary(t: Option<&BandTransferSet>) -> String {
    let Some(t) = t else {
        return "no transfer: dwelling limits judged at the mic".to_owned();
    };
    let mut n = [0usize; 5];
    for b in &t.bands[..LF_BAND_COUNT] {
        n[match b {
            BandTransferBand::Clean { .. } => 0,
            BandTransferBand::Corrected { .. } => 1,
            BandTransferBand::Unchecked { .. } => 2,
            BandTransferBand::Unusable { .. } => 3,
            BandTransferBand::Missing => 4,
        }] += 1;
    }
    let parts: Vec<String> = [
        "clean",
        "background subtracted",
        "no background",
        "bound",
        "not measured",
    ]
    .iter()
    .zip(n)
    .filter(|(_, k)| *k > 0)
    .map(|(w, k)| format!("{k} {w}"))
    .collect();
    format!("transfer 20–200 Hz: {}", parts.join(", "))
}

// ---------------------------------------------------------------------------------------
// Scene

/// What the band view shows.
#[derive(Clone, Debug, PartialEq)]
pub struct BandLeqView {
    /// `FOH SPL`.
    pub meter: String,
    /// Calibration text, with the mic.
    pub cal: String,
    pub text: BandLeqText,
    /// `STALE 3.2 s`.
    pub stale: Option<String>,
}

/// The band view as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct BandLeqScene {
    pub scene: Scene,
    /// Each band's column, low to high.
    pub columns: Vec<Rect>,
    /// The headline's box.
    pub headline: Rect,
    /// The bar scale, dB (bottom, top).
    pub range: (f64, f64),
    pub banners: Vec<BannerRow>,
}

/// Below the lowest limit…
const BELOW_LIMIT_DB: f64 = 20.0;
/// …and above the highest: room to see how far over a band is.
const ABOVE_LIMIT_DB: f64 = 6.0;

/// The bar scale: 10 dB steps around the limits and the values.
pub fn bar_range(bars: &[BandBar]) -> (f64, f64) {
    let limits: Vec<f64> = bars.iter().filter_map(|b| b.limit_db).collect();
    let values: Vec<f64> = bars
        .iter()
        .map(|b| b.leq_db)
        .filter(|v| v.is_finite())
        .collect();
    let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (lo, hi) = if limits.is_empty() {
        if values.is_empty() {
            return (0.0, 100.0);
        }
        (min(&values) - 10.0, max(&values) + ABOVE_LIMIT_DB)
    } else {
        (
            min(&limits) - BELOW_LIMIT_DB,
            max(&limits).max(max(&values)) + ABOVE_LIMIT_DB,
        )
    };
    let lo = (lo / 10.0).floor() * 10.0;
    let hi = ((hi / 10.0).ceil() * 10.0).max(lo + 10.0);
    (lo, hi)
}

/// Lays the band view out in `size`: the meter and calibration on one line, the headline
/// large, the period, the limits' place and the prediction under it, then eleven columns.
pub fn band_leq_scene(
    v: &BandLeqView,
    status: &Status,
    theme: &Theme,
    size: Viewport,
) -> BandLeqScene {
    let mut c = Canvas::new(size, theme);
    let pad = 10.0;
    let strip = canvas::banner_strip(&mut c, status, pad, size.width - 2.0 * pad, size, theme);
    let t = &v.text;
    let fs = theme.font_size;
    let w = (size.width - 2.0 * pad).max(1.0);
    let mut y = strip.rect.bottom() + pad;

    // Caption: the meter and what the bars are, the calibration (and STALE) right.
    let left = format!("{} · {}, {}", v.meter, t.name, t.unit);
    let right = match &v.stale {
        Some(s) => format!("{s} · {}", v.cal),
        None => v.cal.clone(),
    };
    let right = crate::spl::cut(&right, w * 0.5, fs);
    let left = crate::spl::cut(&left, w - canvas::text_width(&right, fs) - fs, fs);
    c.overlay.labels.push(label(
        left,
        [pad, y],
        anchor(HAlign::Left, VAlign::Top),
        fs,
        theme.text,
    ));
    c.overlay.labels.push(label(
        right,
        [size.width - pad, y],
        anchor(HAlign::Right, VAlign::Top),
        fs,
        if v.stale.is_some() {
            theme.banner_warning.background
        } else {
            theme.text_dim
        },
    ));
    y += fs * 1.6;

    // The headline: what to do, large; on the judgement's colour.
    let big = (size.height * 0.06).clamp(fs, theme.big_font_size.max(fs));
    let big = fit(&t.headline, w - pad * 2.0, big, fs * 0.9);
    let head_h = big * 1.6;
    let headline = Rect::new(pad, y, w, head_h);
    let (bg, ink) = tile_colors(t.headline_state, theme);
    c.base.rects.push(FillRect {
        rect: headline,
        color: bg,
        clip: None,
    });
    c.overlay.labels.push(label(
        t.headline.clone(),
        [pad * 2.0, y + head_h / 2.0],
        anchor(HAlign::Left, VAlign::Center),
        big,
        ink,
    ));
    y += head_h + pad * 0.6;

    // The small lines: period, limits' place, correction, filling; then the prediction.
    let mut info = vec![t.period.clone(), t.limits_from.clone()];
    info.extend(t.correction.iter().cloned());
    info.extend(t.filling.iter().cloned());
    info.extend(t.incomplete.iter().cloned());
    let sfs = theme.small_font_size.max(fs * 0.85);
    c.overlay.labels.push(label(
        crate::spl::cut(&info.join(" · "), w, sfs),
        [pad, y],
        anchor(HAlign::Left, VAlign::Top),
        sfs,
        theme.text_dim,
    ));
    y += sfs * 1.5;
    if let Some(p) = &t.predicted {
        let pfs = (big * 0.7).max(fs);
        let pfs = fit(&p.line, w, pfs, sfs);
        let ink = match p.state {
            TileState::Over => theme.banner_fault.background,
            TileState::Near => theme.banner_warning.background,
            _ => theme.text,
        };
        c.overlay.labels.push(label(
            p.line.clone(),
            [pad, y],
            anchor(HAlign::Left, VAlign::Top),
            pfs,
            ink,
        ));
        y += pfs * 1.5;
    }

    // The columns: a y scale left, eleven columns, the band labels under them.
    let range = bar_range(&t.bars);
    let scale_w = fs * 3.0;
    let label_fs = fs.max(((w - scale_w) / 11.0 * 0.22).min(fs * 1.6));
    let below_h = label_fs * 1.4 + sfs * 1.4;
    let area = Rect::new(
        pad + scale_w,
        y + pad * 0.5,
        (w - scale_w).max(1.0),
        (size.height - y - pad * 1.5 - below_h).max(1.0),
    );
    let to_y = |db: f64| -> f32 {
        let k = ((db - range.0) / (range.1 - range.0)).clamp(0.0, 1.0);
        area.bottom() - (k as f32) * area.h
    };
    let mut db = range.0;
    while db <= range.1 + 1e-9 {
        let gy = to_y(db);
        c.base.polylines.push(Polyline {
            points: vec![[area.x, gy], [area.right(), gy]],
            alpha: Vec::new(),
            stroke: theme.grid_major,
            clip: None,
        });
        c.overlay.labels.push(label(
            format::fixed(db, 0),
            [area.x - 4.0, gy],
            anchor(HAlign::Right, VAlign::Center),
            sfs,
            theme.axis_text,
        ));
        db += 10.0;
    }
    let n = t.bars.len().max(1);
    let col_w = area.w / n as f32;
    let gap = (col_w * 0.12).max(2.0);
    let mut columns = Vec::with_capacity(t.bars.len());
    for (i, b) in t.bars.iter().enumerate() {
        let x = area.x + i as f32 * col_w + gap / 2.0;
        let cw = (col_w - gap).max(1.0);
        let col = Rect::new(x, area.y, cw, area.h);
        columns.push(col);
        let (track, bar) = column_colors(b.state, theme);
        c.base.rects.push(FillRect {
            rect: col,
            color: track,
            clip: None,
        });
        if b.leq_db.is_finite() {
            let top = to_y(b.leq_db);
            c.data.rects.push(FillRect {
                rect: Rect::new(x, top, cw, (area.bottom() - top).max(0.0)),
                color: bar,
                clip: None,
            });
        }
        // The headroom: how loud the band may go, a thin line under the limit.
        // Off the scale it would sit on the top edge, reading as a level it is not.
        if let Some(a) = b.allowed_db.filter(|&a| a <= range.1) {
            let ay = to_y(a);
            c.overlay.polylines.push(Polyline {
                points: vec![[x + cw * 0.2, ay], [x + cw * 0.8, ay]],
                alpha: Vec::new(),
                stroke: Stroke::solid(theme.text_dim, 2.0),
                clip: None,
            });
        }
        if let Some(l) = b.limit_db {
            let ly = to_y(l);
            c.overlay.polylines.push(Polyline {
                points: vec![[x - gap * 0.3, ly], [x + cw + gap * 0.3, ly]],
                alpha: Vec::new(),
                stroke: Stroke::solid(theme.text, 3.0),
                clip: None,
            });
        }
        // The value, held low on the column: read up close, the bars read from afar.
        let vfs = (cw * 0.24).clamp(sfs * 0.8, fs * 1.4);
        let value_ink = if b.leq_db.is_finite() && to_y(b.leq_db) < area.bottom() - vfs * 1.6 {
            readable_on(bar, theme)
        } else {
            theme.text
        };
        c.overlay.labels.push(label(
            b.value.clone(),
            [x + cw / 2.0, area.bottom() - 4.0],
            anchor(HAlign::Center, VAlign::Bottom),
            vfs,
            value_ink,
        ));
        let worst = t.worst == Some(i);
        c.overlay.labels.push(label(
            b.label.clone(),
            [x + cw / 2.0, area.bottom() + 4.0],
            anchor(HAlign::Center, VAlign::Top),
            label_fs,
            if worst { theme.text } else { theme.axis_text },
        ));
        let under = match b.state {
            TileState::Over => Some(
                b.recover_s
                    .map_or_else(|| "OVER".to_owned(), crate::leq::time_to),
            ),
            _ => b.allowed_db.map(|a| format!("≤ {}", format::level(a))),
        };
        if let Some(u) = under {
            c.overlay.labels.push(label(
                u,
                [x + cw / 2.0, area.bottom() + 4.0 + label_fs * 1.3],
                anchor(HAlign::Center, VAlign::Top),
                sfs.min(cw * 0.22).max(READABLE),
                if b.state == TileState::Over {
                    theme.banner_fault.background
                } else {
                    theme.text_dim
                },
            ));
        }
        if worst {
            c.overlay.polylines.push(Polyline {
                points: vec![
                    [x, area.y],
                    [x + cw, area.y],
                    [x + cw, area.bottom()],
                    [x, area.bottom()],
                    [x, area.y],
                ],
                alpha: Vec::new(),
                stroke: Stroke::solid(theme.text, 2.0),
                clip: None,
            });
        }
    }
    c.overlay.labels.push(label(
        "Hz",
        [area.x - 4.0, area.bottom() + 4.0],
        anchor(HAlign::Right, VAlign::Top),
        sfs,
        theme.axis_text,
    ));
    BandLeqScene {
        scene: c.into_scene(size),
        columns,
        headline,
        range,
        banners: strip.rows,
    }
}

/// Text smaller than this is left out of a narrow column.
const READABLE: f32 = 8.0;

/// `size`, shrunk down to `min` until `text` fits `w`.
fn fit(text: &str, w: f32, size: f32, min: f32) -> f32 {
    let need = canvas::text_width(text, size);
    if need <= w {
        size
    } else {
        (size * w / need).max(min)
    }
}

/// Text ink on a coloured bar.
fn readable_on(bar: Color, theme: &Theme) -> Color {
    if crate::theme::contrast_ratio(theme.text, bar) >= 3.0 {
        theme.text
    } else {
        theme.background
    }
}

#[cfg(test)]
mod tests;
