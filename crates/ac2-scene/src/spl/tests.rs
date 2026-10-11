use super::*;
use crate::canvas::tests::{intersects, label_box};
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
            position: None,
        },
    }
}

const SINCE: &str = "meter since 4:01 · R resets";

fn readout(f: &SplFrame, cal: &str, age: Option<f64>) -> SplReadout {
    spl_readout(
        f,
        f.meta.level,
        cal.into(),
        age.map(Freshness::from_age),
        SINCE.into(),
    )
}

fn vp(w: f32, h: f32) -> Viewport {
    Viewport {
        width: w,
        height: h,
    }
}

fn texts(s: &Scene) -> Vec<&str> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect()
}

fn number<'a>(s: &'a SplScene, r: &SplReadout) -> &'a crate::primitives::Label {
    s.scene.layers[2]
        .labels
        .iter()
        .find(|l| l.text == r.value)
        .expect("number")
}

#[test]
fn readout_strings() {
    let r = readout(&frame(LevelScale::DbSpl), "cal 94 dB · 3 h ago", Some(0.2));
    assert_eq!(r.metric, "LAF");
    assert_eq!(r.value, "94.0");
    assert_eq!(r.unit, "dB SPL");
    assert_eq!(r.caption, "LAF · dB SPL");
    assert_eq!(r.corrected, None);
    // A measuring-position correction is in every level: the caption says so.
    let mut fc = frame(LevelScale::DbSpl);
    fc.meta.position = Some(ac2_proto::model::PositionCorrection::both(3.0));
    let rc = readout(&fc, "cal 94 dB · 3 h ago", Some(0.2));
    assert_eq!(rc.caption, "LAF · dB SPL corrected +3.0 dB");
    assert_eq!(rc.corrected.as_deref(), Some("corrected +3.0 dB"));
    let stats: Vec<String> = r
        .stats
        .iter()
        .map(|s| format!("{} {}", s.label, s.value))
        .collect();
    assert_eq!(
        stats,
        ["LAeq 92.1", "LCpeak 110.3", "LAFmax 97.2", "LAFmin —"]
    );
    assert_eq!(r.since, SINCE);
    assert_eq!(r.stale, None);
    // 94 dB on the 30 … 130 dB SPL bar.
    assert!((r.bar.fill - 0.6404).abs() < 1e-4, "{}", r.bar.fill);
    assert_eq!(r.bar.ticks.len(), 9);
    let mut f = frame(LevelScale::Dbfs);
    f.meta.weighting = Weighting::C;
    f.meta.time_weighting = TimeWeighting::Slow;
    f.meta.peak_weighting = PeakWeighting::Z;
    let r = spl_readout(
        &f,
        -23.4,
        "uncalibrated".into(),
        Some(Freshness::from_age(3.24)),
        SINCE.into(),
    );
    assert_eq!(r.unit, "dBFS");
    assert_eq!(r.caption, "LCS · dBFS");
    assert_eq!(r.stats[1].label, "LZpeak");
    assert_eq!(r.stats[2].label, "LCSmax");
    assert_eq!(r.stale.as_deref(), Some("STALE 3.2 s"));
    // A stopped meter's number is no current level either, but nothing is late.
    let stopped = Some(Freshness::Stopped { age_s: 23.0 });
    let r2 = spl_readout(&f, -23.4, String::new(), stopped, SINCE.into());
    assert_eq!(r2.stale.as_deref(), Some("STOPPED"));
    // While the audio is stopped the banner says why; the number only says so, no age.
    let out = Some(Freshness::AudioStopped { age_s: 4.8 });
    let r3 = spl_readout(&f, -23.4, String::new(), out, SINCE.into());
    assert_eq!(r3.stale.as_deref(), Some("AUDIO STOPPED"));
    // The bar follows the live level, not the held one: −23.4 dBFS on −100 … 0.
    assert!((r.bar.fill - 0.766).abs() < 1e-4, "{}", r.bar.fill);
    let r = spl_readout(&f, f64::NEG_INFINITY, String::new(), None, SINCE.into());
    assert_eq!(r.bar.fill, 0.0);
    assert_eq!(metric_name(Weighting::Z, TimeWeighting::Impulse), "LZI");
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
    let r = readout(&frame(LevelScale::DbSpl), "cal 94 dB · 3 h ago", Some(5.0));
    let s = spl_scene(
        &r,
        false,
        &Status::default(),
        &Theme::dark(),
        vp(400.0, 200.0),
    );
    assert!(s.banners.is_empty());
    assert_eq!(s.strip.h, 0.0);
    assert_eq!(s.area, Rect::new(12.0, 12.0, 376.0, 176.0));
    let t = texts(&s.scene);
    for want in [
        "LAF · dB SPL",
        "94.0",
        "LAeq 92.1",
        "LCpeak 110.3",
        SINCE,
        "cal 94 dB · 3 h ago",
        "STALE 5.0 s",
    ] {
        assert!(t.contains(&want), "{want} in {t:?}");
    }
    let big = number(&s, &r);
    assert_eq!(big.color, Theme::dark().text_dim);
    assert!(big.size > 3.0 * Theme::dark().font_size, "{}", big.size);
}

/// The number is centred and grows with the pane, up to a stage-sized one; the
/// statistics grow too, and stay smaller than the number's caption never is.
#[test]
fn number_is_centred_and_grows_with_the_pane() {
    let r = readout(&frame(LevelScale::DbSpl), "cal 94 dB · 3 h ago", None);
    let mut last = 0.0;
    for (w, h) in [
        (320.0, 200.0),
        (640.0, 360.0),
        (1000.0, 560.0),
        (1280.0, 800.0),
        (1920.0, 1080.0),
    ] {
        let s = spl_scene(&r, false, &Status::default(), &Theme::dark(), vp(w, h));
        let n = number(&s, &r);
        let b = label_box(n);
        let cx = s.area.x + s.area.w / 2.0;
        assert!((b.x + b.w / 2.0 - cx).abs() < 0.01, "{w}×{h}");
        assert!(n.size > last, "{w}×{h}: {} after {last}", n.size);
        last = n.size;
        // The number dominates: it takes a large part of the pane's height.
        assert!(b.h > 0.3 * h, "{w}×{h}: {} of {h}", b.h);
        let bar = s.bar.expect("bar");
        assert!((bar.x + bar.w / 2.0 - cx).abs() < 0.01);
        assert!(bar.y > b.bottom());
    }
    // Full screen: read across a room.
    assert!(last > 300.0, "{last}");
    let s = spl_scene(
        &r,
        false,
        &Status::default(),
        &Theme::dark(),
        vp(1920.0, 1080.0),
    );
    let stat = s.scene.layers[2]
        .labels
        .iter()
        .find(|l| l.text.starts_with("LAeq"))
        .expect("stat");
    assert!(stat.size > 1.5 * Theme::dark().font_size, "{}", stat.size);
    // A reading of other width (100.0, 9.5) keeps the number's size.
    for v in [100.04, 9.5] {
        let mut f = frame(LevelScale::DbSpl);
        f.meta.level = v;
        let r2 = readout(&f, "cal 94 dB · 3 h ago", None);
        let s2 = spl_scene(
            &r2,
            false,
            &Status::default(),
            &Theme::dark(),
            vp(1280.0, 800.0),
        );
        let s1 = spl_scene(
            &r,
            false,
            &Status::default(),
            &Theme::dark(),
            vp(1280.0, 800.0),
        );
        assert_eq!(number(&s2, &r2).size, number(&s1, &r).size, "{v}");
    }
}

#[test]
fn banners_push_the_readout_down() {
    let r = readout(&frame(LevelScale::DbSpl), "cal 94 dB · 3 h ago", None);
    let s = spl_scene(
        &r,
        false,
        &crate::banner::tests::everything(),
        &Theme::dark(),
        vp(400.0, 300.0),
    );
    assert_eq!(s.banners.len(), crate::banner::MAX_BANNERS);
    assert!(s.strip.h > 0.0);
    assert_eq!(s.area.y, s.strip.bottom() + 12.0);
    assert_eq!(s.area.bottom(), 288.0);
    crate::canvas::tests::assert_banners_clear(&s.scene, &s.banners, &[s.area]);
}

/// From the small panes of a grid (the footer `MM1 34804 · uncalibrated · mic curve: …`
/// wraps or is cut, the statistics wrap) to a full-screen 1920×1080: no two labels
/// overlap, the bar overlaps none, and all stay inside the readout area; the statistics'
/// heading sits above them and under the number's block.
#[test]
fn meter_never_overlaps_at_any_size() {
    let mut f = frame(LevelScale::DbSpl);
    f.meta.weighting = Weighting::Z;
    f.meta.duration = Seconds(360.0);
    let cal = "MM1 34804 · uncalibrated · mic curve: MM1 34804 90°";
    for (stale, stage) in [
        (Some(9.0), false),
        (None, false),
        (Some(9.0), true),
        (None, true),
    ] {
        let r = readout(&f, cal, stale);
        let widths = (220..=900).step_by(20).chain((960..=1920).step_by(96));
        for w in widths {
            let heights = (150..=420).step_by(15).chain((480..=1080).step_by(60));
            for h in heights {
                let size = vp(w as f32, h as f32);
                let s = spl_scene(&r, stage, &Status::default(), &Theme::dark(), size);
                let labels = &s.scene.layers[2].labels;
                let boxes: Vec<(&str, Rect)> = labels
                    .iter()
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
                    if let Some(bar) = s.bar {
                        assert!(!intersects(*b, bar), "{w}×{h}: bar over {t:?}");
                    }
                }
                for (i, (a, ab)) in boxes.iter().enumerate() {
                    for (b, bb) in &boxes[i + 1..] {
                        assert!(!intersects(*ab, *bb), "{w}×{h}: {a:?} overlaps {b:?}");
                    }
                }
                let texts: Vec<&str> = boxes.iter().map(|(t, _)| *t).collect();
                assert!(texts.contains(&"LZeq 92.1"), "{w}×{h}: {texts:?}");
                assert!(texts.contains(&"LZF · dB SPL"), "{w}×{h}: {texts:?}");
                let heading = boxes
                    .iter()
                    .find(|(t, _)| t.starts_with("meter since"))
                    .unwrap_or_else(|| panic!("{w}×{h}: no heading in {texts:?}"));
                let stat = boxes
                    .iter()
                    .find(|(t, _)| t.starts_with("LZeq"))
                    .map(|(_, b)| *b)
                    .expect("stat");
                assert!(heading.1.bottom() <= stat.y + 0.5, "{w}×{h}: heading under");
                if let Some(bar) = s.bar {
                    assert!(
                        heading.1.y >= bar.bottom() - 0.5,
                        "{w}×{h}: heading over bar"
                    );
                }
                // Wide enough, the heading is whole and the footer one row (on the
                // stage only while stale).
                if w >= 600 {
                    assert!(texts.contains(&SINCE), "{w}×{h}: {texts:?}");
                    let footer = !stage || stale.is_some();
                    assert_eq!(texts.contains(&cal), footer, "{w}×{h}: {texts:?}");
                }
                assert!(
                    !texts.iter().any(|t| t.contains("since") && t != &heading.0),
                    "{w}×{h}: the interval is stated once: {texts:?}"
                );
                if h >= 240 {
                    assert!(s.bar.is_some(), "{w}×{h}: no bar");
                }
            }
        }
    }
}

#[test]
fn footer_rows_wrap_at_the_parts() {
    let cal = "MM1 34804 · uncalibrated · mic curve: MM1 34804 90°";
    let one = footer_rows(cal, 600.0, 10.0, 3);
    assert_eq!(one, [[(cal.to_owned(), HAlign::Right)]]);
    let rows = footer_rows(cal, 200.0, 10.0, 3);
    let text: Vec<&str> = rows.iter().map(|r| r[0].0.as_str()).collect();
    assert_eq!(
        text,
        ["MM1 34804 · uncalibrated", "mic curve: MM1 34804 90°"]
    );
    assert!(footer_rows("", 200.0, 10.0, 3).is_empty());
    // Two rows at most: the rest is cut.
    let rows = footer_rows(
        &format!("{cal} · cal from other mic / input"),
        160.0,
        10.0,
        2,
    );
    assert_eq!(rows.len(), 2);
    assert!(rows[1][0].0.ends_with('…'), "{rows:?}");
    assert!(canvas::text_width(&rows[1][0].0, 10.0) <= 160.0);
}

#[test]
fn narrow_meter_wraps_the_statistics() {
    let r = readout(&frame(LevelScale::DbSpl), "cal 94 dB · 3 h ago", None);
    let stats = |w: f32| {
        let s = spl_scene(&r, false, &Status::default(), &Theme::dark(), vp(w, 260.0));
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

/// Full screen: no calibration line unless the reading is stale or stopped; its room
/// goes to the number and the statistics move down. Outside full screen it stays.
#[test]
fn the_stage_has_no_calibration_line_unless_stale() {
    let cal = "MM1 34804 · cal 94 dB · 3 h ago";
    let th = Theme::dark();
    for (w, h) in [(640.0, 400.0), (1280.0, 720.0), (1920.0, 1080.0)] {
        let at = format!("{w}×{h}");
        let scene = |stage, stale: Option<f64>| {
            let r = readout(&frame(LevelScale::DbSpl), cal, stale);
            let s = spl_scene(&r, stage, &Status::default(), &th, vp(w, h));
            (r, s)
        };
        let since_y = |s: &SplScene| {
            s.scene.layers[2]
                .labels
                .iter()
                .find(|l| l.text == SINCE)
                .expect("since")
                .pos[1]
        };
        let (r, bare) = scene(true, None);
        assert!(!texts(&bare.scene).contains(&cal), "{at}");
        let (_, windowed) = scene(false, None);
        assert!(texts(&windowed.scene).contains(&cal), "{at}");
        assert!(
            since_y(&bare) > since_y(&windowed),
            "{at}: the room goes to the number"
        );
        assert!(
            number(&bare, &r).size >= number(&windowed, &r).size,
            "{at}: the number never shrinks for it"
        );
        // Stale or stopped: what the level rests on is back with the warning.
        let (_, stale) = scene(true, Some(5.0));
        let all = texts(&stale.scene);
        assert!(all.contains(&cal), "{at}: {all:?}");
        assert!(all.iter().any(|t| t.starts_with("STALE")), "{at}: {all:?}");
    }
}

/// The heading: since when (local time; the date when not today) and the reset key.
#[test]
fn meter_since_names_the_start_once() {
    const S: u64 = 1_000_000_000;
    // 2026-10-03 04:01:30 UTC, read at 06:00.
    let start = WallNs((20_729 * 86_400 + 4 * 3600 + 90) * S);
    let newest = WallNs(start.0 + 7110 * S);
    let utc = |_: WallNs| 0;
    assert_eq!(
        meter_since(start, newest, utc, Some("R")),
        "meter since 4:01 · R resets"
    );
    assert_eq!(meter_since(start, newest, utc, None), "meter since 4:01");
    let next_day = WallNs(start.0 + 86_400 * S);
    assert_eq!(
        meter_since(start, next_day, utc, Some("R")),
        "meter since 3 Oct 4:01 · R resets"
    );
    // Local time: UTC+2.
    assert_eq!(
        meter_since(start, newest, |_| 7200, Some("R")),
        "meter since 6:01 · R resets"
    );
}

#[test]
fn display_periods() {
    assert_eq!(display_period_s(TimeWeighting::Fast), 0.5);
    assert_eq!(display_period_s(TimeWeighting::Slow), 1.0);
    assert_eq!(display_period_s(TimeWeighting::Impulse), 0.5);
}

/// Frames every 100 ms: the held reading changes twice a second on a fixed grid, and is
/// the frame at the update, not an average; a new configuration or calibration shows at
/// once and starts a new grid.
#[test]
fn hold_updates_at_the_display_rate() {
    const MS: u64 = 1_000_000;
    let mut held = None;
    let mut f = frame(LevelScale::DbSpl);
    let rev = Rev(7);
    let mut updates = Vec::new();
    for k in 0..30u64 {
        f.meta.level = 90.0 + k as f64;
        let t = 1_000 * MS + k * 100 * MS;
        if SplHold::update(&mut held, &f, t, rev, 0.5) {
            updates.push(k);
        }
    }
    assert_eq!(updates, [0, 5, 10, 15, 20, 25]);
    let h = held.clone().expect("held");
    assert_eq!(h.frame.meta.level, 115.0);
    assert_eq!(h.at_ns, 3_500 * MS);
    // Frames arriving off the grid (every 130 ms) still update on it, at the first frame
    // past each half second.
    let mut held = None;
    let mut at = Vec::new();
    for k in 0..40u64 {
        let t = k * 130 * MS;
        if SplHold::update(&mut held, &f, t, rev, 0.5) {
            at.push(held.as_ref().expect("held").at_ns / MS);
        }
    }
    assert_eq!(&at[..5], [0, 500, 1000, 1500, 2000]);
    // Slow: once a second.
    let mut held = None;
    let n = (0..30u64)
        .filter(|k| SplHold::update(&mut held, &f, k * 100 * MS, rev, 1.0))
        .count();
    assert_eq!(n, 3);
    // Another weighting under a new rev shows at once.
    let mut held = None;
    assert!(SplHold::update(&mut held, &f, 0, rev, 0.5));
    assert!(!SplHold::update(&mut held, &f, 100 * MS, rev, 0.5));
    let mut g = f.clone();
    g.meta.time_weighting = TimeWeighting::Slow;
    assert!(SplHold::update(&mut held, &g, 200 * MS, Rev(8), 0.5));
    assert_eq!(held.as_ref().expect("held").at_ns, 200 * MS);
    assert!(!SplHold::update(&mut held, &g, 600 * MS, Rev(8), 1.0));
    // Calibrated meanwhile (same rev): at once too.
    let mut c = g.clone();
    c.meta.scale = LevelScale::Dbfs;
    assert!(SplHold::update(&mut held, &c, 650 * MS, Rev(8), 1.0));
    // A clock going back (another daemon) starts over.
    assert!(SplHold::update(&mut held, &c, 10 * MS, Rev(8), 1.0));
    // Frames that stopped for a while: the next one shows and starts a new grid.
    assert!(SplHold::update(&mut held, &c, 5_000 * MS, Rev(8), 1.0));
    assert_eq!(held.as_ref().expect("held").at_ns, 5_000 * MS);
}
