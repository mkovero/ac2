use ac2_proto::frame::{BandLeqMeta, BandWindowState, LeqFlags};
use ac2_proto::model::{
    BAND_COUNT, BAND_NOMINAL_HZ, BandLeqConfig, BandLeqPreset, BandLimitPlace, BandPeriod,
    BandTransferBand, BandTransferSet, BandWindow, CalStatus, LF_BAND_COUNT, LeqJudgement,
    LevelScale, PredictedLeq, TransferOrigin, Weighting,
};
use ac2_proto::units::{Db, DbSpl, Hz, MeasId, Seconds, WallNs};

use super::*;
use crate::banner::Status;
use crate::theme::Theme;

fn flags(j: LeqJudgement, on_course: bool) -> LeqFlags {
    let f = match j {
        LeqJudgement::NoLimit => LeqFlags::NONE,
        LeqJudgement::NotCalibrated => LeqFlags::LIMIT,
        LeqJudgement::Ok => LeqFlags::LIMIT.with(LeqFlags::JUDGED),
        LeqJudgement::Near => LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(LeqFlags::NEAR),
        LeqJudgement::Over => LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(LeqFlags::OVER),
    };
    if on_course {
        f.with(LeqFlags::ON_COURSE)
    } else {
        f
    }
}

fn transfer(place: &str) -> BandTransferSet {
    let mut bands = [BandTransferBand::Clean {
        attenuation: Db(20.0),
    }; BAND_COUNT];
    bands[2] = BandTransferBand::Unusable { at_least: Db(35.0) };
    bands[3] = BandTransferBand::Corrected {
        attenuation: Db(18.3),
        margin: Db(5.0),
    };
    bands[10] = BandTransferBand::Missing;
    BandTransferSet {
        measured_at: WallNs(0),
        origin: TransferOrigin::Measured,
        bands,
        place: place.into(),
    }
}

/// The 545 low-frequency preset (an LZeq 60 min window per band 20 … 200 Hz, night and
/// day limits) plus an LAeq 15 min window per band, added high to low, only 63 Hz's with
/// a limit (one day and night), the limits moved from `flat 4` through a measured transfer.
fn cfg() -> BandLeqConfig {
    let mut c = BandLeqPreset::Finland545Lf.apply(None);
    for i in (0..LF_BAND_COUNT).rev() {
        let mut w = BandWindow::minutes(Hz(BAND_NOMINAL_HZ[i]), 15, Weighting::A);
        if i == 5 {
            w.limit = Some(DbSpl(70.0));
        }
        c.windows.push(w);
    }
    c.transfer = Some(transfer("flat 4"));
    c
}

/// The frame's column of band `i` (into the 11) of row `row` of [`cfg`]: the quarter's
/// windows were added high to low.
fn col(row: usize, i: usize) -> usize {
    if row == 0 {
        i
    } else {
        LF_BAND_COUNT + (LF_BAND_COUNT - 1 - i)
    }
}

/// The LZeq hour full at night: 63 Hz 3.2 dB over and cooling down for 412 s, 50 Hz near
/// and on course, the rest well under, 200 Hz without a limit. The LAeq quarter half full:
/// 63 Hz under its limit, the rest without one.
fn frame() -> BandLeqFrame {
    let n = LF_BAND_COUNT;
    let state = |i: usize, duration: f64, weighting, elapsed: f64| BandWindowState {
        band: Hz(BAND_NOMINAL_HZ[i]),
        duration: Seconds(duration),
        weighting,
        elapsed: Seconds(elapsed),
        measured: Seconds(elapsed),
        period: BandPeriod::Night,
        period_after_horizon: BandPeriod::Night,
    };
    let mut windows: Vec<BandWindowState> = (0..n)
        .map(|i| state(i, 3600.0, Weighting::Z, 3600.0))
        .collect();
    windows.extend((0..n).rev().map(|i| state(i, 900.0, Weighting::A, 450.0)));
    let mut f = BandLeqFrame {
        meas: MeasId(1),
        meta: BandLeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            horizon: Seconds(60.0),
            correction: Db(0.0),
            limits_from: BandLimitPlace::Transferred,
            windows,
            predicted: Some(PredictedLeq {
                duration: Seconds(3600.0),
                estimate: 23.5,
                at_most: 27.3,
                limit: Some(DbSpl(25.0)),
                judgement: LeqJudgement::Near,
            }),
        },
        leq: vec![f32::NAN; 2 * n],
        limit: vec![f32::NAN; 2 * n],
        allowed: vec![f32::NAN; 2 * n],
        recover: vec![f32::NAN; 2 * n],
        flags: vec![LeqFlags::NONE; 2 * n],
    };
    for i in 0..n {
        let k = col(0, i);
        let limit = (i < 10).then_some(90.0 - i as f32 * 2.0);
        let (leq, j) = match i {
            10 => (55.0, LeqJudgement::NoLimit),
            5 => (80.0 + 3.2, LeqJudgement::Over),
            4 => (82.0 - 1.5, LeqJudgement::Near),
            _ => (60.0, LeqJudgement::Ok),
        };
        f.leq[k] = leq;
        f.limit[k] = limit.unwrap_or(f32::NAN);
        if i < 10 && i != 5 {
            f.allowed[k] = 85.27 - i as f32;
        }
        if i == 5 {
            f.recover[k] = 412.0;
        }
        f.flags[k] = flags(j, i == 4);
    }
    for i in 0..n {
        let k = col(1, i);
        let judged = i == 5;
        f.leq[k] = if judged { 62.0 } else { 40.0 };
        if judged {
            f.limit[k] = 90.0;
            f.allowed[k] = 95.0;
        }
        f.flags[k] = flags(
            if judged {
                LeqJudgement::Ok
            } else {
                LeqJudgement::NoLimit
            },
            false,
        );
    }
    f
}

#[test]
fn a_band_over_is_named_with_its_window_and_how_long_to_back_off() {
    let t = band_leq_text(&cfg(), &frame());
    assert_eq!(t.rows.len(), 2);
    let w = &t.rows[0];
    assert_eq!(w.bars.len(), 11);
    let labels: Vec<&str> = w.bars.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "20", "25", "31.5", "40", "50", "63", "80", "100", "125", "160", "200"
        ]
    );
    assert_eq!(t.worst, Some((0, 5)));
    assert_eq!(
        t.headline,
        "63 Hz band LZeq 60 min 3.2 dB over its limit · cooling down in 6 min 52 s"
    );
    assert_eq!(t.headline_state, TileState::Over);
    let b = &w.bars[5];
    assert_eq!(b.name, "63 Hz band LZeq 60 min");
    assert_eq!(b.value, "83.2");
    assert_eq!(b.limit.as_deref(), Some("limit 80.0 dB"));
    assert_eq!(b.state_text.as_deref(), Some("OVER"));
    assert_eq!(b.headroom, None);
    assert_eq!(b.recover.as_deref(), Some("cooling down in 6 min 52 s"));
    assert_eq!(w.name, "LZeq 60 min");
    assert_eq!(w.caption(), "LZeq 60 min · night limits (22–07)");
    assert_eq!(t.unit, "dB SPL");
    assert_eq!(t.bands, "20–200 Hz");
    assert_eq!(
        t.limits_from.as_deref(),
        Some("limits moved from flat 4 through the band transfer")
    );
    assert_eq!(t.correction, None);
    assert_eq!(
        t.predicted.as_ref().map(|p| p.line.as_str()),
        Some("predicted LAeq 60 min in flat 4 23.5 dB · at most 27.3 dB · limit 25.0 dB · NEAR")
    );
}

#[test]
fn a_second_window_has_its_own_weighting_caption_and_judgement() {
    let t = band_leq_text(&cfg(), &frame());
    let w = &t.rows[1];
    assert_eq!(w.name, "LAeq 15 min");
    // One limit day and night: no period; filling half way.
    assert_eq!(w.caption(), "LAeq 15 min · so far · 7:30 / 15:00");
    assert_eq!(w.bars[5].name, "63 Hz band LAeq 15 min");
    assert_eq!(w.bars[5].state_text.as_deref(), Some("OK"));
    assert_eq!(w.bars[0].state_text, None);
    assert_eq!(w.bars[0].limit, None);
    assert_eq!(w.worst, Some(5));
    assert_eq!(w.allowed_key.as_deref(), Some("until full: stay ≤"));
}

#[test]
fn the_headline_follows_the_worst_window() {
    let mut f = frame();
    // The hour's 63 Hz back under, the quarter's over: the quarter heads.
    let k = col(0, 5);
    f.flags[k] = flags(LeqJudgement::Ok, false);
    f.leq[k] = 70.0;
    f.recover[k] = f32::NAN;
    let k = col(1, 5);
    f.flags[k] = flags(LeqJudgement::Over, false);
    f.leq[k] = 91.0;
    f.recover[k] = 90.0;
    let t = band_leq_text(&cfg(), &f);
    assert_eq!(t.worst, Some((1, 5)));
    assert_eq!(
        t.headline,
        "63 Hz band LAeq 15 min 1.0 dB over its limit · cooling down in 1 min 30 s"
    );
    // Both under: the band nearest its limit, here the hour's 50 Hz near and on course.
    f.flags[k] = flags(LeqJudgement::Ok, false);
    f.leq[k] = 62.0;
    f.recover[k] = f32::NAN;
    let t = band_leq_text(&cfg(), &f);
    assert_eq!(t.worst, Some((0, 4)));
    assert_eq!(
        t.headline,
        "50 Hz band LZeq 60 min 1.5 dB under its limit · on course to go over · next 1 min: \
         stay ≤ 81.2 dB"
    );
    assert_eq!(t.headline_state, TileState::Near);
}

#[test]
fn near_on_course_ok_and_unlimited_bands() {
    let t = band_leq_text(&cfg(), &frame());
    let w = &t.rows[0];
    let near = &w.bars[4];
    assert_eq!(near.state_text.as_deref(), Some("ON COURSE"));
    // 85.27 − 4 floored to 0.1 dB: a ceiling to stay under.
    assert_eq!(near.headroom.as_deref(), Some("next 1 min: stay ≤ 81.2 dB"));
    assert_eq!(near.over_db, Some(-1.5));
    let ok = &w.bars[0];
    assert_eq!(ok.state_text.as_deref(), Some("OK"));
    assert_eq!(ok.recover, None);
    let free = &w.bars[10];
    assert_eq!(free.state_text, None);
    assert_eq!(free.limit, None);
    assert_eq!(free.limit_db, None);
    assert_eq!(w.limit_key.as_deref(), Some("limit"));
    assert_eq!(w.allowed_key.as_deref(), Some("next 1 min: stay ≤"));
}

#[test]
fn without_a_transfer_the_limits_are_judged_at_the_mic_and_nothing_asks_for_one() {
    let mut c = cfg();
    c.transfer = None;
    let mut f = frame();
    f.meta.limits_from = BandLimitPlace::AtMic;
    f.meta.predicted = None;
    let t = band_leq_text(&c, &f);
    assert_eq!(t.limits_from, None);
    assert_eq!(t.predicted, None);
    assert!(t.headline.starts_with("63 Hz band LZeq 60 min 3.2 dB over"));
    assert!(t.rows[0].bars.iter().any(|b| b.state == TileState::Over));
    f.meta.limits_from = BandLimitPlace::Estimated;
    c.transfer = Some(BandTransferSet {
        origin: TransferOrigin::Estimated,
        ..transfer("receiving room")
    });
    assert_eq!(
        band_leq_text(&c, &f).limits_from.as_deref(),
        Some(
            "limits moved from receiving room through an estimated band transfer (typed, not \
             measured)"
        )
    );
}

#[test]
fn filling_period_change_correction_and_offline() {
    let mut f = frame();
    for w in &mut f.meta.windows[..LF_BAND_COUNT] {
        w.elapsed = Seconds(1800.0);
        w.measured = Seconds(1790.0);
        w.period = BandPeriod::Day;
        w.period_after_horizon = BandPeriod::Night;
    }
    f.meta.correction = Db(8.0);
    let t = band_leq_text(&cfg(), &f);
    let w = &t.rows[0];
    assert_eq!(
        w.caption(),
        "LZeq 60 min · day limits (07–22) · headroom for the night limits from 22:00 · so far · \
         30:00 / 1:00:00 · offline for 10 s"
    );
    assert_eq!(t.correction.as_deref(), Some("§13 correction +8 dB"));
    assert_eq!(
        w.bars[4].headroom.as_deref(),
        Some("until full: stay ≤ 81.2 dB")
    );
    for w in &mut f.meta.windows[..LF_BAND_COUNT] {
        w.period = BandPeriod::Night;
        w.period_after_horizon = BandPeriod::Day;
    }
    assert_eq!(
        band_leq_text(&cfg(), &f).rows[0].period.as_deref(),
        Some("night limits (22–07) · headroom for the day limits")
    );
}

#[test]
fn uncalibrated_bands_are_dbfs_and_not_judged() {
    let mut f = frame();
    f.meta.scale = LevelScale::Dbfs;
    for k in 0..f.leq.len() {
        f.leq[k] -= 120.0;
        let lim = f.flags[k].contains(LeqFlags::LIMIT);
        f.flags[k] = if lim { LeqFlags::LIMIT } else { LeqFlags::NONE };
        f.allowed[k] = f32::NAN;
        f.recover[k] = f32::NAN;
    }
    f.meta.predicted = Some(PredictedLeq {
        duration: Seconds(3600.0),
        estimate: f64::NAN,
        at_most: f64::NAN,
        limit: Some(DbSpl(25.0)),
        judgement: LeqJudgement::NotCalibrated,
    });
    let t = band_leq_text(&cfg(), &f);
    assert_eq!(
        t.headline,
        "not calibrated: band levels in dBFS, limits not judged"
    );
    assert_eq!(t.unit, "dBFS");
    assert_eq!(
        t.rows[0].bars[5].state_text.as_deref(),
        Some("not calibrated")
    );
    assert_eq!(t.rows[0].bars[5].limit_db, None);
    assert_eq!(
        t.predicted.expect("predicted").line,
        "predicted LAeq 60 min in flat 4 — · limit 25.0 dB · not calibrated"
    );
}

#[test]
fn nothing_measured_no_band_limits_and_no_windows() {
    let mut f = frame();
    f.leq.iter_mut().for_each(|l| *l = f32::NAN);
    let t = band_leq_text(&cfg(), &f);
    assert_eq!(t.headline, "waiting for the first second");
    assert_eq!(t.rows[0].bars[0].value, "—");
    let mut f = frame();
    f.limit.iter_mut().for_each(|l| *l = f32::NAN);
    f.flags.iter_mut().for_each(|l| *l = LeqFlags::NONE);
    f.allowed.iter_mut().for_each(|l| *l = f32::NAN);
    f.recover.iter_mut().for_each(|l| *l = f32::NAN);
    f.meta.predicted = Some(PredictedLeq {
        duration: Seconds(3600.0),
        estimate: 24.0,
        at_most: 24.0,
        limit: None,
        judgement: LeqJudgement::NoLimit,
    });
    let t = band_leq_text(&cfg(), &f);
    assert_eq!(t.headline, "no band limits");
    assert_eq!(
        t.predicted.expect("predicted").line,
        "predicted LAeq 60 min in flat 4 24.0 dB · no limit now"
    );
    f.meta.windows.clear();
    f.leq.clear();
    let t = band_leq_text(&cfg(), &f);
    assert!(t.rows.is_empty());
    assert_eq!(t.bands, "no bands");
    assert_eq!(band_leq_text(&cfg(), &f).headline, "no band windows");
}

#[test]
fn a_selection_of_bands_is_named_by_its_ranges() {
    assert_eq!(
        bands_text(&(0..LF_BAND_COUNT).collect::<Vec<_>>()),
        "20–200 Hz"
    );
    assert_eq!(bands_text(&[3, 4, 5, 6, 17]), "40–80 Hz, 1000 Hz");
    assert_eq!(bands_text(&[5]), "63 Hz");
    assert_eq!(window_name(900.0, Weighting::C), "LCeq 15 min");
    assert_eq!(
        band_window_name(31.5, 14400.0, Weighting::A),
        "31.5 Hz band LAeq 4 h"
    );
}

#[test]
fn presets_and_transfer_read_in_words() {
    assert_eq!(
        preset_summary(BandLeqPreset::Finland545Lf),
        "Finland STM 545/2015, low frequencies: 20–200 Hz LZeq 60 min, night 74 … 32 dB, day \
         5 dB higher; predicted LAeq 60 min night ≤ 25 dB"
    );
    assert_eq!(
        preset_summary(BandLeqPreset::Finland545LivingRoom),
        "Finland STM 545/2015, living room: 20–200 Hz LZeq 60 min, no limits; predicted LAeq \
         60 min day ≤ 35 dB, night ≤ 30 dB"
    );
    for p in BandLeqPreset::ALL {
        assert!(preset_source(p).contains("not legal advice"), "{p:?}");
    }
    let lf: Vec<usize> = (0..LF_BAND_COUNT).collect();
    let set = transfer("flat 4");
    assert_eq!(
        transfer_summary(Some(&set), &lf),
        "transfer from flat 4, 20–200 Hz: 8 clean, 1 background subtracted, 1 bound, 1 not \
         measured"
    );
    assert_eq!(
        transfer_summary(Some(&set), &[5, 17]),
        "transfer from flat 4, 63 Hz, 1000 Hz: 2 clean"
    );
    assert_eq!(
        transfer_summary(None, &lf),
        "no band transfer: limits judged at the mic as typed"
    );
    let estimated = BandTransferSet {
        origin: TransferOrigin::Estimated,
        ..transfer("receiving room")
    };
    assert_eq!(
        transfer_summary(Some(&estimated), &lf),
        "estimated transfer from receiving room, 20–200 Hz: 10 bands typed, not measured \
         (measure it when you can reach receiving room)"
    );
    assert_eq!(
        transfer_band_text(31.5, &set.bands[2]),
        "31.5 Hz ≥ 35.0 dB (under the background: a bound)"
    );
    assert_eq!(
        transfer_band_text(40.0, &set.bands[3]),
        "40 Hz 18.3 dB (background subtracted, 5.0 dB over it)"
    );
}

fn view(c: &BandLeqConfig, f: &BandLeqFrame) -> BandLeqView {
    BandLeqView {
        meter: "FOH SPL".into(),
        cal: "M30 · calibrated".into(),
        text: band_leq_text(c, f),
        stale: None,
    }
}

const SIZE: Viewport = Viewport {
    width: 1200.0,
    height: 700.0,
};

fn texts(s: &BandLeqScene) -> Vec<String> {
    s.scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.clone()))
        .collect()
}

#[test]
fn the_scene_stacks_a_row_of_columns_per_window_under_the_headline() {
    let theme = Theme::default();
    let s = band_leq_scene(&view(&cfg(), &frame()), &Status::default(), &theme, SIZE);
    assert_eq!(s.columns.len(), 2);
    for row in &s.columns {
        assert_eq!(row.len(), 11);
        assert!(row.windows(2).all(|w| w[1].x > w[0].right()));
    }
    // The second window's row sits wholly under the first's.
    let bottom0 = s.columns[0].iter().map(|r| r.bottom()).fold(0.0, f32::max);
    assert!(s.columns[1].iter().all(|r| r.y > bottom0));
    // Limits 72 … 90 dB: the scale runs from 20 dB under the lowest to above the highest.
    assert_eq!(s.ranges[0], (50.0, 100.0));
    let texts = texts(&s);
    for want in [
        "63 Hz band LZeq 60 min 3.2 dB over its limit · cooling down in 6 min 52 s",
        "FOH SPL · band Leq 20–200 Hz, dB SPL",
        "predicted LAeq 60 min in flat 4 23.5 dB · at most 27.3 dB · limit 25.0 dB · NEAR",
        "LZeq 60 min · night limits (22–07)",
        "LAeq 15 min · so far · 7:30 / 15:00",
        "31.5",
        "83.2",
    ] {
        assert!(texts.iter().any(|t| t == want), "{want:?} not in {texts:?}");
    }
    assert!(
        texts.iter().any(|t| t.contains("limits moved from flat 4")),
        "{texts:?}"
    );
    for row in &s.columns {
        for c in row {
            assert!(c.y >= s.headline.bottom());
            assert!(c.bottom() <= SIZE.height && c.right() <= SIZE.width);
        }
    }
}

/// The view names no room unless the operator did: without a transfer nothing mentions
/// one, and no output of the band view, its presets or the transfer step says `bedroom`
/// except the 545 low-frequency rule's own source.
#[test]
fn no_place_is_named_that_the_operator_did_not_name() {
    use crate::band_transfer::{Phase, SpanRole, next_step};
    let theme = Theme::default();
    let mut c = cfg();
    c.transfer = None;
    let mut f = frame();
    f.meta.limits_from = BandLimitPlace::AtMic;
    f.meta.predicted = None;
    let s = band_leq_scene(&view(&c, &f), &Status::default(), &theme, SIZE);
    let mut all = texts(&s);
    assert!(
        !all.iter().any(|t| t.contains("transfer")),
        "a view without a transfer talks about one: {all:?}"
    );
    let full = band_leq_scene(&view(&cfg(), &frame()), &Status::default(), &theme, SIZE);
    all.extend(texts(&full));
    for p in BandLeqPreset::ALL {
        all.push(preset_summary(p));
        if p != BandLeqPreset::Finland545Lf {
            all.push(preset_source(p));
        }
    }
    let lf: Vec<usize> = (0..LF_BAND_COUNT).collect();
    all.push(transfer_summary(None, &lf));
    all.push(transfer_summary(Some(&transfer("receiving room")), &lf));
    for r in SpanRole::ALL {
        all.push(r.title("receiving room"));
        all.push(r.what("receiving room"));
    }
    for spans in [
        [Phase::Unmarked; 3],
        [Phase::Marked, Phase::Unmarked, Phase::Unmarked],
        [Phase::Marked, Phase::Marking, Phase::Unmarked],
        [Phase::Marked, Phase::Marked, Phase::Unmarked],
        [Phase::Marked, Phase::Marked, Phase::Marking],
        [Phase::Marked; 3],
    ] {
        all.push(next_step(spans, false, "FOH SPL", "receiving room"));
    }
    all.push(next_step(
        [Phase::Marked; 3],
        true,
        "FOH SPL",
        "receiving room",
    ));
    for t in &all {
        assert!(!t.to_lowercase().contains("bedroom"), "{t:?}");
        assert!(!t.contains("dwelling"), "{t:?}");
    }
}

/// The thin marks of a scene: two-point polylines in the allowed mark's stroke.
fn allowed_marks(s: &BandLeqScene, theme: &Theme) -> usize {
    s.scene
        .layers
        .iter()
        .flat_map(|l| l.polylines.iter())
        .filter(|p| p.stroke == Stroke::solid(theme.text_dim, 2.0))
        .count()
}

#[test]
fn an_allowed_level_off_the_scale_is_neither_marked_nor_keyed() {
    let theme = Theme::default();
    let mut f = frame();
    // One row only, to count its marks.
    let n = LF_BAND_COUNT;
    f.meta.windows.truncate(n);
    f.leq.truncate(n);
    f.limit.truncate(n);
    f.allowed.truncate(n);
    f.recover.truncate(n);
    f.flags.truncate(n);
    let scene =
        |f: &BandLeqFrame| band_leq_scene(&view(&cfg(), f), &Status::default(), &theme, SIZE);
    // Nine bands with an allowed level on the scale (50 … 100 dB): nine marks and a key
    // (one more polyline in the key).
    assert_eq!(allowed_marks(&scene(&f), &theme), 10);
    for (i, a) in f.allowed.iter_mut().enumerate() {
        if a.is_finite() {
            *a = if i % 2 == 0 { 127.8 } else { 31.0 };
        }
    }
    let s = scene(&f);
    assert_eq!(s.ranges[0], (50.0, 100.0));
    assert_eq!(allowed_marks(&s, &theme), 0);
    let t = texts(&s);
    assert!(!t.iter().any(|l| l.contains("stay ≤")), "{t:?}");
    assert!(t.iter().any(|l| l == "≤ 127.8"), "{t:?}");
    assert!(t.iter().any(|l| l == "limit"), "{t:?}");
}

/// A lone band window: a row of one bar, as wide as one of eleven, named by its band.
#[test]
fn a_single_band_window_is_one_bar_named_by_its_band() {
    let mut c = BandLeqConfig {
        windows: vec![BandWindow::minutes(Hz(20.0), 1, Weighting::Z)],
        predicted: None,
        correction: Default::default(),
        transfer: None,
    };
    c.windows[0].limit = Some(DbSpl(80.0));
    let mut f = frame();
    f.meta.limits_from = BandLimitPlace::AtMic;
    f.meta.predicted = None;
    f.meta.windows = vec![BandWindowState {
        band: Hz(20.0),
        duration: Seconds(60.0),
        weighting: Weighting::Z,
        elapsed: Seconds(60.0),
        measured: Seconds(60.0),
        period: BandPeriod::Day,
        period_after_horizon: BandPeriod::Day,
    }];
    f.leq = vec![72.0];
    f.limit = vec![80.0];
    f.allowed = vec![f32::NAN];
    f.recover = vec![f32::NAN];
    f.flags = vec![flags(LeqJudgement::Ok, false)];
    let t = band_leq_text(&c, &f);
    assert_eq!(t.bands, "20 Hz");
    assert_eq!(t.rows.len(), 1);
    let w = &t.rows[0];
    assert_eq!(w.caption(), "LZeq 1 min");
    assert_eq!(w.bars.len(), 1);
    assert_eq!(w.bars[0].label, "20");
    assert_eq!(w.bars[0].name, "20 Hz band LZeq 1 min");
    assert_eq!(w.bars[0].limit.as_deref(), Some("limit 80.0 dB"));
    assert_eq!(t.worst, Some((0, 0)));
    assert_eq!(t.headline, "20 Hz band LZeq 1 min 8.0 dB under its limit");
    let s = band_leq_scene(&view(&c, &f), &Status::default(), &Theme::default(), SIZE);
    let lines = texts(&s);
    assert!(lines.iter().any(|l| l == "LZeq 1 min"), "{lines:?}");
    assert!(
        lines[0].starts_with("FOH SPL · band Leq 20 Hz,"),
        "{lines:?}"
    );
    // A bar, not a slab: no wider than one of the 545 table's eleven.
    let full = band_leq_scene(
        &view(&cfg(), &frame()),
        &Status::default(),
        &Theme::default(),
        SIZE,
    );
    assert_eq!(s.columns[0].len(), 1);
    assert!((s.columns[0][0].w - full.columns[0][0].w).abs() < 0.5);
}

/// Windows of one length and weighting share a row whatever their order; another length,
/// another weighting or another way of judging day and night is a row of its own.
#[test]
fn windows_are_grouped_by_length_and_weighting_low_band_to_high() {
    let t = band_leq_text(&cfg(), &frame());
    assert_eq!(t.rows.len(), 2);
    for r in &t.rows {
        let hz: Vec<f64> = r.bars.iter().map(|b| b.nominal_hz).collect();
        assert_eq!(hz, BAND_NOMINAL_HZ[..LF_BAND_COUNT]);
    }
    // The quarter's bars point at their own columns, added high to low.
    assert_eq!(t.rows[1].bars[0].window, 2 * LF_BAND_COUNT - 1);
    assert_eq!(t.rows[1].bars[5].name, "63 Hz band LAeq 15 min");
    // 63 Hz LZeq 60 min without a day offset: one limit day and night, its own row.
    let mut c = cfg();
    c.windows[5].day_offset = None;
    let t = band_leq_text(&c, &frame());
    let names: Vec<String> = t.rows.iter().map(BandRowText::caption).collect();
    assert_eq!(
        names,
        [
            "LZeq 60 min · night limits (22–07)",
            "LZeq 60 min",
            "LAeq 15 min · so far · 7:30 / 15:00"
        ]
    );
    assert_eq!(t.rows[1].bars.len(), 1);
    assert_eq!(t.worst, Some((1, 0)), "the 63 Hz bar, over, in its own row");
    assert_eq!(
        windows_summary(&c.windows),
        [
            "20–50 Hz, 80–200 Hz LZeq 60 min, night 74 … 32 dB, day 5 dB higher",
            "63 Hz LZeq 60 min, 42 dB",
            "20–200 Hz LAeq 15 min, 70 dB"
        ]
    );
}
