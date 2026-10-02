//! SPL readout block: big number, unit, metric name, interval statistics and the
//! calibration state (decisions 7a/7b).
//!
//! Metric names follow IEC 61672 notation: `L` + frequency weighting + time weighting
//! (`LAF` = A-weighted, Fast), `LAeq`, `LCpeak`, `LAFmax`, `LAFmin`.

use ac2_proto::frame::SplFrame;
use ac2_proto::model::{CalStatus, LevelScale, PeakWeighting, TimeWeighting, Weighting};
use ac2_proto::units::WallNs;

use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, anchor, label};
use crate::format;
use crate::primitives::{HAlign, Rect, Scene, VAlign, Viewport};
use crate::theme::Theme;
use crate::time::{self, ClockOffset, Freshness};

fn w_letter(w: Weighting) -> &'static str {
    match w {
        Weighting::A => "A",
        Weighting::C => "C",
        Weighting::Z => "Z",
    }
}

fn tw_letter(t: TimeWeighting) -> &'static str {
    match t {
        TimeWeighting::Fast => "F",
        TimeWeighting::Slow => "S",
        TimeWeighting::Impulse => "I",
    }
}

fn pw_letter(p: PeakWeighting) -> &'static str {
    match p {
        PeakWeighting::C => "C",
        PeakWeighting::Z => "Z",
    }
}

/// One secondary statistic.
#[derive(Clone, Debug, PartialEq)]
pub struct SplStat {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplReadout {
    /// `LAF`.
    pub metric: String,
    /// `94.0`.
    pub value: String,
    /// `dB SPL` or `dBFS`.
    pub unit: String,
    /// LAeq, LCpeak, LAFmax, LAFmin.
    pub stats: Vec<SplStat>,
    /// `over 1 min 23 s`.
    pub interval: String,
    /// `cal 3 h ago`, `cal from other mic / input`, `uncalibrated`; `· mic curve` when the
    /// curve is applied.
    pub cal: String,
    /// `STALE 3.2 s` when the frame is stale.
    pub stale: Option<String>,
}

/// Calibration text of a readout (decisions 7a/7b, `docs/design/q7-calibration.md` §3):
/// the calibration's age when it belongs to this device + input + mic, a mismatch warning
/// when it belongs to another mic or input, `uncalibrated` otherwise; `· mic curve` when the
/// mic's correction curve is applied. The age is on the daemon clock (`offset`).
pub fn cal_text(
    cal: CalStatus,
    mic_curve: bool,
    client_now: WallNs,
    offset: ClockOffset,
) -> String {
    let base = match cal {
        CalStatus::Uncalibrated => "uncalibrated".to_string(),
        CalStatus::OtherMicOrInput { .. } => "cal from other mic / input".to_string(),
        CalStatus::Verified { calibrated_at } => format!(
            "cal {}",
            format::ago(time::age_s(calibrated_at, client_now, offset))
        ),
    };
    if mic_curve {
        format!("{base} · mic curve")
    } else {
        base
    }
}

pub fn spl_readout(frame: &SplFrame, cal: String, freshness: Option<Freshness>) -> SplReadout {
    let m = &frame.meta;
    let w = w_letter(m.weighting);
    let tw = tw_letter(m.time_weighting);
    let stat = |label: String, v: f64| SplStat {
        label,
        value: format::level(v),
    };
    SplReadout {
        metric: format!("L{w}{tw}"),
        value: format::level(m.level),
        unit: match m.scale {
            LevelScale::Dbfs => "dBFS",
            LevelScale::DbSpl => "dB SPL",
        }
        .to_string(),
        stats: vec![
            stat(format!("L{w}eq"), m.leq),
            stat(format!("L{}peak", pw_letter(m.peak_weighting)), m.lpeak),
            stat(format!("L{w}{tw}max"), m.lmax),
            stat(format!("L{w}{tw}min"), m.lmin),
        ],
        interval: format!("over {}", format::duration(m.duration.0)),
        cal,
        stale: freshness
            .filter(Freshness::is_stale)
            .map(|f| format!("STALE {}", format::age(f.age_s()))),
    }
}

/// Lays the readout out in `size` below the banner strip: metric top-left, big number with
/// its unit, a row of statistics, interval and calibration at the bottom.
pub fn spl_scene(r: &SplReadout, status: &Status, theme: &Theme, size: Viewport) -> SplScene {
    let mut c = Canvas::new(size, theme);
    let pad = 12.0;
    let strip = canvas::banner_strip(&mut c, status, pad, size.width - 2.0 * pad, size, theme);
    let top = strip.rect.bottom();
    let area = Rect::new(
        pad,
        top + pad,
        size.width - 2.0 * pad,
        (size.height - top - 2.0 * pad).max(1.0),
    );
    let main = if r.stale.is_some() {
        theme.text_dim
    } else {
        theme.text
    };
    c.overlay.labels.push(label(
        r.metric.clone(),
        [area.x, area.y],
        anchor(HAlign::Left, VAlign::Top),
        theme.font_size * 1.4,
        theme.text_dim,
    ));
    if let Some(s) = &r.stale {
        c.overlay.labels.push(label(
            s.clone(),
            [area.right(), area.y],
            anchor(HAlign::Right, VAlign::Top),
            theme.font_size,
            theme.banner_warning.background,
        ));
    }
    let base = area.y + area.h * 0.62;
    let split = area.x + area.w * 0.66;
    c.overlay.labels.push(label(
        r.value.clone(),
        [split, base],
        anchor(HAlign::Right, VAlign::Baseline),
        theme.big_font_size,
        main,
    ));
    c.overlay.labels.push(label(
        r.unit.clone(),
        [split + 8.0, base],
        anchor(HAlign::Left, VAlign::Baseline),
        theme.font_size * 1.6,
        main,
    ));
    // One row of statistics when every cell fits its share of the width, else two rows.
    let texts: Vec<String> = r
        .stats
        .iter()
        .map(|s| format!("{} {}", s.label, s.value))
        .collect();
    let widest = texts
        .iter()
        .map(|t| canvas::text_width(t, theme.font_size))
        .fold(0.0, f32::max);
    let per_row = if (widest + 12.0) * texts.len() as f32 <= area.w {
        texts.len().max(1)
    } else {
        texts.len().div_ceil(2).max(1)
    };
    let row = area.y + area.h * 0.78;
    for (i, t) in texts.into_iter().enumerate() {
        let (r_i, c_i) = (i / per_row, i % per_row);
        c.overlay.labels.push(label(
            t,
            [
                area.x + area.w * (c_i as f32 + 0.5) / per_row as f32,
                row + r_i as f32 * 1.4 * theme.font_size,
            ],
            anchor(HAlign::Center, VAlign::Center),
            theme.font_size,
            theme.text,
        ));
    }
    c.overlay.labels.push(label(
        r.interval.clone(),
        [area.x, area.bottom()],
        anchor(HAlign::Left, VAlign::Bottom),
        theme.small_font_size,
        theme.text_dim,
    ));
    c.overlay.labels.push(label(
        r.cal.clone(),
        [area.right(), area.bottom()],
        anchor(HAlign::Right, VAlign::Bottom),
        theme.small_font_size,
        theme.text_dim,
    ));
    SplScene {
        scene: c.into_scene(size),
        area,
        strip: strip.rect,
        banners: strip.rows,
    }
}

/// The SPL meter as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct SplScene {
    pub scene: Scene,
    /// Where the readout is laid out, below the banner strip.
    pub area: Rect,
    /// Banner strip above the readout; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::frame::SplMeta;
    use ac2_proto::units::{MeasId, Seconds};

    const H: u64 = 3600 * 1_000_000_000;

    fn frame(scale: LevelScale) -> SplFrame {
        SplFrame {
            meas: MeasId(4),
            meta: SplMeta {
                scale,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
                level: 94.04,
                lmax: 97.25,
                lmin: f64::NEG_INFINITY,
                leq: 92.06,
                lpeak: 110.31,
                duration: Seconds(83.9),
                cal: CalStatus::Verified {
                    calibrated_at: WallNs(97 * H),
                },
                mic_curve: false,
            },
        }
    }

    #[test]
    fn readout_strings() {
        let r = spl_readout(
            &frame(LevelScale::DbSpl),
            "cal 3 h ago".into(),
            Some(Freshness::from_age(0.2)),
        );
        assert_eq!(r.metric, "LAF");
        assert_eq!(r.value, "94.0");
        assert_eq!(r.unit, "dB SPL");
        let stats: Vec<String> = r
            .stats
            .iter()
            .map(|s| format!("{} {}", s.label, s.value))
            .collect();
        assert_eq!(
            stats,
            ["LAeq 92.1", "LCpeak 110.3", "LAFmax 97.2", "LAFmin —"]
        );
        assert_eq!(r.interval, "over 1 min 23 s");
        assert_eq!(r.stale, None);
        let r = spl_readout(
            &frame(LevelScale::Dbfs),
            "uncalibrated".into(),
            Some(Freshness::from_age(3.24)),
        );
        assert_eq!(r.unit, "dBFS");
        assert_eq!(r.stale.as_deref(), Some("STALE 3.2 s"));
    }

    #[test]
    fn calibration_state() {
        let now = WallNs(100 * H);
        let off = ClockOffset(0);
        let verified = CalStatus::Verified {
            calibrated_at: WallNs(97 * H - 1),
        };
        assert_eq!(cal_text(verified, false, now, off), "cal 3 h ago");
        assert_eq!(
            cal_text(verified, true, now, off),
            "cal 3 h ago · mic curve"
        );
        let other = CalStatus::OtherMicOrInput {
            calibrated_at: WallNs(99 * H),
        };
        assert_eq!(
            cal_text(other, false, now, off),
            "cal from other mic / input"
        );
        assert_eq!(
            cal_text(CalStatus::Uncalibrated, false, now, off),
            "uncalibrated"
        );
        assert_eq!(
            cal_text(CalStatus::Uncalibrated, true, now, off),
            "uncalibrated · mic curve"
        );
        // The clock offset applies: client 2 h behind the daemon.
        let off = ClockOffset(2 * H as i64);
        assert_eq!(
            cal_text(verified, false, WallNs(98 * H), off),
            "cal 3 h ago"
        );
        // Minutes and days.
        let v = |ago: u64| CalStatus::Verified {
            calibrated_at: WallNs(100 * H - ago),
        };
        assert_eq!(
            cal_text(v(H / 6), false, now, ClockOffset(0)),
            "cal 10 min ago"
        );
        assert_eq!(
            cal_text(v(50 * H), false, now, ClockOffset(0)),
            "cal 2 d ago"
        );
    }

    #[test]
    fn scene_carries_the_strings() {
        let r = spl_readout(
            &frame(LevelScale::DbSpl),
            "cal 3 h ago".into(),
            Some(Freshness::from_age(5.0)),
        );
        let s = spl_scene(
            &r,
            &Status::default(),
            &Theme::dark(),
            Viewport {
                width: 400.0,
                height: 200.0,
            },
        );
        assert!(s.banners.is_empty());
        assert_eq!(s.strip.h, 0.0);
        assert_eq!(s.area, Rect::new(12.0, 12.0, 376.0, 176.0));
        let s = s.scene;
        let texts: Vec<&str> = s
            .layers
            .iter()
            .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
            .collect();
        for want in [
            "LAF",
            "94.0",
            "dB SPL",
            "LAeq 92.1",
            "LCpeak 110.3",
            "over 1 min 23 s",
            "cal 3 h ago",
            "STALE 5.0 s",
        ] {
            assert!(texts.contains(&want), "{want} in {texts:?}");
        }
        let big = s.layers[2]
            .labels
            .iter()
            .find(|l| l.text == "94.0")
            .expect("value");
        assert_eq!(big.size, Theme::dark().big_font_size);
        assert_eq!(big.color, Theme::dark().text_dim);
    }

    #[test]
    fn banners_push_the_readout_down() {
        let r = spl_readout(&frame(LevelScale::DbSpl), "cal 3 h ago".into(), None);
        let size = Viewport {
            width: 400.0,
            height: 300.0,
        };
        let s = spl_scene(
            &r,
            &crate::banner::tests::everything(),
            &Theme::dark(),
            size,
        );
        assert_eq!(s.banners.len(), crate::banner::MAX_BANNERS);
        assert!(s.strip.h > 0.0);
        assert_eq!(s.area.y, s.strip.bottom() + 12.0);
        assert_eq!(s.area.bottom(), 288.0);
        crate::canvas::tests::assert_banners_clear(&s.scene, &s.banners, &[s.area]);
    }

    #[test]
    fn narrow_meter_wraps_the_statistics() {
        use crate::canvas::tests::{intersects, label_box};
        let r = spl_readout(&frame(LevelScale::DbSpl), "cal 3 h ago".into(), None);
        let stats = |w: f32| {
            let s = spl_scene(
                &r,
                &Status::default(),
                &Theme::dark(),
                Viewport {
                    width: w,
                    height: 260.0,
                },
            );
            s.scene.layers[2]
                .labels
                .iter()
                .filter(|l| r.stats.iter().any(|st| l.text.starts_with(&st.label)))
                .cloned()
                .collect::<Vec<_>>()
        };
        let wide = stats(900.0);
        assert_eq!(wide.len(), 4);
        assert!(wide.iter().all(|l| l.pos[1] == wide[0].pos[1]));
        let narrow = stats(330.0);
        assert_eq!(narrow.len(), 4);
        assert!(narrow[2].pos[1] > narrow[0].pos[1]);
        for (i, a) in narrow.iter().enumerate() {
            for b in &narrow[i + 1..] {
                assert!(
                    !intersects(label_box(a), label_box(b)),
                    "{:?} overlaps {:?}",
                    a.text,
                    b.text
                );
            }
        }
    }
}
