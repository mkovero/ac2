//! Two rigs in one app. Each daemon numbers its measurements, traces, inputs and topics on
//! its own, so the two mirrors here list the same ids: what is kept by them stays with the
//! rig whose link delivered it.

use super::leq::spl_data;
use super::*;

/// The second rig, as its hello reply names it.
const PUPU: RigId = RigId(0x9e9e);

/// The test grid on the test daemon (with an SPL meter), and a second rig in the
/// background, connected and mirroring the same daemon state: the same measurement ids,
/// topics and inputs.
fn two_rigs() -> T {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    let slot = t.st.add_rig("pupu".into(), None, None);
    let r = to(
        &mut t,
        RigId(u64::from(slot)),
        ConnEvent::Connected {
            target: "pupu".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c2".into()),
            rig_id: PUPU,
            name: "pupu".into(),
        },
    );
    assert!(r.is_empty(), "{r:?}");
    t
}

/// An event from `rig`'s link.
fn to(t: &mut T, rig: RigId, e: ConnEvent) -> Vec<(RigId, Request)> {
    t.st.update(Msg::Conn(rig, Box::new(e)), &t.keys)
}

fn pupu(t: &T) -> &RigState {
    t.st.find_rig(PUPU).expect("pupu in the background")
}

/// A levels frame of inputs 0 and 1 at `ppm` dBFS.
fn levels(ppm: f32) -> ConnEvent {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{ClipFlags, SessionLevelsFrame};
    use ac2_proto::{Frame, FrameData};
    let data = FrameData::SessionLevels(SessionLevelsFrame {
        meta: ac2_proto::frame::SessionLevelsMeta {
            channels: vec![0, 1],
            ballistic: Default::default(),
            pair_levels: vec![],
            loudness: vec![],
        },
        peak: vec![ppm; 2],
        rms: vec![ppm; 2],
        clip: vec![ClipFlags::NONE; 2],
        ppm: vec![ppm; 2],
        true_peak: vec![f32::NEG_INFINITY; 2],
        max: vec![f32::NEG_INFINITY; 2],
        overs: vec![0; 2],
    });
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame {
            stamp: ac2_proto::samples::stamp(None),
            data,
        }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string().into(), f);
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }))
}

/// A spectrum frame of measurement 2 at `level` dBFS.
fn spectrum_at(seq: u64, level: f32) -> ConnEvent {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    use ac2_proto::{Frame, FrameData};
    let grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: 8,
    };
    let data = FrameData::Spec(SpecFrame {
        meas: MeasId(2),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            level: ac2_proto::model::SpecLevel::Tone,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            smoothing: None,
            math: None,
            bands: None,
        },
        level: vec![level; 5],
    });
    let mut stamp = ac2_proto::samples::stamp(Some(grid.clone()));
    stamp.seq = seq;
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame { stamp, data }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string().into(), f);
    let mut grids = std::collections::BTreeMap::new();
    grids.insert(grid.id(), Arc::new(grid));
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    }))
}

/// The second rig takes its id and name from its hello; the active one keeps its own, and
/// the app shows the active one.
#[test]
fn each_rig_is_named_by_its_hello() {
    let t = two_rigs();
    assert_eq!((t.st.active(), t.st.rig.name.as_str()), (RIG, "test rig"));
    assert_eq!(pupu(&t).name, "pupu");
    assert!(matches!(
        &pupu(&t).conn,
        ConnState::Connected { rig_id: PUPU, name, .. } if name == "pupu"
    ));
    assert_eq!(
        t.st.rigs().map(|r| r.id).collect::<Vec<_>>(),
        vec![RIG, PUPU]
    );
}

/// The same measurement ids on both rigs: the active rig selects its own, the background
/// rig's selection is untouched by it, and a mirror of either drops only its own choices.
#[test]
fn selection_and_choices_stay_with_their_rig() {
    let mut t = two_rigs();
    to(&mut t, PUPU, mirror(with_spl()));
    t.st.background[0].selected = Some(MeasId(4));
    t.st.background[0].hidden_meas.insert(MeasId(2));
    t.st.background[0]
        .edits
        .insert(MeasId(1), LiveEdit { inverted: true });
    assert_eq!(t.st.rig.selected, Some(MeasId(1)));
    t.st.apply(Msg::SelectMeas(MeasId(2)), &t.keys);
    assert_eq!(t.st.rig.selected, Some(MeasId(2)));
    // The active daemon drops its SPL meter: its own choices on it go, pupu's stay.
    t.remirror(|s| s.measurements.retain(|m| m.id != MeasId(4)));
    let p = pupu(&t);
    assert_eq!(p.selected, Some(MeasId(4)));
    assert!(p.hidden_meas.contains(&MeasId(2)));
    assert_eq!(p.edits.get(&MeasId(1)), Some(&LiveEdit { inverted: true }));
    assert!(t.st.rig.hidden_meas.is_empty());
    // Pupu's daemon drops measurement 1: pupu's edit of it goes, the active rig's stays.
    let had = t.st.rig.edits.get(&MeasId(1)).copied();
    let mut s = with_spl();
    s.measurements.retain(|m| m.id != MeasId(1));
    to(&mut t, PUPU, mirror(s));
    assert!(!pupu(&t).edits.contains_key(&MeasId(1)));
    assert_eq!(t.st.rig.edits.get(&MeasId(1)).copied(), had);
    assert_eq!(t.st.rig.selected, Some(MeasId(2)));
}

/// Frames of the same topics from both rigs: the SPL hold, the Leq history, the flying
/// peaks of inputs 0 and 1 and the peak hold each fold into the rig that sent them.
#[test]
fn frames_fold_into_the_rig_that_sent_them() {
    let mut t = two_rigs();
    let mut held = with_spl();
    held.measurements[0].config.kind = MeasKind::Spectrum {
        config: SpectrumConfig {
            display: ac2_proto::model::SpectrumDisplay {
                peak: PeakMode::Hold,
                ..Default::default()
            },
            ..match spectrum() {
                MeasKind::Spectrum { config } => config,
                _ => unreachable!(),
            }
        },
    };
    t.conn(mirror(held.clone()));
    to(&mut t, PUPU, mirror(held));
    let none = ac2_proto::frame::LeqFlags::NONE;
    to(
        &mut t,
        PUPU,
        spl_data(1, 1_000, 97.0, TimeWeighting::Fast, 1),
    );
    to(&mut t, PUPU, leq_data(1, 100, 91.0, none));
    to(&mut t, PUPU, levels(-3.0));
    to(&mut t, PUPU, spectrum_at(1, -20.0));
    let p = pupu(&t);
    assert!(p.spl_hold.contains_key(&MeasId(4)));
    assert!(p.leq.history.contains_key(&MeasId(4)));
    assert!(p.programme.flying.contains_key(&0) && p.programme.flying.contains_key(&1));
    assert!(p.peaks.contains_key(&MeasId(2)));
    assert!(p.data.is_some());
    let a = &t.st.rig;
    assert!(a.spl_hold.is_empty() && a.leq.history.is_empty(), "{a:?}");
    assert!(a.programme.flying.is_empty() && a.peaks.is_empty());
    assert!(a.data.is_none());
    // The active rig's own frames of the same topics land in its own maps alone.
    t.conn(spl_data(1, 1_000, 85.0, TimeWeighting::Fast, 1));
    t.conn(spectrum_at(1, -40.0));
    assert!(t.st.rig.spl_hold.contains_key(&MeasId(4)));
    let peak = |r: &RigState| r.peaks[&MeasId(2)].2.values().to_vec();
    assert_eq!(peak(&t.st.rig), vec![-40.0; 5]);
    assert_eq!(peak(pupu(&t)), vec![-20.0; 5]);
}

/// A stored trace fetched over one rig's link is that rig's, under an id the other rig
/// uses for its own.
#[test]
fn traces_are_kept_per_rig() {
    let mut t = two_rigs();
    let mut s = with_spl();
    s.traces.push(sweep_meta(7));
    to(&mut t, PUPU, mirror(s));
    let (d, g) = sweep_data(7);
    to(&mut t, PUPU, ConnEvent::Trace(d, g));
    assert!(pupu(&t).traces.contains_key(&TraceId(7)));
    assert!(t.st.rig.traces.is_empty());
}

/// What a background rig's events ask goes over its own link; what the active rig's keys
/// ask goes over the active one's. Each link is synced on its own.
#[test]
fn requests_name_their_rig() {
    let mut t = two_rigs();
    // Pupu's SPL meter listed: its Leq history is rebuilt from pupu's log.
    let r = to(&mut t, PUPU, mirror(with_spl()));
    assert!(
        matches!(
            r.first(),
            Some((
                PUPU,
                Request::LeqBackfill {
                    meas: MeasId(4),
                    ..
                }
            ))
        ) && r.iter().all(|(rig, _)| *rig == PUPU),
        "{r:?}"
    );
    // Space arms the active rig's stimulus.
    t.st.rig.stimulus.level = Some(Dbfs(-20.0));
    let r =
        t.st.update(Msg::Key(Chord::parse("Space").expect("key")), &t.keys);
    assert!(
        !r.is_empty() && r.iter().all(|(rig, _)| *rig == RIG),
        "{r:?}"
    );
    let sync = t.st.sync_link();
    let topics = |rig| {
        sync.iter()
            .find_map(|(r, q)| match q {
                Request::Topics(x) if *r == rig => Some(x.clone()),
                _ => None,
            })
            .expect("topics")
    };
    // No pane shows pupu: only what folds shown or not, its SPL meter's.
    let spl = |stream| Topic::Data {
        meas: MeasId(4),
        stream,
    };
    assert_eq!(
        topics(PUPU),
        [spl(Stream::Spl), spl(Stream::Leq)].into_iter().collect()
    );
    assert_eq!(topics(RIG), t.st.wanted_topics());
    assert!(t.st.sync_link().is_empty(), "sent once");
}

/// Pupu's link going down drops pupu's mirror and data alone; the panes and the active
/// rig's selection and mirror stay. An event for a rig the app does not hold is dropped.
#[test]
fn a_rig_going_down_leaves_the_other() {
    let mut t = two_rigs();
    to(&mut t, PUPU, mirror(with_spl()));
    to(&mut t, PUPU, levels(-3.0));
    let views = t.st.layout.views.clone();
    let selected = t.st.rig.selected;
    to(
        &mut t,
        PUPU,
        ConnEvent::Failed {
            target: "pupu".into(),
            error: "gone".into(),
            retry_in: std::time::Duration::from_secs(2),
        },
    );
    let p = pupu(&t);
    assert!(p.mirror.is_none() && p.data.is_none());
    assert!(matches!(p.conn, ConnState::Failed { .. }));
    assert!(t.st.rig.mirror.is_some() && t.st.connected());
    assert_eq!(t.st.rig.selected, selected);
    assert_eq!(t.st.layout.views, views);
    assert!(to(&mut t, RigId(0xdead), levels(-1.0)).is_empty());
    assert!(t.st.rig.programme.flying.is_empty());
}

/// A window opens for the rig shown and remembers it until it closes.
#[test]
fn a_window_remembers_its_rig() {
    let mut t = two_rigs();
    assert_eq!(t.st.window_rig, None);
    t.st.apply(Msg::Command(CommandId::Calibrations), &t.keys);
    assert_ne!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.window_rig, Some(RIG));
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.window_rig, None);
}

/// Stimulus outputs and session choices are remembered per rig: the same device id on
/// another rig is another device.
#[test]
fn devices_are_remembered_per_rig() {
    let mut t = T::new();
    t.st.prefs
        .outputs
        .insert((PUPU, "fake:loop".into()), vec![1]);
    t.st.rig.stim_device = None;
    t.remirror(|_| {});
    // The test daemon's output device is fake:loop: pupu's outputs for it are not its.
    assert_eq!(t.st.rig.stimulus.outputs, vec![0]);
    t.st.prefs
        .outputs
        .insert((RIG, "fake:loop".into()), vec![3]);
    t.st.rig.stim_device = None;
    t.remirror(|_| {});
    assert_eq!(t.st.rig.stimulus.outputs, vec![3]);
}

/// Two rigs, both mirroring the test daemon (the same measurement ids), the first shown.
fn two_mirrored() -> T {
    let mut t = two_rigs();
    to(&mut t, PUPU, mirror(with_spl()));
    t
}

/// A pick in the rigs drop-down.
fn pick(t: &mut T, row: RigRow) -> Vec<(RigId, Request)> {
    t.st.update(Msg::RigPick(row), &t.keys)
}

/// The rigs drop-down lists each rig in the order added, then Connect… and closing the rig
/// shown; a pick shows that rig and its keys and windows are the operator's.
#[test]
fn the_drop_down_switches_the_rig_shown() {
    let mut t = two_mirrored();
    assert_eq!(
        t.st.rig_menu_rows(),
        vec![
            RigRow::Rig(RIG),
            RigRow::Rig(PUPU),
            RigRow::Connect,
            RigRow::Close(RIG)
        ]
    );
    t.st.apply(Msg::Command(CommandId::Rigs), &t.keys);
    assert!(matches!(t.st.overlay, Overlay::RigMenu(_)));
    pick(&mut t, RigRow::Rig(PUPU));
    assert_eq!(t.st.active(), PUPU);
    assert_eq!(t.st.rig.id, PUPU);
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(t.st.find_rig(RIG).is_some());
    // Space arms pupu's stimulus now.
    t.st.rig.stimulus.level = Some(Dbfs(-20.0));
    let r =
        t.st.update(Msg::Key(Chord::parse("Space").expect("key")), &t.keys);
    assert!(
        !r.is_empty() && r.iter().all(|(rig, _)| *rig == PUPU),
        "{r:?}"
    );
}

/// While a window is open the rig shown cannot change: the window acts on the rig it
/// opened on. The drop-down does not open over it.
#[test]
fn no_switch_under_a_window() {
    let mut t = two_mirrored();
    t.st.apply(Msg::Command(CommandId::Calibrations), &t.keys);
    let window = t.st.overlay.clone();
    assert_ne!(window, Overlay::None);
    t.st.apply(Msg::Command(CommandId::Rigs), &t.keys);
    assert_eq!(t.st.overlay, window);
    pick(&mut t, RigRow::Rig(PUPU));
    assert_eq!(t.st.active(), RIG);
    assert_eq!(t.st.window_rig, Some(RIG));
}

/// A reply to what the operator asked of a rig arrives after another rig is shown: it is
/// still that rig's, handled in full.
#[test]
fn replies_after_a_switch_reach_their_rig() {
    let mut t = two_mirrored();
    t.st.rig.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Arming);
    pick(&mut t, RigRow::Rig(PUPU));
    let r = to(&mut t, RIG, ConnEvent::Stimulus(StimEvent::Armed));
    assert!(r.iter().all(|(rig, _)| *rig == RIG), "{r:?}");
    assert_eq!(
        t.st.find_rig(RIG).expect("rig").stimulus.phase,
        StimPhase::Armed
    );
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle, "pupu's untouched");
}

/// A pane given a measurement on one rig keeps showing that rig's after a switch, though
/// the other rig has a measurement of the same id: its ids are resolved against its own
/// rig alone. A pane with no pick of its own follows the rig shown.
#[test]
fn a_pane_keeps_its_rig_through_a_switch() {
    let mut t = two_mirrored();
    let p = t.st.layout.focus;
    t.st.layout.split_pane(p, Axis::Row, Side::B);
    let q = t.st.layout.focus;
    t.st.layout.view_mut(p).expect("pane").kind = PaneKind::Spectrum;
    t.st.layout.view_mut(q).expect("pane").kind = PaneKind::Spectrum;
    t.st.layout.view_mut(p).expect("pane").meas = Some(MeasId(2));
    t.st.layout.view_mut(q).expect("pane").meas = None;
    pick(&mut t, RigRow::Rig(PUPU));
    assert_eq!(t.st.layout.view(p).expect("pane").rig, PaneRig::Rig(RIG));
    assert_eq!(t.st.layout.view(q).expect("pane").rig, PaneRig::Active);
    // Pupu has a MeasId(2) too: the pinned pane never shows it.
    assert!(t.st.pane_meas(p).is_none());
    assert!(t.st.pane_meas(q).is_some());
    assert!(
        t.st.pane_title(p)
            .starts_with(&format!("{} · ", t.st.rig_name(RIG)))
    );
    let shown =
        t.st.visit(RIG, |st| st.pane_meas(p).map(|m| m.id))
            .flatten();
    assert_eq!(shown, Some(MeasId(2)));
    // Back on the first rig the pane is the shown rig's again.
    pick(&mut t, RigRow::Rig(RIG));
    assert_eq!(t.st.layout.view(p).expect("pane").rig, PaneRig::Active);
    assert_eq!(t.st.pane_meas(p).map(|m| m.id), Some(MeasId(2)));
}

/// What is stored as a measurement's layout is its own rig's panes alone.
#[test]
fn a_measurement_layout_never_holds_another_rigs_pane() {
    let mut t = two_mirrored();
    let p = t.st.layout.focus;
    t.st.layout.split_pane(p, Axis::Row, Side::B);
    t.st.layout.view_mut(p).expect("pane").kind = PaneKind::Spectrum;
    t.st.layout.view_mut(p).expect("pane").meas = Some(MeasId(2));
    let both = t.st.pane_layout();
    pick(&mut t, RigRow::Rig(PUPU));
    let own = t.st.pane_layout();
    fn count(l: &ac2_proto::model::PaneLayout) -> usize {
        match l {
            ac2_proto::model::PaneLayout::Pane { .. } => 1,
            ac2_proto::model::PaneLayout::Split { a, b, .. } => count(a) + count(b),
        }
    }
    assert_eq!(count(&own), count(&both) - 1, "{own:?}");
}

/// A key on a pane of the rig not shown acts on that pane's rig.
#[test]
fn a_pane_key_goes_to_the_panes_rig() {
    let mut t = two_mirrored();
    let p = t.st.layout.focus;
    t.st.layout.view_mut(p).expect("pane").kind = PaneKind::Spectrum;
    t.st.layout.view_mut(p).expect("pane").meas = Some(MeasId(2));
    pick(&mut t, RigRow::Rig(PUPU));
    t.st.layout.set_focus(p);
    // Peak hold on the spectrum pane: a display edit of the measurement it shows.
    let r = t.st.update(Msg::Command(CommandId::PeakHold), &t.keys);
    assert!(
        !r.is_empty() && r.iter().all(|(rig, _)| *rig == RIG),
        "{r:?}"
    );
}

/// Shift+Esc stops the stimulus of every rig, the shown one's and those in the
/// background, from anywhere.
#[test]
fn stop_anywhere_stops_every_rig() {
    let mut t = two_mirrored();
    t.st.rig.stimulus.phase = StimPhase::Armed;
    t.st.background
        .iter_mut()
        .for_each(|r| r.stimulus.phase = StimPhase::Firing);
    assert_eq!(t.st.live_elsewhere(), vec!["pupu".to_string()]);
    let r = t.st.update(Msg::Key(STOP_ANYWHERE), &t.keys);
    let stops: Vec<RigId> = r
        .iter()
        .filter(|(_, q)| matches!(q, Request::StimStop))
        .map(|(rig, _)| *rig)
        .collect();
    assert_eq!(stops, vec![RIG, PUPU]);
    assert_eq!(pupu(&t).stimulus.phase, StimPhase::Stopping);
}

/// Closing the rig shown shows the next one first; the closed rig goes to the app, which
/// closes its link (stopping the stimulus it holds and releasing its lease first). Its
/// panes stay, saying it is not connected, and it is no longer remembered.
#[test]
fn closing_a_rig() {
    let mut t = two_mirrored();
    t.st.prefs.rigs.push(crate::prefs::RememberedRig {
        id: RIG,
        name: "ac2d test".into(),
        addr: crate::prefs::RigAddr::Local,
    });
    let p = t.st.layout.focus;
    t.st.layout.view_mut(p).expect("pane").kind = PaneKind::Spectrum;
    t.st.layout.view_mut(p).expect("pane").meas = Some(MeasId(2));
    pick(&mut t, RigRow::Close(RIG));
    assert_eq!(t.st.active(), PUPU);
    assert!(t.st.find_rig(RIG).is_none());
    assert_eq!(t.st.closing.len(), 1);
    assert_eq!(t.st.closing[0].id, RIG);
    assert!(t.st.prefs.rigs.is_empty());
    assert_eq!(t.st.layout.view(p).expect("pane").rig, PaneRig::Rig(RIG));
    let name = t.st.rig_name(RIG);
    assert_eq!(
        t.st.pane_title(p),
        format!("{name} · {}", PaneKind::Spectrum.title())
    );
    // The last rig stays.
    assert_eq!(
        t.st.rig_menu_rows(),
        vec![RigRow::Rig(PUPU), RigRow::Connect]
    );
}

/// The same daemon reached twice is one rig: the second link is closed.
#[test]
fn a_rig_already_held_is_not_added_twice() {
    let mut t = two_rigs();
    let slot = t.st.add_rig("pupu again".into(), None, None);
    to(
        &mut t,
        RigId(u64::from(slot)),
        ConnEvent::Connected {
            target: "pupu again".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c3".into()),
            rig_id: PUPU,
            name: "pupu".into(),
        },
    );
    assert_eq!(t.st.rigs().count(), 2);
    assert_eq!(t.st.closing.len(), 1);
}

/// A rig connected where it was reached is remembered for the next start, by its id.
#[test]
fn a_connected_rig_is_remembered() {
    let mut t = T::new();
    let addr = crate::prefs::RigAddr::Network {
        host: "pupu.local".into(),
        port: 47820,
        fingerprint: Some("ab12".into()),
    };
    let slot = t.st.add_rig("pupu".into(), None, Some(addr.clone()));
    to(
        &mut t,
        RigId(u64::from(slot)),
        ConnEvent::Connected {
            target: "pupu".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c2".into()),
            rig_id: PUPU,
            name: "pupu".into(),
        },
    );
    assert_eq!(
        t.st.prefs.rigs,
        vec![crate::prefs::RememberedRig {
            id: PUPU,
            name: "pupu".into(),
            addr
        }]
    );
    assert!(t.st.prefs_dirty);
}

/// Each rig's link is asked for its input meters while that rig's session is open, which
/// rig is shown or not: the Inputs list shows the shown rig's, and a switch neither turns
/// another rig's meters off nor asks them of the wrong link.
#[test]
fn input_meters_follow_each_rig_through_switches() {
    const BARE: RigId = RigId(0xba5e);
    let mut t = T::fresh();
    t.st.layout = grid();
    t.st.pane_auto = false;
    let mut meters: std::collections::HashMap<RigId, bool> = Default::default();
    fn note(m: &mut std::collections::HashMap<RigId, bool>, r: Vec<(RigId, Request)>) {
        for (rig, q) in r {
            if let Request::Meters(on) = q {
                m.insert(rig, on);
            }
        }
    }
    for (id, name, state) in [
        (RIG, "test rig", with_spl()),
        (PUPU, "pupu", with_spl()),
        (BARE, "bare", no_session_state()),
    ] {
        let slot = if id == RIG {
            t.st.rig.slot
        } else {
            t.st.add_rig(name.into(), None, None)
        };
        let hello = ConnEvent::Connected {
            target: name.into(),
            server: "ac2d test".into(),
            client_id: ClientId(name.into()),
            rig_id: id,
            name: name.into(),
        };
        let now = t.st.rig_in_slot(slot).expect("rig").id;
        note(&mut meters, to(&mut t, now, hello));
        note(&mut meters, to(&mut t, id, mirror(state)));
    }
    let want = |m: &std::collections::HashMap<RigId, bool>| {
        let on = |r| m.get(&r).copied().unwrap_or(false);
        (on(RIG), on(PUPU), on(BARE))
    };
    assert_eq!(want(&meters), (true, true, false), "{meters:?}");
    for to_rig in [PUPU, BARE, RIG, BARE, PUPU, RIG] {
        let r = pick(&mut t, RigRow::Rig(to_rig));
        assert_eq!(t.st.active(), to_rig);
        note(&mut meters, r);
        assert_eq!(
            want(&meters),
            (true, true, false),
            "after showing {to_rig:?}: {meters:?}"
        );
    }
}

/// Two rigs mirrored, pupu's measurement 2 named apart from the first rig's (the same id,
/// another measurement), and pane `p`'s list open on the first rig.
fn picker_open() -> (T, PaneId) {
    let mut t = two_rigs();
    let mut s = with_spl();
    s.measurements[0].config.name = "Pupu sub".into();
    to(&mut t, PUPU, mirror(s));
    let p = t.st.layout.focus;
    t.st.update(Msg::PaneMenu(p), &t.keys);
    assert!(matches!(t.st.overlay, Overlay::PaneMenu(m) if m.pane == p));
    (t, p)
}

/// The rows of pane `p`'s list taken by a pick (no heading, no kind of pane).
fn menu_picks(t: &T, p: PaneId) -> Vec<PaneMenuRow> {
    t.st.pane_menu_rows(p)
        .into_iter()
        .map(|(r, _)| r)
        .filter(|r| {
            matches!(
                r,
                PaneMenuRow::Meas(_)
                    | PaneMenuRow::RigMeas(..)
                    | PaneMenuRow::Meter(_)
                    | PaneMenuRow::RigMeter(..)
            )
        })
        .collect()
}

/// With two rigs the pane's list has a heading per rig, the rig shown first, and each
/// rig's measurements under it by that rig's ids: measurement 2 twice, one per rig.
#[test]
fn the_pane_list_offers_every_rigs_measurements() {
    let (t, p) = picker_open();
    let rows = t.st.pane_menu_rows(p);
    let heads: Vec<(RigId, String)> = rows
        .iter()
        .filter_map(|(r, s)| match r {
            PaneMenuRow::Heading(id) => Some((*id, s.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        heads,
        vec![(RIG, t.st.rig_name(RIG)), (PUPU, "pupu".to_string())]
    );
    assert_eq!(rows[0].0, PaneMenuRow::Heading(RIG));
    let picks = menu_picks(&t, p);
    assert!(picks.contains(&PaneMenuRow::Meas(MeasId(2))));
    assert!(picks.contains(&PaneMenuRow::RigMeas(PUPU, MeasId(2))));
    let label = |want: PaneMenuRow| {
        rows.iter()
            .find(|(r, _)| *r == want)
            .map(|(_, s)| s.clone())
            .expect("listed")
    };
    assert!(label(PaneMenuRow::Meas(MeasId(2))).ends_with("Sub"));
    assert!(label(PaneMenuRow::RigMeas(PUPU, MeasId(2))).ends_with("Pupu sub"));
    // Pupu's session inputs are offered as pupu's meters.
    assert!(
        picks
            .iter()
            .any(|r| matches!(r, PaneMenuRow::RigMeter(PUPU, _)))
    );
    // The highlight never rests on a heading: Home lands on the first measurement.
    let mut t = t;
    t.key("Home");
    let Overlay::PaneMenu(m) = t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert!(rows[m.index].0.pickable(), "{:?}", rows[m.index]);
    assert!(matches!(rows[m.index].0, PaneMenuRow::Meas(_)));
}

/// Picking pupu's measurement 2 sets the pane on pupu: it shows pupu's, never the first
/// rig's measurement of that id, and the selection stays on the first rig's row.
#[test]
fn a_pick_of_another_rigs_measurement_pins_the_pane() {
    let (mut t, p) = picker_open();
    let selected = t.st.rig.selected;
    t.st.update(
        Msg::PanePick(p, PaneMenuRow::RigMeas(PUPU, MeasId(2))),
        &t.keys,
    );
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(PUPU), Some(MeasId(2))));
    assert_eq!(v.kind, PaneKind::Spectrum);
    assert!(
        t.st.pane_meas(p).is_none(),
        "not the first rig's measurement 2"
    );
    let name =
        t.st.visit(PUPU, |st| st.pane_meas(p).map(|m| m.config.name.clone()));
    assert_eq!(name.flatten().as_deref(), Some("Pupu sub"));
    assert_eq!(t.st.rig.selected, selected);
    assert!(t.st.pane_title(p).starts_with("pupu · "));
    // Opened again, its list highlights pupu's row.
    t.st.update(Msg::PaneMenu(p), &t.keys);
    let Overlay::PaneMenu(m) = t.st.overlay else {
        panic!("{:?}", t.st.overlay);
    };
    assert_eq!(
        t.st.pane_menu_rows(p)[m.index].0,
        PaneMenuRow::RigMeas(PUPU, MeasId(2))
    );
    // The first rig's own row puts the pane back on the rig shown.
    t.st.update(Msg::PanePick(p, PaneMenuRow::Meas(MeasId(2))), &t.keys);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Active, Some(MeasId(2))));
    assert_eq!(
        t.st.pane_meas(p).map(|m| m.config.name.as_str()),
        Some("Sub")
    );
}

/// Pupu's inputs picked: a Programme pane on pupu's meters, by pupu's channel numbers.
#[test]
fn another_rigs_meters_in_a_pane() {
    let (mut t, p) = picker_open();
    let row = menu_picks(&t, p)
        .into_iter()
        .find(|r| matches!(r, PaneMenuRow::RigMeter(PUPU, _)))
        .expect("pupu's meters");
    let PaneMenuRow::RigMeter(_, m) = row else {
        unreachable!()
    };
    t.st.update(Msg::PanePick(p, row), &t.keys);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!(
        (v.kind, v.rig, v.meters, v.meas),
        (PaneKind::Programme, PaneRig::Rig(PUPU), Some(m), None)
    );
    assert_eq!(v.modes.programme, ac2_scene::view::ProgrammeMode::Meters);
}

/// A measurement row chosen in the list while another rig's pane has the focus: the rig's
/// own pane shows it, the pinned pane stays as it was.
#[test]
fn a_row_chosen_leaves_another_rigs_pane() {
    let (mut t, p) = picker_open();
    t.st.update(
        Msg::PanePick(p, PaneMenuRow::RigMeas(PUPU, MeasId(2))),
        &t.keys,
    );
    let pinned = *t.st.layout.view(p).expect("pane");
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert_eq!(*t.st.layout.view(p).expect("pane"), pinned);
    let f = t.st.layout.focus;
    assert_ne!(f, p, "a pane of its own split off");
    assert_eq!(t.st.layout.view(f).expect("pane").rig, PaneRig::Active);
    assert_eq!(t.st.pane_meas(f).map(|m| m.id), Some(MeasId(1)));
}

/// The layout prefs keep a pinned pane's rig and its measurement's name on that rig; read
/// back at the next start, the pane waits for that rig, held under another slot and
/// reached anywhere, and shows its measurement once the rig says hello with its id.
#[test]
fn prefs_pin_a_pane_to_its_rig_by_id() {
    let (mut t, p) = picker_open();
    t.st.update(
        Msg::PanePick(p, PaneMenuRow::RigMeas(PUPU, MeasId(2))),
        &t.keys,
    );
    let prefs = t.st.layout_prefs();
    let pane = prefs
        .panes
        .as_ref()
        .and_then(|l| l.views.iter().find(|v| v.id == p))
        .expect("the pane remembered");
    assert_eq!(pane.rig, Some((PUPU, "pupu".to_string())));
    assert_eq!(pane.measurement.as_deref(), Some("Pupu sub"));
    // The next start: the first rig only; pupu is reached later, at a new address.
    let ui = crate::prefs::UiPrefs {
        layout: prefs,
        ..Default::default()
    };
    let mut t = T::new();
    t.st.set_prefs(ui);
    t.conn(mirror(with_spl()));
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(PUPU), None));
    assert_eq!(t.st.rig_name(PUPU), "pupu");
    let slot = t.st.add_rig("pupu.elsewhere".into(), None, None);
    to(
        &mut t,
        RigId(u64::from(slot)),
        ConnEvent::Connected {
            target: "pupu.elsewhere".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c2".into()),
            rig_id: PUPU,
            name: "pupu".into(),
        },
    );
    let mut s = with_spl();
    s.measurements[0].config.name = "Pupu sub".into();
    s.measurements[0].id = MeasId(7);
    to(&mut t, PUPU, mirror(s));
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(PUPU), Some(MeasId(7))));
}

/// A rig closed and reached again (another address, its daemon restarted with other ids):
/// its pinned pane shows the same measurement, found by name on the rig of that id.
#[test]
fn a_closed_rig_back_shows_its_panes_again() {
    let (mut t, p) = picker_open();
    t.st.update(
        Msg::PanePick(p, PaneMenuRow::RigMeas(PUPU, MeasId(2))),
        &t.keys,
    );
    pick(&mut t, RigRow::Rig(PUPU));
    pick(&mut t, RigRow::Close(PUPU));
    assert_eq!(t.st.active(), RIG);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(PUPU), None));
    assert!(t.st.pane_title(p).starts_with("pupu · "));
    // The app has closed its link meanwhile.
    t.st.closing.clear();
    let slot = t.st.add_rig("10.0.0.9".into(), None, None);
    to(
        &mut t,
        RigId(u64::from(slot)),
        ConnEvent::Connected {
            target: "10.0.0.9".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c3".into()),
            rig_id: PUPU,
            name: "pupu".into(),
        },
    );
    let mut s = with_spl();
    s.measurements[0].config.name = "Pupu sub".into();
    s.measurements[0].id = MeasId(9);
    to(&mut t, PUPU, mirror(s));
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(PUPU), Some(MeasId(9))));
}

/// A measurement's stored panes read for the rig whose layout it is: `ThisRig` and a pin
/// to that rig's own id are its panes, a pin to another rig stays that rig's, named.
#[test]
fn a_stored_pane_names_its_rig() {
    use ac2_proto::model::layout as wire;
    let mut t = two_mirrored();
    fn first(l: &wire::PaneLayout) -> &wire::LayoutPane {
        match l {
            wire::PaneLayout::Pane { pane } => pane,
            wire::PaneLayout::Split { a, .. } => first(a),
        }
    }
    let base = t.st.pane_layout();
    let pane = first(&base);
    let with = |rig: wire::RigRef, meas: u32| {
        let mut p = pane.clone();
        p.rig = rig;
        p.kind = wire::PaneKind::Spectrum;
        p.content = Some(wire::PaneContent::Meas { meas: MeasId(meas) });
        wire::PaneLayout::Pane { pane: Box::new(p) }
    };
    let l = wire::PaneLayout::Split {
        axis: wire::SplitAxis::Row,
        ratio: 0.5,
        a: Box::new(with(wire::RigRef::ThisRig, 2)),
        b: Box::new(wire::PaneLayout::Split {
            axis: wire::SplitAxis::Row,
            ratio: 0.5,
            a: Box::new(with(
                wire::RigRef::Rig {
                    id: RIG,
                    name: "this one".into(),
                },
                2,
            )),
            b: Box::new(with(
                wire::RigRef::Rig {
                    id: RigId(0x55),
                    name: "kettu".into(),
                },
                12,
            )),
        }),
    };
    assert!(l.check().is_ok(), "{:?}", l.check());
    t.st.rig.chosen = Some(MeasId(2));
    t.st.show_layout(&l);
    let rigs: Vec<(PaneRig, Option<MeasId>)> =
        t.st.layout
            .panes()
            .into_iter()
            .filter_map(|p| t.st.layout.view(p).map(|v| (v.rig, v.meas)))
            .collect();
    assert_eq!(
        rigs,
        vec![
            (PaneRig::Active, Some(MeasId(2))),
            (PaneRig::Active, Some(MeasId(2))),
            (PaneRig::Rig(RigId(0x55)), Some(MeasId(12))),
        ]
    );
    assert_eq!(t.st.rig_name(RigId(0x55)), "kettu");
    // Stored again, the rig's own layout holds its own panes alone, as `ThisRig`.
    fn rigs_of(l: &wire::PaneLayout, out: &mut Vec<wire::RigRef>) {
        match l {
            wire::PaneLayout::Pane { pane } => out.push(pane.rig.clone()),
            wire::PaneLayout::Split { a, b, .. } => {
                rigs_of(a, out);
                rigs_of(b, out);
            }
        }
    }
    let mut out = Vec::new();
    rigs_of(&t.st.pane_layout(), &mut out);
    assert_eq!(out, vec![wire::RigRef::ThisRig; 2]);
}
