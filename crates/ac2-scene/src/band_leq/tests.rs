use ac2_proto::frame::BandLeqMeta;
use ac2_proto::model::{
    BAND_NOMINAL_HZ, BandLeqBand, BandLeqPreset, BandLimitPlace, BandPeriod, BandTransferBand,
    BandTransferSet, CalStatus, LF_BAND_COUNT, LeqJudgement, LevelScale, PredictedLeq,
};
use ac2_proto::units::{Db, DbSpl, Hz, Seconds, WallNs};

use super::*;
use crate::banner::Status;
use crate::theme::Theme;

/// A night hour half full, limits transferred: 63 Hz 3.2 dB over and cooling down for
/// 412 s, 50 Hz near and on course, the rest well under, 200 Hz without a limit.
fn meta() -> BandLeqMeta {
    let bands = (0..LF_BAND_COUNT)
        .map(|i| {
            let limit = (i < 10).then_some(90.0 - i as f64 * 2.0);
            let (leq, judgement) = match i {
                10 => (55.0, LeqJudgement::NoLimit),
                5 => (80.0 + 3.2, LeqJudgement::Over),
                4 => (82.0 - 1.5, LeqJudgement::Near),
                _ => (60.0, LeqJudgement::Ok),
            };
            BandLeqBand {
                nominal: Hz(BAND_NOMINAL_HZ[i]),
                leq,
                limit,
                judgement,
                on_course: i == 4,
                allowed: (i < 10 && i != 5).then_some(85.27 - i as f64),
                recover: (i == 5).then_some(Seconds(412.0)),
            }
        })
        .collect();
    BandLeqMeta {
        scale: LevelScale::DbSpl,
        cal: CalStatus::Uncalibrated,
        mic_curve: false,
        duration: Seconds(3600.0),
        horizon: Seconds(60.0),
        elapsed: Seconds(3600.0),
        measured: Seconds(3600.0),
        period: BandPeriod::Night,
        period_after_horizon: BandPeriod::Night,
        correction: Db(0.0),
        limits_from: BandLimitPlace::Transferred,
        bands,
        worst: Some(5),
        predicted: Some(PredictedLeq {
            estimate: 23.5,
            at_most: 27.3,
            limit: Some(DbSpl(25.0)),
            judgement: LeqJudgement::Near,
        }),
    }
}

#[test]
fn a_band_over_is_named_with_how_long_to_back_off() {
    let t = band_leq_text(&meta());
    assert_eq!(t.bars.len(), 11);
    let labels: Vec<&str> = t.bars.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "20", "25", "31.5", "40", "50", "63", "80", "100", "125", "160", "200"
        ]
    );
    assert_eq!(t.worst, Some(5));
    assert_eq!(
        t.headline,
        "63 Hz band Leq 3.2 dB over its limit · cooling down in 6 min 52 s"
    );
    assert_eq!(t.headline_state, TileState::Over);
    let b = &t.bars[5];
    assert_eq!(b.name, "63 Hz band Leq");
    assert_eq!(b.value, "83.2");
    assert_eq!(b.limit.as_deref(), Some("limit 80.0 dB"));
    assert_eq!(b.state_text.as_deref(), Some("OVER"));
    assert_eq!(b.headroom, None);
    assert_eq!(b.recover.as_deref(), Some("cooling down in 6 min 52 s"));
    assert_eq!(t.name, "band Leq 60 min, unweighted");
    assert_eq!(t.unit, "dB SPL");
    assert_eq!(t.period, "night limits (22–07)");
    assert_eq!(t.limits_from, "limits transferred from the dwelling");
    assert_eq!(t.filling, None);
    assert_eq!(t.correction, None);
    assert_eq!(
        t.predicted.as_ref().map(|p| p.line.as_str()),
        Some("predicted dwelling LAeq 23.5 dB · at most 27.3 dB · limit 25.0 dB · NEAR")
    );
}

#[test]
fn near_on_course_ok_and_unlimited_bands() {
    let t = band_leq_text(&meta());
    let near = &t.bars[4];
    assert_eq!(near.state_text.as_deref(), Some("ON COURSE"));
    // 85.27 − 4 floored to 0.1 dB: a ceiling to stay under.
    assert_eq!(near.headroom.as_deref(), Some("next 1 min: stay ≤ 81.2 dB"));
    assert_eq!(near.over_db, Some(-1.5));
    let ok = &t.bars[0];
    assert_eq!(ok.state_text.as_deref(), Some("OK"));
    assert_eq!(ok.recover, None);
    let free = &t.bars[10];
    assert_eq!(free.state_text, None);
    assert_eq!(free.limit, None);
    assert_eq!(free.limit_db, None);
}

#[test]
fn the_worst_band_under_its_limit_says_how_loud_it_may_go() {
    let mut m = meta();
    m.worst = Some(4);
    m.bands[5].judgement = LeqJudgement::Ok;
    m.bands[5].leq = 70.0;
    let t = band_leq_text(&m);
    assert_eq!(
        t.headline,
        "50 Hz band Leq 1.5 dB under its limit · on course to go over · next 1 min: stay ≤ 81.2 dB"
    );
    assert_eq!(t.headline_state, TileState::Near);
}

#[test]
fn filling_period_change_correction_and_offline() {
    let mut m = meta();
    m.elapsed = Seconds(1800.0);
    m.measured = Seconds(1790.0);
    m.period = BandPeriod::Day;
    m.period_after_horizon = BandPeriod::Night;
    m.correction = Db(8.0);
    m.limits_from = BandLimitPlace::AtMic;
    m.predicted = None;
    let t = band_leq_text(&m);
    assert_eq!(t.filling.as_deref(), Some("so far · 30:00 / 1:00:00"));
    assert_eq!(t.incomplete.as_deref(), Some("offline for 10 s"));
    assert_eq!(
        t.period,
        "day limits (07–22) · headroom for the night limits from 22:00"
    );
    assert_eq!(t.correction.as_deref(), Some("§13 correction +8 dB"));
    assert_eq!(t.limits_from, "dwelling limits at the mic (no transfer)");
    assert_eq!(
        t.bars[4].headroom.as_deref(),
        Some("until full: stay ≤ 81.2 dB")
    );
    assert_eq!(t.predicted, None);
    m.period = BandPeriod::Night;
    m.period_after_horizon = BandPeriod::Day;
    assert_eq!(
        band_leq_text(&m).period,
        "night limits (22–07) · headroom for the day limits"
    );
}

#[test]
fn uncalibrated_bands_are_dbfs_and_not_judged() {
    let mut m = meta();
    m.scale = LevelScale::Dbfs;
    for b in &mut m.bands {
        b.leq -= 120.0;
        b.judgement = if b.limit.is_some() {
            LeqJudgement::NotCalibrated
        } else {
            LeqJudgement::NoLimit
        };
        b.allowed = None;
        b.recover = None;
    }
    m.worst = None;
    m.predicted = Some(PredictedLeq {
        estimate: f64::NAN,
        at_most: f64::NAN,
        limit: Some(DbSpl(25.0)),
        judgement: LeqJudgement::NotCalibrated,
    });
    let t = band_leq_text(&m);
    assert_eq!(
        t.headline,
        "not calibrated: band levels in dBFS, limits not judged"
    );
    assert_eq!(t.unit, "dBFS");
    assert_eq!(t.bars[5].state_text.as_deref(), Some("not calibrated"));
    assert_eq!(t.bars[5].limit_db, None);
    assert_eq!(
        t.predicted.expect("predicted").line,
        "predicted dwelling LAeq — · limit 25.0 dB · not calibrated"
    );
}

#[test]
fn nothing_measured_and_no_band_limits() {
    let mut m = meta();
    for b in &mut m.bands {
        b.leq = f64::NAN;
    }
    assert_eq!(band_leq_text(&m).headline, "waiting for the first second");
    assert_eq!(band_leq_text(&m).bars[0].value, "—");
    let mut m = meta();
    for b in &mut m.bands {
        b.limit = None;
        b.judgement = LeqJudgement::NoLimit;
        b.allowed = None;
        b.recover = None;
    }
    m.worst = None;
    m.predicted = Some(PredictedLeq {
        estimate: 24.0,
        at_most: 24.0,
        limit: None,
        judgement: LeqJudgement::NoLimit,
    });
    let t = band_leq_text(&m);
    assert_eq!(t.headline, "no band limits");
    assert_eq!(
        t.predicted.expect("predicted").line,
        "predicted dwelling LAeq 24.0 dB · no night limit"
    );
}

#[test]
fn presets_and_transfer_read_in_words() {
    assert_eq!(
        preset_summary(BandLeqPreset::Finland545Lf),
        "Finland STM 545/2015, low frequencies (bedroom): band Leq 60 min, unweighted 20 … 200 Hz, \
         night 74 … 32 dB, day 5 dB higher; predicted dwelling LAeq night ≤ 25 dB"
    );
    assert_eq!(
        preset_summary(BandLeqPreset::Finland545LivingRoom),
        "Finland STM 545/2015, living room: predicted dwelling LAeq day ≤ 35 dB, night ≤ 30 dB"
    );
    for p in BandLeqPreset::ALL {
        assert!(preset_source(p).contains("not legal advice"), "{p:?}");
    }
    let mut bands = [BandTransferBand::Clean {
        attenuation: Db(20.0),
    }; ac2_proto::model::BAND_COUNT];
    bands[2] = BandTransferBand::Unusable { at_least: Db(35.0) };
    bands[3] = BandTransferBand::Corrected {
        attenuation: Db(18.3),
        margin: Db(5.0),
    };
    bands[10] = BandTransferBand::Missing;
    let set = BandTransferSet {
        measured_at: WallNs(0),
        bands,
    };
    assert_eq!(
        transfer_summary(Some(&set)),
        "transfer 20–200 Hz: 8 clean, 1 background subtracted, 1 bound, 1 not measured"
    );
    assert_eq!(
        transfer_summary(None),
        "no transfer: dwelling limits judged at the mic"
    );
    assert_eq!(
        transfer_band_text(31.5, &bands[2]),
        "31.5 Hz ≥ 35.0 dB (under the background: a bound)"
    );
    assert_eq!(
        transfer_band_text(40.0, &bands[3]),
        "40 Hz 18.3 dB (background subtracted, 5.0 dB over it)"
    );
}

#[test]
fn the_scene_draws_eleven_columns_and_the_headline() {
    let theme = Theme::default();
    let size = Viewport {
        width: 1200.0,
        height: 700.0,
    };
    let v = BandLeqView {
        meter: "FOH SPL".into(),
        cal: "M30 · calibrated".into(),
        text: band_leq_text(&meta()),
        stale: None,
    };
    let s = band_leq_scene(&v, &Status::default(), &theme, size);
    assert_eq!(s.columns.len(), 11);
    assert!(s.columns.windows(2).all(|w| w[1].x > w[0].right()));
    // Limits 72 … 90 dB: the scale runs from 20 dB under the lowest to above the highest.
    assert_eq!(s.range, (50.0, 100.0));
    let texts: Vec<&str> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect();
    for want in [
        "63 Hz band Leq 3.2 dB over its limit · cooling down in 6 min 52 s",
        "FOH SPL · band Leq 60 min, unweighted, dB SPL",
        "predicted dwelling LAeq 23.5 dB · at most 27.3 dB · limit 25.0 dB · NEAR",
        "31.5",
        "83.2",
        "6 min",
        "≤ 81.2",
    ] {
        assert!(texts.contains(&want), "{want:?} not in {texts:?}");
    }
    // Every column sits under the headline and inside the pane.
    for c in &s.columns {
        assert!(c.y >= s.headline.bottom());
        assert!(c.bottom() <= size.height && c.right() <= size.width);
    }
}
