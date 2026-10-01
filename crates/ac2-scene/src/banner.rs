//! Fault banners: which ones show, in which order, and where.
//!
//! Order (highest first) follows how much of the screen a condition invalidates and what
//! the operator must do first:
//!
//! | # | banner | severity | from |
//! |---|---|---|---|
//! | 1 | DAEMON NOT RESPONDING | fault | no keepalive for > 1.5 s |
//! | 2 | CLIP | fault | protection `CLIP` |
//! | 3 | NO REFERENCE | fault | protection `NO_REFERENCE` |
//! | 4 | NO SIGNAL | fault | protection `NO_SIGNAL` |
//! | 5 | STALE · age | warning | newest live frame older than 1 s (decision 2a) |
//! | 6 | OUTPUT TIMING JUMP | warning | loopback timing `jumped` |
//! | 7 | NO DELAY ESTIMATE | info | TF measurement without an accepted delay |
//!
//! Layout: centred at the top of the area, stacked downwards in that order, at most
//! [`MAX_BANNERS`] rows and never more than fit; when some do not fit, the last row says
//! how many more there are.

use ac2_proto::frame::ProtectionFlags;
use ac2_proto::model::{MeasKind, Measurement, TimingState};

use crate::format;
use crate::primitives::{Anchor, FillRect, HAlign, Layer, Rect, VAlign};
use crate::theme::Theme;
use crate::time::{DAEMON_SILENT_AFTER_S, STALE_AFTER_S};

pub const MAX_BANNERS: usize = 3;
pub const BANNER_HEIGHT: f32 = 24.0;
pub const BANNER_GAP: f32 = 4.0;
pub const BANNER_MAX_WIDTH: f32 = 460.0;
/// Inset of the banner stack from the top of the area.
pub const BANNER_TOP: f32 = 8.0;

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
    Clip,
    NoReference,
    NoSignal,
    Stale,
    OutputTimingJump,
    NoDelayEstimate,
}

impl BannerKind {
    pub fn severity(self) -> Severity {
        match self {
            Self::DaemonNotResponding | Self::Clip | Self::NoReference | Self::NoSignal => {
                Severity::Fault
            }
            Self::Stale | Self::OutputTimingJump => Severity::Warning,
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
    pub timing: Option<TimingState>,
    pub no_delay_estimate: bool,
}

/// True when a TF measurement has no delay the operator can rely on: none inserted yet, or
/// tracking without a current finding.
pub fn no_delay_estimate(m: &Measurement) -> bool {
    if !matches!(m.config.kind, MeasKind::Transfer { .. }) {
        return false;
    }
    match &m.delay {
        None => true,
        Some(d) => d.tracking && d.last_finding.is_none(),
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
            Some("reference input below its floor; check the loopback patch".into()),
        ));
    }
    if p.contains(ProtectionFlags::NO_SIGNAL) {
        out.push(banner(
            BannerKind::NoSignal,
            "NO SIGNAL".into(),
            Some("measurement input below its floor; check mic and input".into()),
        ));
    }
    if let Some(age) = s.frame_age_s
        && age > STALE_AFTER_S
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
    if s.no_delay_estimate {
        out.push(banner(
            BannerKind::NoDelayEstimate,
            "NO DELAY ESTIMATE".into(),
            Some("phase is not aligned; find or set the delay".into()),
        ));
    }
    out.sort_by_key(|b| b.kind);
    out
}

/// One placed banner row.
#[derive(Clone, Debug, PartialEq)]
pub struct BannerRow {
    pub rect: Rect,
    pub severity: Severity,
    pub text: String,
    pub detail: Option<String>,
}

/// Places `banners` (already in priority order) in `area`.
pub fn layout_banners(banners: &[Banner], area: Rect) -> Vec<BannerRow> {
    let fit = ((area.h - BANNER_TOP + BANNER_GAP) / (BANNER_HEIGHT + BANNER_GAP)).floor();
    let capacity = (fit.max(0.0) as usize).min(MAX_BANNERS);
    if capacity == 0 || banners.is_empty() {
        return Vec::new();
    }
    let w = (area.w - 16.0).clamp(0.0, BANNER_MAX_WIDTH);
    let x = area.x + (area.w - w) / 2.0;
    let rect = |i: usize| {
        Rect::new(
            x,
            area.y + BANNER_TOP + i as f32 * (BANNER_HEIGHT + BANNER_GAP),
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

/// Draws placed rows: a filled bar, the text on the left, the detail on the right.
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
        layer.labels.push(crate::canvas::label(
            r.text.clone(),
            [r.rect.x + 10.0, cy],
            Anchor {
                h: HAlign::Left,
                v: VAlign::Center,
            },
            theme.font_size,
            colors.text,
        ));
        if let Some(d) = &r.detail {
            layer.labels.push(crate::primitives::Label {
                text: d.clone(),
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
mod tests {
    use super::*;
    use ac2_proto::units::Samples;

    fn texts(b: &[Banner]) -> Vec<&str> {
        b.iter().map(|b| b.text.as_str()).collect()
    }

    fn everything() -> Status {
        Status {
            daemon_silence_s: 3.2,
            protection: ProtectionFlags::CLIP
                .with(ProtectionFlags::NO_REFERENCE)
                .with(ProtectionFlags::NO_SIGNAL),
            frame_age_s: Some(4.25),
            timing: Some(TimingState::Jumped {
                from: Samples(480),
                to: Samples(-512),
            }),
            no_delay_estimate: true,
        }
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
                "NO SIGNAL",
                "STALE · 4.2 s",
                "OUTPUT TIMING JUMP",
                "NO DELAY ESTIMATE"
            ]
        );
        assert_eq!(b[0].detail.as_deref(), Some("no keepalive for 3.2 s"));
        assert_eq!(
            b[5].detail.as_deref(),
            Some("loopback offset 480 → −512 samples")
        );
        assert_eq!(b[0].severity, Severity::Fault);
        assert_eq!(b[4].severity, Severity::Warning);
        assert_eq!(b[6].severity, Severity::Info);
        // Severity never increases down the list.
        assert!(b.windows(2).all(|w| w[0].severity <= w[1].severity));
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
    }

    #[test]
    fn stacking_and_overflow() {
        let b = banners(&everything());
        let area = Rect::new(0.0, 0.0, 800.0, 400.0);
        let rows = layout_banners(&b, area);
        assert_eq!(rows.len(), MAX_BANNERS);
        assert_eq!(rows[0].text, "DAEMON NOT RESPONDING");
        assert_eq!(rows[1].text, "CLIP");
        assert_eq!(rows[2].text, "+5 more");
        assert_eq!(rows[2].severity, Severity::Fault);
        assert_eq!(
            rows[2].detail.as_deref(),
            Some(
                "NO REFERENCE · NO SIGNAL · STALE · 4.2 s · OUTPUT TIMING JUMP · NO DELAY ESTIMATE"
            )
        );
        // Stacked downwards without overlap, centred, capped width.
        for w in rows.windows(2) {
            assert!(w[1].rect.y >= w[0].rect.bottom() + BANNER_GAP - 1e-3);
        }
        assert_eq!(rows[0].rect.w, BANNER_MAX_WIDTH);
        assert!((rows[0].rect.x + rows[0].rect.w / 2.0 - 400.0).abs() < 1e-3);
        assert_eq!(rows[0].rect.y, BANNER_TOP);

        // Exactly MAX_BANNERS fit without an overflow row.
        let rows = layout_banners(&b[..3], area);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].text, "NO REFERENCE");

        // A short pane holds fewer rows.
        let short = Rect::new(
            0.0,
            0.0,
            300.0,
            BANNER_TOP + 2.0 * BANNER_HEIGHT + BANNER_GAP,
        );
        let rows = layout_banners(&b, short);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].text, "+6 more");
        assert_eq!(rows[0].rect.w, 284.0);
        assert!(layout_banners(&b, Rect::new(0.0, 0.0, 300.0, 20.0)).is_empty());
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
}
