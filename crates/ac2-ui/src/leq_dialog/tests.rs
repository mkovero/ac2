//! Tests of the Leq windows dialog.

use super::*;
use ac2_proto::model::{PeakWeighting, TimeWeighting};
use ac2_proto::units::Rev;

fn meter() -> Measurement {
    Measurement {
        id: MeasId(4),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "FOH SPL".into(),
            kind: MeasKind::Spl {
                config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
            },
        },
        config_rev: Rev(3),
        running: true,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
    }
}

#[test]
fn opens_on_the_meter_windows() {
    let d = LeqDialog::new(&meter(), true).expect("spl");
    let names: Vec<String> = d.rows.iter().map(|r| r.cell(Col::Length)).collect();
    assert_eq!(
        names,
        [
            "LAeq 1 min",
            "LAeq 5 min",
            "LAeq 10 min",
            "LAeq 30 min",
            "LAeq 60 min"
        ]
    );
    assert_eq!(d.horizon_s(), 60);
    assert_eq!(d.preset_text(), "none (keep these windows)");
    assert_eq!(d.leq_config().expect("valid"), LeqConfig::default_windows());
    let mut tf = meter();
    tf.config.kind = MeasKind::Transfer {
        config: ac2_proto::model::TransferConfig::with_inputs(0, 1),
    };
    assert!(LeqDialog::new(&tf, true).is_none());
}

/// Keys only: a preset, a typed limit, a new window, a removal.
#[test]
fn keyboard_flow() {
    let mut d = LeqDialog::new(&meter(), true).expect("spl");
    assert!(d.preset_note().contains("replaces the windows"));
    // → on the preset row: DIN 15905-5 is its 30 min window, nothing else.
    d.cycle(1);
    assert_eq!(
        d.preset_text(),
        "DIN 15905-5: LAeq 30 min ≤ 99 dB, LCpeak ≤ 135 dB"
    );
    assert!(
        d.preset_source()
            .expect("source")
            .contains("not legal advice")
    );
    assert_eq!(names(&d), ["LAeq 30 min 99"]);
    // → → → → : WHO, its 15 min window only.
    for _ in 0..4 {
        d.cycle(1);
    }
    assert_eq!(d.preset_text(), "WHO safe listening: LAeq 15 min ≤ 100 dB");
    assert_eq!(names(&d), ["LAeq 15 min 100"]);
    // ↓ ↓ to the window; Insert adds a longer one after it, without a limit.
    d.move_row(1);
    d.move_row(1);
    assert_eq!(
        d.focus,
        Focus::Window {
            row: 0,
            col: Col::Length
        }
    );
    d.add_window();
    assert_eq!(names(&d), ["LAeq 15 min 100", "LAeq 30 min "]);
    // Tab Tab to its limit; type 102.
    d.move_cell(1);
    d.move_cell(1);
    assert_eq!(
        d.focus,
        Focus::Window {
            row: 1,
            col: Col::Limit
        }
    );
    assert!(d.selected, "a text cell arrives selected");
    d.type_text("102 dB");
    // → on the length: longer, 60 min.
    d.move_cell(-1);
    d.move_cell(-1);
    d.cycle(1);
    assert_eq!(d.rows[1].cell(Col::Length), "LAeq 60 min");
    // Weighting → C.
    d.move_cell(1);
    d.cycle(1);
    assert_eq!(d.rows[1].cell(Col::Length), "LCeq 60 min");
    // Another, and Delete removes it again.
    d.add_window();
    assert_eq!(d.rows.len(), 3);
    d.remove_window();
    assert_eq!(d.rows.len(), 2);
    let c = d.leq_config().expect("valid");
    assert_eq!(c.windows[0].limit, Some(DbSpl(100.0)));
    assert_eq!(c.windows[1].limit, Some(DbSpl(102.0)));
    assert_eq!(c.windows[1].weighting, Weighting::C);
    assert_eq!(c.windows[1].duration, Seconds(3600.0));
    // The horizon row: ← to 30 s.
    d.focus = Focus::Horizon;
    d.cycle(-1);
    assert_eq!(d.horizon_s(), 30);
    let MeasKind::Spl { config } = d.meas_config().expect("valid").kind else {
        panic!()
    };
    assert_eq!(config.input, 1);
    assert_eq!(config.peak_weighting, PeakWeighting::C);
    assert_eq!(config.leq.horizon, Seconds(30.0));
}

fn names(d: &LeqDialog) -> Vec<String> {
    d.rows
        .iter()
        .map(|r| format!("{} {}", r.cell(Col::Length), r.limit))
        .collect()
}

/// → through every preset: each shows exactly its own windows and limits, shortest
/// first; back to "none" the meter's windows return as they were.
#[test]
fn cycles_through_every_preset() {
    let mut d = LeqDialog::new(&meter(), true).expect("spl");
    d.rows[0].limit = "110".into();
    let start = d.rows.clone();
    for (i, p) in LeqPreset::ALL.iter().enumerate() {
        d.cycle(1);
        assert_eq!(d.preset, Some(i));
        assert_eq!(d.error, None, "{p:?}");
        assert_eq!(d.preset_text(), ac2_scene::leq_preset::summary(*p));
        let c = d.leq_config().expect("valid");
        assert_eq!(c.windows, LeqPreset::windows_of(&[*p]), "{p:?}");
        match p {
            LeqPreset::France => {
                assert_eq!(names(&d), ["LAeq 15 min 102", "LCeq 15 min 118"]);
            }
            LeqPreset::Flanders100 => {
                assert_eq!(names(&d), ["LAeq 15 min ", "LAeq 60 min 100"]);
            }
            LeqPreset::Brussels100 => {
                assert_eq!(names(&d), ["LAeq 60 min 100", "LCeq 60 min 115"]);
            }
            _ => {}
        }
    }
    // → at the end stays; ← all the way back to none restores the windows.
    d.cycle(1);
    assert_eq!(d.preset, Some(LeqPreset::ALL.len() - 1));
    for _ in 0..LeqPreset::ALL.len() {
        d.cycle(-1);
    }
    assert_eq!(d.preset, None);
    assert_eq!(d.preset_text(), "none (keep these windows)");
    assert_eq!(d.rows, start);
}

/// An edit after a preset keeps it: the windows it set stay when the preset row moves
/// back to "none", and the next preset replaces them in turn.
#[test]
fn an_edit_keeps_the_preset() {
    let mut d = LeqDialog::new(&meter(), true).expect("spl");
    d.cycle(1);
    assert_eq!(names(&d), ["LAeq 30 min 99"]);
    d.focus_cell(0, Col::Limit);
    d.type_text("101");
    d.focus = Focus::Preset;
    d.cycle(-1);
    assert_eq!(d.preset, None);
    assert_eq!(names(&d), ["LAeq 30 min 101"]);
    d.cycle(1);
    d.cycle(1);
    assert_eq!(names(&d), ["LAeq 60 min 93"]);
    d.cycle(-1);
    d.cycle(-1);
    assert_eq!(names(&d), ["LAeq 30 min 101"], "the edited windows return");
}

#[test]
fn refusals_name_the_window() {
    let mut d = LeqDialog::new(&meter(), true).expect("spl");
    d.focus_cell(3, Col::Limit);
    d.type_text("loud");
    let e = d.leq_config().expect_err("a word is no limit");
    assert!(e.starts_with("window 4 (LAeq 30 min) limit"), "{e}");
    d.select_all();
    d.type_text("200");
    assert!(
        d.leq_config()
            .expect_err("out of range")
            .contains("30 … 150 dB SPL")
    );
    d.select_all();
    d.backspace();
    assert!(d.leq_config().is_ok(), "empty: no limit");
    for _ in 0..3 {
        d.add_window();
    }
    d.add_window();
    assert!(d.error.as_deref().is_some_and(|e| e.contains("at most 8")));
}

/// The band meter's rows come before the rig's two (scope, cues): Tab reaches them, ←/→ turn it on with a preset,
/// ↓ walks its windows, Insert adds a band window there, Shift+Insert's range row adds
/// one per band, typing sets a limit, Enter sends them with the windows.
#[test]
fn the_band_meter_rows() {
    let mut d = LeqDialog::new(&meter(), true).expect("spl");
    d.move_cell(-3);
    let meter_row = Focus::Band(BandFocus::Field(BandField::Meter));
    assert_eq!(d.focus, meter_row);
    d.cycle(2);
    assert_eq!(
        d.bands.meter,
        BandMeter::Preset(ac2_proto::model::BandLeqPreset::Finland545Lf)
    );
    assert_eq!(d.bands.rows.len(), 11);
    // The meter; the 20 Hz window's row.
    d.move_row(1);
    let w = |row, col| Focus::Band(BandFocus::Window { row, col });
    assert_eq!(d.focus, w(0, BandCol::Band));
    d.move_cell(2);
    d.cycle(-2);
    assert_eq!(d.bands.rows[0].cell(BandCol::Length), "LAeq 60 min");
    assert_eq!(d.bands.meter, BandMeter::On, "an edit: the operator's own");
    d.move_cell(1);
    assert_eq!(d.focus, w(0, BandCol::Limit));
    assert!(d.selected);
    d.type_text("30");
    // ↓ keeps the column: the 25 Hz window's limit.
    d.move_row(1);
    assert_eq!(d.focus, w(1, BandCol::Limit));
    d.add_window();
    assert_eq!(d.focus, w(2, BandCol::Band));
    // Up from 25 Hz, every band to 200 Hz is an LZeq 60 min window: 250 Hz.
    assert_eq!(d.bands.rows[2].name(), "250 Hz band LZeq 60 min");
    // Shift+Insert: the range row under the heading; Enter adds it.
    d.open_range();
    assert_eq!(d.focus, Focus::Band(BandFocus::Range(RangeCol::From)));
    assert!(d.on_range());
    d.move_cell(2);
    d.cycle(-2);
    assert_eq!(
        d.bands.range.map(|r| r.cell(RangeCol::Length)),
        Some("LZeq 15 min".into())
    );
    d.add_range();
    assert_eq!(d.bands.rows.len(), 23);
    assert_eq!(d.focus, w(12, BandCol::Band));
    assert_eq!(d.bands.rows[22].name(), "200 Hz band LZeq 15 min");
    // Delete on an open range row closes it.
    d.open_range();
    d.remove_window();
    assert_eq!(d.bands.range, None);
    assert_eq!(d.bands.rows.len(), 23);
    d.focus = w(22, BandCol::Margin);
    d.move_row(1);
    assert_eq!(d.focus, Focus::Band(BandFocus::Field(BandField::Impulse)));
    d.cycle(1);
    let MeasKind::Spl { config } = d.meas_config().expect("valid").kind else {
        panic!()
    };
    let b = config.bands.expect("on");
    assert_eq!(b.windows.len(), 23);
    assert_eq!(b.windows[0].weighting, Weighting::A);
    assert_eq!(b.windows[0].limit, Some(DbSpl(30.0)));
    assert_eq!(b.windows[0].day_offset, Some(Db(5.0)));
    assert_eq!(b.windows[2].duration, Seconds(3600.0));
    assert_eq!(b.windows[2].limit, None);
    assert_eq!(b.correction.db(), 5.0);
    assert_eq!(config.leq, d.leq_config().expect("windows"));
    // Delete on a band window removes it.
    d.focus = w(1, BandCol::Margin);
    d.remove_window();
    assert_eq!(d.bands.rows.len(), 22);
    assert_eq!(d.focus, w(1, BandCol::Margin));
}

/// Under the windows: the peak limits and the position correction, reached with ↓ and
/// Tab, typed; a preset sets its peak limits and back at "none" they return; the
/// correction is the operator's and no preset touches it.
#[test]
fn peak_limits_and_the_position_correction() {
    let mut d = LeqDialog::new(&meter(), true).expect("spl");
    // ↑ from the preset row wraps to the rig's rows (the cues' fold, the scope), then the
    // band meter (off: its one row); above it the peaks' correction.
    d.move_row(-1);
    assert_eq!(d.focus, Focus::CueFold);
    d.move_row(-1);
    assert_eq!(d.focus, Focus::Scope);
    d.move_row(-1);
    assert_eq!(d.focus, Focus::Band(BandFocus::Field(BandField::Meter)));
    d.move_row(-1);
    assert_eq!(d.focus, Focus::Extra(Extra::PositionPeak));
    d.move_row(-1);
    assert_eq!(d.focus, Focus::Extra(Extra::Position));
    assert!(d.selected);
    d.type_text("4 dB");
    // Shift+Tab twice: LCpeak's limit.
    d.move_cell(-1);
    d.move_cell(-1);
    assert_eq!(d.focus, Focus::Extra(Extra::LcPeak));
    d.type_text("130");
    let MeasKind::Spl { config } = d.meas_config().expect("valid").kind else {
        panic!()
    };
    assert_eq!(
        config.leq.peaks.lcpeak.as_ref().map(|l| l.limit),
        Some(DbSpl(130.0))
    );
    assert_eq!(
        config.leq.peaks.lcpeak.as_ref().map(|l| l.warn_margin),
        Some(Db(3.0))
    );
    assert_eq!(config.leq.peaks.lafmax, None);
    assert_eq!(config.position, Some(PositionCorrection::both(4.0)));
    // A different peak correction.
    d.focus = Focus::Extra(Extra::PositionPeak);
    d.type_text("2");
    assert_eq!(
        d.position().expect("valid").map(|p| (p.level, p.peak)),
        Some((Db(4.0), Db(2.0)))
    );
    // Swiss 100: its LAFmax 125 replaces the peak limits; the correction stays.
    d.focus = Focus::Preset;
    for _ in 0..4 {
        d.cycle(1);
    }
    assert_eq!(
        d.preset_text(),
        "Swiss V-NISSG 100 dB: LAeq 60 min ≤ 100 dB, LAFmax ≤ 125 dB"
    );
    assert_eq!(d.extra_text(Extra::LcPeak), "");
    assert_eq!(d.extra_text(Extra::LafMax), "125");
    assert_eq!(d.extra_text(Extra::Position), "4 dB");
    for _ in 0..4 {
        d.cycle(-1);
    }
    assert_eq!(d.extra_text(Extra::LcPeak), "130", "back at none");
    // Refusals name the field.
    d.focus = Focus::Extra(Extra::LafMax);
    d.type_text("loud");
    assert!(
        d.leq_config()
            .expect_err("a word")
            .starts_with("LAFmax limit")
    );
    d.select_all();
    d.type_text("200");
    assert!(d.leq_config().expect_err("range").contains("60 … 170"));
    d.select_all();
    d.backspace();
    d.focus = Focus::Extra(Extra::Position);
    d.select_all();
    d.type_text("40");
    assert!(d.meas_config().expect_err("range").contains("±30"));
    d.select_all();
    d.backspace();
    assert!(
        d.meas_config()
            .expect_err("a peak correction alone")
            .contains("needs one for the levels")
    );
    // A meter with a correction opens with it.
    let mut m = meter();
    if let MeasKind::Spl { config } = &mut m.config.kind {
        config.position = Some(PositionCorrection::both(-2.5));
    }
    let d = LeqDialog::new(&m, true).expect("spl");
    assert_eq!(
        (
            d.extra_text(Extra::Position),
            d.extra_text(Extra::PositionPeak)
        ),
        ("-2.5".to_owned(), String::new())
    );
}
