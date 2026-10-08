//! Fault banners: which ones show, in which order, and where.
//!
//! Order (highest first) follows how much of the screen a condition invalidates and what
//! the operator must do first:
//!
//! | # | banner | severity | from |
//! |---|---|---|---|
//! | 1 | DAEMON NOT RESPONDING | fault | no keepalive for > 1.5 s |
//! | 2 | AUDIO STOPPED · what, since | fault | `session.stopped` (the daemon is reopening the session) |
//! | 3 | CLIP | fault | protection `CLIP` |
//! | 4 | NO REFERENCE | fault | protection `NO_REFERENCE` (the detail names the stimulus keys while nothing here plays) |
//! | 5 | CHECK ROUTING | fault | protection `CHECK_ROUTING` (inputs identical or swapped) |
//! | 6 | NO SIGNAL | fault | protection `NO_SIGNAL` |
//! | 7 | NO AVERAGE · n OF m POSITIONS | fault | the shown spatial average has fewer than two usable positions |
//! | 8 | STALE · age | warning | newest live frame older than 1 s (decision 2a) |
//! | 9 | AVERAGE · n OF m POSITIONS | warning | the shown spatial average left positions out (the detail names them and why) |
//! | 10 | OUTPUT TIMING JUMP | warning | loopback timing `jumped` |
//! | 11 | CLOCK DRIFT · ppm | warning | loopback timing drift `warning` (output and input on different clocks) |
//! | 12 | NO DELAY ESTIMATE | info | TF measurement without a delay, the finder refused, or an ambiguous finding awaits a pick (the detail says which) |
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
    NoDelayEstimate,
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
            Self::Stale | Self::MathIncomplete | Self::OutputTimingJump | Self::ClockDrift => {
                Severity::Warning
            }
            Self::NoDelayEstimate => Severity::Info,
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
pub(crate) mod tests {
    use super::*;
    use ac2_proto::units::Samples;

    fn texts(b: &[Banner]) -> Vec<&str> {
        b.iter().map(|b| b.text.as_str()).collect()
    }

    pub(crate) fn everything() -> Status {
        Status {
            daemon_silence_s: 3.2,
            protection: ProtectionFlags::CLIP
                .with(ProtectionFlags::NO_REFERENCE)
                .with(ProtectionFlags::CHECK_ROUTING)
                .with(ProtectionFlags::NO_SIGNAL),
            frame_age_s: Some(4.25),
            stale_after_s: None,
            timing: Some(TimingState::Jumped {
                from: Samples(480),
                to: Samples(-512),
            }),
            clock_drift_ppm: Some(-52.4),
            audio_stopped: None,
            no_delay_estimate: Some(NoDelayEstimate::NotFound),
            math: None,
            drive: Default::default(),
        }
    }

    #[test]
    fn clock_drift_alone_names_the_value_and_what_it_spares() {
        let b = banners(&Status {
            clock_drift_ppm: Some(3.24),
            timing: Some(TimingState::Locked {
                offset: Samples(2000),
            }),
            ..Status::default()
        });
        assert_eq!(texts(&b), ["CLOCK DRIFT · 3.2 ppm"]);
        assert_eq!(
            b[0].detail.as_deref(),
            Some("output and input clocks differ (0.03 ms per 10 s); loopback TF unaffected")
        );
        assert_eq!(drift_ppm(150.4), "150 ppm");
        assert_eq!(drift_ppm(-9.94), "9.9 ppm");
    }

    #[test]
    fn audio_stopped_follows_the_daemon_and_replaces_stale() {
        use ac2_proto::model::{AudioStopped, Recovery, StopCause};
        use ac2_proto::units::WallNs;
        let stopped = AudioStopped {
            since: WallNs(1_791_232_572_000_000_000),
            cause: StopCause::NotDelivering { after_ms: 1000 },
            recovery: Recovery::Opening {
                attempt: 1,
                started: WallNs(1_791_232_573_000_000_000),
            },
        };
        let b = banners(&Status {
            daemon_silence_s: 2.0,
            frame_age_s: Some(4.0),
            protection: ProtectionFlags::NO_SIGNAL,
            audio_stopped: Some(crate::audio::audio_stopped_text(
                &stopped,
                WallNs(1_791_232_574_000_000_000),
                |_| 0,
            )),
            ..Status::default()
        });
        assert_eq!(
            texts(&b),
            [
                "DAEMON NOT RESPONDING",
                "AUDIO STOPPED · device not delivering since 20:36",
                "NO SIGNAL"
            ]
        );
        assert_eq!(b[1].severity, Severity::Fault);
        assert_eq!(
            b[1].detail.as_deref(),
            Some("reopening the session (attempt 1) · measurements paused")
        );
    }

    #[test]
    fn healthy_shows_nothing() {
        assert!(banners(&Status::default()).is_empty());
        let s = Status {
            daemon_silence_s: 1.5,
            frame_age_s: Some(1.0),
            timing: Some(TimingState::Locked { offset: Samples(3) }),
            ..Status::default()
        };
        assert!(banners(&s).is_empty());
    }

    #[test]
    fn priority_order_and_text() {
        let b = banners(&everything());
        assert_eq!(
            texts(&b),
            [
                "DAEMON NOT RESPONDING",
                "CLIP",
                "NO REFERENCE",
                "CHECK ROUTING",
                "NO SIGNAL",
                "STALE · 4.2 s",
                "OUTPUT TIMING JUMP",
                "CLOCK DRIFT · 52 ppm",
                "NO DELAY ESTIMATE"
            ]
        );
        assert_eq!(
            b[7].detail.as_deref(),
            Some("output and input clocks differ (0.52 ms per 10 s); loopback TF unaffected")
        );
        assert_eq!(b[7].severity, Severity::Warning);
        assert_eq!(b[0].detail.as_deref(), Some("no keepalive for 3.2 s"));
        assert_eq!(
            b[6].detail.as_deref(),
            Some("loopback offset 480 → −512 samples")
        );
        assert_eq!(b[0].severity, Severity::Fault);
        assert_eq!(b[3].severity, Severity::Fault);
        assert_eq!(b[5].severity, Severity::Warning);
        assert_eq!(b[8].severity, Severity::Info);
        // Severity never increases down the list.
        assert!(b.windows(2).all(|w| w[0].severity <= w[1].severity));
    }

    #[test]
    fn check_routing_ranks_right_after_no_reference() {
        let only = |p: ProtectionFlags| {
            texts(&banners(&Status {
                protection: p,
                ..Status::default()
            }))
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
        };
        assert_eq!(only(ProtectionFlags::CHECK_ROUTING), ["CHECK ROUTING"]);
        assert_eq!(
            only(
                ProtectionFlags::NO_SIGNAL
                    .with(ProtectionFlags::CHECK_ROUTING)
                    .with(ProtectionFlags::NO_REFERENCE)
            ),
            ["NO REFERENCE", "CHECK ROUTING", "NO SIGNAL"]
        );
        let b = banners(&Status {
            protection: ProtectionFlags::CHECK_ROUTING,
            ..Status::default()
        });
        assert_eq!(b[0].severity, Severity::Fault);
        assert!(
            b[0].detail
                .as_deref()
                .is_some_and(|d| d.contains("input patch"))
        );
        // Other flags raise nothing.
        assert!(
            only(ProtectionFlags::WEAK_REFERENCE.with(ProtectionFlags::DISCONTINUITY)).is_empty()
        );
    }

    #[test]
    fn no_delay_estimate_says_why() {
        use ac2_proto::model::{
            DelayBand, DelayConfidence, DelayFinding, DelayOutcome, DelayState, LogGridSpec,
            MeasConfig, TfAveraging, TransferConfig,
        };
        use ac2_proto::units::{MeasId, Rev, Seconds, WallNs};
        let refused = DelayFinding {
            outcome: DelayOutcome::NoEstimate {
                reasons: vec![NoEstimateReason::LowPsr, NoEstimateReason::LowBandSnr],
            },
            confidence: DelayConfidence {
                psr_db: None,
                psr_acq_db: None,
                band_snr_db: None,
                excited_fraction: None,
                uncertainty_samples: None,
                pulse_width_samples: None,
                period: None,
            },
            band: DelayBand::Sub,
            observation: Seconds(4.0),
            candidates: vec![],
            found_at: WallNs(0),
        };
        let mut m = Measurement {
            id: MeasId(1),
            config: MeasConfig {
                name: "tf".into(),
                kind: MeasKind::Transfer {
                    config: TransferConfig {
                        reference_input: 0,
                        measurement_input: 1,
                        averaging: TfAveraging::Fifo { blocks: 4 },
                        grid: LogGridSpec {
                            ppo: 12,
                            k_min: -12,
                            k_max: 12,
                        },
                        smoothing: None,
                        depth: ac2_proto::model::DepthPolicy::EqualConfidence,
                    },
                },
            },
            config_rev: Rev(1),
            running: true,
            delay: Some(DelayState {
                applied: Seconds(0.0),
                applied_samples: 0.0,
                nudged: Seconds(0.0),
                nudged_samples: 0.0,
                tracking: false,
                awaiting_pick: false,
                last_finding: Some(refused),
            }),
            grid_id: None,
        };
        let why = no_delay_estimate(&m);
        assert_eq!(
            why,
            Some(NoDelayEstimate::Refused(vec![
                NoEstimateReason::LowPsr,
                NoEstimateReason::LowBandSnr
            ]))
        );
        let b = banners(&Status {
            no_delay_estimate: why,
            ..Status::default()
        });
        assert_eq!(b[0].text, "NO DELAY ESTIMATE");
        assert_eq!(
            b[0].detail.as_deref(),
            Some("finder: no clear peak, too noisy in band")
        );
        // No finding and not tracking: the operator's delay stands.
        if let Some(d) = &mut m.delay {
            d.last_finding = None;
        }
        assert_eq!(no_delay_estimate(&m), None);
        if let Some(d) = &mut m.delay {
            d.tracking = true;
        }
        assert_eq!(no_delay_estimate(&m), Some(NoDelayEstimate::NotFound));
        // An ambiguous finding nobody picked from: tracking is paused, and says so.
        if let Some(d) = &mut m.delay {
            d.awaiting_pick = true;
        }
        let why = no_delay_estimate(&m);
        assert_eq!(why, Some(NoDelayEstimate::AwaitingPick));
        let b = banners(&Status {
            no_delay_estimate: why,
            ..Status::default()
        });
        assert_eq!(b[0].text, "NO DELAY ESTIMATE");
        assert_eq!(
            b[0].detail.as_deref(),
            Some("ambiguous: pick a candidate or set the delay · tracking paused")
        );
    }

    #[test]
    fn stale_threshold() {
        let at = |age| {
            texts(&banners(&Status {
                frame_age_s: Some(age),
                ..Status::default()
            }))
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
        };
        assert!(at(1.0).is_empty());
        assert_eq!(at(1.04), ["STALE · 1.0 s"]);
        assert_eq!(at(75.0), ["STALE · 1 min"]);
        // A once-a-second stream's frame 1.5 s old is between frames, not stale.
        let once = |age| {
            banners(&Status {
                frame_age_s: Some(age),
                stale_after_s: Some(3.0),
                ..Status::default()
            })
        };
        assert!(once(1.5).is_empty());
        assert_eq!(texts(&once(3.5)), ["STALE · 3.5 s"]);
    }

    #[test]
    fn stacking_and_overflow() {
        let b = banners(&everything());
        let area = Rect::new(0.0, 0.0, 800.0, 400.0);
        let rows = layout_banners(&b, area);
        assert_eq!(rows.len(), MAX_BANNERS);
        assert_eq!(rows[0].text, "DAEMON NOT RESPONDING");
        assert_eq!(rows[1].text, "CLIP");
        assert_eq!(rows[2].text, "+7 more");
        assert_eq!(rows[2].severity, Severity::Fault);
        assert_eq!(
            rows[2].detail.as_deref(),
            Some(
                "NO REFERENCE · CHECK ROUTING · NO SIGNAL · STALE · 4.2 s · OUTPUT TIMING JUMP · CLOCK DRIFT · 52 ppm · NO DELAY ESTIMATE"
            )
        );
        // Stacked downwards without overlap, centred, capped width.
        for w in rows.windows(2) {
            assert!(w[1].rect.y >= w[0].rect.bottom() + BANNER_GAP - 1e-3);
        }
        assert_eq!(rows[0].rect.w, BANNER_MAX_WIDTH);
        assert!((rows[0].rect.x + rows[0].rect.w / 2.0 - 400.0).abs() < 1e-3);
        assert_eq!(rows[0].rect.y, BANNER_PAD);

        // Exactly MAX_BANNERS fit without an overflow row.
        let rows = layout_banners(&b[..3], area);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].text, "NO REFERENCE");

        // A short pane holds fewer rows.
        let short = Rect::new(
            0.0,
            0.0,
            300.0,
            2.0 * BANNER_PAD + 2.0 * BANNER_HEIGHT + BANNER_GAP,
        );
        let rows = layout_banners(&b, short);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].text, "+8 more");
        assert_eq!(rows[0].rect.w, 284.0);
        assert!(layout_banners(&b, Rect::new(0.0, 0.0, 300.0, 20.0)).is_empty());
    }

    #[test]
    fn strip_height_follows_the_rows() {
        let size = Viewport {
            width: 800.0,
            height: 600.0,
        };
        let none = banner_strip(&[], 48.0, 740.0, size);
        assert!(none.rows.is_empty());
        assert_eq!(none.rect.h, 0.0);
        let b = banners(&everything());
        let one = banner_strip(&b[..1], 48.0, 740.0, size);
        assert_eq!(one.rect.h, 2.0 * BANNER_PAD + BANNER_HEIGHT);
        let all = banner_strip(&b, 48.0, 740.0, size);
        assert_eq!(all.rows.len(), MAX_BANNERS);
        assert_eq!(
            all.rect.h,
            2.0 * BANNER_PAD + 3.0 * BANNER_HEIGHT + 2.0 * BANNER_GAP
        );
        // Rows are centred over the given span and inside the strip.
        for r in &all.rows {
            assert!((r.rect.x + r.rect.w / 2.0 - (48.0 + 370.0)).abs() < 1e-3);
            assert!(r.rect.y >= all.rect.y && r.rect.bottom() <= all.rect.bottom());
        }
        // A short view gives the strip at most half its height.
        let short = banner_strip(
            &b,
            0.0,
            300.0,
            Viewport {
                width: 300.0,
                height: 100.0,
            },
        );
        assert!(short.rect.h <= 50.0);
        assert_eq!(short.rows.len(), 1);
        assert_eq!(short.rows[0].text, "+9 more");
    }

    #[test]
    fn draw_uses_severity_colours() {
        let theme = Theme::dark();
        let rows = layout_banners(&banners(&everything()), Rect::new(0.0, 0.0, 800.0, 400.0));
        let mut layer = Layer::default();
        draw_banners(&mut layer, &rows, &theme);
        assert_eq!(layer.rects.len(), 3);
        assert_eq!(layer.rects[0].color, theme.banner_fault.background);
        assert_eq!(layer.labels[0].text, "DAEMON NOT RESPONDING");
        assert_eq!(layer.labels[0].color, theme.banner_fault.text);
    }

    /// A long headline (the audio host's stop with its time) shortens part by part in a
    /// narrow pane, as the SPL pane at its default split is, and never leaves its row.
    #[test]
    fn long_headlines_shorten_to_their_row() {
        let theme = Theme::dark();
        let text = "AUDIO STOPPED · audio host ended the stream at 22:36";
        let s = Status {
            audio_stopped: Some(crate::audio::AudioStoppedText {
                banner: text.into(),
                detail: "reopening (attempt 2) · measurements paused".into(),
                bar: [String::new(), String::new()],
                status: String::new(),
            }),
            ..Status::default()
        };
        let mut seen = Vec::new();
        for w in [200.0, 300.0, 360.0, 420.0, 600.0, 900.0, 1300.0] {
            let rows = layout_banners(&banners(&s), Rect::new(0.0, 0.0, w, 200.0));
            let mut layer = Layer::default();
            draw_banners(&mut layer, &rows, &theme);
            let l = &layer.labels[0];
            let right = l.pos[0] + crate::canvas::text_width(&l.text, l.size);
            assert!(
                right <= rows[0].rect.right() - 10.0 + 0.01,
                "{w}: {}",
                l.text
            );
            assert_eq!(rows[0].text, text);
            seen.push(l.text.clone());
        }
        assert_eq!(seen[0], "AUDIO STOPPED");
        assert_eq!(seen.last().map(String::as_str), Some(text));
        assert_eq!(fit_headline("AUDIO STOPPED", 60.0, 14.0), "AUDIO…");
    }

    /// NO REFERENCE's detail follows this app's stimulus, and every variant fits beside the
    /// banner text in a full-width row in every theme.
    #[test]
    fn no_reference_reminds_of_the_stimulus_keys() {
        use crate::stimulus::Drive;
        let detail = |drive: Drive| {
            let s = Status {
                protection: ProtectionFlags::NO_REFERENCE,
                drive: drive.clone(),
                ..Status::default()
            };
            let b = banners(&s);
            assert_eq!(b[0].text, "NO REFERENCE");
            let d = b[0].detail.clone().unwrap_or_default();
            for theme in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
                let rows = layout_banners(&b, Rect::new(0.0, 0.0, 800.0, 200.0));
                let mut layer = Layer::default();
                draw_banners(&mut layer, &rows, &theme);
                assert!(
                    layer.labels.iter().any(|l| l.text == d),
                    "{d} not drawn at {} px",
                    theme.font_size
                );
            }
            d
        };
        assert_eq!(
            detail(Drive::Idle {
                arm: "Space".into(),
                fire: "Enter".into(),
            }),
            "stimulus off: Space arms, Enter starts it"
        );
        assert_eq!(
            detail(Drive::Armed {
                fire: "Enter".into(),
            }),
            "stimulus armed: Enter starts it"
        );
        assert_eq!(
            detail(Drive::SweepView),
            "stimulus off: arm it from a transfer pane"
        );
        assert_eq!(
            detail(Drive::Playing),
            "reference silent: check the loopback cable"
        );
    }

    #[test]
    fn narrow_rows_drop_the_detail() {
        let theme = Theme::dark();
        let s = Status {
            no_delay_estimate: Some(NoDelayEstimate::NotFound),
            ..Status::default()
        };
        let draw = |w: f32| {
            let rows = layout_banners(&banners(&s), Rect::new(0.0, 0.0, w, 200.0));
            let mut layer = Layer::default();
            draw_banners(&mut layer, &rows, &theme);
            (rows, layer)
        };
        let (rows, wide) = draw(800.0);
        assert_eq!(wide.labels.len(), 2);
        assert!(rows[0].detail.is_some());
        // Too narrow for both: the banner text stays, the detail goes (it stays in the
        // row data for other uses).
        let (rows, narrow) = draw(300.0);
        assert_eq!(narrow.labels.len(), 1);
        assert_eq!(narrow.labels[0].text, "NO DELAY ESTIMATE");
        assert!(rows[0].detail.is_some());
    }

    /// A math average without enough positions is a fault right under the signal faults;
    /// one missing positions is a warning after STALE; a full one says nothing.
    #[test]
    fn math_average_banners() {
        use crate::math::MathStatus;
        use ac2_proto::frame::{MathState, OperandState, OperandStatus as MemberStatus};
        use ac2_proto::model::{MathConfig, MathDomain, Operand, PhaseBasis};
        use ac2_proto::units::MeasId;
        let avg = |s: &[MemberStatus]| {
            let of: Vec<Operand> = (1..=s.len() as u32)
                .map(|m| Operand::Meas { meas: MeasId(m) })
                .collect();
            let c = MathConfig::power_average(
                ac2_proto::model::TraceOwner::Imported,
                MathDomain::Transfer,
                of.clone(),
            );
            let a = MathState {
                operands: of
                    .into_iter()
                    .zip(s)
                    .map(|(operand, status)| OperandState {
                        operand,
                        status: *status,
                    })
                    .collect(),
                phase: PhaseBasis::SharedTimeBase,
            };
            Some((
                "Audience".to_string(),
                MathStatus::new(&c, &a, |o| match o {
                    Operand::Meas { meas } => format!("Seat {}", meas.0),
                    Operand::Trace { trace } => format!("trace {trace}"),
                }),
            ))
        };
        let base = Status {
            protection: ProtectionFlags::NO_SIGNAL,
            frame_age_s: Some(2.0),
            ..Status::default()
        };
        let none = Status {
            math: avg(&[MemberStatus::Included, MemberStatus::Stopped]),
            ..base.clone()
        };
        let b = banners(&none);
        assert_eq!(
            texts(&b),
            [
                "NO SIGNAL",
                "NO AVERAGE · 1 OF 2 POSITIONS",
                "STALE · 2.0 s"
            ]
        );
        assert_eq!(b[1].severity, Severity::Fault);
        assert_eq!(
            b[1].detail.as_deref(),
            Some("Audience: left out Seat 2: stopped")
        );
        let short = Status {
            math: avg(&[
                MemberStatus::Included,
                MemberStatus::Included,
                MemberStatus::Settling,
            ]),
            ..base.clone()
        };
        let b = banners(&short);
        assert_eq!(
            texts(&b),
            ["NO SIGNAL", "STALE · 2.0 s", "AVERAGE · 2 OF 3 POSITIONS"]
        );
        assert_eq!(b[2].severity, Severity::Warning);
        let full = Status {
            math: avg(&[MemberStatus::Included; 3]),
            ..base
        };
        assert_eq!(texts(&banners(&full)), ["NO SIGNAL", "STALE · 2.0 s"]);
    }
}
