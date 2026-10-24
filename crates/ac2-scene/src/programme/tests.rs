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
    assert_eq!(i.ppm_text(), "−12.3 dBFS");
    assert_eq!(i.true_peak_text(), "−1.2 dBTP");
    assert_eq!(i.max_text(), "max −3.1 dBFS");
    let line = input(Scale::Dbu { fs_dbu: 16.3 });
    assert_eq!(line.ppm_text(), "+4.0 dBu");
    assert_eq!(line.true_peak_text(), "−1.2 dBTP", "dBTP on every scale");
    assert_eq!(line.max_text(), "max +13.2 dBu");
    let mic = input(Scale::DbSpl { fs_db_spl: 120.5 });
    assert_eq!(mic.ppm_text(), "108.2 dB SPL");
    let mut none = input(Scale::Dbfs);
    none.ppm = None;
    none.max = Some(f32::NEG_INFINITY);
    none.true_peak_dbtp = None;
    assert_eq!(none.ppm_text(), "—");
    assert_eq!(none.max_text(), "max —");
    assert_eq!(none.true_peak_text(), "—");
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
    let bars_only = layout(rect, false);
    assert_eq!(bars_only.gonio, None);
    assert_eq!(bars_only.correlation, None);
    assert_eq!(bars_only.bars, rect);

    let full = layout(rect, true);
    assert_eq!(
        full.gonio,
        Some(Rect::new(10.0, 20.0, 200.0, 300.0 - CHIP_H))
    );
    assert_eq!(
        full.correlation,
        Some(Rect::new(10.0, 320.0 - CHIP_H, 200.0, CHIP_H))
    );
    assert_eq!(full.bars, Rect::new(210.0, 20.0, 200.0, 300.0));

    let short = layout(Rect::new(0.0, 0.0, 400.0, 100.0), true);
    assert_eq!(short.gonio, None);
    assert_eq!(short.correlation, Some(Rect::new(0.0, 0.0, CHIP_W, 100.0)));
    assert_eq!(short.bars, Rect::new(CHIP_W, 0.0, 400.0 - CHIP_W, 100.0));

    let narrow = layout(Rect::new(0.0, 0.0, 100.0, 100.0), true);
    assert_eq!(
        narrow.correlation.expect("a chip").w,
        50.0,
        "the chip never crowds the bars out"
    );
    let pane = ProgrammePane {
        inputs: vec![],
        pair: None,
        ballistic: Ballistic::TruePeak,
    };
    assert_eq!(pane.layout(rect), bars_only);
}
