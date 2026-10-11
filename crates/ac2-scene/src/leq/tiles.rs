//! The Leq windows as tiles: each window's value, state and instruction in words.

use super::*;

/// How a tile is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileState {
    /// No limit: values only.
    NoLimit,
    /// A limit, not judged (dBFS).
    NotCalibrated,
    Ok,
    Near,
    Over,
}

impl From<LeqJudgement> for TileState {
    fn from(j: LeqJudgement) -> Self {
        match j {
            LeqJudgement::NoLimit => TileState::NoLimit,
            LeqJudgement::NotCalibrated => TileState::NotCalibrated,
            LeqJudgement::Ok => TileState::Ok,
            LeqJudgement::Near => TileState::Near,
            LeqJudgement::Over => TileState::Over,
        }
    }
}

/// What a tile stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileKind {
    /// A rolling Leq window.
    Window,
    /// A peak limit: its value is the highest second of the hold.
    Peak(PeakQuantity),
}

/// Every string of one window's (or peak limit's) tile.
#[derive(Clone, Debug, PartialEq)]
pub struct LeqTile {
    pub kind: TileKind,
    /// `LAeq 30 min`, `LCpeak`.
    pub name: String,
    /// `96.4`, or `—` before anything was measured.
    pub value: String,
    /// `dB SPL` or `dBFS`.
    pub unit: String,
    /// The unit with the window's own weighting, shown with its value — `dB(A)`, `dBFS (C)` —
    /// so a window is never read in the weighting of an SPL meter shown beside it.
    pub weighted_unit: String,
    pub state: TileState,
    /// `OVER`, `NEAR`, `ON COURSE`, `OK`, `not calibrated`; none without a limit.
    pub state_text: Option<String>,
    /// `on course — over in 12 min` while a filling window's Leq so far is above its limit.
    pub course: Option<String>,
    /// `limit 99.0 dB`.
    pub limit: Option<String>,
    /// `next 1 min: stay ≤ 101.5 dB`; `until full: stay ≤ 98.2 dB` while the window fills
    /// for longer than the horizon (the level that, held to the end of the fill, spends what
    /// is left of its budget). None when it cannot recover within the horizon.
    pub headroom: Option<String>,
    /// `cooling down in 7 min 30 s` when it cannot recover within the horizon: the time
    /// until the window is back under its limit if the level stays at the limit (no time
    /// when the daemon has none).
    pub recover: Option<String>,
    /// `so far · 12:30 / 30:00` while the window fills: the value is the Leq so far.
    pub filling: Option<String>,
    /// `offline for 1 min 50 s`: time in the window with no audio (not counted as silence).
    pub incomplete: Option<String>,
    /// A peak limit's value: `highest of the last 10 s`.
    pub held: Option<String>,
    /// `corrected +5.0 dB`: the measuring-position correction in the value.
    pub corrected: Option<String>,
    /// The figures the columns draw: the window's weighting and length (s), the Leq as
    /// shown (rounded to 0.1 dB, NaN before anything was measured), its limit when judged,
    /// the floored headroom when it can recover within the horizon (and whether it holds
    /// until the window is full rather than for the horizon), the time to recover when it
    /// cannot, how much of the window has elapsed (s), the Leq it ends at if the rest is
    /// silent (the bar of a filling column; the Leq once full), whether it is on course to
    /// go over and the seconds until it spends its budget then.
    pub weighting: Weighting,
    pub duration_s: f64,
    pub leq_db: f64,
    pub limit_db: Option<f64>,
    pub allowed_db: Option<f64>,
    pub allowed_until_full: bool,
    pub recover_s: Option<f64>,
    pub elapsed_s: f64,
    pub least_db: f64,
    pub on_course: bool,
    pub over_in_s: Option<f64>,
    /// The bar's length while it eases to [`LeqTile::bar_db`] ([`crate::bar_ease`]), dB;
    /// `None`: drawn at `bar_db`.
    pub drawn_db: Option<f64>,
}

impl LeqTile {
    /// The window has not covered its whole length yet.
    pub fn filling(&self) -> bool {
        self.elapsed_s < self.duration_s
    }

    /// The level the bar shows: while filling, the Leq the window ends at if the rest is
    /// silent, so the bar climbs towards the limit line as the budget is spent and reaches
    /// it when going over becomes certain; once full, the Leq.
    pub fn bar_db(&self) -> f64 {
        if self.filling() {
            self.least_db
        } else {
            self.leq_db
        }
    }
}

/// A time to come, coarse enough not to flicker every second: `45 s`, `12 min`, `1 h 05 min`.
pub fn time_to(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return format::NO_VALUE.to_string();
    }
    let s = seconds.floor() as u64;
    if s < 120 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
    }
}

/// A unit with the window's own weighting, `dB(A)`, `dBFS (C)`, marked when corrected.
fn weighted_unit(scale: LevelScale, w: Weighting, corrected: bool) -> String {
    let u = match scale {
        LevelScale::DbSpl => format!("dB({})", w_letter(w)),
        LevelScale::Dbfs => format!("dBFS ({})", w_letter(w)),
    };
    if corrected {
        format!("{u} {CORRECTED_MARK}")
    } else {
        u
    }
}

/// The tile of peak limit `q` from its state in the frame.
fn peak_tile(
    q: PeakQuantity,
    p: &LeqPeak,
    limit: f64,
    scale: LevelScale,
    corrected: Option<String>,
) -> LeqTile {
    let state = TileState::from(p.judgement);
    let judged = matches!(state, TileState::Ok | TileState::Near | TileState::Over);
    let hold = f64::from(LeqPeak::HOLD_S);
    LeqTile {
        kind: TileKind::Peak(q),
        name: peak_name(q).to_owned(),
        value: format::level(p.level),
        unit: unit_of(scale).to_owned(),
        weighted_unit: weighted_unit(scale, peak_weighting(q), corrected.is_some()),
        state,
        state_text: state_text(state, false),
        course: None,
        limit: Some(format!("limit {} dB", format::level(limit))),
        headroom: None,
        recover: None,
        filling: None,
        incomplete: None,
        held: Some(format!("highest of the last {}", length(hold))),
        corrected,
        weighting: peak_weighting(q),
        duration_s: hold,
        leq_db: if p.level.is_finite() {
            (p.level * 10.0).round() / 10.0
        } else {
            f64::NAN
        },
        limit_db: judged.then_some(limit),
        allowed_db: None,
        allowed_until_full: false,
        recover_s: None,
        elapsed_s: hold,
        least_db: p.level,
        on_course: false,
        over_in_s: None,
        drawn_db: None,
    }
}

fn unit_of(scale: LevelScale) -> &'static str {
    match scale {
        LevelScale::DbSpl => "dB SPL",
        LevelScale::Dbfs => "dBFS",
    }
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

/// The tiles of a meter's windows from its configuration and newest `leq` frame, then one
/// per peak limit (LCpeak, LAFmax). A frame of another configuration (a different window
/// count) gives none.
pub fn leq_tiles(cfg: &LeqConfig, f: &LeqFrame) -> Vec<LeqTile> {
    let n = cfg.windows.len();
    if [
        f.leq.len(),
        f.elapsed.len(),
        f.measured.len(),
        f.allowed.len(),
        f.recover.len(),
        f.least.len(),
        f.over_in.len(),
        f.flags.len(),
    ]
    .iter()
    .any(|&l| l != n)
    {
        return Vec::new();
    }
    let unit = unit_of(f.meta.scale);
    let corrected = f.meta.position.as_ref().map(position_text);
    let horizon = length(f.meta.horizon.0);
    let mut tiles: Vec<LeqTile> = cfg
        .windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let flags = f.flags[i];
            let state = TileState::from(flags.judgement());
            let elapsed = f64::from(f.elapsed[i]);
            let measured = f64::from(f.measured[i]);
            let duration = w.duration.0;
            let judged = flags.contains(LeqFlags::JUDGED);
            let cannot = flags.contains(LeqFlags::CANNOT_RECOVER);
            let on_course = judged && flags.contains(LeqFlags::ON_COURSE);
            let filling = elapsed < duration;
            // A window filling for at least the horizon has its headroom until it is full.
            let until_full = filling && duration - elapsed >= f.meta.horizon.0;
            // Floored: the figure is a ceiling to stay under (the margin only absorbs the
            // f32 the frame carries it in).
            let allowed_db = (judged && !cannot)
                .then(|| (f64::from(f.allowed[i]) * 10.0 + 1e-3).floor() / 10.0)
                .filter(|a| a.is_finite());
            let headroom = (judged && !cannot).then(|| {
                let a = format::level(allowed_db.unwrap_or(f64::NAN));
                if until_full {
                    format!("until full: stay ≤ {a} dB")
                } else {
                    format!("next {horizon}: stay ≤ {a} dB")
                }
            });
            let recover_s = (judged && cannot)
                .then(|| f64::from(f.recover[i]))
                .filter(|r| r.is_finite());
            let recover = (judged && cannot).then(|| match recover_s {
                Some(r) => format!("cooling down in {}", format::duration(r)),
                None => "cooling down".to_string(),
            });
            let leq = f64::from(f.leq[i]);
            let over_in_s = on_course
                .then(|| f64::from(f.over_in[i]))
                .filter(|t| t.is_finite());
            LeqTile {
                kind: TileKind::Window,
                name: window_name(w),
                value: format::level(leq),
                unit: unit.to_string(),
                weighted_unit: weighted_unit(f.meta.scale, w.weighting, corrected.is_some()),
                state,
                state_text: state_text(state, on_course),
                course: on_course.then(|| match over_in_s {
                    Some(t) => format!("on course — over in {}", time_to(t)),
                    None => "on course to go over".into(),
                }),
                limit: w.limit.map(|l| format!("limit {} dB", format::level(l.0))),
                headroom,
                recover,
                filling: filling
                    .then(|| format!("so far · {} / {}", clock(elapsed), clock(duration))),
                // Seconds inside the window with no audio at all (daemon down, device lost):
                // not counted as silence, so the operator is told how long it was.
                incomplete: flags.contains(LeqFlags::INCOMPLETE).then(|| {
                    format!(
                        "offline for {}",
                        length((elapsed.min(duration) - measured).max(1.0).round())
                    )
                }),
                held: None,
                corrected: corrected.clone(),
                weighting: w.weighting,
                duration_s: duration,
                leq_db: if leq.is_finite() {
                    (leq * 10.0).round() / 10.0
                } else {
                    f64::NAN
                },
                limit_db: w.limit.filter(|_| judged).map(|l| l.0),
                allowed_db,
                allowed_until_full: until_full,
                recover_s,
                elapsed_s: elapsed,
                least_db: f64::from(f.least[i]),
                on_course,
                over_in_s,
                drawn_db: None,
            }
        })
        .collect();
    for (q, p) in [
        (PeakQuantity::LcPeak, f.meta.lcpeak),
        (PeakQuantity::LafMax, f.meta.lafmax),
    ] {
        if let (Some(p), Some(l)) = (p, cfg.peaks.get(q)) {
            tiles.push(peak_tile(q, &p, l.limit.0, f.meta.scale, corrected.clone()));
        }
    }
    tiles
}

/// A tile's instruction is at most this many times its header's size.
pub(super) const TILE_INSTRUCTION_EM: f32 = 1.6;

/// Background and text colour of a tile.
pub fn tile_colors(state: TileState, theme: &Theme) -> (Color, Color) {
    match state {
        TileState::Over => (theme.banner_fault.background, theme.banner_fault.text),
        TileState::Near => (theme.banner_warning.background, theme.banner_warning.text),
        TileState::NoLimit | TileState::NotCalibrated | TileState::Ok => {
            (theme.plot_background, theme.text)
        }
    }
}
