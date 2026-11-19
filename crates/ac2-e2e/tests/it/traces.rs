//! Stored traces against the rig: captures in slots, a target curve from a file, moved and
//! compared; a capture re-smoothed by the daemon; smoothed live curves; the measurement tree
//! and what Delete on a measurement that owns traces asks.

use crate::harness::{Driver, R, connected, noise, on_rig};
use crate::window::{DELAY_TOWER, MAIN_L, MIC_FFT, Rig};
use ac2_proto::model::{SmoothingFraction, TraceOwner};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::TraceId;
use ac2_scene::theme::Theme;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, PaneKind};
use std::time::Instant;

fn now() -> ac2_ui::scenes::Now {
    ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    }
}

const SIZE: ac2_scene::primitives::Viewport = ac2_scene::primitives::Viewport::new(1000.0, 450.0);

/// The transfer pane's legend rows.
fn legend(s: &AppState) -> Vec<String> {
    ac2_ui::scenes::transfer(
        s,
        crate::common::pane(s, PaneKind::Transfer),
        &Theme::dark(),
        SIZE,
        now(),
    )
    .legend
    .iter()
    .map(|e| e.text.clone())
    .collect()
}

/// Slot `n` (1…9) holds a trace and its data is fetched. The mirror lists a capture before
/// the app's own fetch of its columns returns, so a slot alone says nothing of the data.
fn slot_with_data(s: &AppState, n: usize) -> bool {
    s.slots()[n - 1].is_some_and(|t| s.rig.traces.contains_key(&t.id))
}

/// Z, the chooser's first row (a target curve from a file), the house curve's path typed:
/// one more stored trace, its data fetched, the dialog closed.
fn import_target(d: &mut Driver) -> R {
    // The new trace is told by its id, not by a count of fetched data: an earlier
    // capture's fetch may land before or with the target's, in any order.
    let before: Vec<TraceId> = d.st.stored_traces().iter().map(|t| t.id).collect();
    d.key("Z");
    assert!(
        matches!(d.st.overlay, Overlay::Choose(_)),
        "{:?}",
        d.st.overlay
    );
    d.key("Enter");
    assert!(
        matches!(d.st.overlay, Overlay::Prompt(_)),
        "{:?}",
        d.st.overlay
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ac2-traces/tests/fixtures/house_curve.txt");
    d.send(Msg::Text(path.to_string_lossy().into_owned()));
    d.key("Enter");
    d.until("the target stored with its data", |s| {
        s.overlay == Overlay::None
            && s.stored_traces()
                .iter()
                .any(|t| !before.contains(&t.id) && s.rig.traces.contains_key(&t.id))
    })
}

/// The id of the stored trace named `name`.
fn trace_named(s: &AppState, name: &str) -> Option<TraceId> {
    s.rig
        .traces
        .values()
        .find(|(t, _)| t.meta.edit.name == name)
        .map(|(t, _)| t.meta.id)
}

/// Delay tower selected and dropped on the transfer pane: what it shows from then on.
fn delay_tower_on_screen(d: &mut Driver) {
    let tf = crate::common::pane(&d.st, PaneKind::Transfer);
    d.send(Msg::SelectMeas(DELAY_TOWER));
    d.drop_meas(tf, DELAY_TOWER);
}

/// Main L captured into slot 1, Delay tower into slot 2, and the house curve imported while
/// Delay tower is on screen (it files under it). 2 hides slot 2 and shows it again; the
/// target moved under Main L. With Main L's group and its capture and the target compared
/// in Delay tower's pane, the legend says the capture shares the epoch's time base (Δt to
/// the reference) and the target is independent.
#[test]
fn stored_traces_and_a_target_moved_and_compared() -> R {
    let rig = Rig::start()?;
    let _noise = noise(&rig.cli)?;
    let mut d = on_rig(&rig)?;
    d.key("Ctrl+1");
    d.until("slot 1 with its data", |s| slot_with_data(s, 1))?;
    delay_tower_on_screen(&mut d);
    d.key("Ctrl+2");
    d.until("slot 2 with its data", |s| slot_with_data(s, 2))?;
    import_target(&mut d)?;
    assert_eq!(d.st.rig.traces.len(), 3);
    let target = trace_named(&d.st, "house_curve").ok_or("the target")?;

    d.key("2");
    d.until("slot 2 hidden", |s| {
        s.slots()[1].is_some_and(|t| !t.edit.visible)
    })?;
    d.key("2");
    d.until("slot 2 shown, every trace visible", |s| {
        s.slots()[1].is_some_and(|t| t.edit.visible)
            && s.rig.traces.values().all(|(t, _)| t.meta.edit.visible)
    })?;

    // The target moved under Main L (the move dialog's row of Main L).
    d.send(Msg::SelectTrace(target));
    d.send(Msg::Command(CommandId::MoveTrace));
    let main_l = TraceOwner::Meas { meas: MAIN_L };
    {
        let Overlay::Choose(c) = &mut d.st.overlay else {
            return Err(format!("no move dialog: {:?}", d.st.overlay).into());
        };
        c.index = c
            .owners
            .iter()
            .position(|o| *o == main_l)
            .ok_or("Main L among the owners")?;
    }
    d.key("Enter");
    d.until("the target under Main L", |s| {
        s.rig
            .traces
            .get(&target)
            .is_some_and(|(t, _)| t.meta.edit.owner == main_l)
    })?;
    // A click on its row again: nothing selected.
    d.send(Msg::SelectTrace(target));

    {
        use ac2_proto::model::layout::EntryRef;
        let mut with = vec![EntryRef::Group { meas: MAIN_L }];
        with.extend(
            d.st.rig
                .traces
                .values()
                .filter(|(t, _)| !t.meta.edit.name.starts_with("Delay tower"))
                .map(|(t, _)| EntryRef::Trace { trace: t.meta.id }),
        );
        crate::common::draw_in(&mut d.st, PaneKind::Transfer, &with);
    }
    let l = legend(&d.st);
    assert_eq!(l.len(), 5, "{l:?}");
    assert!(l.iter().any(|x| x == "house_curve · cmp · indep."), "{l:?}");
    assert!(
        l.iter().any(|x| x.starts_with("Main L S1 · cmp · Δt")),
        "{l:?}"
    );
    Ok(())
}

/// A capture selected takes the smoothing keys: K re-smooths the stored capture a step at a
/// time up to 1/3 octave, the daemon serving it anew at each setting, while the live
/// measurement keeps its own.
#[test]
fn a_capture_re_smoothed_by_the_daemon() -> R {
    let rig = Rig::start()?;
    let _noise = noise(&rig.cli)?;
    let mut d = on_rig(&rig)?;
    // Wait for the curve to settle: a capture of the first frames is all settling.
    d.tf_frames(MAIN_L, 240)?;
    d.key("Ctrl+1");
    d.until("slot 1 with its data", |s| {
        s.slots()[0].is_some() && s.rig.traces.len() == 1
    })?;
    let id = d.st.slots()[0].map(|t| t.id).ok_or("slot 1")?;
    d.send(Msg::SelectTrace(id));
    assert_eq!(d.st.rig.selected_trace, Some(id));
    let live_before = d.st.meas(MAIN_L).map(|m| m.config.kind.clone());
    let before = d.st.rig.traces[&id].0.mag_db.clone();
    // K steps coarser from the capture's own setting, each step from what the daemon took:
    // 1/48 … 1/3 octave.
    for want in [
        SmoothingFraction::FortyEighth,
        SmoothingFraction::TwentyFourth,
        SmoothingFraction::Twelfth,
        SmoothingFraction::Sixth,
        SmoothingFraction::Third,
    ] {
        d.key("K");
        d.until("the capture re-smoothed a step", |s| {
            s.rig
                .traces
                .get(&id)
                .is_some_and(|(t, _)| t.meta.edit.smoothing.is_some_and(|x| x.fraction == want))
        })?;
    }
    d.until("its data served at the new setting", |s| {
        s.rig.traces.get(&id).is_some_and(|(t, _)| {
            t.mag_db
                .iter()
                .zip(&before)
                .any(|(a, b)| a.is_finite() && b.is_finite() && (a - b).abs() > 1e-3)
        })
    })?;
    let caption =
        d.st.smoothing_caption(crate::common::pane(&d.st, PaneKind::Transfer))
            .unwrap_or_default();
    assert_eq!(caption, "slot 1 (Main L S1): smoothing 1/3 oct");
    assert_eq!(
        d.st.meas(MAIN_L).map(|m| m.config.kind.clone()),
        live_before,
        "the live measurement untouched"
    );
    Ok(())
}

/// 1/6 octave on Main L and on Mic 1 FFT from the app: the transfer pane draws the phase the
/// daemon smoothed (wrapped, and the unwrapped steps follow it), its legend says the
/// smoothing, the spectrum's axis says the level is smoothed. The transfer stack (magnitude,
/// phase, coherence) becomes Main L's own panes on the daemon.
#[test]
fn smoothed_phase_and_spectrum() -> R {
    use ac2_proto::model::MeasKind;
    let rig = Rig::start()?;
    let _noise = noise(&rig.cli)?;
    let mut d = on_rig(&rig)?;
    let sixth = |s: &AppState, m| {
        s.meas(m).is_some_and(|m| match &m.config.kind {
            MeasKind::Transfer { config } => config
                .smoothing
                .is_some_and(|x| x.fraction == SmoothingFraction::Sixth),
            MeasKind::Spectrum { config } => config.smoothing == Some(SmoothingFraction::Sixth),
            _ => false,
        })
    };
    for (kind, m) in [(PaneKind::Transfer, MAIN_L), (PaneKind::Spectrum, MIC_FFT)] {
        let p = crate::common::pane(&d.st, kind);
        d.send(Msg::FocusPane(p));
        d.send(Msg::SelectMeas(m));
        d.send(Msg::Command(CommandId::Smooth6));
        d.until("1/6 octave on the daemon", |s| sixth(s, m))?;
    }
    // A frame made after the change: its legend and its phase.
    let tf = Topic::Data {
        meas: MAIN_L,
        stream: Stream::Tf,
    };
    let seq = |s: &AppState| {
        s.rig
            .data
            .as_ref()
            .and_then(|x| x.latest.get(&tf))
            .map_or(0, |f| f.frame.stamp.seq)
    };
    let from = seq(&d.st);
    d.until("frames after the change", |s| seq(s) > from + 2)?;
    let st = &d.st;
    let s = ac2_ui::scenes::transfer(
        st,
        crate::common::pane(st, PaneKind::Transfer),
        &Theme::dark(),
        SIZE,
        now(),
    );
    assert_eq!(s.legend[0].text, "Main L · ref · 1/6 oct");
    let frame = st
        .rig
        .data
        .as_ref()
        .and_then(|x| x.latest.get(&tf))
        .and_then(|f| match &f.frame.data {
            ac2_proto::FrameData::Tf(t) => Some(t.clone()),
            _ => None,
        })
        .ok_or("a transfer frame")?;
    let t = &s.traces[0];
    let diff = |a: f64, b: f64| {
        let d = (a - b).rem_euclid(360.0);
        d.min(360.0 - d)
    };
    let mut compared = 0;
    for (i, p) in t.phase_wrapped_deg.iter().enumerate() {
        let want = f64::from(frame.phase[i]);
        if !p.is_finite() || !want.is_finite() {
            continue;
        }
        // The frame reached the app in hundredths of a degree.
        assert!(
            diff(*p, want) < 0.006,
            "column {i}: drawn {p} vs the daemon's {want}"
        );
        compared += 1;
        if i > 0 && frame.phase[i - 1].is_finite() && t.phase_unwrapped_deg[i - 1].is_finite() {
            let step = t.phase_unwrapped_deg[i] - t.phase_unwrapped_deg[i - 1];
            let want_step = want - f64::from(frame.phase[i - 1]);
            assert!(diff(step, want_step) < 0.011, "column {i}");
        }
    }
    assert!(compared > 100, "{compared} columns compared");

    let sp = ac2_ui::scenes::spectrum(
        st,
        crate::common::pane(st, PaneKind::Spectrum),
        &Theme::dark(),
        SIZE,
        now(),
    );
    assert_eq!(sp.unit, "dBFS per 11.7 Hz bin (tone, 1/6 oct smoothed)");
    assert_eq!(sp.caption, "Hann window");

    // The transfer stack from the transfer pane, Main L chosen.
    let tf_pane = crate::common::pane(&d.st, PaneKind::Transfer);
    d.send(Msg::FocusPane(tf_pane));
    d.send(Msg::SelectMeas(MAIN_L));
    d.send(Msg::Command(CommandId::TransferStack));
    let views: Vec<_> = {
        let l = &d.st.layout;
        l.panes()
            .into_iter()
            .filter_map(|id| l.view(id))
            .filter(|v| v.kind == PaneKind::Transfer && !v.shows_ir())
            .map(|v| v.modes.transfer)
            .collect()
    };
    use ac2_ui::state::TransferView as V;
    assert_eq!(views, [V::Magnitude, V::Phase, V::Coherence]);
    d.until("Main L's own panes on the daemon", |s| {
        s.working().dirty && s.meas(MAIN_L).is_some_and(|m| m.layout.is_some())
    })
}

/// A daemon with no audio I/O: the transfer pane says how to open a session; a target
/// imported without one is drawn on no pane, and the hint stays in the pane's centre.
#[test]
fn no_session_hint_stays_with_an_imported_target() -> R {
    let (_daemon, mut d) = connected()?;
    d.synced()?;
    assert_eq!(
        d.st.empty_hint(d.st.layout.focus, &d.keys)
            .map(|h| h.text)
            .as_deref(),
        Some("No session open — Alt+S, then New session")
    );
    import_target(&mut d)?;
    assert!(!d.st.pane_shows_stored(d.st.layout.focus));
    let h =
        d.st.empty_hint(d.st.layout.focus, &d.keys)
            .ok_or("no hint")?;
    assert_eq!(h.place, ac2_ui::state::HintPlace::Centre);
    assert!(h.text.starts_with("No session open"), "{}", h.text);
    Ok(())
}

/// The measurement tree: Main L's two captures under it, Delay tower's capture and the
/// target (imported with Delay tower on screen) under Delay tower, nothing under Imported;
/// Delay tower's group folds. Delete on Main L, which owns traces, asks what becomes of
/// them (Keep / Delete / Cancel); Esc keeps everything. Z's second row stores a flat target
/// with no dialog.
#[test]
fn measurement_tree_and_delete_choices() -> R {
    let rig = Rig::start()?;
    let _noise = noise(&rig.cli)?;
    let mut d = on_rig(&rig)?;
    d.key("Ctrl+1");
    d.until("slot 1 with its data", |s| slot_with_data(s, 1))?;
    d.key("Ctrl+2");
    d.until("slot 2 with its data", |s| slot_with_data(s, 2))?;
    delay_tower_on_screen(&mut d);
    d.key("Ctrl+3");
    d.until("slot 3 with its data", |s| slot_with_data(s, 3))?;
    import_target(&mut d)?;
    d.until("four stored traces with data", |s| s.rig.traces.len() == 4)?;
    let names =
        |s: &AppState| -> Vec<String> { s.tree_rows().iter().map(|r| r.name.clone()).collect() };
    let n = names(&d.st);
    let at = n.iter().position(|x| x == "house_curve").ok_or("listed")?;
    assert_eq!(n[at - 1], "Delay tower S3", "{n:?}");
    let s1 = n.iter().position(|x| x == "Main L S1").ok_or("S1")?;
    assert_eq!(n[s1 + 1], "Main L S2", "{n:?}");
    assert!(!n.iter().any(|x| x == "Imported"), "{n:?}");

    d.send(Msg::ToggleGroup(TraceOwner::Meas { meas: DELAY_TOWER }));
    let n = names(&d.st);
    assert!(
        !n.iter()
            .any(|x| x == "house_curve" || x == "Delay tower S3"),
        "folded: {n:?}"
    );
    assert!(n.iter().any(|x| x == "Main L S1"), "{n:?}");

    d.send(Msg::SelectMeas(MAIN_L));
    d.key("Delete");
    let Overlay::Choose(c) = &d.st.overlay else {
        return Err(format!("no question: {:?}", d.st.overlay).into());
    };
    assert_eq!(
        c.choices.len(),
        3,
        "Keep / Delete / Cancel: {:?}",
        c.choices
    );
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);
    d.pump();
    assert_eq!(d.st.measurements().len(), 4);
    assert_eq!(d.st.rig.traces.len(), 4);

    // Flat needs no dialog.
    d.key("Z");
    d.key("Down");
    d.key("Enter");
    d.until("flat stored", |s| trace_named(s, "flat").is_some())?;
    let flat = trace_named(&d.st, "flat").ok_or("flat")?;
    let meta = &d.st.rig.traces[&flat].0.meta;
    assert_eq!(meta.kind, ac2_proto::model::TraceKind::Target);
    assert!(matches!(
        meta.source,
        ac2_proto::model::TraceSource::Generated { .. }
    ));
    Ok(())
}
