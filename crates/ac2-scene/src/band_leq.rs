//! The band meter of an SPL meter (`docs/design/band-leq.md`): per band window a row of
//! bars, one per shown band, each the band's rolling Leq against its limit line; the worst
//! band of the window nearest to (or over) its limit named with what to do about it, where
//! the limits come from when a transfer moved them, and the predicted level at the
//! transfer's place. Read from a stage: the headline says which band eats the budget and
//! for how long to back off; the bars show where in the spectrum.
//!
//! Display truth only: the daemon judges (the `band_leq` frame carries every judgement and
//! figure), this module words and colours it, with the Leq windows' vocabulary (`LZeq 60
//! min`, `stay ≤ …`, `cooling down in …`) and the alarms' names (`63 Hz band LZeq 60 min`).

use ac2_proto::frame::BandLeqFrame;
use ac2_proto::model::{
    BAND_NOMINAL_HZ, BandLeqConfig, BandLeqPreset, BandLimitPlace, BandLimitSet, BandPeriod,
    BandTransferBand, BandTransferSet, BandWindow, LeqJudgement, LevelScale, PredictedLeq,
    TransferOrigin, Weighting,
};

use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, anchor, label};
use crate::format;
use crate::leq::{TileState, clock, column_colors, length, tile_colors, w_letter};
use crate::primitives::{Color, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport};
use crate::theme::Theme;

/// A band window's name, as a Leq window's: `LZeq 60 min`.
pub fn window_name(duration_s: f64, weighting: Weighting) -> String {
    format!("L{}eq {}", w_letter(weighting), length(duration_s))
}

/// A band of a band window as the alarms, the CLI and the app say it: `63 Hz band LZeq 60
/// min`.
pub fn band_window_name(nominal_hz: f64, duration_s: f64, weighting: Weighting) -> String {
    format!(
        "{} Hz band {}",
        band_label(nominal_hz),
        window_name(duration_s, weighting)
    )
}

/// A band's nominal centre as its bar is labelled: `20`, `31.5`, `200`.
pub fn band_label(nominal_hz: f64) -> String {
    format!("{nominal_hz}")
}

/// A run of bands in words: `20–200 Hz`, `63 Hz`, `40–80 Hz, 1000 Hz` (indices into
/// [`BAND_NOMINAL_HZ`], low to high).
pub fn bands_text(bands: &[usize]) -> String {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &b in bands {
        match runs.last_mut() {
            Some((_, hi)) if *hi + 1 == b => *hi = b,
            _ => runs.push((b, b)),
        }
    }
    if runs.is_empty() {
        return "no bands".to_owned();
    }
    runs.iter()
        .map(|&(lo, hi)| {
            let (a, b) = (
                band_label(BAND_NOMINAL_HZ[lo]),
                band_label(BAND_NOMINAL_HZ[hi]),
            );
            if lo == hi {
                format!("{a} Hz")
            } else {
                format!("{a}–{b} Hz")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The place a transfer moves the limits from, as every text names it.
pub fn place_of(t: Option<&BandTransferSet>) -> &str {
    t.map_or(BandTransferSet::DEFAULT_PLACE, |t| t.place.as_str())
}

/// Every string and figure of one band's bar.
#[derive(Clone, Debug, PartialEq)]
pub struct BandBar {
    pub nominal_hz: f64,
    /// `63`.
    pub label: String,
    /// `63 Hz band LZeq 60 min`.
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

impl BandBar {
    /// Severity, as the daemon ranks the worst band: ok, near, on course, over.
    fn rank(&self) -> u8 {
        match self.state {
            TileState::Over => 3,
            TileState::Near if self.on_course => 2,
            TileState::Near => 1,
            _ => 0,
        }
    }
}

/// The predicted level at the transfer's place as shown.
#[derive(Clone, Debug, PartialEq)]
pub struct PredictedText {
    /// `predicted LAeq 60 min in flat 4 bedroom 23.5 dB · at most 27.3 dB · limit 25.0 dB ·
    /// NEAR`.
    pub line: String,
    pub state: TileState,
}

/// One band window as shown.
#[derive(Clone, Debug, PartialEq)]
pub struct BandWindowText {
    /// `LZeq 60 min`.
    pub name: String,
    pub bars: Vec<BandBar>,
    /// Index of the worst band, as the daemon picked it.
    pub worst: Option<usize>,
    /// `night limits (22–07)`, `day limits (07–22) · headroom for the night limits from
    /// 22:00`; none for a window whose limits hold day and night.
    pub period: Option<String>,
    /// `so far · 30:00 / 1:00:00` while the window fills.
    pub filling: Option<String>,
    /// `offline for 10 s`: time in the window with no audio.
    pub incomplete: Option<String>,
    /// The key to the thick mark across a column: `limit`; none when no band has a judged
    /// limit.
    pub limit_key: Option<String>,
    /// The key to the thin mark under it: `next 1 min: stay ≤` (`until full: stay ≤`); none
    /// when no band has one.
    pub allowed_key: Option<String>,
}

impl BandWindowText {
    /// `LZeq 60 min · night limits (22–07) · so far · 30:00 / 1:00:00`.
    pub fn caption(&self) -> String {
        let mut parts = vec![self.name.clone()];
        parts.extend(self.period.iter().cloned());
        parts.extend(self.filling.iter().cloned());
        parts.extend(self.incomplete.iter().cloned());
        parts.join(" · ")
    }
}

/// Everything the band view says.
#[derive(Clone, Debug, PartialEq)]
pub struct BandLeqText {
    /// `dB SPL` or `dBFS`.
    pub unit: String,
    /// The bands shown, `20–200 Hz`.
    pub bands: String,
    pub windows: Vec<BandWindowText>,
    /// The window and band the headline is about: the worst band of the window nearest to
    /// (or over) its limit.
    pub worst: Option<(usize, usize)>,
    /// `63 Hz band LZeq 60 min 3.2 dB over its limit · cooling down in 6 min 52 s`.
    pub headline: String,
    pub headline_state: TileState,
    /// `limits moved from flat 4 bedroom through the band transfer`; none without a
    /// transfer (the limits are judged at the mic as typed).
    pub limits_from: Option<String>,
    /// `§13 correction +5 dB`.
    pub correction: Option<String>,
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

fn finite(v: f32) -> Option<f64> {
    v.is_finite().then_some(f64::from(v))
}

/// The period line: the set the window is judged by and, when the headroom is computed
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

/// Where the limits judged at the mic come from, when a transfer moved them.
pub fn limits_from_text(p: BandLimitPlace, place: &str) -> Option<String> {
    match p {
        BandLimitPlace::AtMic => None,
        BandLimitPlace::Transferred => Some(format!(
            "limits moved from {place} through the band transfer"
        )),
        BandLimitPlace::Estimated => Some(format!(
            "limits moved from {place} through an estimated band transfer (typed, not measured)"
        )),
    }
}

/// One window's text from its columns of a `band_leq` frame.
fn window_text(cfg: &BandLeqConfig, f: &BandLeqFrame, w: usize) -> BandWindowText {
    let m = &f.meta;
    let st = m.windows[w];
    let duration = st.duration.0;
    let elapsed = st.elapsed.0;
    let filling = elapsed < duration;
    // A window filling for at least the horizon has its headroom until it is full.
    let until_full = filling && duration - elapsed >= m.horizon.0;
    let horizon = length(m.horizon.0);
    let bars: Vec<BandBar> = m
        .bands
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            let k = f.col(w, i);
            let nominal = BAND_NOMINAL_HZ[usize::from(b)];
            let flags = f.flags[k];
            let judgement = flags.judgement();
            let is_judged = judged(judgement);
            let state = TileState::from(judgement);
            let on_course = is_judged && flags.contains(ac2_proto::frame::LeqFlags::ON_COURSE);
            // Floored: a ceiling to stay under.
            let allowed_db = finite(f.allowed[k])
                .filter(|_| is_judged)
                .map(|a| (a * 10.0 + 1e-6).floor() / 10.0);
            let headroom = allowed_db.map(|a| {
                let a = format::level(a);
                if until_full {
                    format!("until full: stay ≤ {a} dB")
                } else {
                    format!("next {horizon}: stay ≤ {a} dB")
                }
            });
            let recover_s = finite(f.recover[k]).filter(|_| is_judged);
            let cannot = is_judged && allowed_db.is_none() && state == TileState::Over;
            let recover = match recover_s {
                Some(r) => Some(format!("cooling down in {}", format::duration(r))),
                None if cannot => Some("cooling down".to_owned()),
                None => None,
            };
            let leq = f64::from(f.leq[k]);
            let leq_db = rounded(leq);
            let limit = finite(f.limit[k]);
            let limit_db = limit.filter(|_| is_judged);
            BandBar {
                nominal_hz: nominal,
                label: band_label(nominal),
                name: band_window_name(nominal, duration, st.weighting),
                value: format::level(leq),
                leq_db,
                limit_db,
                limit: limit.map(|l| format!("limit {} dB", format::level(l))),
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
    // The period matters only to a window whose limits differ by day and night.
    let day_night = cfg
        .windows
        .get(w)
        .is_some_and(|c| matches!(c.limits, BandLimitSet::NightDay { .. }) && c.limits.any());
    let incomplete = (elapsed.min(duration) - st.measured.0 >= 1.0).then(|| {
        format!(
            "offline for {}",
            length((elapsed.min(duration) - st.measured.0).round())
        )
    });
    BandWindowText {
        name: window_name(duration, st.weighting),
        worst: st.worst.map(usize::from).filter(|&i| i < bars.len()),
        period: day_night.then(|| period_text(st.period, st.period_after_horizon)),
        filling: filling.then(|| format!("so far · {} / {}", clock(elapsed), clock(duration))),
        incomplete,
        limit_key: bars
            .iter()
            .any(|b| b.limit_db.is_some())
            .then(|| "limit".to_owned()),
        allowed_key: bars.iter().any(|b| b.allowed_db.is_some()).then(|| {
            if until_full {
                "until full: stay ≤".to_owned()
            } else {
                format!("next {horizon}: stay ≤")
            }
        }),
        bars,
    }
}

/// The band view's text from a `band_leq` frame and the configuration it was made under
/// (its windows' limits and the transfer's place).
pub fn band_leq_text(cfg: &BandLeqConfig, f: &BandLeqFrame) -> BandLeqText {
    let m = &f.meta;
    let unit = match m.scale {
        LevelScale::DbSpl => "dB SPL",
        LevelScale::Dbfs => "dBFS",
    };
    let windows: Vec<BandWindowText> = (0..m.windows.len())
        .map(|w| window_text(cfg, f, w))
        .collect();
    // The headline follows the window nearest to (or over) its limit: the most severe
    // worst band, then the one furthest above (or least below) its limit.
    let worst = windows
        .iter()
        .enumerate()
        .filter_map(|(w, t)| t.worst.map(|i| (w, i, &t.bars[i])))
        .max_by(|a, b| {
            let ex = |b: &BandBar| b.over_db.unwrap_or(f64::NEG_INFINITY);
            a.2.rank()
                .cmp(&b.2.rank())
                .then(ex(a.2).total_cmp(&ex(b.2)))
                // Equal: the earlier window, as the operator ordered them.
                .then(b.0.cmp(&a.0))
        })
        .map(|(w, i, _)| (w, i));
    let place = place_of(cfg.transfer.as_ref());
    let (headline, headline_state) = headline(f, &windows, worst);
    let shown: Vec<usize> = m.bands.iter().map(|&b| usize::from(b)).collect();
    BandLeqText {
        unit: unit.to_owned(),
        bands: bands_text(&shown),
        windows,
        worst,
        headline,
        headline_state,
        limits_from: limits_from_text(m.limits_from, place),
        correction: (m.correction.0 != 0.0)
            .then(|| format!("§13 correction {} dB", format::signed(m.correction.0, 0))),
        predicted: m.predicted.map(|p| predicted_text(&p, place)),
    }
}

fn headline(
    f: &BandLeqFrame,
    windows: &[BandWindowText],
    worst: Option<(usize, usize)>,
) -> (String, TileState) {
    if f.meta.scale == LevelScale::Dbfs {
        return (
            "not calibrated: band levels in dBFS, limits not judged".to_owned(),
            TileState::NotCalibrated,
        );
    }
    if windows.is_empty() {
        let state = f
            .meta
            .predicted
            .map_or(TileState::NoLimit, |p| p.judgement.into());
        return ("no band windows".to_owned(), state);
    }
    if windows
        .iter()
        .flat_map(|w| &w.bars)
        .all(|b| !b.leq_db.is_finite())
    {
        return (
            "waiting for the first second".to_owned(),
            TileState::NoLimit,
        );
    }
    let Some(b) = worst.map(|(w, i)| &windows[w].bars[i]) else {
        let state = f
            .meta
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

/// The predicted level at the transfer's place.
pub fn predicted_text(p: &PredictedLeq, place: &str) -> PredictedText {
    let mut parts = vec![format!(
        "predicted LAeq {} in {place} {}",
        length(p.duration.0),
        with_db(p.estimate)
    )];
    // The unusable bands at their bound can only raise it: shown when they do.
    if p.at_most.is_finite() && (!p.estimate.is_finite() || p.at_most - p.estimate >= 0.05) {
        parts.push(format!("at most {} dB", format::level(p.at_most)));
    }
    match p.limit {
        Some(l) => parts.push(format!("limit {} dB", format::level(l.0))),
        None => parts.push("no limit now".to_owned()),
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
    let c = p.apply(None);
    let shown: Vec<usize> = c.band_indices().unwrap_or_default();
    let mut parts = Vec::new();
    for w in &c.windows {
        parts.push(format!(
            "{}, {}, {}",
            window_name(w.duration.0, w.weighting),
            bands_text(&shown),
            limits_summary(w, &shown)
        ));
    }
    if let Some(pr) = c.predicted {
        let mut pred = Vec::new();
        if let Some(d) = pr.day {
            pred.push(format!("day ≤ {} dB", d.0));
        }
        if let Some(n) = pr.night {
            pred.push(format!("night ≤ {} dB", n.0));
        }
        if !pred.is_empty() {
            parts.push(format!(
                "predicted LAeq {} {}",
                length(pr.duration.0),
                pred.join(", ")
            ));
        }
    }
    format!("{}: {}", p.name(), parts.join("; "))
}

/// A window's limits on the shown bands (indices) in words: `night 74 … 32 dB, day 5 dB
/// higher`, `70 … 42 dB`, `no limits`; the first and last shown band that has one.
pub fn limits_summary(w: &BandWindow, shown: &[usize]) -> String {
    let lims: Vec<f64> = shown
        .iter()
        .filter_map(|&b| w.limits.night().get(b).copied().flatten().map(|l| l.0))
        .collect();
    match (lims.first(), lims.last(), w.limits.day_offset()) {
        (Some(a), Some(b), Some(o)) if a == b => {
            format!("night {a} dB, day {} dB higher", o.0)
        }
        (Some(a), Some(b), Some(o)) => format!("night {a} … {b} dB, day {} dB higher", o.0),
        (Some(a), Some(b), None) if a == b => format!("{a} dB"),
        (Some(a), Some(b), None) => format!("{a} … {b} dB"),
        _ => "no limits".to_owned(),
    }
}

/// Where a band preset's figures come from, with the caveats it carries.
pub fn preset_source(p: BandLeqPreset) -> String {
    format!(
        "{} — informational, not legal advice; a prediction from FOH is not a measurement \
         where the limits apply",
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

/// The transfer of the shown bands (indices, low to high) in a line: `transfer from flat 4
/// bedroom, 20–200 Hz: 8 clean, 1 corrected, 1 bound, 1 not measured`; `no band transfer:
/// limits judged at the mic as typed`.
pub fn transfer_summary(t: Option<&BandTransferSet>, shown: &[usize]) -> String {
    let Some(t) = t else {
        return "no band transfer: limits judged at the mic as typed".to_owned();
    };
    let place = &t.place;
    let bands = bands_text(shown);
    let of_shown = || shown.iter().filter_map(|&b| t.bands.get(b));
    if t.origin == TransferOrigin::Estimated {
        let typed = of_shown()
            .filter(|b| !matches!(b, BandTransferBand::Missing))
            .count();
        return format!(
            "estimated transfer from {place}, {bands}: {typed} bands typed, not measured \
             (measure it when you can reach {place})"
        );
    }
    let mut n = [0usize; 5];
    for b in of_shown() {
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
    format!("transfer from {place}, {bands}: {}", parts.join(", "))
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
    /// Per window, each band's column, low to high.
    pub columns: Vec<Vec<Rect>>,
    /// The headline's box.
    pub headline: Rect,
    /// Per window, the bar scale, dB (bottom, top).
    pub ranges: Vec<(f64, f64)>,
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
/// large, the limits' place and the prediction under it, then one row of columns per
/// window, stacked in the configuration's order, each under its caption and key.
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

    // Caption: the meter and the bands, the calibration (and STALE) right.
    let left = format!("{} · band Leq {}, {}", v.meter, t.bands, t.unit);
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

    // The meter's small lines: the limits' place and the correction; then the prediction.
    let sfs = theme.small_font_size.max(fs * 0.85);
    let info: Vec<String> = t
        .limits_from
        .iter()
        .chain(t.correction.iter())
        .cloned()
        .collect();
    if !info.is_empty() {
        c.overlay.labels.push(label(
            crate::spl::cut(&info.join(" · "), w, sfs),
            [pad, y],
            anchor(HAlign::Left, VAlign::Top),
            sfs,
            theme.text_dim,
        ));
        y += sfs * 1.5;
    }
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

    // The windows, stacked: each its caption and key, then its columns.
    let n_win = t.windows.len().max(1);
    let section_h = ((size.height - y - pad) / n_win as f32).max(1.0);
    let mut columns = Vec::with_capacity(t.windows.len());
    let mut ranges = Vec::with_capacity(t.windows.len());
    for (wi, win) in t.windows.iter().enumerate() {
        let top = y + wi as f32 * section_h;
        let range = bar_range(&win.bars);
        let worst = t
            .worst
            .filter(|&(w, _)| w == wi)
            .map(|(_, i)| i)
            .or(win.worst);
        let cols = draw_window(&mut c, win, worst, range, (top, section_h), pad, w, theme);
        columns.push(cols);
        ranges.push(range);
    }
    BandLeqScene {
        scene: c.into_scene(size),
        columns,
        headline,
        ranges,
        banners: strip.rows,
    }
}

/// One window's caption, key and columns in the section `(top, height)`; its columns.
#[allow(clippy::too_many_arguments)]
fn draw_window(
    c: &mut Canvas,
    t: &BandWindowText,
    worst: Option<usize>,
    range: (f64, f64),
    (top, height): (f32, f32),
    pad: f32,
    w: f32,
    theme: &Theme,
) -> Vec<Rect> {
    let fs = theme.font_size;
    let sfs = theme.small_font_size.max(fs * 0.85);
    let mut y = top;
    // The key to the two marks across the columns, right of the caption: a mark is keyed
    // only where one is drawn, so an allowed level off every column's scale has none.
    let limit_stroke = Stroke::solid(theme.text, 3.0);
    let allowed_stroke = Stroke::solid(theme.text_dim, 2.0);
    let allowed_drawn = |b: &BandBar| b.allowed_db.filter(|a| (range.0..=range.1).contains(a));
    let mut keys: Vec<(&str, Stroke)> = Vec::new();
    if let Some(k) = &t.limit_key {
        keys.push((k, limit_stroke));
    }
    if let Some(k) = t
        .allowed_key
        .as_ref()
        .filter(|_| t.bars.iter().any(|b| allowed_drawn(b).is_some()))
    {
        keys.push((k, allowed_stroke));
    }
    // Each mark left of its word, the word placed from the mark: an estimated text width
    // then widens only the gap to the next pair, never the one inside a pair.
    let mark_w = sfs * 1.6;
    let pair_w = |text: &str| mark_w + sfs * 0.4 + canvas::text_width(text, sfs);
    let keys_w: f32 = keys.iter().map(|(t, _)| pair_w(t) + sfs * 1.5).sum();
    let right = pad + w;
    let key_x = right - keys_w + sfs * 1.5;
    let mut x = key_x;
    for (text, stroke) in &keys {
        let mark_y = y + sfs * 0.6;
        c.overlay.polylines.push(Polyline {
            points: vec![[x, mark_y], [x + mark_w, mark_y]],
            alpha: Vec::new(),
            stroke: *stroke,
            clip: None,
        });
        c.overlay.labels.push(label(
            *text,
            [x + mark_w + sfs * 0.4, y],
            anchor(HAlign::Left, VAlign::Top),
            sfs,
            theme.text_dim,
        ));
        x += pair_w(text) + sfs * 1.5;
    }
    c.overlay.labels.push(label(
        crate::spl::cut(&t.caption(), (key_x - sfs - pad).max(1.0), sfs),
        [pad, y],
        anchor(HAlign::Left, VAlign::Top),
        sfs,
        theme.text,
    ));
    y += sfs * 1.5;

    // The columns: a y scale left, one column per band, the band labels under them.
    let scale_w = fs * 3.0;
    let label_fs = fs.max(((w - scale_w) / 11.0 * 0.22).min(fs * 1.6));
    let below_h = label_fs * 1.4 + sfs * 1.4;
    let area = Rect::new(
        pad + scale_w,
        y + pad * 0.5,
        (w - scale_w).max(1.0),
        (top + height - y - pad * 1.5 - below_h).max(1.0),
    );
    let to_y = |db: f64| -> f32 {
        let k = ((db - range.0) / (range.1 - range.0)).clamp(0.0, 1.0);
        area.bottom() - (k as f32) * area.h
    };
    // Grid lines every 10 dB, fewer when the section is short.
    let step = if area.h < sfs * 8.0 { 20.0 } else { 10.0 };
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
        db += step;
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
        // Off the scale it would sit on the top or bottom edge, reading as a level it is not.
        if let Some(a) = allowed_drawn(b) {
            let ay = to_y(a);
            c.overlay.polylines.push(Polyline {
                points: vec![[x + cw * 0.2, ay], [x + cw * 0.8, ay]],
                alpha: Vec::new(),
                stroke: allowed_stroke,
                clip: None,
            });
        }
        if let Some(l) = b.limit_db {
            let ly = to_y(l);
            c.overlay.polylines.push(Polyline {
                points: vec![[x - gap * 0.3, ly], [x + cw + gap * 0.3, ly]],
                alpha: Vec::new(),
                stroke: limit_stroke,
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
        let worst = worst == Some(i);
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
    columns
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
