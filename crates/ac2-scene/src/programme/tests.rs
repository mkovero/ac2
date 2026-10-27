use super::*;

mod bars;
mod blocks;
mod dock;
mod plate;
mod rows;

/// A pane of peak bars, one per input, as the UI fills it with no loudness source.
pub(super) fn pane_of(
    inputs: Vec<ProgrammeInput>,
    pair: Option<ProgrammePair>,
    ballistic: Ballistic,
) -> ProgrammePane {
    ProgrammePane {
        bars: peak_bars(&inputs, ballistic),
        inputs,
        pair,
        ballistic,
        loudness_target: -23.0,
        loudness: None,
        plate: crate::plate::PlatePlace::default(),
        plate_hover: None,
        view: crate::view::ProgrammeMode::Meters,
    }
}

fn input(scale: Scale) -> ProgrammeInput {
    ProgrammeInput {
        label: "Main L".into(),
        scale,
        ppm: Some(-12.34),
        true_peak_dbtp: Some(-1.2),
        max: Some(-3.06),
        overs: 0,
        clipped: false,
        flying_peak: Some(-6.0),
        offers_vu: false,
    }
}

#[test]
fn levels_read_on_their_scale() {
    let i = input(Scale::Dbfs);
    assert_eq!(i.ppm_text(Ballistic::Type1), "−12.3 dBFS");
    assert_eq!(i.true_peak_text(), "−1.2 dBTP");
    assert_eq!(i.max_text(Ballistic::Type1), "max −3.1 dBFS");
    let line = input(Scale::Dbu { fs_dbu: 16.3 });
    assert_eq!(line.ppm_text(Ballistic::Type1), "+4.0 dBu");
    assert_eq!(line.true_peak_text(), "−1.2 dBTP", "dBTP on every scale");
    assert_eq!(line.max_text(Ballistic::Type1), "max +13.2 dBu");
    let mic = input(Scale::DbSpl { fs_db_spl: 120.5 });
    assert_eq!(mic.ppm_text(Ballistic::Type1), "108.2 dB SPL");
    let mut none = input(Scale::Dbfs);
    none.ppm = None;
    none.max = Some(f32::NEG_INFINITY);
    none.true_peak_dbtp = None;
    assert_eq!(none.ppm_text(Ballistic::Type1), "—");
    assert_eq!(none.max_text(Ballistic::TruePeak), "max —");
    assert_eq!(none.true_peak_text(), "—");
}

/// A true-peak reading and its max are the dBTP quantity, as the true peak reads elsewhere;
/// VU falling back to the true peak reads it too. A calibrated scale keeps its unit.
#[test]
fn a_true_peak_reading_is_in_dbtp() {
    let i = input(Scale::Dbfs);
    assert_eq!(i.ppm_text(Ballistic::TruePeak), "−12.3 dBTP");
    assert_eq!(i.max_text(Ballistic::TruePeak), "max −3.1 dBTP");
    assert_eq!(i.ppm_text(Ballistic::Vu), "−12.3 dBTP");
    assert_eq!(i.max_text(Ballistic::Vu), "max −3.1 dBTP");
    assert_eq!(i.unit(Ballistic::Type2), "dBFS");
    let line = input(Scale::Dbu { fs_dbu: 16.3 });
    assert_eq!(line.ppm_text(Ballistic::TruePeak), "+4.0 dBu");
    let mic = input(Scale::DbSpl { fs_db_spl: 120.5 });
    assert_eq!(mic.max_text(Ballistic::TruePeak), "max 117.4 dB SPL");
}

#[test]
fn bar_fills_the_meter_span_on_every_scale() {
    assert_eq!(bar_fill(Some(0.0)), 1.0);
    assert_eq!(bar_fill(Some(-30.0)), 0.5);
    assert_eq!(bar_fill(Some(-60.0)), 0.0);
    assert_eq!(bar_fill(Some(-90.0)), 0.0);
    assert_eq!(bar_fill(Some(3.0)), 1.0);
    assert_eq!(bar_fill(Some(f32::NEG_INFINITY)), 0.0);
    assert_eq!(bar_fill(None), 0.0);
    let line = input(Scale::Dbu { fs_dbu: 16.3 });
    assert_eq!(
        line.bar_fill(),
        bar_fill(Some(-12.34)),
        "the unit moves the text, not the bar"
    );
    assert_eq!(line.flying_peak_fill(), 0.9);
}

#[test]
fn overs_clip_and_notes_keep_their_words() {
    let mut i = input(Scale::Dbfs);
    assert_eq!(i.overs_text(), "");
    assert_eq!(i.clip_text(), "");
    i.overs = 1;
    assert_eq!(i.overs_text(), "1 over");
    i.overs = 2;
    assert_eq!(i.overs_text(), "2 overs");
    i.clipped = true;
    assert_eq!(i.clip_text(), "CLIP");
    assert_eq!(i.note(Ballistic::Vu), Some(VU_FALLBACK));
    assert_eq!(i.note(Ballistic::TruePeak), None);
    i.offers_vu = true;
    assert_eq!(i.note(Ballistic::Vu), None);
    assert_eq!(VU_FALLBACK, "true peak shown · VU needs a line calibration");
}

#[test]
fn overs_slots_keep_their_width() {
    assert_eq!(overs_slot(0).chars().count(), OVERS_SLOT);
    assert_eq!(overs_slot(123_456), "123k overs");
    assert_eq!(overs_slot(u32::MAX), " 1M+ overs");
}

#[test]
fn correlation_line_keeps_its_width() {
    let lines = [
        Correlation::NoData,
        Correlation::Value(0.874),
        Correlation::Value(-0.42),
        Correlation::Value(f32::NAN),
        Correlation::Quiet(Side::Left),
        Correlation::Quiet(Side::Right),
        Correlation::Quiet(Side::Both),
    ]
    .map(|c| correlation_line(c).text);
    assert_eq!(lines[1], "correlation +0.87      ");
    assert!(correlation_line(Correlation::Value(-0.42)).warning);
    for l in &lines {
        assert_eq!(l.chars().count(), lines[0].chars().count(), "{l:?}");
    }
}

#[test]
fn ballistic_words() {
    assert_eq!(Ballistic::TruePeak.word(), "true peak");
    assert_eq!(Ballistic::Type1.word(), "Type I");
    assert_eq!(Ballistic::Type2.word(), "Type II");
    assert_eq!(Ballistic::Vu.word(), "VU");
}

#[test]
fn correlation_chip_texts() {
    assert_eq!(
        Correlation::Value(0.874).chip(),
        CorrelationChip {
            text: "+0.87".into(),
            warning: false,
        }
    );
    assert_eq!(
        Correlation::Value(-0.42).chip(),
        CorrelationChip {
            text: "−0.42".into(),
            warning: true,
        }
    );
    assert_eq!(Correlation::Value(0.0).chip().text, "0.00");
    assert!(!Correlation::Value(0.0).chip().warning);
    assert_eq!(Correlation::Value(1.3).chip().text, "+1.00", "clamped");
    assert_eq!(Correlation::Value(f32::NAN).chip().text, "—");
    assert_eq!(Correlation::NoData.chip().text, "—");
    assert_eq!(Correlation::Quiet(Side::Right).chip().text, "right quiet");
    assert_eq!(Correlation::Quiet(Side::Left).chip().text, "left quiet");
    assert_eq!(Correlation::Quiet(Side::Both).chip().text, "both quiet");
    assert!(!Correlation::Quiet(Side::Right).chip().warning);
}

#[test]
fn window_and_title() {
    assert_eq!(window_text(0.3), "0.3 s");
    assert_eq!(window_text(1.0), "1 s");
    assert_eq!(window_text(0.0), "—");
    let pane = pane_of(
        vec![input(Scale::Dbfs)],
        Some(ProgrammePair {
            name: "Main L + Main R".into(),
            correlation: Correlation::Value(0.9),
            window_s: 0.3,
        }),
        Ballistic::Type2,
    );
    assert_eq!(pane.title(), "Type II · Main L + Main R · 0.3 s");
    let solo = ProgrammePane {
        pair: None,
        ..pane.clone()
    };
    assert_eq!(solo.title(), "Type II");
}

#[test]
fn flying_peak_holds_a_second_then_falls_20_db_a_second() {
    let mut f = FlyingPeak::default();
    assert_eq!(f.update(0.0, Some(-10.0)), Some(-10.0));
    assert_eq!(f.update(0.5, Some(-30.0)), Some(-10.0), "held");
    assert_eq!(
        f.update(1.0, Some(-30.0)),
        Some(-10.0),
        "held to the second"
    );
    let v = f.update(1.5, Some(-30.0)).expect("falling");
    assert!((v - -20.0).abs() < 1e-4, "20 dB/s × 0.5 s: {v}");
    let v = f.update(2.0, Some(-30.0)).expect("falling");
    assert!((v - -30.0).abs() < 1e-4, "{v}");
    assert_eq!(
        f.update(2.5, Some(-30.0)),
        Some(-30.0),
        "a peak lifts a fallen marker"
    );
    assert_eq!(
        f.update(2.6, Some(-5.0)),
        Some(-5.0),
        "a louder peak takes over at once"
    );
    assert_eq!(f.update(3.5, None), Some(-5.0), "still held");
    let mut f = FlyingPeak::default();
    assert_eq!(f.update(0.0, None), None);
    f.update(0.0, Some(-55.0));
    assert_eq!(f.update(1.5, None), None, "fallen under the floor: gone");
    assert_eq!(f.value(), None);
}

#[test]
fn layout_cases() {
    let rect = Rect::new(10.0, 20.0, 600.0, 300.0);
    let bars_only = layout(rect, false);
    assert_eq!(bars_only.gonio, None);
    assert_eq!(bars_only.correlation, None);
    assert_eq!(bars_only.bars, rect);
    // A pair's Meters view: the strip under the bars, as wide as they are.
    let pair = layout(rect, true);
    assert_eq!(pair.gonio, None);
    assert_eq!(pair.bars, Rect::new(10.0, 20.0, 600.0, 300.0 - CHIP_H));
    assert_eq!(
        pair.correlation,
        Some(Rect::new(10.0, 320.0 - CHIP_H, 600.0, CHIP_H))
    );
    let pane = pane_of(vec![], None, Ballistic::TruePeak);
    assert_eq!(
        pane.layout(rect, 13.0, 11.0, crate::view::PlotChrome::Full),
        bars_only
    );
}

pub(super) fn pane_with_pair(correlation: Correlation) -> ProgrammePane {
    let mut l = input(Scale::Dbfs);
    l.label = "Main L".into();
    let mut r = input(Scale::Dbfs);
    r.label = "Main R".into();
    pane_of(
        vec![l, r],
        Some(ProgrammePair {
            name: "Main L + Main R".into(),
            correlation,
            window_s: 1.0,
        }),
        Ballistic::TruePeak,
    )
}

pub(super) fn texts(s: &crate::primitives::Scene) -> Vec<String> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|x| x.text.clone()))
        .collect()
}

#[test]
fn missing_inputs_say_which() {
    assert_eq!(missing_text(&[2]), "input 3 not in this session");
    assert_eq!(missing_text(&[2, 3]), "inputs 3+4 not in this session");
}

#[test]
fn scene_draws_bars_readouts_chip_and_field() {
    use crate::primitives::Viewport;
    use crate::theme::Theme;
    use crate::view::{PlotChrome, Style};
    let theme = Theme::dark();
    let size = Viewport {
        width: 600.0,
        height: 400.0,
    };
    let p = pane_with_pair(Correlation::Quiet(Side::Right));
    let full = programme_scene(
        &p,
        Style {
            chrome: PlotChrome::Full,
            theme: &theme,
        },
        size,
        None,
    );
    let t = texts(&full.scene);
    // The Meters view of a pair: its bars and the chip's strip under them.
    for want in ["Main L", "Main R", "right quiet", "correlation"] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
    assert_eq!(full.gonio, None);
    // A true-peak bar reads dBTP, its max too, and no second true-peak line repeats it.
    assert!(t.iter().any(|x| x == "−12.3 dBTP"), "{t:?}");
    assert!(t.iter().any(|x| x == "max"), "{t:?}");
    assert!(t.iter().any(|x| x == "−3.1 dBTP"), "{t:?}");
    assert!(!t.iter().any(|x| x == "−1.2 dBTP"), "{t:?}");
    let mut type1 = p.clone();
    type1.ballistic = Ballistic::Type1;
    let style = Style {
        chrome: PlotChrome::Full,
        theme: &theme,
    };
    let t1 = texts(&programme_scene(&type1, style, size, None).scene);
    for want in ["−12.3 dBFS", "−1.2 dBTP", "max", "−3.1 dBFS"] {
        assert!(t1.iter().any(|x| x == want), "{want} in {t1:?}");
    }
    let mut g = p.clone();
    g.view = crate::view::ProgrammeMode::Goniometer;
    let full = programme_scene(&g, style, size, None);
    let t = texts(&full.scene);
    for want in ["right quiet", "correlation", "L", "R"] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
    let sq = full.gonio.expect("a pair's Goniometer view");
    assert_eq!(sq.w, sq.h);
    let grid = |s: &ProgrammeScene| s.scene.layers[0].polylines.len();
    let bare = programme_scene(
        &g,
        Style {
            chrome: PlotChrome::Bare,
            theme: &theme,
        },
        size,
        None,
    );
    assert_eq!(grid(&full), 3, "the outline and the two axes");
    assert_eq!(grid(&bare), 0, "T hides the grid");
    let gone = programme_scene(
        &p,
        Style {
            chrome: PlotChrome::Full,
            theme: &theme,
        },
        size,
        Some("inputs 3+4 not in this session"),
    );
    assert_eq!(texts(&gone.scene), vec!["inputs 3+4 not in this session"]);
    assert!(gone.gonio.is_none());
}

/// The readouts under the bars stay where the column puts them whatever they read: the
/// values right-aligned at one edge, `max`, the overs and CLIP at their own places.
#[test]
fn bar_readouts_keep_their_places() {
    use crate::primitives::Viewport;
    use crate::theme::Theme;
    use crate::view::{PlotChrome, Style};
    let theme = Theme::dark();
    let style = Style {
        chrome: PlotChrome::Full,
        theme: &theme,
    };
    let size = Viewport {
        width: 600.0,
        height: 400.0,
    };
    let places = |level: Option<f32>, overs: u32, clipped: bool| {
        let mut p = pane_with_pair(Correlation::Value(0.5));
        p.ballistic = Ballistic::Type1;
        for i in &mut p.inputs {
            (i.ppm, i.max, i.true_peak_dbtp) = (level, level, level);
            i.overs = overs;
            i.clipped = clipped;
        }
        p.bars = peak_bars(&p.inputs, p.ballistic);
        let scene = programme_scene(&p, style, size, None).scene;
        let mut out: Vec<(String, [u32; 2])> = scene
            .layers
            .iter()
            .flat_map(|l| l.labels.iter())
            .map(|l| {
                let class = if l.text.contains("over") {
                    "overs".to_owned()
                } else if l.text.ends_with("dBFS") || l.text.ends_with("dBTP") || l.text == NO_VALUE
                {
                    "value".to_owned()
                } else {
                    l.text.clone()
                };
                (class, l.pos.map(f32::to_bits))
            })
            .collect();
        out.sort();
        out
    };
    let all = places(Some(-0.1), 12, true);
    for level in [None, Some(-0.1), Some(-60.0), Some(-120.0)] {
        for overs in [0, 1, 12, 999] {
            for clipped in [false, true] {
                let got = places(level, overs, clipped);
                for g in &got {
                    assert!(all.contains(g), "{g:?} moved: {level:?} {overs} {clipped}");
                }
            }
        }
    }
}
