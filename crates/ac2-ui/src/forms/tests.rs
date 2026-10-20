use ac2_proto::model::{
    BackendKind, ClockRelation, DeviceId, DeviceSelector, LoopbackRoute, PeakWeighting, Resolution,
    SessionConfig, SpecAveraging, TfAveraging, Window,
};
use ac2_proto::units::{Hz, WallNs};

use super::*;
use ac2_proto::model::SmoothingMode;

fn open(inputs: Vec<u16>, loopback: Option<LoopbackRoute>) -> OpenSession {
    OpenSession {
        config: SessionConfig {
            backend: None,
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: inputs,
            output_channels: 2,
            sample_rate_hz: None,
            buffer_frames: None,
            loopback,
        },
        backend: BackendKind::Fake,
        input_device: DeviceId("card".into()),
        output_device: DeviceId("card".into()),
        sample_rate_hz: 48_000,
        buffer_frames: 256,
        clock: ClockRelation::SingleCallback,
        opened_at: WallNs(0),
        replay: None,
        clock_source: None,
        driver_change: None,
        driver_reset: None,
        stated_latency: None,
    }
}

fn names(o: &OpenSession) -> Vec<(u16, String)> {
    o.config
        .input_channels
        .iter()
        .map(|c| (*c, format!("In {}", c + 1)))
        .collect()
}

#[test]
fn rates_and_buffers() {
    assert_eq!(parse_rate("48k"), Ok(Some(48_000)));
    assert_eq!(parse_rate("44.1 kHz"), Ok(Some(44_100)));
    assert_eq!(parse_rate("96000"), Ok(Some(96_000)));
    assert_eq!(parse_rate(""), Ok(None));
    assert!(parse_rate("fast").is_err());
    assert_eq!(parse_buffer("256"), Ok(Some(256)));
    assert_eq!(parse_buffer(""), Ok(None));
    assert!(parse_buffer("0").is_err());
}

#[test]
fn measurement_defaults_match_the_cli() {
    let o = open(
        vec![0, 1],
        Some(LoopbackRoute {
            output: 0,
            input: 0,
        }),
    );
    let f = Form::measurement(FormKind::Transfer, Some(&o), &[], &names(&o), &[]);
    let c = f.meas_config(Some(&o)).expect("tf");
    assert_eq!(c.name, "TF 1");
    // `ac2 meas new tf --ref 1 --meas 2`.
    assert_eq!(
        c.kind,
        MeasKind::Transfer {
            config: TransferConfig {
                reference_input: 0,
                measurement_input: 1,
                averaging: TfAveraging::Fifo { blocks: 8 },
                resolution: Resolution::FortyEighth,
                smoothing: None,
                depth: DepthPolicy::EqualConfidence,
            }
        }
    );
    let f = Form::measurement(FormKind::Spectrum, Some(&o), &[], &names(&o), &[]);
    let c = f.meas_config(Some(&o)).expect("spectrum");
    assert_eq!(
        c.kind,
        MeasKind::Spectrum {
            config: SpectrumConfig {
                input: 1,
                resolution: SpecResolution::Fixed { fft_len: 65_536 },
                window: Window::Hann,
                averaging: SpecAveraging::Off,
                smoothing: None,
                level: SpecLevel::Tone,
                display: Default::default(),
            }
        }
    );
    let mut f = Form::measurement(FormKind::Spectrum, Some(&o), &[], &names(&o), &[]);
    f.focus = f
        .fields
        .iter()
        .position(|x| x.id == FieldId::Smoothing)
        .expect("smoothing field");
    f.cycle(2);
    let MeasKind::Spectrum { config } = f.meas_config(Some(&o)).expect("spectrum").kind else {
        panic!("kind");
    };
    assert_eq!(config.smoothing, Some(SmoothingFraction::Sixth));
    let c = Form::measurement(FormKind::Rta, Some(&o), &[], &names(&o), &[])
        .meas_config(Some(&o))
        .expect("rta");
    assert_eq!(
        c.kind,
        MeasKind::Rta {
            config: RtaConfig {
                input: 1,
                fraction: BandFraction::Third,
                f_lo: Hz(20.0),
                f_hi: Hz(20_000.0),
                weighting: Weighting::Z,
                averaging: SpecAveraging::Off,
                display: Default::default(),
            }
        }
    );
    let c = Form::measurement(FormKind::Spl, Some(&o), &[], &names(&o), &[])
        .meas_config(Some(&o))
        .expect("spl");
    assert_eq!(
        c.kind,
        MeasKind::Spl {
            config: SplConfig {
                cue: None,
                bands: None,
                input: 1,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
                leq: ac2_proto::model::LeqConfig::default_windows(),
                position: None,
            }
        }
    );
}

#[test]
fn spectrum_and_rta_dialogs_set_averaging_and_edit_keeps_the_rest() {
    let o = open(vec![0, 1], None);
    let at = |f: &Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
    let mut f = Form::measurement(FormKind::Rta, Some(&o), &[], &names(&o), &[]);
    let i = at(&f, FieldId::Averaging);
    assert_eq!(f.fields[i].label, "Averaging");
    assert_eq!(f.fields[i].display(), "off");
    let Value::Choice { options, .. } = &f.fields[i].value else {
        panic!("choice");
    };
    assert_eq!(
        options,
        &[
            "off",
            "exp τ 0.5 s",
            "exp τ 1 s",
            "exp τ 2 s",
            "exp τ 5 s",
            "exp τ 10 s",
            "exp τ 30 s",
            "FIFO 16 frames",
            "FIFO 64 frames"
        ]
    );
    f.focus = i;
    f.cycle(3);
    assert_eq!(f.fields[i].display(), "exp τ 2 s");
    let c = f.meas_config(Some(&o)).expect("rta");
    let MeasKind::Rta { config } = c.kind else {
        panic!("kind");
    };
    assert_eq!(
        config.averaging,
        SpecAveraging::Exponential {
            time_constant: Seconds(2.0)
        }
    );
    // The spectrum dialog has the same control.
    let f = Form::measurement(FormKind::Spectrum, Some(&o), &[], &names(&o), &[]);
    assert_eq!(f.fields[at(&f, FieldId::Averaging)].display(), "off");

    // Editing an RTA made by the CLI with a FIFO no preset has: the field shows it,
    // and the RTA's range and weighting stay its own.
    let m = Measurement {
        id: MeasId(7),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "Room".into(),
            kind: MeasKind::Rta {
                config: RtaConfig {
                    f_lo: Hz(100.0),
                    weighting: Weighting::A,
                    averaging: SpecAveraging::Fifo { frames: 2400 },
                    ..RtaConfig::on_input(1, BandFraction::Sixth)
                },
            },
        },
        config_rev: ac2_proto::units::Rev(1),
        running: true,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
        eq: Vec::new(),
    };
    let mut f = Form::edit_spec(&m, &names(&o), Some(o.sample_rate_hz)).expect("edit");
    assert_eq!(f.kind.title(), "Edit RTA");
    assert_eq!(f.kind.verb(), "Apply");
    assert_eq!(f.edits(), Some(MeasId(7)));
    assert_eq!(f.fields[at(&f, FieldId::Fraction)].display(), "1/6 octave");
    let i = at(&f, FieldId::Averaging);
    assert_eq!(f.fields[i].display(), "FIFO 2400 frames");
    let unchanged = f.meas_config(Some(&o)).expect("rta");
    assert_eq!(unchanged, m.config);
    f.focus = i;
    f.cycle(-20);
    let MeasKind::Rta { config } = f.meas_config(Some(&o)).expect("rta").kind else {
        panic!("kind");
    };
    assert_eq!(config.averaging, SpecAveraging::Off);
    assert_eq!((config.f_lo, config.weighting), (Hz(100.0), Weighting::A));
    // An input the session no longer captures is not swapped for another.
    let mut gone = m.clone();
    if let MeasKind::Rta { config } = &mut gone.config.kind {
        config.input = 5;
    }
    let f = Form::edit_spec(&gone, &names(&o), Some(o.sample_rate_hz)).expect("edit");
    assert_eq!(
        f.fields[at(&f, FieldId::Input)].display(),
        "In 6 (not captured)"
    );
    let e = f.meas_config(Some(&o)).expect_err("refused");
    assert!(e.contains("not captured by the audio I/O"), "{e}");
}

#[test]
fn a_spectrums_fft_size_and_window_are_editable() {
    let o = open(vec![0, 1], None);
    let at = |f: &Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
    // New: the default length and window, the resolution's hint at the session's rate.
    let f = Form::measurement(FormKind::Spectrum, Some(&o), &[], &names(&o), &[]);
    let size = &f.fields[at(&f, FieldId::FftSize)];
    assert_eq!(size.display(), "65536 pt");
    assert_eq!(size.hint, "0.73 Hz bins · 1365 ms window · 5.9 updates/s");
    assert_eq!(f.fields[at(&f, FieldId::Window)].display(), "Hann");

    let m = Measurement {
        id: MeasId(2),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "Mic".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig {
                    resolution: SpecResolution::Fixed { fft_len: 4096 },
                    window: Window::FlatTop,
                    averaging: SpecAveraging::Fifo { frames: 8 },
                    ..SpectrumConfig::on_input(1)
                },
            },
        },
        config_rev: ac2_proto::units::Rev(1),
        running: true,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
        eq: Vec::new(),
    };
    let mut e = Form::edit_spec(&m, &names(&o), Some(48_000)).expect("edit");
    let i = at(&e, FieldId::FftSize);
    assert_eq!(e.fields[i].display(), "4096 pt");
    assert_eq!(
        e.fields[i].hint,
        "11.7 Hz bins · 85.3 ms window · 47 updates/s, 30 rows/s"
    );
    assert_eq!(e.fields[at(&e, FieldId::Window)].display(), "flat-top");
    assert_eq!(e.meas_config(Some(&o)).expect("same"), m.config);
    // A longer window: finer bins, fewer updates, the hint follows the field.
    e.focus = i;
    e.cycle(2);
    assert_eq!(e.fields[i].display(), "16384 pt");
    assert_eq!(
        e.fields[i].hint,
        "2.93 Hz bins · 341 ms window · 23 updates/s"
    );
    let w = at(&e, FieldId::Window);
    e.focus = w;
    e.cycle(-2);
    let MeasKind::Spectrum { config } = e.meas_config(Some(&o)).expect("spectrum").kind else {
        panic!("kind");
    };
    assert_eq!(
        (config.resolution, config.window),
        (SpecResolution::Fixed { fft_len: 16_384 }, Window::Hann)
    );
    assert_eq!(config.averaging, SpecAveraging::Fifo { frames: 8 });
    // The full range, and no hint without a session rate.
    e.focus = i;
    e.cycle(-20);
    assert_eq!(e.fields[i].display(), "64 pt");
    // Past the longest FFT: the multi-resolution bands, on each column grid.
    e.cycle(11);
    assert_eq!(e.fields[i].display(), "multi-res 1/12 oct");
    e.cycle(2);
    assert_eq!(e.fields[i].display(), "multi-res 1/48 oct");
    assert_eq!(e.fields[i].hint, ac2_scene::format::MULTIRES_HINT);
    let MeasKind::Spectrum { config } = e.meas_config(Some(&o)).expect("spectrum").kind else {
        panic!("kind");
    };
    assert_eq!(config.resolution, SpecResolution::MULTI);
    let e = Form::edit_spec(&m, &names(&o), None).expect("edit");
    assert_eq!(e.fields[at(&e, FieldId::FftSize)].hint, "");
}

#[test]
fn inputs_are_picked_by_name_and_mics_come_first() {
    let o = open(
        vec![0, 1, 2, 3],
        Some(LoopbackRoute {
            output: 0,
            input: 2,
        }),
    );
    // The loopback is the reference, the first named mic the measurement.
    let f = Form::measurement(FormKind::Transfer, Some(&o), &[], &names(&o), &[3]);
    assert_eq!(f.channel(FieldId::Reference), Some(2));
    assert_eq!(f.channel(FieldId::Measurement), Some(3));
    let at = |f: &Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
    assert_eq!(f.fields[at(&f, FieldId::Measurement)].display(), "In 4");
    // ←/→ walks the captured inputs by name and stops at the ends.
    let mut f = f;
    f.focus = at(&f, FieldId::Measurement);
    f.cycle(1);
    assert_eq!(f.channel(FieldId::Measurement), Some(3));
    f.cycle(-3);
    assert_eq!(f.channel(FieldId::Measurement), Some(0));
    assert_eq!(f.fields[f.focus].display(), "In 1");
    f.cycle(-1);
    assert_eq!(f.channel(FieldId::Measurement), Some(0));
    // The same input twice is refused in plain words.
    f.cycle(1);
    f.cycle(1);
    assert_eq!(f.channel(FieldId::Measurement), Some(2));
    let e = f.meas_config(Some(&o)).expect_err("same input");
    assert!(e.contains("same input"), "{e}");
}

#[test]
fn measurement_choices() {
    let o = open(vec![0, 1], None);
    let mut f = Form::measurement(FormKind::Transfer, Some(&o), &[], &names(&o), &[]);
    let at = |f: &Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
    f.focus = at(&f, FieldId::Smoothing);
    f.cycle(2);
    f.focus = at(&f, FieldId::Depth);
    f.cycle(1);
    f.focus = at(&f, FieldId::Name);
    f.backspace();
    f.type_text("main");
    let c = f.meas_config(Some(&o)).expect("tf");
    assert_eq!(c.name, "TF main");
    let MeasKind::Transfer { config } = c.kind else {
        panic!("kind");
    };
    // Phase is smoothed with the magnitude.
    assert_eq!(
        config.smoothing,
        Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::MagnitudePhase
        })
    );
    assert_eq!(
        config.depth,
        DepthPolicy::FastLf {
            max_settle_s: Seconds(1.0)
        }
    );
    assert!(f.set_channel(FieldId::Measurement, 0));
    assert!(f.meas_config(Some(&o)).is_err());

    let mut f = Form::measurement(FormKind::Spl, Some(&o), &[], &names(&o), &[]);
    // A by default; → steps on to C.
    f.focus = at(&f, FieldId::Weighting);
    f.cycle(1);
    f.focus = at(&f, FieldId::TimeWeighting);
    // ← at the first choice stays there; → steps on and stops at the last.
    f.cycle(-1);
    assert_eq!(f.fields[f.focus].display(), "fast (125 ms)");
    f.cycle(5);
    let MeasKind::Spl { config } = f.meas_config(Some(&o)).expect("spl").kind else {
        panic!("kind");
    };
    assert_eq!(config.weighting, Weighting::C);
    assert_eq!(config.time_weighting, TimeWeighting::Impulse);
    // Typing into a choice does nothing; focus wraps.
    f.type_text("x");
    f.move_focus(1);
    assert_eq!(f.focus, 0);
    f.move_focus(-1);
    assert_eq!(f.focus, f.fields.len() - 1);
}

/// The settings of the sweep measurement the dialog makes.
fn swept(f: &Form, o: &OpenSession) -> Result<SweepConfig, String> {
    match f.sweep_meas(Some(o), None)?.kind {
        MeasKind::Sweep { config } => Ok(config),
        _ => Err("not a sweep".into()),
    }
}

fn outs() -> Vec<(u16, String)> {
    vec![(0, "Out 1".into()), (1, "Out 2".into())]
}

fn focus(f: &mut Form, id: FieldId) {
    let i = f.fields.iter().position(|x| x.id == id).expect("field");
    f.focus_field(i);
}

/// Duration and repeats step in order from the 3 s / 1× defaults and stop at the ends:
/// 6 s is one → away, and holding a key never wraps to the other end.
/// Transfer and sweep dialogs have a Resolution choice (1/48 by default); editing a
/// transfer measurement shows its settings and keeps what the dialog does not show.
#[test]
fn resolution_in_the_transfer_and_sweep_dialogs() {
    let o = open(
        vec![0, 1],
        Some(LoopbackRoute {
            output: 1,
            input: 0,
        }),
    );
    let at = |f: &Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
    let mut f = Form::measurement(FormKind::Transfer, Some(&o), &[], &names(&o), &[]);
    let i = at(&f, FieldId::Resolution);
    assert_eq!(f.fields[i].label, "Resolution");
    assert_eq!(f.fields[i].display(), "1/48 oct");
    assert_eq!(f.fields[i].hint, RESOLUTION_HINT);
    f.focus = i;
    f.cycle(1);
    let c = f.meas_config(Some(&o)).expect("tf");
    let MeasKind::Transfer { config } = &c.kind else {
        panic!("kind");
    };
    assert_eq!(config.resolution, Resolution::NinetySixth);
    let m = Measurement {
        id: MeasId(3),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "Main".into(),
            kind: MeasKind::Transfer {
                config: TransferConfig {
                    averaging: TfAveraging::Fifo { blocks: 32 },
                    resolution: Resolution::TwentyFourth,
                    depth: DepthPolicy::FastLf {
                        max_settle_s: Seconds(2.5),
                    },
                    ..config.clone()
                },
            },
        },
        config_rev: ac2_proto::units::Rev(1),
        running: true,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
        eq: Vec::new(),
    };
    let mut e = Form::edit_spec(&m, &names(&o), Some(o.sample_rate_hz)).expect("edit");
    assert_eq!(e.kind, FormKind::TransferEdit);
    assert_eq!(e.kind.verb(), "Apply");
    assert_eq!(e.edits(), Some(MeasId(3)));
    let i = at(&e, FieldId::Resolution);
    assert_eq!(e.fields[i].display(), "1/24 oct");
    assert_eq!(e.meas_config(Some(&o)).expect("same"), m.config);
    e.focus = i;
    e.cycle(-1);
    let MeasKind::Transfer { config: edited } = e.meas_config(Some(&o)).expect("tf").kind else {
        panic!("kind");
    };
    assert_eq!(edited.resolution, Resolution::Twelfth);
    assert_eq!(edited.averaging, TfAveraging::Fifo { blocks: 32 });

    let mut s = Form::sweep(Some(&o), 0, &names(&o), &outs(), &[], Some(Dbfs(-50.0)));
    assert_eq!(
        swept(&s, &o).expect("sweep").resolution,
        Resolution::FortyEighth
    );
    focus(&mut s, FieldId::Resolution);
    s.cycle(1);
    let c = swept(&s, &o).expect("sweep");
    assert_eq!(c.resolution, Resolution::NinetySixth);
    let sm = Measurement {
        id: MeasId(4),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "Sweep 1".into(),
            kind: MeasKind::Sweep { config: c },
        },
        config_rev: ac2_proto::units::Rev(1),
        running: false,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
        eq: Vec::new(),
    };
    let se = Form::edit_sweep(&sm, Some(&o), &names(&o), &outs()).expect("edit");
    assert_eq!(
        se.fields[at(&se, FieldId::Resolution)].display(),
        "1/96 oct"
    );
}

#[test]
fn sweep_steppers_are_sorted_and_stop_at_the_ends() {
    let o = open(
        vec![0, 1],
        Some(LoopbackRoute {
            output: 1,
            input: 0,
        }),
    );
    let mut f = Form::sweep(Some(&o), 0, &names(&o), &outs(), &[], Some(Dbfs(-50.0)));
    let duration = |f: &Form| swept(f, &o).expect("plan").sweep;
    let repeats = |f: &Form| swept(f, &o).expect("plan").repeats;
    assert_eq!(duration(&f).duration, Seconds(3.0));
    assert_eq!(repeats(&f), 1);
    focus(&mut f, FieldId::Duration);
    f.cycle(1);
    assert_eq!(duration(&f).duration, Seconds(6.0));
    f.cycle(1);
    f.cycle(1);
    assert_eq!(duration(&f).duration, Seconds(12.0));
    for _ in 0..3 {
        f.cycle(-1);
    }
    assert_eq!(duration(&f).duration, Seconds(1.0));
    f.cycle(-1);
    assert_eq!(duration(&f).duration, Seconds(1.0));
    let shown: Vec<f64> = DURATIONS.iter().map(|d| d.1).collect();
    assert!(shown.is_sorted(), "{shown:?}");

    focus(&mut f, FieldId::Repeats);
    f.cycle(-1);
    assert_eq!(repeats(&f), 1);
    for want in [2, 4, 8, 8] {
        f.cycle(1);
        assert_eq!(repeats(&f), want);
    }
}

/// Without a session loopback nothing guesses the reference: the dialog asks for it and
/// refuses to arm until it is chosen. With one, the loopback input is the default.
#[test]
fn sweep_reference_is_the_loopback_or_chosen() {
    let o = open(vec![0, 1, 2], None);
    let mut f = Form::sweep(Some(&o), 0, &names(&o), &outs(), &[2], Some(Dbfs(-50.0)));
    assert_eq!(f.channel(FieldId::Reference), None);
    assert_eq!(f.fields[0].display(), "choose the reference");
    let e = swept(&f, &o).expect_err("no reference");
    assert!(e.contains("choose the reference"), "{e}");
    // → takes the first input, ← from nothing the last.
    f.focus_field(0);
    f.cycle(-1);
    assert_eq!(f.channel(FieldId::Reference), Some(2));
    f.cycle(-1);
    assert_eq!(f.channel(FieldId::Reference), Some(1));
    let p = swept(&f, &o).expect("plan");
    assert_eq!((p.reference_input, p.measurement_input), (1, 2));

    let o = open(
        vec![0, 1, 2],
        Some(LoopbackRoute {
            output: 1,
            input: 2,
        }),
    );
    let f = Form::sweep(Some(&o), 0, &names(&o), &outs(), &[1], Some(Dbfs(-50.0)));
    assert_eq!(f.channel(FieldId::Reference), Some(2));
    assert_eq!(f.channel(FieldId::Measurement), Some(1));
    assert!(swept(&f, &o).is_ok());
}

/// A text field gets its text selected with the focus (and by Ctrl+A): typing replaces
/// the default name or level instead of appending to it.
#[test]
fn typing_replaces_a_selected_default() {
    let o = open(
        vec![0, 1],
        Some(LoopbackRoute {
            output: 1,
            input: 0,
        }),
    );
    let mut f = Form::sweep(Some(&o), 2, &names(&o), &outs(), &[], Some(Dbfs(-50.0)));
    assert_eq!(f.text(FieldId::Name), "Sweep 3");
    focus(&mut f, FieldId::Name);
    assert!(f.selected);
    f.type_text("1083 ");
    f.type_text("on axis");
    assert_eq!(f.text(FieldId::Name), "1083 on axis");
    // Ctrl+A then Backspace clears it; plain Backspace takes one character.
    f.select_all();
    f.backspace();
    assert_eq!(f.text(FieldId::Name), "");
    f.type_text("ab");
    f.backspace();
    assert_eq!(f.text(FieldId::Name), "a");

    focus(&mut f, FieldId::Level);
    assert_eq!(f.text(FieldId::Level), "-50.0");
    f.type_text("-40");
    assert_eq!(f.text(FieldId::Level), "-40");
    // A choice is never "selected": Ctrl+A there changes nothing.
    focus(&mut f, FieldId::Duration);
    assert!(!f.selected);
    f.select_all();
    assert!(!f.selected);
}
