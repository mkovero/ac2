//! Reducer tests of the smoothing keys: on the pane's measurement, on an RTA, on a selected slot.

use super::*;

#[test]
fn smoothing_keys_step_the_pane_measurement() {
    let mut t = T::new();
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("smoothing off")
    );
    // K coarser: off → 1/48 of magnitude and phase, with the measurement's config
    // otherwise unchanged.
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "m1".into(),
            smoothing(
                SmoothingFraction::FortyEighth,
                SmoothingMode::MagnitudePhase
            )
        )
    );
    match r.as_slice() {
        [
            Request::Call {
                cmd: Command::MeasUpdate { config, .. },
                what,
            },
        ] => {
            assert_eq!(what, "Main L: smoothing 1/48 oct");
            let MeasKind::Transfer { config } = &config.kind else {
                unreachable!()
            };
            let MeasKind::Transfer { config: was } = transfer() else {
                unreachable!()
            };
            assert_eq!(config.averaging, was.averaging);
            assert_eq!(config.resolution, was.resolution);
        }
        other => panic!("{other:?}"),
    }
    // Finer than off: nothing sent, said.
    assert!(t.key("Shift+K").is_empty());
    assert!(t.last_toast().contains("finest"), "{}", t.last_toast());

    // The mirror says 1/6 magnitude only (set by a script): K → 1/3 (mode kept),
    // Shift+K → 1/12, at 1/3 K stops.
    let mut s = daemon_state();
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Sixth, SmoothingMode::Magnitude);
    }
    t.conn(mirror(s.clone()));
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("smoothing 1/6 oct mag only")
    );
    assert_eq!(
        smoothing_set(&t.key("K")).1,
        smoothing(SmoothingFraction::Third, SmoothingMode::Magnitude)
    );
    assert_eq!(
        smoothing_set(&t.key("Shift+K")).1,
        smoothing(SmoothingFraction::Twelfth, SmoothingMode::Magnitude)
    );
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Third, SmoothingMode::Magnitude);
    }
    t.conn(mirror(s));
    assert!(t.key("K").is_empty());
    assert!(t.last_toast().contains("widest"), "{}", t.last_toast());
    // Palette entries set a step directly.
    let r = t.st.update(Msg::Command(CommandId::SmoothOff), &t.keys);
    assert_eq!(smoothing_set(&r).1, None);
    let r = t.st.update(Msg::Command(CommandId::Smooth24), &t.keys);
    assert_eq!(
        smoothing_set(&r).1,
        smoothing(SmoothingFraction::TwentyFourth, SmoothingMode::Magnitude)
    );
    // In the spectrum pane K acts on its spectrum: power smoothing, no phase to name.
    t.key("Alt+2");
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Spectrum))
            .as_deref(),
        Some("smoothing off")
    );
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "m2".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Magnitude)
        )
    );
    match r.as_slice() {
        [Request::Call { what, .. }] => assert_eq!(what, "Sub: smoothing 1/48 oct"),
        other => panic!("{other:?}"),
    }
    // The transfer pane's caption still speaks of its own measurement.
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("smoothing 1/3 oct mag only")
    );
}

/// RTA bands are fractional-octave already: K in the spectrum pane on an RTA says so and
/// sends nothing.
#[test]
fn smoothing_keys_explain_rta() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements[0].config.kind = MeasKind::Rta {
        config: ac2_proto::model::RtaConfig::on_input(1, ac2_proto::model::BandFraction::Third),
    };
    t.conn(mirror(s));
    t.key("Alt+2");
    assert_eq!(t.st.smoothing_caption(t.pane(PaneKind::Spectrum)), None);
    assert!(t.key("K").is_empty());
    assert!(
        t.last_toast().contains("already are fractional-octave"),
        "{}",
        t.last_toast()
    );
}

#[test]
fn smoothing_keys_change_a_selected_slot() {
    let mut t = T::new();
    let mut spec = stored(11, Some(4), 2);
    spec.kind = TraceKind::Spectrum {
        scale: LevelScale::Dbfs,
        level: ac2_proto::model::SpecLevel::Tone,
    };
    let mut locked = stored(12, Some(5), 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![stored(10, Some(3), 2), spec, locked]));
    // A click on slot 3 selects it: K changes its trace, the caption says so.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("slot 3 (t10): smoothing off")
    );
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "t10".into(),
            smoothing(
                SmoothingFraction::FortyEighth,
                SmoothingMode::MagnitudePhase
            )
        )
    );
    match r.as_slice() {
        [
            Request::Call {
                cmd: Command::TraceUpdate { edit, .. },
                what,
            },
        ] => {
            assert_eq!(what, "slot 3 (t10): smoothing 1/48 oct");
            // Its other edits are sent unchanged.
            let mut want = stored(10, Some(3), 2).edit;
            want.smoothing = edit.smoothing;
            assert_eq!(*edit, want);
        }
        other => panic!("{other:?}"),
    }
    // A spectrum slot is power-smoothed; its caption goes to the spectrum pane.
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Spectrum))
            .as_deref(),
        Some("slot 4 (t11): smoothing off")
    );
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("smoothing off")
    );
    assert_eq!(
        smoothing_set(&t.key("K")),
        (
            "t11".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Magnitude)
        )
    );
    // A locked slot says so and sends nothing.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key("K").is_empty());
    assert!(t.last_toast().contains("locked"));
    // A second click deselects; the keys act on the pane's measurement again.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert_eq!(t.st.selected_trace, None);
    assert_eq!(smoothing_set(&t.key("K")).0, "m1");
    // A pane click keeps the slot selected; a measurement's row deselects it.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.st.update(Msg::FocusPane(t.pane(PaneKind::Transfer)), &t.keys);
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert_eq!(t.st.selected_trace, None);
    // A selected trace that goes away is forgotten.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.conn(with_traces(vec![]));
    assert_eq!(t.st.selected_trace, None);
}
