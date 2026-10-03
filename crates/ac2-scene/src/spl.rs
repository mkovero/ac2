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
/// what the calibration rests on and its age when it belongs to this device + input + mic
/// (`cal 94 dB · 3 h ago`, `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 3 h
/// ago`), a mismatch warning when it belongs to another mic or input, `uncalibrated`
/// otherwise; `· mic curve` when the
/// mic's correction curve is applied. The age is on the daemon clock (`offset`).
pub fn cal_text(
    cal: CalStatus,
    mic_curve: bool,
    client_now: WallNs,
    offset: ClockOffset,
) -> String {
    let base = match cal {
        CalStatus::Uncalibrated => None,
        CalStatus::Verified { calibrated_at, .. }
        | CalStatus::OtherMicOrInput { calibrated_at, .. } => {
            crate::cal::status_text(cal, time::age_s(calibrated_at, client_now, offset))
        }
    }
    .unwrap_or_else(|| "uncalibrated".to_string());
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

/// Between the parts of the footer when it wraps.
const FOOTER_SEP: &str = " · ";

/// `text` cut to `width` at `size` with a trailing `…` (whole characters).
fn cut(text: &str, width: f32, size: f32) -> String {
    if canvas::text_width(text, size) <= width {
        return text.to_owned();
    }
    let mut out: String = text.to_owned();
    while !out.is_empty() && canvas::text_width(&format!("{out}…"), size) > width {
        out.pop();
    }
    format!("{}…", out.trim_end())
}

/// The footer's rows from the top, each `(text, align)` pieces: the interval left and the
/// calibration right on one row when both fit, else every part (`over 6 min`, `MM1 34804`,
/// `uncalibrated`, `mic curve: …`) packed into rows left to right, a part too wide for a row
/// cut with `…`. At most `max_rows`; what does not fit goes into the last row, cut.
fn footer_rows(
    interval: &str,
    cal: &str,
    width: f32,
    size: f32,
    max_rows: usize,
) -> Vec<Vec<(String, HAlign)>> {
    let gap = 2.0 * size;
    if canvas::text_width(interval, size) + gap + canvas::text_width(cal, size) <= width {
        return vec![vec![
            (interval.to_owned(), HAlign::Left),
            (cal.to_owned(), HAlign::Right),
        ]];
    }
    let parts = std::iter::once(interval).chain(cal.split(FOOTER_SEP));
    let mut rows: Vec<String> = Vec::new();
    for p in parts.filter(|p| !p.is_empty()) {
        let full = rows.len() >= max_rows.max(1);
        match rows.last_mut() {
            Some(row)
                if canvas::text_width(&format!("{row}{FOOTER_SEP}{p}"), size) <= width || full =>
            {
                row.push_str(FOOTER_SEP);
                row.push_str(p);
            }
            _ => rows.push(p.to_owned()),
        }
    }
    rows.into_iter()
        .map(|r| vec![(cut(&r, width, size), HAlign::Left)])
        .collect()
}

/// Lays the readout out in `size` below the banner strip: metric top-left, big number with
/// its unit, a row of statistics, interval and calibration at the bottom. Laid out from the
/// bottom up, so on a small pane the footer wraps and the statistics and the number move up
/// (the number shrinks if it must) rather than run into each other.
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
    // Bottom up: the footer, the statistics above it, the number above them.
    let small = theme.small_font_size;
    let fs = theme.font_size;
    // A short pane spends fewer rows on the footer (cut instead) to keep the number large.
    let max_rows = match area.h {
        h if h >= 200.0 => 3,
        h if h >= 160.0 => 2,
        _ => 1,
    };
    let footer = footer_rows(&r.interval, &r.cal, area.w, small, max_rows);
    let line_h = 1.25 * small;
    let footer_top = area.bottom() - footer.len() as f32 * line_h;
    let gap = 6.0;
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
    let stat_rows = texts.len().div_ceil(per_row);
    // Row centres 1.4 em apart; a row's text is 1.25 em tall.
    let stats_span = (stat_rows.saturating_sub(1)) as f32 * 1.4 * fs;
    let row = (area.y + area.h * 0.78).min(footer_top - gap - 0.625 * fs - stats_span);
    let stats_top = row - 0.625 * fs;
    // The number sits on its baseline with 0.3 em below it and 0.95 em above; it shrinks
    // when the room between the metric name and the statistics is less than its size.
    let metric_bottom = area.y + 1.25 * 1.4 * fs;
    let room = stats_top - gap - (metric_bottom + gap);
    let split = area.x + area.w * 0.66;
    // Nor wider than the room left of the unit.
    let per_em = canvas::text_width(&r.value, 1.0).max(1.0);
    let big = (room / 1.25)
        .min((split - area.x) / per_em)
        .min(theme.big_font_size)
        .max(1.6 * fs);
    let base = (area.y + area.h * 0.62).min(stats_top - gap - 0.3 * big);
    c.overlay.labels.push(label(
        r.value.clone(),
        [split, base],
        anchor(HAlign::Right, VAlign::Baseline),
        big,
        main,
    ));
    c.overlay.labels.push(label(
        r.unit.clone(),
        [split + 8.0, base],
        anchor(HAlign::Left, VAlign::Baseline),
        fs * 1.6,
        main,
    ));
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
    for (i, pieces) in footer.into_iter().enumerate() {
        let y = footer_top + (i + 1) as f32 * line_h;
        for (text, h) in pieces {
            let x = match h {
                HAlign::Right => area.right(),
                _ => area.x,
            };
            c.overlay.labels.push(label(
                text,
                [x, y],
                anchor(h, VAlign::Bottom),
                small,
                theme.text_dim,
            ));
        }
    }
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
                    basis: ac2_proto::model::CalBasis::Acoustic {
                        calibrator_level: ac2_proto::units::DbSpl(94.0),
                    },
                },
                mic_curve: false,
            },
        }
    }

    #[test]
    fn readout_strings() {
        let r = spl_readout(
            &frame(LevelScale::DbSpl),
            "cal 94 dB · 3 h ago".into(),
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
            basis: ac2_proto::model::CalBasis::Acoustic {
                calibrator_level: ac2_proto::units::DbSpl(94.0),
            },
        };
        assert_eq!(cal_text(verified, false, now, off), "cal 94 dB · 3 h ago");
        assert_eq!(
            cal_text(verified, true, now, off),
            "cal 94 dB · 3 h ago · mic curve"
        );
        let other = CalStatus::OtherMicOrInput {
            calibrated_at: WallNs(99 * H),
            basis: ac2_proto::model::CalBasis::Acoustic {
                calibrator_level: ac2_proto::units::DbSpl(94.0),
            },
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
            "cal 94 dB · 3 h ago"
        );
        // Minutes and days.
        let v = |ago: u64| CalStatus::Verified {
            calibrated_at: WallNs(100 * H - ago),
            basis: ac2_proto::model::CalBasis::Acoustic {
                calibrator_level: ac2_proto::units::DbSpl(94.0),
            },
        };
        assert_eq!(
            cal_text(v(H / 6), false, now, ClockOffset(0)),
            "cal 94 dB · 10 min ago"
        );
        assert_eq!(
            cal_text(v(50 * H), false, now, ClockOffset(0)),
            "cal 94 dB · 2 d ago"
        );
    }

    #[test]
    fn scene_carries_the_strings() {
        let r = spl_readout(
            &frame(LevelScale::DbSpl),
            "cal 94 dB · 3 h ago".into(),
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
            "cal 94 dB · 3 h ago",
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
        let r = spl_readout(
            &frame(LevelScale::DbSpl),
            "cal 94 dB · 3 h ago".into(),
            None,
        );
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

    /// On the small panes of a grid the footer (`over 6 min · MM1 34804 · uncalibrated · mic
    /// curve: …`) wraps or is cut and everything above it moves up: no two labels overlap and
    /// all stay inside the readout area, at every size from a narrow bottom-row pane up.
    #[test]
    fn small_meter_never_overlaps_its_footer() {
        use crate::canvas::tests::{intersects, label_box};
        let mut f = frame(LevelScale::DbSpl);
        f.meta.weighting = Weighting::Z;
        f.meta.duration = Seconds(360.0);
        let cal = "MM1 34804 · uncalibrated · mic curve: MM1 34804 90°";
        let r = spl_readout(&f, cal.into(), Some(Freshness::from_age(9.0)));
        for w in (220..=900).step_by(20) {
            for h in (150..=420).step_by(15) {
                let size = Viewport {
                    width: w as f32,
                    height: h as f32,
                };
                let s = spl_scene(&r, &Status::default(), &Theme::dark(), size);
                let labels = &s.scene.layers[2].labels;
                // The unit beside the number may leave a very narrow pane; the rest may not.
                let boxes: Vec<(&str, Rect)> = labels
                    .iter()
                    .filter(|l| l.text != r.unit)
                    .map(|l| (l.text.as_str(), label_box(l)))
                    .collect();
                for (t, b) in &boxes {
                    assert!(
                        b.x >= s.area.x - 0.5
                            && b.right() <= s.area.right() + 0.5
                            && b.bottom() <= s.area.bottom() + 0.5
                            && b.y >= s.area.y - 0.5,
                        "{w}×{h}: {t:?} {b:?} outside {:?}",
                        s.area
                    );
                }
                for (i, (a, ab)) in boxes.iter().enumerate() {
                    for (b, bb) in &boxes[i + 1..] {
                        assert!(!intersects(*ab, *bb), "{w}×{h}: {a:?} overlaps {b:?}");
                    }
                }
                let texts: Vec<&str> = boxes.iter().map(|(t, _)| *t).collect();
                assert!(texts.contains(&"LZeq 92.1"), "{w}×{h}: {texts:?}");
                assert!(
                    texts.iter().any(|t| t.contains("over 6 min")),
                    "{w}×{h}: {texts:?}"
                );
                // Wide enough, the footer is one row: the interval left, the calibration right.
                if w >= 600 {
                    assert!(texts.contains(&cal), "{w}×{h}: {texts:?}");
                }
            }
        }
    }

    #[test]
    fn footer_rows_wrap_at_the_parts() {
        let cal = "MM1 34804 · uncalibrated · mic curve: MM1 34804 90°";
        let one = footer_rows("over 6 min", cal, 600.0, 10.0, 3);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].len(), 2);
        let rows = footer_rows("over 6 min", cal, 200.0, 10.0, 3);
        let text: Vec<&str> = rows.iter().map(|r| r[0].0.as_str()).collect();
        assert_eq!(
            text,
            [
                "over 6 min · MM1 34804",
                "uncalibrated",
                "mic curve: MM1 34804 90°"
            ]
        );
        // Two rows at most: the rest is cut.
        let rows = footer_rows("over 6 min", cal, 160.0, 10.0, 2);
        assert_eq!(rows.len(), 2);
        assert!(rows[1][0].0.ends_with('…'), "{rows:?}");
        assert!(canvas::text_width(&rows[1][0].0, 10.0) <= 160.0);
    }

    #[test]
    fn narrow_meter_wraps_the_statistics() {
        use crate::canvas::tests::{intersects, label_box};
        let r = spl_readout(
            &frame(LevelScale::DbSpl),
            "cal 94 dB · 3 h ago".into(),
            None,
        );
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
