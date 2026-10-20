//! Reducer tests of slots, averages of stored traces, math channels and measurement edits.

use super::*;

#[test]
fn slots_capture_and_replace() {
    let mut t = T::new();
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            meas: MeasId(1),
            slot: 3,
            replace: None,
            name,
        }] if name == "Main L S3"
    ));
    // Slots are the daemon's: a mirrored trace in slot 3 is what Ctrl+3 replaces.
    t.conn(with_traces(vec![stored(9, Some(3), 2)]));
    assert_eq!(t.st.slots()[2].map(|t| t.id), Some(TraceId(9)));
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            slot: 3,
            replace: Some(TraceId(9)),
            ..
        }]
    ));
    // A locked trace is not deleted; it only gives up the slot.
    let mut locked = stored(9, Some(3), 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![locked]));
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            slot: 3,
            replace: None,
            ..
        }]
    ));
    // Spectrum measurements capture too.
    t.st.selected = Some(MeasId(2));
    let r = t.key("Ctrl+1");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            meas: MeasId(2),
            slot: 1,
            ..
        }]
    ));
    // A trace deleted daemon-side frees its slot.
    t.conn(mirror(daemon_state()));
    assert!(t.st.slots()[2].is_none());
}

#[test]
fn digits_show_and_hide_slots_alt_digits_focus_panes() {
    let mut t = T::new();
    t.conn(with_traces(vec![stored(9, Some(3), 2)]));
    let r = t.key("3");
    let [
        Request::Call {
            cmd: Command::TraceUpdate { trace, edit },
            what,
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    assert_eq!(*trace, TraceId(9));
    assert!(!edit.visible);
    assert_eq!(edit.slot, Some(3));
    assert_eq!(what, "slot 3 (t9) hidden");
    // The pane did not change focus.
    assert_eq!(t.focus_kind(), PaneKind::Transfer);
    // An empty slot says how to fill it.
    assert!(t.key("5").is_empty());
    assert!(t.last_toast().contains("slot 5 is empty"));
    t.key("Alt+2");
    assert_eq!(t.focus_kind(), PaneKind::Spectrum);
    t.key("Alt+1");
    assert_eq!(t.focus_kind(), PaneKind::Transfer);
}

#[test]
fn m_averages_the_shown_stored_traces() {
    let mut t = T::new();
    // One shown trace is not enough.
    t.conn(with_traces(vec![captured(4, Some(1), 2)]));
    assert!(t.key("M").is_empty());
    assert!(
        t.st.toasts
            .last()
            .is_some_and(|x| x.severity == Severity::Warning)
    );
    let mut hidden = captured(6, Some(3), 2);
    hidden.edit.visible = false;
    let mut target = captured(7, None, 2);
    target.kind = TraceKind::Target;
    t.conn(with_traces(vec![
        captured(4, Some(2), 2),
        captured(5, Some(1), 2),
        hidden,
        target,
    ]));
    let r = t.key("M");
    let [
        Request::Call {
            cmd:
                Command::TraceAverage {
                    traces,
                    method,
                    reference,
                    name,
                },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    // Slot order, hidden and target traces left out, reference = the first.
    assert_eq!(traces, &[TraceId(5), TraceId(4)]);
    assert_eq!(*method, AverageMethod::Power);
    assert_eq!(*reference, DelayReference::Trace { trace: TraceId(5) });
    assert_eq!(name, "avg S1+S2");
    // The phase reference (decision 8b) wins when it is one of them.
    t.st.view.tf.phase_reference = Some(TraceKey::Stored(TraceId(4)));
    let r = t.key("M");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::TraceAverage {
                reference: DelayReference::Trace { trace: TraceId(4) },
                ..
            },
            ..
        }]
    ));
    // Complex averaging from the palette.
    t.key("Ctrl+K");
    t.text("average complex");
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::TraceAverage {
                method: AverageMethod::Complex,
                ..
            },
            ..
        }]
    ));
}

/// Shift+M opens the math channel dialog: A is what the pane shows, B the next of its kind
/// (live or stored), the operator ÷, the name the expression; Enter creates it.
#[test]
fn math_channel_dialog_by_name() {
    use crate::forms::{FieldId, FormKind};
    use ac2_proto::model::{MathDomain, MathExpr, MathOp, Operand};
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(4, Some(2), 2),
        stored(5, Some(7), 2),
    ]));
    t.key("Shift+M");
    assert!(matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::Math));
    assert_eq!(form(&t).fields[0].display(), "Main L (live)");
    assert_eq!(form(&t).text(FieldId::Name), "Main L ÷ t4");
    // B steps on to the other stored trace; the name follows.
    t.key("Down");
    t.key("Down");
    t.key("Right");
    assert_eq!(form(&t).text(FieldId::Name), "Main L ÷ t5");
    let r = t.key("Enter");
    let c = created(&r).unwrap_or_else(|| panic!("{r:?}"));
    assert_eq!(c.name, "Main L ÷ t5");
    let MeasKind::Math { config } = &c.kind else {
        panic!("{c:?}");
    };
    assert_eq!(config.domain, MathDomain::Transfer);
    assert_eq!(
        config.expr,
        MathExpr::Binary {
            a: Operand::Meas { meas: MeasId(1) },
            op: MathOp::Divide,
            b: Operand::Trace { trace: TraceId(5) },
        }
    );
    // From the palette too; with nothing of a kind to pair, it says what is missing.
    t.conn(with_traces(vec![]));
    t.key("Ctrl+K");
    t.text("new math");
    t.key("Enter");
    assert!(matches!(t.st.overlay, Overlay::None));
    assert!(
        t.last_toast()
            .contains("two transfer functions, spectra or RTAs"),
        "{}",
        t.last_toast()
    );
}

/// The selected math channel edits in the same dialog: Enter sends `meas.update` with its
/// name kept.
#[test]
fn math_channel_edit() {
    use crate::forms::FormKind;
    use ac2_proto::model::{MathConfig, MathDomain, MathExpr, MathOp, Operand};
    let mut t = T::new();
    let mut s = daemon_state();
    s.traces = vec![stored(4, Some(2), 2)];
    s.measurements.push(meas(
        3,
        "Prediction",
        MeasKind::Math {
            config: MathConfig::of(
                ac2_proto::model::TraceOwner::Imported,
                MathDomain::Transfer,
                MathExpr::Binary {
                    a: Operand::Meas { meas: MeasId(1) },
                    op: MathOp::Add,
                    b: Operand::Trace { trace: TraceId(4) },
                },
            ),
        },
    ));
    t.conn(mirror(s));
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    t.key("Ctrl+K");
    t.text("edit measurement");
    t.key("Enter");
    assert!(matches!(&t.st.overlay, Overlay::Form(f) if f.kind == FormKind::MathEdit));
    // Operator: + → − (complex difference).
    t.key("Down");
    t.key("Right");
    let r = t.key("Enter");
    let [
        Request::Call {
            cmd: Command::MeasUpdate { meas, config, .. },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    assert_eq!(*meas, MeasId(3));
    assert_eq!(config.name, "Prediction");
    let MeasKind::Math { config } = &config.kind else {
        panic!()
    };
    assert!(matches!(
        config.expr,
        MathExpr::Binary {
            op: MathOp::Subtract,
            ..
        }
    ));
}

#[test]
fn edit_meas_on_a_spectrum_changes_its_averaging_and_keeps_the_rest() {
    let mut t = T::new();
    t.conn(mirror(daemon_state()));
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    t.key("Ctrl+K");
    t.text("edit measurement");
    t.key("Enter");
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(f.kind, FormKind::SpectrumEdit);
    let shown: Vec<(String, String)> = f
        .fields
        .iter()
        .map(|x| (x.label.clone(), x.display()))
        .collect();
    assert_eq!(
        shown,
        [
            ("Input".to_owned(), "2 · Input 2".to_owned()),
            ("Name".to_owned(), "Sub".to_owned()),
            ("Smoothing".to_owned(), "off".to_owned()),
            ("FFT length".to_owned(), "8192 pt".to_owned()),
            ("Window".to_owned(), "Hann".to_owned()),
            ("Level".to_owned(), "tone (dBFS)".to_owned()),
            ("Averaging".to_owned(), "off".to_owned()),
        ]
    );
    // Averaging: off → exp τ 0.5 s → exp τ 1 s.
    for _ in 0..6 {
        t.key("Down");
    }
    t.key("Right");
    t.key("Right");
    let r = t.key("Enter");
    let [
        Request::Call {
            cmd: Command::MeasUpdate { meas, config, .. },
            what,
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    assert_eq!(*meas, MeasId(2));
    assert_eq!(what, "Sub changed");
    let MeasKind::Spectrum { config } = &config.kind else {
        panic!("{config:?}");
    };
    assert_eq!(
        config.averaging,
        SpecAveraging::Exponential {
            time_constant: Seconds(1.0)
        }
    );
    // What the dialog does not show stays the spectrum's own.
    assert_eq!(
        (config.resolution, config.input),
        (ac2_proto::model::SpecResolution::Fixed { fft_len: 8192 }, 1)
    );
}

#[test]
fn edit_meas_switches_a_spectrum_to_multi_resolution() {
    let mut t = T::new();
    t.conn(mirror(daemon_state()));
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    t.key("Ctrl+K");
    t.text("edit measurement");
    t.key("Enter");
    // Resolution: 8192 pt → … → the largest FFT → multi-res 1/48 oct.
    for _ in 0..3 {
        t.key("Down");
    }
    let field = |t: &T| {
        let Overlay::Form(f) = &t.st.overlay else {
            panic!("{:?}", t.st.overlay);
        };
        let x = &f.fields[f.focus];
        (x.label.clone(), x.display(), x.hint.clone())
    };
    assert_eq!(field(&t).0, "FFT length");
    for _ in 0..20 {
        if field(&t).1 == "multi-res 1/48 oct" {
            break;
        }
        t.key("Right");
    }
    let (_, shown, hint) = field(&t);
    assert_eq!(shown, "multi-res 1/48 oct");
    assert_eq!(hint, ac2_scene::format::MULTIRES_HINT);
    // Crossing to multi-resolution takes its default level, density.
    t.key("Down");
    t.key("Down");
    assert_eq!(field(&t).0, "Level");
    assert_eq!(field(&t).1, "density (dBFS/Hz)");
    let r = t.key("Enter");
    let [
        Request::Call {
            cmd: Command::MeasUpdate { config, .. },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    let MeasKind::Spectrum { config } = &config.kind else {
        panic!("{config:?}");
    };
    assert_eq!(
        config.resolution,
        ac2_proto::model::SpecResolution::Multi {
            grid: ac2_proto::model::Resolution::FortyEighth
        }
    );
    assert_eq!(config.averaging, SpecAveraging::Off);
    assert_eq!(config.level, ac2_proto::model::SpecLevel::Density);
}

#[test]
fn edit_meas_sets_a_fixed_spectrum_to_density() {
    let mut t = T::new();
    t.conn(mirror(daemon_state()));
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    t.key("Ctrl+K");
    t.text("edit measurement");
    t.key("Enter");
    for _ in 0..5 {
        t.key("Down");
    }
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(f.fields[f.focus].label, "Level");
    t.key("Right");
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(f.fields[f.focus].display(), "density (dBFS/Hz)");
    assert!(
        f.fields[f.focus].hint.contains("per hertz"),
        "{:?}",
        f.fields[f.focus].hint
    );
    let r = t.key("Enter");
    let [
        Request::Call {
            cmd: Command::MeasUpdate { config, .. },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    let MeasKind::Spectrum { config } = &config.kind else {
        panic!("{config:?}");
    };
    assert_eq!(config.level, ac2_proto::model::SpecLevel::Density);
    assert_eq!(
        config.resolution,
        ac2_proto::model::SpecResolution::Fixed { fft_len: 8192 }
    );
}
