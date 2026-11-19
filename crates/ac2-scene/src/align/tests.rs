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

/// Below the readout's resolution neither system is named: the side is the sign of noise.
#[test]
fn a_delay_reading_zero_names_neither() {
    let c = cand(0.000_4, AlignSide::B, false, 3.0);
    assert_eq!(
        suggestion(&c, "Sub", "Main", 20.0),
        "no delay · polarity normal"
    );
    let c = cand(-0.000_4, AlignSide::A, true, 41.0);
    assert_eq!(short(&c, "Sub", "Main"), "no delay inverted, ±41°");
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
    Viewport::new(1280.0, 720.0)
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

/// The shared frequency cursor in the alignment view: a line in both plots at the column
/// it snapped to and, on the cursor values' plate, the frequency and what each pair of
/// curves reads there, in the units the magnitude view's readout gives them in.
#[test]
fn the_cursor_reads_both_plots_on_the_readout_plate() {
    use crate::plate::PlateKind;
    use crate::view::PlotChrome;
    let freqs: Vec<f64> = (0..100).map(|i| 20.0 * 1.06f64.powi(i)).collect();
    let state = fit_state();
    let mut cv = curves(freqs.len());
    // Column 40: 20 · 1.06⁴⁰ = 205.7 Hz.
    cv.phase_before[40] = 120.04;
    cv.sum_before[40] = -9.46;
    cv.sum_after[40] = f32::NAN;
    let color = Color::rgb(0.2, 0.6, 1.0);
    let shown = Shown {
        a: "Sub",
        b: "Main",
        band: band(60.0, 120.0),
        state: &state,
        curves: &cv,
        freqs: &freqs,
        color,
    };
    let theme = Theme::dark();
    let draw = |v: &ViewState| {
        alignment_scene(&Content::Shown(shown), &Status::default(), v, &theme, vp())
    };
    let off = draw(&ViewState::default());
    assert_eq!((off.cursor.clone(), off.readout_box), (None, None));
    let lines_off = off.scene.layers[1].polylines.len();

    let mut v = ViewState {
        cursor_hz: Some(200.0),
        ..ViewState::default()
    };
    let s = draw(&v);
    let cur = s.cursor.clone().expect("cursor");
    assert!((cur.freq_hz - freqs[40]).abs() < 1e-9, "{cur:?}");
    assert_eq!(cur.freq, "206 Hz");
    assert_eq!(cur.measured, "as measured  +120°  −9.5 dB");
    assert_eq!(cur.aligned, "aligned  +3°  —");
    // A line in each plot, under the plate.
    let x = s.x_axis.mapping.to_px(freqs[40]);
    let cursor_lines: Vec<_> = s.scene.layers[1].polylines[lines_off..].iter().collect();
    assert_eq!(cursor_lines.len(), 2);
    for (l, (_, plot)) in cursor_lines.iter().zip(&s.plots) {
        assert!((l.points[0][0] - x).abs() < 1e-3 && l.stroke == theme.cursor);
        assert_eq!(l.clip, Some(*plot));
    }
    // The plate: three rows, the curves' rows in their colours, over both plots.
    let b = s.readout_box.expect("plate");
    assert_eq!(s.plates(), vec![b]);
    assert_eq!(b.kind, PlateKind::Readout);
    assert_eq!(b.area.y, s.plots[0].1.y);
    assert_eq!(b.area.bottom(), s.plots[1].1.bottom());
    let rows: Vec<(&str, Color)> = s.scene.layers[2]
        .labels
        .iter()
        .filter(|l| l.pos[1] > b.rect.y && l.pos[1] < b.rect.bottom() && l.pos[0] > b.rect.x)
        .map(|l| (l.text.as_str(), l.color))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("206 Hz", theme.text),
            ("as measured  +120°  −9.5 dB", theme.text_dim),
            ("aligned  +3°  —", color),
        ]
    );
    // Dragged: where it was dropped, at the size set.
    v.plates.readout.move_to(0.0, 1.0);
    v.plates.readout.set_scale(1.5);
    let m = draw(&v).readout_box.expect("plate");
    assert_eq!(m.scale, 1.5);
    assert_eq!(m.rect.x, b.area.x + crate::legend::INSET);
    assert_eq!(m.rect.bottom(), b.area.bottom() - crate::legend::INSET);
    // T's steps hide the cursor and its plate as in the other views.
    for chrome in [PlotChrome::Full, PlotChrome::NoGrid, PlotChrome::Bare] {
        v.chrome = chrome;
        assert_eq!(
            draw(&v).readout_box.is_some(),
            chrome.cursor(),
            "{chrome:?}"
        );
    }
    // Nothing to read without curves.
    let empty = alignment_scene(
        &Content::Empty("none"),
        &Status::default(),
        &ViewState {
            cursor_hz: Some(200.0),
            ..ViewState::default()
        },
        &theme,
        vp(),
    );
    assert_eq!((empty.cursor, empty.readout_box), (None, None));
}
