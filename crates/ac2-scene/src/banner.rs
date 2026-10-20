//! Fault banners: which ones show, in which order, and where.
//!
//! Order (highest first) follows how much of the screen a condition invalidates and what
//! the operator must do first:
//!
//! | # | banner | severity | from |
//! |---|---|---|---|
//! | 1 | DAEMON NOT RESPONDING | fault | no keepalive for > 1.5 s |
//! | 2 | AUDIO STOPPED · what, since | fault | `session.stopped` (the daemon is reopening the audio I/O) |
//! | 3 | CLIP | fault | protection `CLIP` |
//! | 4 | NO REFERENCE | fault | protection `NO_REFERENCE` (the detail names the stimulus keys while nothing here plays) |
//! | 5 | CHECK ROUTING | fault | protection `CHECK_ROUTING` (inputs identical or swapped) |
//! | 6 | NO SIGNAL | fault | protection `NO_SIGNAL` |
//! | 7 | NO AVERAGE · n OF m POSITIONS | fault | the shown spatial average has fewer than two usable positions |
//! | 8 | STALE · age | warning | newest live frame older than 1 s (decision 2a) |
//! | 9 | AVERAGE · n OF m POSITIONS | warning | the shown spatial average left positions out (the detail names them and why) |
//! | 10 | OUTPUT TIMING JUMP | warning | loopback timing `jumped` |
//! | 11 | CLOCK DRIFT · ppm | warning | loopback timing drift `warning` (output and input on different clocks) |
//! | 12 | LOG NOT SAVED · reason | warning | the daemon cannot write an SPL meter's log (`autosave.log_failures`; the detail names the logs and for how long) |
//! | 13 | NO DELAY ESTIMATE | info | TF measurement without a delay, the finder refused, or an ambiguous finding awaits a pick (the detail says which) |
//! | 14 | FILE DAMAGED · what | info | the daemon started on a damaged state file, set it aside and went on (`rig.damaged_files`; the detail names each file and what was done) |
//!
//! AUDIO STOPPED comes right after the daemon itself: no audio invalidates everything
//! measured, and the daemon is already working on it. It replaces STALE, whose cause it
//! names: every frame is old because none can come.
//!
//! CLOCK DRIFT sits under OUTPUT TIMING JUMP: both are output-side timing, a jump is the
//! newer event, and drift does not touch a transfer function on the measured loopback
//! reference (reference and measurement hear the same drifting stimulus on one input clock),
//! which the detail says so the operator does not distrust a valid trace.
//!
//! LOG NOT SAVED is about the disk, not the signal: what is on screen is valid, only its
//! copy on disk (which a restart restores and a report is built from) misses a stretch, so
//! it ranks under every measurement warning. It shows on every pane, like the daemon's
//! own faults: the operator must free space before more of the history is lost.
//!
//! FILE DAMAGED is history: the daemon already recovered (a power cut tore a file), and
//! what is measured now is sound. It ranks last, and stays until the daemon restarts so
//! the operator who next looks learns that a trace, a log or a setting may be missing.
//!
//! CHECK ROUTING sits right under NO REFERENCE: a mis-patched reference invalidates every
//! transfer value just as a missing one does, and fixing the patch comes before any other
//! signal check.
//!
//! Layout: a strip above the plots, outside every data area, so a banner never hides the
//! trace, legend or cursor values it is warning about. Rows are centred over the plots and
//! stacked downwards in that order, at most [`MAX_BANNERS`] and never more than fit; when
//! some do not fit, the last row says how many more there are. The strip only exists while
//! a banner is up; the plots below shrink by its height ([`banner_strip`]).

use ac2_proto::frame::ProtectionFlags;
use ac2_proto::model::{MeasKind, Measurement, NoEstimateReason, TimingState};

use crate::format;
use crate::math::MathStatus;
use crate::primitives::{Anchor, FillRect, HAlign, Layer, Rect, VAlign, Viewport};
use crate::theme::Theme;
use crate::time::{DAEMON_SILENT_AFTER_S, STALE_AFTER_S};

pub const MAX_BANNERS: usize = 3;
pub const BANNER_HEIGHT: f32 = 24.0;
pub const BANNER_GAP: f32 = 4.0;
pub const BANNER_MAX_WIDTH: f32 = 460.0;
/// Space above and below the stack inside the strip.
pub const BANNER_PAD: f32 = 4.0;
/// Largest share of the view height the strip may take: the plots must stay readable while
/// faults are up, and the overflow row names whatever did not fit.
pub const STRIP_MAX_FRACTION: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Fault,
    Warning,
    Info,
}

/// Banner kinds in priority order (derive order = display order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BannerKind {
    DaemonNotResponding,
    AudioStopped,
    Clip,
    NoReference,
    CheckRouting,
    NoSignal,
    NoMathResult,
    Stale,
    MathIncomplete,
    OutputTimingJump,
    ClockDrift,
    LogNotSaved,
    NoDelayEstimate,
    FileDamaged,
}

impl BannerKind {
    pub fn severity(self) -> Severity {
        match self {
            Self::DaemonNotResponding
            | Self::AudioStopped
            | Self::Clip
            | Self::NoReference
            | Self::CheckRouting
            | Self::NoSignal
            | Self::NoMathResult => Severity::Fault,
            Self::Stale
            | Self::MathIncomplete
            | Self::OutputTimingJump
            | Self::ClockDrift
            | Self::LogNotSaved => Severity::Warning,
            Self::NoDelayEstimate | Self::FileDamaged => Severity::Info,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Banner {
    pub kind: BannerKind,
    pub severity: Severity,
    pub text: String,
    /// What to check; never asserts a cause the data does not show.
    pub detail: Option<String>,
}

/// What the banners are derived from; times are computed by the caller ([`crate::time`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Seconds since the last keepalive arrived (client clock).
    pub daemon_silence_s: f64,
    /// Protection flags of the newest frame(s) shown.
    pub protection: ProtectionFlags,
    /// Age of the newest live frame shown; `None` when nothing live is shown.
    pub frame_age_s: Option<f64>,
    /// The STALE threshold of that frame's stream, s (a once-a-second stream waits 3 s);
    /// `None`: [`STALE_AFTER_S`].
    pub stale_after_s: Option<f64>,
    pub timing: Option<TimingState>,
    /// Output-vs-input clock drift, ppm, when the daemon judged it a warning
    /// (`TimingStatus.drift`).
    pub clock_drift_ppm: Option<f64>,
    /// The open session's audio stopped ([`crate::audio::audio_stopped_text`]).
    pub audio_stopped: Option<crate::audio::AudioStoppedText>,
    pub no_delay_estimate: Option<NoDelayEstimate>,
    /// The shown math channel (its name and what its newest frame combined).
    pub math: Option<(String, MathStatus)>,
    /// SPL logs the daemon cannot write ([`crate::autosave::log_failure_text`]).
    pub log_not_saved: Option<crate::autosave::LogFailureText>,
    /// State files the daemon found damaged at start ([`crate::autosave::damage_text`]).
    pub file_damaged: Option<crate::autosave::DamageText>,
    /// What this app's stimulus is doing: NO REFERENCE's detail names the keys that start
    /// it while nothing here plays.
    pub drive: crate::stimulus::Drive,
}

/// Why a TF measurement has no delay the operator can rely on.
#[derive(Clone, Debug, PartialEq)]
pub enum NoDelayEstimate {
    /// None found yet: no delay state, or tracking without a finding.
    NotFound,
    /// The last finder run refused, for these reasons.
    Refused(Vec<NoEstimateReason>),
    /// The last finding is ambiguous and the operator has not picked a candidate; tracking
    /// is paused meanwhile (decision 1c).
    AwaitingPick,
}

/// Whether (and why) a TF measurement has no delay the operator can rely on: none inserted
/// yet, tracking without a current finding, or a last finding that refused.
pub fn no_delay_estimate(m: &Measurement) -> Option<NoDelayEstimate> {
    if !matches!(m.config.kind, MeasKind::Transfer { .. }) {
        return None;
    }
    let Some(d) = &m.delay else {
        return Some(NoDelayEstimate::NotFound);
    };
    if d.awaiting_pick {
        return Some(NoDelayEstimate::AwaitingPick);
    }
    match &d.last_finding {
        Some(f) => f
            .no_estimate()
            .map(|r| NoDelayEstimate::Refused(r.to_vec())),
        None if d.tracking => Some(NoDelayEstimate::NotFound),
        None => None,
    }
}

fn banner(kind: BannerKind, text: String, detail: Option<String>) -> Banner {
    Banner {
        kind,
        severity: kind.severity(),
        text,
        detail,
    }
}

/// Active banners, highest priority first.
pub fn banners(s: &Status) -> Vec<Banner> {
    let mut out = Vec::new();
    if s.daemon_silence_s > DAEMON_SILENT_AFTER_S {
        out.push(banner(
            BannerKind::DaemonNotResponding,
            "DAEMON NOT RESPONDING".into(),
            Some(format!(
                "no keepalive for {}",
                format::age(s.daemon_silence_s)
            )),
        ));
    }
    if let Some(a) = &s.audio_stopped {
        out.push(banner(
            BannerKind::AudioStopped,
            a.banner.clone(),
            Some(a.detail.clone()),
        ));
    }
    let p = s.protection;
    if p.contains(ProtectionFlags::CLIP) {
        out.push(banner(
            BannerKind::Clip,
            "CLIP".into(),
            Some("an input is clipping; check gain".into()),
        ));
    }
    if p.contains(ProtectionFlags::NO_REFERENCE) {
        out.push(banner(
            BannerKind::NoReference,
            "NO REFERENCE".into(),
            Some(crate::stimulus::no_reference_detail(&s.drive)),
        ));
    }
    if p.contains(ProtectionFlags::CHECK_ROUTING) {
        out.push(banner(
            BannerKind::CheckRouting,
            "CHECK ROUTING".into(),
            Some(
                "reference and measurement look identical or swapped; check the input patch".into(),
            ),
        ));
    }
    if p.contains(ProtectionFlags::NO_SIGNAL) {
        out.push(banner(
            BannerKind::NoSignal,
            "NO SIGNAL".into(),
            Some("measurement input below its floor; check mic and input".into()),
        ));
    }
    if let Some((name, a)) = &s.math
        && let Some((fault, text, detail)) = a.banner(name)
    {
        let kind = if fault {
            BannerKind::NoMathResult
        } else {
            BannerKind::MathIncomplete
        };
        out.push(banner(kind, text, Some(detail)));
    }
    if let Some(age) = s.frame_age_s
        && age > s.stale_after_s.unwrap_or(STALE_AFTER_S)
        && s.audio_stopped.is_none()
    {
        out.push(banner(
            BannerKind::Stale,
            format!("STALE · {}", format::age(age)),
            Some("no fresh data; traces show their last frame".into()),
        ));
    }
    if let Some(TimingState::Jumped { from, to }) = s.timing {
        out.push(banner(
            BannerKind::OutputTimingJump,
            "OUTPUT TIMING JUMP".into(),
            Some(format!(
                "loopback offset {} → {} samples",
                format::fixed(from.0 as f64, 0),
                format::fixed(to.0 as f64, 0)
            )),
        ));
    }
    if let Some(ppm) = s.clock_drift_ppm {
        out.push(banner(
            BannerKind::ClockDrift,
            format!("CLOCK DRIFT · {}", drift_ppm(ppm)),
            Some(format!(
                "output and input clocks differ ({} per 10 s); loopback TF unaffected",
                format::ms(ppm.abs() * 1e-6 * 10.0, 2)
            )),
        ));
    }
    if let Some(l) = &s.log_not_saved {
        out.push(banner(
            BannerKind::LogNotSaved,
            l.banner.clone(),
            Some(l.detail.clone()),
        ));
    }
    if let Some(why) = &s.no_delay_estimate {
        let detail = match why {
            NoDelayEstimate::NotFound => "phase is not aligned; find or set the delay".into(),
            NoDelayEstimate::Refused(r) => {
                format!("finder: {}", crate::finding::no_estimate_reasons(r))
            }
            NoDelayEstimate::AwaitingPick => {
                "ambiguous: pick a candidate or set the delay · tracking paused".into()
            }
        };
        out.push(banner(
            BannerKind::NoDelayEstimate,
            "NO DELAY ESTIMATE".into(),
            Some(detail),
        ));
    }
    if let Some(d) = &s.file_damaged {
        out.push(banner(
            BannerKind::FileDamaged,
            d.banner.clone(),
            Some(d.detail.clone()),
        ));
    }
    out.sort_by_key(|b| b.kind);
    out
}

/// A drift in ppm as the banner and status lines show it: whole ppm from 10 up, one
/// decimal below (the threshold is 2 ppm), unsigned: which clock is fast does not change
/// what the operator must do.
pub fn drift_ppm(ppm: f64) -> String {
    let decimals = if ppm.abs() >= 10.0 { 0 } else { 1 };
    format!("{} ppm", format::fixed(ppm.abs(), decimals))
}

/// One placed banner row.
#[derive(Clone, Debug, PartialEq)]
pub struct BannerRow {
    pub rect: Rect,
    pub severity: Severity,
    pub text: String,
    pub detail: Option<String>,
}

/// Strip of banner rows above the plots.
#[derive(Clone, Debug, PartialEq)]
pub struct BannerStrip {
    /// Full view width from the top; zero height when no banner is up.
    pub rect: Rect,
    pub rows: Vec<BannerRow>,
}

/// Lays out the strip for a view of `size`, rows centred over `x .. x + w` (the plots).
pub fn banner_strip(banners: &[Banner], x: f32, w: f32, size: Viewport) -> BannerStrip {
    let area = Rect::new(x, 0.0, w, size.height * STRIP_MAX_FRACTION);
    let rows = layout_banners(banners, area);
    let h = rows
        .last()
        .map_or(0.0, |r| r.rect.bottom() + BANNER_PAD - area.y);
    BannerStrip {
        rect: Rect::new(0.0, 0.0, size.width, h),
        rows,
    }
}

/// Places `banners` (already in priority order) in `area`, padded by [`BANNER_PAD`] above
/// and below.
pub fn layout_banners(banners: &[Banner], area: Rect) -> Vec<BannerRow> {
    let fit = ((area.h - 2.0 * BANNER_PAD + BANNER_GAP) / (BANNER_HEIGHT + BANNER_GAP)).floor();
    let capacity = (fit.max(0.0) as usize).min(MAX_BANNERS);
    if capacity == 0 || banners.is_empty() {
        return Vec::new();
    }
    let w = (area.w - 16.0).clamp(0.0, BANNER_MAX_WIDTH);
    let x = area.x + (area.w - w) / 2.0;
    let rect = |i: usize| {
        Rect::new(
            x,
            area.y + BANNER_PAD + i as f32 * (BANNER_HEIGHT + BANNER_GAP),
            w,
            BANNER_HEIGHT,
        )
    };
    let (shown, overflow) = if banners.len() > capacity {
        (capacity - 1, Some(&banners[capacity - 1..]))
    } else {
        (banners.len(), None)
    };
    let mut rows: Vec<BannerRow> = banners[..shown]
        .iter()
        .enumerate()
        .map(|(i, b)| BannerRow {
            rect: rect(i),
            severity: b.severity,
            text: b.text.clone(),
            detail: b.detail.clone(),
        })
        .collect();
    if let Some(rest) = overflow {
        let names: Vec<&str> = rest.iter().map(|b| b.text.as_str()).collect();
        rows.push(BannerRow {
            rect: rect(shown),
            severity: rest
                .iter()
                .map(|b| b.severity)
                .min()
                .unwrap_or(Severity::Info),
            text: format!("+{} more", rest.len()),
            detail: Some(names.join(" · ")),
        });
    }
    rows
}

/// Draws placed rows: a filled bar, the text on the left, the detail on the right. In a row
/// too narrow for both, the detail is left out: the fault itself must stay readable.
/// A banner's headline as it fits `width` at `size`: whole, else its ` · ` parts from the
/// end off (`AUDIO STOPPED · audio host ended the stream at 22:36` → `AUDIO STOPPED`), the
/// first part cut with `…` only when even it does not fit. The full text stays in the row.
pub fn fit_headline(text: &str, width: f32, size: f32) -> String {
    let fits = |t: &str| crate::canvas::text_width(t, size) <= width;
    if fits(text) {
        return text.to_string();
    }
    let parts: Vec<&str> = text.split(" · ").collect();
    for n in (1..parts.len()).rev() {
        let t = parts[..n].join(" · ");
        if fits(&t) {
            return t;
        }
    }
    let mut out = parts[0].to_string();
    while !out.is_empty() && !fits(&format!("{out}…")) {
        out.pop();
    }
    format!("{}…", out.trim_end())
}

pub fn draw_banners(layer: &mut Layer, rows: &[BannerRow], theme: &Theme) {
    for r in rows {
        let colors = match r.severity {
            Severity::Fault => theme.banner_fault,
            Severity::Warning => theme.banner_warning,
            Severity::Info => theme.banner_info,
        };
        layer.rects.push(FillRect {
            rect: r.rect,
            color: colors.background,
            clip: None,
        });
        let cy = r.rect.y + r.rect.h / 2.0;
        let text = fit_headline(&r.text, r.rect.w - 20.0, theme.font_size);
        layer.labels.push(crate::primitives::Label {
            clip: Some(r.rect),
            ..crate::canvas::label(
                text.clone(),
                [r.rect.x + 10.0, cy],
                Anchor {
                    h: HAlign::Left,
                    v: VAlign::Center,
                },
                theme.font_size,
                colors.text,
            )
        });
        let fits = |d: &str| {
            10.0 + crate::canvas::text_width(&text, theme.font_size)
                + 16.0
                + crate::canvas::text_width(d, theme.small_font_size)
                + 10.0
                <= r.rect.w
        };
        if let Some(d) = r.detail.as_deref().filter(|d| fits(d)) {
            layer.labels.push(crate::primitives::Label {
                text: d.to_string(),
                pos: [r.rect.right() - 10.0, cy],
                anchor: Anchor {
                    h: HAlign::Right,
                    v: VAlign::Center,
                },
                size: theme.small_font_size,
                color: colors.text,
                clip: Some(r.rect),
            });
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
