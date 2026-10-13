//! The band meter of an SPL meter (`docs/design/band-leq.md`): the band windows of one
//! length and weighting drawn as one row of bars ordered by frequency, each bar a window's
//! rolling band Leq against its own limit line; the band nearest to (or over) its limit
//! named with what to do about it, where
//! the limits come from when a transfer moved them, and the predicted level at the
//! transfer's place. Read from a stage: the headline says which band eats the budget and
//! for how long to back off; the bars show where in the spectrum.
//!
//! Display truth only: the daemon judges (the `band_leq` frame carries every judgement and
//! figure), this module words and colours it, with the Leq windows' vocabulary (`LZeq 60
//! min`, `stay ≤ …`, `cooling down in …`) and the alarms' names (`63 Hz band LZeq 60 min`).

use ac2_proto::frame::BandLeqFrame;
use ac2_proto::model::{
    BAND_NOMINAL_HZ, BandLeqConfig, BandLeqPreset, BandLimitPlace, BandPeriod, BandTransferBand,
    BandTransferSet, BandWindow, LeqJudgement, LevelScale, PredictedLeq, TransferOrigin, Weighting,
    band_index,
};

use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, anchor, label};
use crate::format;
use crate::leq::{TileState, clock, column_colors, length, tile_colors, w_letter};
use crate::primitives::{Color, FillRect, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport};
use crate::theme::Theme;
use crate::view::PlotChrome;

mod draw;
mod summary;
pub use draw::{BandLeqScene, BandLeqView, band_leq_scene, bar_range};
pub use summary::{
    limits_summary, predicted_text, preset_source, preset_summary, transfer_band_text,
    transfer_summary, windows_summary,
};

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

/// Every string and figure of one band window's bar.
#[derive(Clone, Debug, PartialEq)]
pub struct BandBar {
    /// The window: its index in the frame and the configuration.
    pub window: usize,
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
    /// `limit 42.0 dB`; none without a limit, or without limit text.
    pub limit: Option<String>,
    pub state: TileState,
    /// `OVER`, `NEAR`, `ON COURSE`, `OK`, `not calibrated`; none without a limit. Without
    /// limit text `OVER`, `NEAR` and `OK` are left to the bar's colour.
    pub state_text: Option<String>,
    pub on_course: bool,
    /// Floored steady level allowed (a ceiling to stay under).
    pub allowed_db: Option<f64>,
    /// `next 1 min: stay ≤ 41.3 dB`, `until full: stay ≤ 40.2 dB`; none without limit text.
    pub headroom: Option<String>,
    pub recover_s: Option<f64>,
    /// `cooling down in 6 min 52 s` when it cannot recover within the horizon.
    pub recover: Option<String>,
    /// Leq − limit, dB (positive: over), when judged and measured.
    pub over_db: Option<f64>,
    /// Under the band's label: the time it cools down in when over (`OVER` when the daemon
    /// has no time), else the level to stay under (`≤ 41.3`). Without limit text the bar's
    /// colour says over and the level is not written: only a cooling-down time is.
    pub under: Option<String>,
    /// The bar's length while it eases to `leq_db` ([`crate::bar_ease`]), dB; `None`:
    /// drawn at `leq_db`.
    pub drawn_db: Option<f64>,
}

impl BandBar {
    /// Whether its limit is judged.
    fn judged(&self) -> bool {
        matches!(
            self.state,
            TileState::Ok | TileState::Near | TileState::Over
        )
    }

    /// Severity: ok, near, on course, over.
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

/// The band windows of one length and weighting (and one way of judging day and night),
/// as one row of bars.
#[derive(Clone, Debug, PartialEq)]
pub struct BandRowText {
    /// `LZeq 60 min`.
    pub name: String,
    /// One per window, low band to high.
    pub bars: Vec<BandBar>,
    /// Index of the worst bar: the most severe judgement, then the furthest above (or
    /// least below) its limit; none when nothing is judged.
    pub worst: Option<usize>,
    /// `night limits (22–07)`, `day limits (07–22) · headroom for the night limits from
    /// 22:00`; none for windows whose limits hold day and night.
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

impl BandRowText {
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
    /// The bands of every window, `20–200 Hz`, `20 Hz, 63 Hz`.
    pub bands: String,
    /// The windows grouped by length and weighting, in the order of each group's first
    /// window.
    pub rows: Vec<BandRowText>,
    /// The row and bar the headline is about: the band nearest to (or over) its limit.
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

fn state_text(state: TileState, on_course: bool, chrome: PlotChrome) -> Option<String> {
    match state {
        TileState::Ok | TileState::Over if !chrome.limit_text() => None,
        TileState::Near if !chrome.limit_text() && !on_course => None,
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

/// The configuration's window `w` when it is the frame's (a frame made under the previous
/// configuration is read without it).
fn config_of<'a>(cfg: &'a BandLeqConfig, f: &BandLeqFrame, w: usize) -> Option<&'a BandWindow> {
    let st = f.meta.windows.get(w)?;
    cfg.windows
        .get(w)
        .filter(|c| c.band == st.band && c.duration == st.duration && c.weighting == st.weighting)
}

/// Its window's day and night are judged apart.
fn day_night(c: Option<&BandWindow>) -> bool {
    c.is_some_and(|c| c.day_offset.is_some())
}

/// The band windows grouped into rows: one per length, weighting and way of judging day and
/// night, in the order of each row's first window; each row's windows low band to high.
pub fn group_rows<K: PartialEq>(keys: &[(f64, K)]) -> Vec<Vec<usize>> {
    let mut rows: Vec<(usize, Vec<usize>)> = Vec::new();
    for (w, (_, k)) in keys.iter().enumerate() {
        match rows.iter_mut().find(|(first, _)| keys[*first].1 == *k) {
            Some((_, ws)) => ws.push(w),
            None => rows.push((w, vec![w])),
        }
    }
    rows.into_iter()
        .map(|(_, mut ws)| {
            ws.sort_by(|&a, &b| keys[a].0.total_cmp(&keys[b].0));
            ws
        })
        .collect()
}

/// One window's bar from its column of a `band_leq` frame.
fn bar(f: &BandLeqFrame, k: usize, chrome: PlotChrome) -> BandBar {
    let m = &f.meta;
    let st = m.windows[k];
    let duration = st.duration.0;
    let filling = st.elapsed.0 < duration;
    // A window filling for at least the horizon has its headroom until it is full.
    let until_full = filling && duration - st.elapsed.0 >= m.horizon.0;
    let horizon = length(m.horizon.0);
    let nominal = st.band.0;
    let flags = f.flags[k];
    let judgement = flags.judgement();
    let is_judged = judged(judgement);
    let state = TileState::from(judgement);
    let on_course = is_judged && flags.contains(ac2_proto::frame::LeqFlags::ON_COURSE);
    // Floored: a ceiling to stay under.
    let allowed_db = finite(f.allowed[k])
        .filter(|_| is_judged)
        .map(|a| (a * 10.0 + 1e-6).floor() / 10.0);
    let headroom = allowed_db.filter(|_| chrome.limit_text()).map(|a| {
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
        window: k,
        nominal_hz: nominal,
        label: band_label(nominal),
        name: band_window_name(nominal, duration, st.weighting),
        value: format::level(leq),
        leq_db,
        limit_db,
        limit: limit
            .filter(|_| chrome.limit_text())
            .map(|l| format!("limit {} dB", format::level(l))),
        state,
        state_text: state_text(state, on_course, chrome),
        on_course,
        allowed_db,
        headroom,
        recover_s,
        recover,
        over_db: limit_db
            .filter(|_| leq_db.is_finite())
            .map(|l| rounded(leq_db - l)),
        under: match state {
            TileState::Over => match recover_s {
                Some(r) => Some(crate::leq::time_to(r)),
                None => chrome.limit_text().then(|| "OVER".to_owned()),
            },
            _ => allowed_db
                .filter(|_| chrome.limit_text())
                .map(|a| format!("≤ {}", format::level(a))),
        },
        drawn_db: None,
    }
}

/// The worse of two judged bars: the more severe, then the further above (or less below)
/// its limit.
fn worse(a: &BandBar, b: &BandBar) -> std::cmp::Ordering {
    let ex = |b: &BandBar| b.over_db.unwrap_or(f64::NEG_INFINITY);
    a.rank().cmp(&b.rank()).then(ex(a).total_cmp(&ex(b)))
}

/// One row's text from its windows (frame indices, low band to high).
fn row_text(
    cfg: &BandLeqConfig,
    f: &BandLeqFrame,
    ws: &[usize],
    chrome: PlotChrome,
) -> BandRowText {
    let m = &f.meta;
    // One length: the windows fill, and miss audio, together.
    let st = m.windows[ws[0]];
    let duration = st.duration.0;
    let elapsed = st.elapsed.0;
    let filling = elapsed < duration;
    let until_full = filling && duration - elapsed >= m.horizon.0;
    let horizon = length(m.horizon.0);
    let bars: Vec<BandBar> = ws.iter().map(|&k| bar(f, k, chrome)).collect();
    // The period matters only to windows whose limits differ by day and night.
    let day_night = ws.iter().any(|&k| {
        config_of(cfg, f, k).is_some_and(|c| c.limit.is_some() && c.day_offset.is_some())
    });
    let incomplete = (elapsed.min(duration) - st.measured.0 >= 1.0).then(|| {
        format!(
            "offline for {}",
            length((elapsed.min(duration) - st.measured.0).round())
        )
    });
    // Ties: the lower band.
    let worst = bars
        .iter()
        .enumerate()
        .filter(|(_, b)| b.judged())
        .max_by(|(i, a), (j, b)| worse(a, b).then(j.cmp(i)))
        .map(|(i, _)| i);
    BandRowText {
        name: window_name(duration, st.weighting),
        worst,
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
/// (its windows' limits and the transfer's place), with the state words and limit texts
/// or without limit text (`chrome`).
pub fn band_leq_text(cfg: &BandLeqConfig, f: &BandLeqFrame, chrome: PlotChrome) -> BandLeqText {
    let m = &f.meta;
    let unit = match m.scale {
        LevelScale::DbSpl => "dB SPL",
        LevelScale::Dbfs => "dBFS",
    };
    let keys: Vec<(f64, _)> = (0..m.windows.len())
        .map(|w| {
            let st = m.windows[w];
            (
                st.band.0,
                (
                    st.duration.0.to_bits(),
                    st.weighting,
                    day_night(config_of(cfg, f, w)),
                ),
            )
        })
        .collect();
    let rows: Vec<BandRowText> = group_rows(&keys)
        .iter()
        .map(|ws| row_text(cfg, f, ws, chrome))
        .collect();
    // The headline follows the band nearest to (or over) its limit; equal: the earlier
    // row, as the operator ordered them, then the lower band.
    let worst = rows
        .iter()
        .enumerate()
        .filter_map(|(r, t)| t.worst.map(|i| (r, i, &t.bars[i])))
        .max_by(|a, b| worse(a.2, b.2).then(b.0.cmp(&a.0)))
        .map(|(r, i, _)| (r, i));
    let place = place_of(cfg.transfer.as_ref());
    let (headline, headline_state) = headline(f, &rows, worst);
    let mut shown: Vec<usize> = m
        .windows
        .iter()
        .filter_map(|w| band_index(w.band.0))
        .collect();
    shown.sort_unstable();
    shown.dedup();
    BandLeqText {
        unit: unit.to_owned(),
        bands: bands_text(&shown),
        rows,
        worst,
        headline,
        headline_state,
        limits_from: limits_from_text(m.limits_from, place),
        correction: (m.correction.0 != 0.0)
            .then(|| format!("§13 correction {} dB", format::signed(m.correction.0, 0))),
        predicted: m.predicted.map(|p| predicted_text(&p, place, chrome)),
    }
}

fn headline(
    f: &BandLeqFrame,
    windows: &[BandRowText],
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

#[cfg(test)]
mod tests;
