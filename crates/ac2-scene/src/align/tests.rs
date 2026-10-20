use ac2_proto::model::{
    AlignBand, AlignCandidate, AlignCurves, AlignFit, AlignNoFit, AlignOutcome, AlignSide,
    AlignState, Summation,
};
use ac2_proto::units::{Hz, Seconds};

use super::view::{AlignPlot, Content, Shown, alignment_scene};
use super::*;
use crate::banner::Status;
use crate::primitives::{Color, Scene, Viewport};
use crate::theme::Theme;
use crate::view::ViewState;

fn band(lo: f64, hi: f64) -> AlignBand {
    AlignBand {
        lo: Hz(lo),
        hi: Hz(hi),
    }
}

fn cand(ms: f64, delayed: AlignSide, inverted: bool, res: f64) -> AlignCandidate {
    AlignCandidate {
        delay: Seconds(ms / 1000.0),
        delayed,
        inverted,
        residual_deg: res,
        score: 0.9,
    }
}

fn fit_state() -> AlignState {
    AlignState {
        outcome: AlignOutcome::Fit {
            fit: AlignFit {
                best: cand(4.21, AlignSide::B, false, 12.0),
                other_polarity: Some(cand(9.8, AlignSide::B, true, 41.0)),
                runner_up: None,
                before: Summation {
                    mean_db: -7.9,
                    worst_db: -21.0,
                },
                after: Summation {
                    mean_db: -0.3,
                    worst_db: -1.25,
                },
                columns: 48,
            },
        },
    }
}

fn texts(s: &Scene) -> Vec<&str> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect()
}

#[test]
fn band_reads_in_hertz_or_across_the_kilohertz() {
    assert_eq!(band_text(band(60.0, 120.0)), "60–120 Hz");
    assert_eq!(band_text(band(800.0, 1800.0)), "800 Hz–1.80 kHz");
}

#[test]
fn suggestion_names_who_to_delay_metres_and_polarity() {
    let c = cand(4.21, AlignSide::B, false, 12.0);
    assert_eq!(
        suggestion(&c, "Sub", "Main", 20.0),
        "Main: delay 4.21 ms · 1.44 m @ 20 °C · polarity normal"
    );
    let c = cand(1.2, AlignSide::A, true, 14.0);
    assert_eq!(
        suggestion(&c, "Sub", "Main", 20.0),
        "Sub: delay 1.20 ms · 0.41 m @ 20 °C · polarity inverted"
    );
    assert_eq!(short(&c, "Sub", "Main"), "Sub +1.20 ms inverted, ±14°");
}

#[test]
fn readout_of_a_fit() {
    let r = readout(&fit_state(), "Sub", "Main", band(60.0, 120.0), 20.0);
    assert!(r.fit);
    assert_eq!(
        r.headline,
        "Main: delay 4.21 ms · 1.44 m @ 20 °C · polarity normal"
    );
    assert_eq!(
        r.lines,
        vec![
            "residual ±12° over 60–120 Hz · summation −7.9 dB → −0.3 dB (worst −21.0 dB → −1.2 dB)"
                .to_string(),
            "other polarity: Main +9.80 ms inverted, ±41°".to_string(),
        ]
    );
    let AlignOutcome::Fit { fit } = fit_state().outcome else {
        unreachable!()
    };
    assert_eq!(legend_note(&fit, "Sub", "Main"), "aligned: Main +4.21 ms");
}

#[test]
fn readout_names_a_runner_up_when_the_band_cannot_tell() {
    let mut s = fit_state();
    let AlignOutcome::Fit { fit } = &mut s.outcome else {
        unreachable!()
    };
    fit.runner_up = Some(cand(15.3, AlignSide::B, false, 14.0));
    let r = readout(&s, "Sub", "Main", band(60.0, 120.0), 20.0);
    assert_eq!(
        r.lines.last().expect("a readout has lines"),
        "also fits: Main +15.30 ms, ±14° — the band is too narrow to tell: pick by distance"
    );
}

#[test]
fn readout_without_a_fit_says_why() {
    let s = |why| AlignState {
        outcome: AlignOutcome::NoFit { why },
    };
    let b = band(60.0, 120.0);
    let r = readout(
        &s(AlignNoFit::FewColumns { found: 1 }),
        "Sub",
        "Main",
        b,
        20.0,
    );
    assert!(!r.fit);
    assert_eq!(r.headline, "No fit");
    assert_eq!(
        r.lines,
        vec!["1 column with both curves in 60–120 Hz: widen the band, or check both reach it"]
    );
    let r = readout(&s(AlignNoFit::NoWeight), "Sub", "Main", b, 20.0);
    assert_eq!(
        r.lines,
        vec!["no coherent signal from both curves in 60–120 Hz"]
    );
    let r = readout(&s(AlignNoFit::FewOperands), "Sub", "Main", b, 20.0);
    assert_eq!(r.lines, vec!["waiting for both curves (see the banner)"]);
}

/// A sub falling and a main rising, crossing at 90 Hz: the prefill is where they are alike.
#[test]
fn overlap_is_where_both_play_alike() {
    let freqs: Vec<f64> = (0..200).map(|i| 20.0 * 1.03f64.powi(i)).collect();
    let sub: Vec<f32> = freqs
        .iter()
        .map(|f| (-40.0 * (f / 90.0).log10().max(0.0)) as f32)
        .collect();
    let main: Vec<f32> = freqs
        .iter()
        .map(|f| (40.0 * (f / 90.0).log10().min(0.0)) as f32)
        .collect();
    let (lo, hi) = overlap(&freqs, &sub, &main).expect("an overlap");
    // 6 dB apart at 90·10^(±6/40) = 64 / 127 Hz, on the grid's columns, two significant digits.
    assert_eq!((lo, hi), (65.0, 130.0));
    let none = vec![-60.0f32; freqs.len()];
    assert_eq!(overlap(&freqs, &sub, &none), None);
}

fn curves(n: usize) -> AlignCurves {
    AlignCurves {
        phase_before: (0..n).map(|i| (i as f32 * 7.0) % 360.0 - 180.0).collect(),
        phase_after: vec![3.0; n],
        sum_before: vec![-8.0; n],
        sum_after: vec![-0.5; n],
    }
}

fn vp() -> Viewport {
    Viewport {
        width: 1280.0,
        height: 720.0,
    }
}

#[test]
fn view_shows_the_readout_and_two_plots() {
    let freqs: Vec<f64> = (0..100).map(|i| 20.0 * 1.06f64.powi(i)).collect();
    let state = fit_state();
    let cv = curves(freqs.len());
    let shown = Shown {
        a: "Sub",
        b: "Main",
        band: band(60.0, 120.0),
        state: &state,
        curves: &cv,
        freqs: &freqs,
        color: Color::WHITE,
    };
    let s = alignment_scene(
        &Content::Shown(shown),
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        vp(),
    );
    assert_eq!(
        s.titles,
        vec![
            "phase difference Sub − Main (°)",
            "summation (dB, 0 = they add fully)"
        ]
    );
    let t = texts(&s.scene);
    assert!(
        t.contains(&"Main: delay 4.21 ms · 1.44 m @ 20 °C · polarity normal"),
        "{t:?}"
    );
    assert!(
        t.contains(&"other polarity: Main +9.80 ms inverted, ±41°"),
        "{t:?}"
    );
    assert_eq!(
        s.plots.iter().map(|p| p.0).collect::<Vec<_>>(),
        vec![AlignPlot::Phase, AlignPlot::Summation]
    );
    let (phase, sum) = (s.plots[0].1, s.plots[1].1);
    assert!(phase.bottom() < sum.y, "{phase:?} {sum:?}");
    assert_eq!(s.empty, None);
}

#[test]
fn view_without_an_alignment_says_how_to_make_one() {
    let why = "No alignment in Sub's group — Alt+M aligns two transfer curves";
    let s = alignment_scene(
        &Content::Empty(why),
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        vp(),
    );
    assert_eq!(s.readout, None);
    assert!(texts(&s.scene).contains(&why), "{:?}", texts(&s.scene));
}
