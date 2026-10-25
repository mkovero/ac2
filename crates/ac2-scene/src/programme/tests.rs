use super::*;

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
    assert_eq!(VU_FALLBACK, "VU needs a line calibration · true peak shown");
}

#[test]
fn inputs_row_lines_read_in_fixed_slots() {
    let mut i = input(Scale::Dbfs);
    assert_eq!(
        i.row_lines(Ballistic::Type1),
        [
            "Type I  −12.3 dBFS     ",
            "max   −3.1 dBFS           ",
            "true peak   −1.2 dBTP",
        ]
    );
    assert_eq!(
        i.row_lines(Ballistic::TruePeak),
        ["true peak  −12.3 dBTP     ", "max   −3.1 dBTP           "]
    );
    // VU on an uncalibrated input reads the true peak, and says so.
    assert_eq!(i.row_lines(Ballistic::Vu), i.row_lines(Ballistic::TruePeak));
    let line = input(Scale::Dbu { fs_dbu: 16.3 });
    assert_eq!(
        line.row_lines(Ballistic::Type2),
        [
            "Type II   +4.0 dBu     ",
            "max  +13.2 dBu           ",
            "true peak   −1.2 dBTP",
        ]
    );
    i.ppm = None;
    i.max = None;
    i.overs = 2;
    i.clipped = true;
    assert_eq!(
        i.row_lines(Ballistic::TruePeak),
        ["true peak      — dBTP CLIP", "max      — dBTP    2 overs"]
    );
    i.overs = 1;
    assert_eq!(
        i.row_lines(Ballistic::TruePeak)[1],
        "max      — dBTP    1 over "
    );
}

/// Every line keeps its character count (the Inputs list sets it in a monospace font)
/// whatever the levels, the overs count and CLIP: the row never shifts or wraps.
#[test]
fn inputs_row_lines_keep_their_width() {
    for scale in [
        Scale::Dbfs,
        Scale::Dbu { fs_dbu: 16.3 },
        Scale::DbSpl { fs_db_spl: 120.5 },
    ] {
        for ballistic in [Ballistic::TruePeak, Ballistic::Type1, Ballistic::Vu] {
            let mut widths = None;
            for level in [
                None,
                Some(f32::NEG_INFINITY),
                Some(-0.1),
                Some(-60.0),
                Some(-120.0),
                Some(3.0),
            ] {
                for overs in [0, 1, 12, 999, 9999, 123_456, u32::MAX] {
                    for clipped in [false, true] {
                        let mut i = input(scale);
                        i.offers_vu = matches!(scale, Scale::Dbu { .. });
                        (i.ppm, i.max, i.true_peak_dbtp) = (level, level, level);
                        i.overs = overs;
                        i.clipped = clipped;
                        let w: Vec<usize> = i
                            .row_lines(ballistic)
                            .iter()
                            .map(|l| l.chars().count())
                            .collect();
                        let first = widths.get_or_insert_with(|| w.clone());
                        assert_eq!(
                            *first, w,
                            "{scale:?} {ballistic:?} {level:?} {overs} {clipped}"
                        );
                    }
                }
            }
        }
    }
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
    let pane = ProgrammePane {
        inputs: vec![input(Scale::Dbfs)],
        pair: Some(ProgrammePair {
            name: "Main L + Main R".into(),
            correlation: Correlation::Value(0.9),
            window_s: 0.3,
        }),
        ballistic: Ballistic::Type2,
        loudness: None,
    };
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
    let rect = Rect::new(10.0, 20.0, 400.0, 300.0);
    let bars_only = layout(rect, false, false, 2);
    assert_eq!(bars_only.gonio, None);
    assert_eq!(bars_only.correlation, None);
    assert_eq!(bars_only.bars, rect);

    let full = layout(rect, true, false, 2);
    assert_eq!(
        full.gonio,
        Some(Rect::new(10.0, 20.0, 200.0, 300.0 - CHIP_H))
    );
    assert_eq!(
        full.correlation,
        Some(Rect::new(10.0, 320.0 - CHIP_H, 200.0, CHIP_H))
    );
    assert_eq!(full.bars, Rect::new(210.0, 20.0, 200.0, 300.0));

    let short = layout(Rect::new(0.0, 0.0, 400.0, 100.0), true, false, 2);
    assert_eq!(short.gonio, None);
    assert_eq!(short.correlation, Some(Rect::new(0.0, 0.0, CHIP_W, 100.0)));
    assert_eq!(short.bars, Rect::new(CHIP_W, 0.0, 400.0 - CHIP_W, 100.0));

    let narrow = layout(Rect::new(0.0, 0.0, 100.0, 100.0), true, false, 2);
    assert_eq!(
        narrow.correlation.expect("a chip").w,
        50.0,
        "the chip never crowds the bars out"
    );
    let pane = ProgrammePane {
        inputs: vec![],
        pair: None,
        ballistic: Ballistic::TruePeak,
        loudness: None,
    };
    assert_eq!(pane.layout(rect), bars_only);
}

fn pane_with_pair(correlation: Correlation) -> ProgrammePane {
    let mut l = input(Scale::Dbfs);
    l.label = "Main L".into();
    let mut r = input(Scale::Dbfs);
    r.label = "Main R".into();
    ProgrammePane {
        inputs: vec![l, r],
        pair: Some(ProgrammePair {
            name: "Main L + Main R".into(),
            correlation,
            window_s: 1.0,
        }),
        ballistic: Ballistic::TruePeak,
        loudness: None,
    }
}

fn texts(s: &crate::primitives::Scene) -> Vec<String> {
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
    for want in ["Main L", "Main R", "right quiet", "correlation", "L", "R"] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
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
    let sq = full.gonio.expect("a pair on a tall pane has a goniometer");
    assert_eq!(sq.w, sq.h);
    let grid = |s: &ProgrammeScene| s.scene.layers[0].polylines.len();
    let bare = programme_scene(
        &p,
        Style {
            chrome: PlotChrome::Bare,
            theme: &theme,
        },
        size,
        None,
    );
    assert!(grid(&full) > 3);
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

#[test]
fn the_loudness_column_goes_before_the_goniometer() {
    // Two bars need 32 + 2 · 96 = 224 px, the goniometer 120, the column 190: 534 px.
    let wide = layout(Rect::new(0.0, 0.0, 600.0, 300.0), true, true, 2);
    assert_eq!(
        wide.loudness,
        Some(Rect::new(600.0 - LOUDNESS_W, 0.0, LOUDNESS_W, 300.0))
    );
    let g = wide.gonio.expect("a goniometer");
    assert_eq!(
        g.w,
        600.0 - LOUDNESS_W - 224.0,
        "the bars keep their readouts"
    );
    assert_eq!(wide.bars.w, 224.0);
    assert_eq!(wide.bars.right(), 600.0 - LOUDNESS_W);
    // A wide pane splits the rest 1:1 as without the column.
    let wider = layout(Rect::new(0.0, 0.0, 1000.0, 300.0), true, true, 2);
    assert_eq!(
        wider.gonio.expect("a goniometer").w,
        (1000.0 - LOUDNESS_W) / 2.0
    );
    let narrow = layout(Rect::new(0.0, 0.0, 520.0, 300.0), true, true, 2);
    assert_eq!(narrow.loudness, None, "the column goes first");
    assert!(narrow.gonio.is_some(), "the goniometer stays");
    assert_eq!(
        narrow,
        layout(Rect::new(0.0, 0.0, 520.0, 300.0), true, false, 2)
    );
    // Bars alone: one bar needs 128 px beside the column.
    assert!(
        layout(Rect::new(0.0, 0.0, 318.0, 300.0), false, true, 1)
            .loudness
            .is_some()
    );
    assert_eq!(
        layout(Rect::new(0.0, 0.0, 317.0, 300.0), false, true, 1).loudness,
        None
    );
    assert_eq!(
        layout(Rect::new(0.0, 0.0, 400.0, 300.0), false, true, 3).loudness,
        None
    );
}

#[test]
fn scene_draws_the_loudness_column() {
    use super::loudness::{LoudnessColumn, LoudnessReadout};
    use crate::primitives::Viewport;
    use crate::theme::Theme;
    use crate::view::{PlotChrome, Style};
    let theme = Theme::dark();
    let size = Viewport {
        width: 800.0,
        height: 400.0,
    };
    let mut p = pane_with_pair(Correlation::Value(0.5));
    p.loudness = Some(LoudnessColumn::Readout(LoudnessReadout {
        title: "Main L + Main R".into(),
        momentary: -23.06,
        short_term: -22.84,
        integrated: Some(-23.0),
        range: Some(6.2),
        duration_s: 10.0,
        max_true_peak: -1.2,
        since: "since 20:14 · R resets".into(),
    }));
    let style = |chrome| Style {
        chrome,
        theme: &theme,
    };
    let t = texts(&programme_scene(&p, style(PlotChrome::Full), size, None).scene);
    for want in [
        "Mom −23.1",
        "Short −22.8",
        "Int −23.0 LUFS",
        "LRA 6.2 LU (settling)",
        "TP max −1.2 dBTP",
        "since 20:14 · R resets",
    ] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
    let bare = texts(&programme_scene(&p, style(PlotChrome::Bare), size, None).scene);
    assert!(bare.iter().any(|x| x == "Int −23.0 LUFS"), "{bare:?}");
    assert!(
        !bare.iter().any(|x| x.starts_with("since")),
        "T hides the since line"
    );
    let narrow = Viewport {
        width: 400.0,
        height: 400.0,
    };
    let t = texts(&programme_scene(&p, style(PlotChrome::Full), narrow, None).scene);
    assert!(!t.iter().any(|x| x.starts_with("Mom")), "{t:?}");

    // A message wraps in the column; its words read in order.
    p.loudness = Some(LoudnessColumn::Message(
        super::loudness::RATE_TEXT.to_owned(),
    ));
    let t = texts(&programme_scene(&p, style(PlotChrome::Full), size, None).scene);
    let start = t
        .iter()
        .position(|x| x.starts_with("loudness"))
        .expect("the message");
    let joined = t[start..].join(" ");
    assert!(joined.starts_with(super::loudness::RATE_TEXT), "{t:?}");
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
