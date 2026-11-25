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
    let r = t.key_acked("K");
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
    assert!(t.key_acked("Shift+K").is_empty());
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
        smoothing_set(&t.key_acked("K")).1,
        smoothing(SmoothingFraction::Third, SmoothingMode::Magnitude)
    );
    assert_eq!(
        smoothing_set(&t.key_acked("Shift+K")).1,
        smoothing(SmoothingFraction::Twelfth, SmoothingMode::Magnitude)
    );
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Third, SmoothingMode::Magnitude);
    }
    t.conn(mirror(s));
    assert!(t.key_acked("K").is_empty());
    assert!(t.last_toast().contains("widest"), "{}", t.last_toast());
    // Palette entries set a step directly.
    let r = t.st.apply(Msg::Command(CommandId::SmoothOff), &t.keys);
    assert_eq!(smoothing_set(&r).1, None);
    t.ack(&r);
    let r = t.st.apply(Msg::Command(CommandId::Smooth24), &t.keys);
    assert_eq!(
        smoothing_set(&r).1,
        smoothing(SmoothingFraction::TwentyFourth, SmoothingMode::Magnitude)
    );
    t.ack(&r);
    // In the spectrum pane K acts on its spectrum: power smoothing, no phase to name.
    t.key_acked("Alt+2");
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Spectrum))
            .as_deref(),
        Some("smoothing off")
    );
    let r = t.key_acked("K");
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
    // Sub's: the spectrum pane draws it.
    spec.edit.owner = ac2_proto::model::TraceOwner::Meas { meas: MeasId(2) };
    let mut locked = captured(12, Some(5), 2);
    locked.edit.locked = true;
    // Main L's capture: the transfer pane draws it, so its title names it.
    let mut slot = stored(10, Some(3), 2);
    slot.edit.owner = ac2_proto::model::TraceOwner::Meas { meas: MeasId(1) };
    t.conn(with_traces(vec![slot, spec, locked]));
    // A click on slot 3 selects it: K changes its trace, the caption says so.
    t.st.apply(Msg::SelectTrace(TraceId(10)), &t.keys);
    assert_eq!(t.st.rig.selected_trace, Some(TraceId(10)));
    assert_eq!(
        t.st.smoothing_caption(t.pane(PaneKind::Transfer))
            .as_deref(),
        Some("slot 3 (t10): smoothing off")
    );
    let r = t.key_acked("K");
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
            want.owner = ac2_proto::model::TraceOwner::Meas { meas: MeasId(1) };
            assert_eq!(*edit, want);
        }
        other => panic!("{other:?}"),
    }
    // A spectrum slot is power-smoothed; its caption goes to the spectrum pane.
    t.st.apply(Msg::SelectTrace(TraceId(11)), &t.keys);
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
    // The transfer pane does not draw it: K there stays with the pane's measurement.
    assert_eq!(smoothing_set(&t.key_acked("K")).0, "m1");
    t.key_acked("Alt+2");
    assert_eq!(
        smoothing_set(&t.key_acked("K")),
        (
            "t11".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Magnitude)
        )
    );
    t.key_acked("Alt+1");
    // A locked slot says so and sends nothing.
    t.st.apply(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key_acked("K").is_empty());
    assert!(t.last_toast().contains("locked"));
    // A second click deselects; the keys act on the pane's measurement again.
    t.st.apply(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert_eq!(t.st.rig.selected_trace, None);
    assert_eq!(smoothing_set(&t.key_acked("K")).0, "m1");
    // A pane click keeps the slot selected; a measurement's row deselects it.
    t.st.apply(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.st.apply(Msg::FocusPane(t.pane(PaneKind::Transfer)), &t.keys);
    assert_eq!(t.st.rig.selected_trace, Some(TraceId(10)));
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert_eq!(t.st.rig.selected_trace, None);
    // A selected trace that goes away is forgotten.
    t.st.apply(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.conn(with_traces(vec![]));
    assert_eq!(t.st.rig.selected_trace, None);
}

/// K acts on what the focused pane shows: a transfer trace selected in the list is not on
/// the spectrum pane, so K there changes the spectrum's smoothing and leaves the trace; on
/// a pane with no curve to smooth it changes nothing anywhere.
#[test]
fn smoothing_keys_stay_with_the_focused_pane() {
    let mut t = T::new();
    t.conn(with_traces(vec![captured(10, Some(3), 2)]));
    t.st.apply(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.key("Alt+2");
    assert_eq!(t.focus_kind(), PaneKind::Spectrum);
    assert_eq!(t.st.rig.selected_trace, Some(TraceId(10)));
    assert_eq!(
        smoothing_set(&t.key("K")),
        (
            "m2".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Magnitude)
        )
    );
    // Back on the transfer pane, which draws the trace: K is the trace's.
    t.key("Alt+1");
    assert_eq!(smoothing_set(&t.key("K")).0, "t10");
    // An imported trace is on no pane: selected, it leaves K with the pane's measurement.
    t.conn(with_traces(vec![stored(13, None, 2)]));
    t.st.apply(Msg::SelectTrace(TraceId(13)), &t.keys);
    assert_eq!(smoothing_set(&t.key("K")).0, "m1");
    // The SPL pane has nothing to smooth: the command from the palette says so, and
    // reaches for no other pane's curve.
    t.key("Alt+4");
    let r =
        t.st.apply(Msg::Command(CommandId::SmoothCoarser), &t.keys.clone());
    assert!(r.is_empty(), "{r:?}");
    assert!(
        t.last_toast()
            .contains("no transfer or spectrum curve to smooth"),
        "{}",
        t.last_toast()
    );
}

/// Alt+K / Alt+Shift+K step the focused transfer pane's measurement through the
/// resolutions and stop at the ends; another pane has no resolution to set.
#[test]
fn resolution_keys_step_the_transfer_measurement() {
    use ac2_proto::model::{Resolution, TransferResolution};
    fn sent(r: &[Request]) -> TransferResolution {
        match r {
            [
                Request::Call {
                    cmd: Command::MeasUpdate { config, .. },
                    ..
                },
            ] => match &config.kind {
                MeasKind::Transfer { config } => config.resolution,
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }
    let mut t = T::new();
    // The measurement is at the default, 1/48: one step each way.
    assert_eq!(sent(&t.key_acked("Alt+K")), Resolution::TwentyFourth.into());
    assert_eq!(
        sent(&t.key_acked("Alt+Shift+K")),
        Resolution::NinetySixth.into()
    );
    // Finer than 1/96: the analysis's own bins, the finest step.
    t.key_answered("Alt+Shift+K");
    assert_eq!(
        sent(&t.key_answered("Alt+Shift+K")),
        TransferResolution::Bins
    );
    assert_eq!(
        t.last_toast(),
        "Main L: resolution bins (averaging starts over)"
    );
    assert!(t.key_acked("Alt+Shift+K").is_empty());
    assert_eq!(t.last_toast(), "Main L: bins is the finest resolution");
    assert_eq!(
        sent(&t.key_answered("Alt+K")),
        Resolution::NinetySixth.into()
    );
    for _ in 0..3 {
        t.key_answered("Alt+K");
    }
    assert!(t.key_acked("Alt+K").is_empty());
    assert_eq!(
        t.last_toast(),
        "Main L: 1/12 oct is the coarsest resolution"
    );
    // The SPL pane has no transfer measurement.
    t.key_acked("Alt+4");
    let r =
        t.st.apply(Msg::Command(CommandId::ResolutionCoarser), &t.keys.clone());
    assert!(r.is_empty(), "{r:?}");
    assert!(
        t.last_toast().contains("no transfer measurement"),
        "{}",
        t.last_toast()
    );
}
