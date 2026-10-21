use super::*;
use ac2_proto::units::Phon;

#[test]
fn names_follow_the_other_trace_names() {
    assert_eq!(name(&TargetCurve::Flat), "flat");
    assert_eq!(name(&TargetCurve::house_tilt()), "tilt −1.0 dB/oct @ 1 kHz");
    let shelf = TargetCurve::Tilt {
        slope_db_per_oct: -1.0,
        pivot: Hz(1000.0),
        lf_shelf: Some(LfShelf {
            gain: Db(6.0),
            corner: Hz(100.0),
        }),
    };
    assert_eq!(
        name(&shelf),
        "tilt −1.0 dB/oct @ 1 kHz, LF +6.0 dB @ 100 Hz"
    );
    let odd = TargetCurve::Tilt {
        slope_db_per_oct: 0.5,
        pivot: Hz(31.5),
        lf_shelf: None,
    };
    assert_eq!(name(&odd), "tilt +0.5 dB/oct @ 31.5 Hz");
    assert_eq!(
        name(&TargetCurve::loudness_shift()),
        "loudness 85 → 100 phon"
    );
    let half = TargetCurve::LoudnessShift {
        from: Phon(82.5),
        to: Phon(70.0),
    };
    assert_eq!(name(&half), "loudness 82.5 → 70 phon");
}

#[test]
fn only_the_loudness_shift_carries_a_note() {
    assert_eq!(note(&TargetCurve::Flat), None);
    assert_eq!(note(&TargetCurve::house_tilt()), None);
    assert_eq!(
        note(&TargetCurve::loudness_shift()),
        Some("perceived shift, not an EQ target")
    );
}

#[test]
fn the_chooser_offers_a_file_and_the_generated_curves_but_no_bare_contour() {
    let q = choices();
    assert_eq!(q.title, "Target curve");
    let rows: Vec<&str> = q.choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        rows,
        [
            "Load a file… — a curve exported from another analyzer, or typed by hand",
            "Flat — 0 dB at every frequency",
            "House tilt… — dB per octave about a pivot, with an optional LF shelf",
            "Loudness shift… — how a level change is heard, not an EQ target",
        ]
    );
    assert!(q.choices.iter().all(|c| c.blocked.is_none()));
    assert!(!rows.iter().any(|r| r.contains("contour")));
}

#[test]
fn the_tilt_dialog_text_parses_with_and_without_units() {
    use tilt::*;
    assert_eq!(
        tilt_from_text(
            DEFAULT_SLOPE,
            DEFAULT_PIVOT,
            DEFAULT_SHELF_GAIN,
            DEFAULT_SHELF_CORNER
        ),
        Ok(TargetCurve::house_tilt())
    );
    assert_eq!(
        tilt_from_text("-0.5", "2k", "6", "80"),
        Ok(TargetCurve::Tilt {
            slope_db_per_oct: -0.5,
            pivot: Hz(2000.0),
            lf_shelf: Some(LfShelf {
                gain: Db(6.0),
                corner: Hz(80.0)
            }),
        })
    );
    // A 0 dB shelf is no shelf; its corner is then not read.
    assert_eq!(
        tilt_from_text("-1", "1k", "0 dB", "nonsense"),
        Ok(TargetCurve::house_tilt())
    );
    let e = tilt_from_text("-8", "1k", "off", "").expect_err("refused");
    assert!(e.contains("slope"), "{e}");
    let e = tilt_from_text("-1 dB", "1k", "off", "").expect_err("refused");
    assert!(e.starts_with("slope:"), "{e}");
    let e = tilt_from_text("-1", "1k", "6", "5 kHz").expect_err("refused");
    assert!(e.contains("corner"), "{e}");
}

#[test]
fn the_loudness_dialog_text_parses_and_refuses_a_level_out_of_range() {
    use loudness::*;
    assert_eq!(
        loudness_from_text(DEFAULT_FROM, DEFAULT_TO),
        Ok(TargetCurve::loudness_shift())
    );
    assert_eq!(
        loudness_from_text("60", "80 phons"),
        Ok(TargetCurve::LoudnessShift {
            from: Phon(60.0),
            to: Phon(80.0)
        })
    );
    let e = loudness_from_text("85", "120").expect_err("refused");
    assert!(e.contains("20 … 100 phon"), "{e}");
    let e = loudness_from_text("85", "85").expect_err("refused");
    assert!(e.contains("no shift"), "{e}");
    let e = loudness_from_text("85 dB", "90").expect_err("refused");
    assert!(e.starts_with("from:"), "{e}");
}
