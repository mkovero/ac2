//! Many rigs against two real daemons: a rig gone quiet greys out and comes back, the Inputs
//! list meters the rig shown, a session's own panes load on either rig, a pin finds its
//! rig on another address; the list and the panes reach another rig only once the two
//! daemons are paired.

use super::*;
use crate::harness::R;
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, SessionRef, Signal};
use ac2_proto::units::{Dbfs, Hz, RigId};
use ac2_ui::connect::Choice;
use ac2_ui::prefs::RigAddr;
use ac2_ui::state::PaneRig;
use egui_kittest::kittest::Queryable as _;

/// A rig named `name`, its identity kept in `id_file` (the same rig when started again).
fn named(name: &'static str, id_file: &std::path::Path) -> R<Rig> {
    let id_file = id_file.to_path_buf();
    Rig::start_with(move |c| {
        c.name = name.into();
        c.rig_id = ac2d::RigIdSource::File(id_file);
    })
}

/// The app on `pupu`, able to open the connect dialog (no network browse, an empty key
/// directory), once pupu is live: and pupu's id.
fn on_pupu(pupu: &Rig, keys: &std::path::Path) -> (Harness<'static, App>, RigId) {
    let mut opts = options(pupu);
    opts.connect = Some(ac2_ui::app::ConnectSetup {
        key_dir: keys.to_path_buf(),
        client_name: "ac2-ui test".into(),
        discovery: false,
    });
    let mut h = harness(opts);
    step_until(&mut h, "pupu live", live);
    let id = h.state().state.rig.id;
    (h, id)
}

/// Connect a rig… from the palette, the dialog's choice `kettu` at its endpoints, shown once
/// its mirror is in step (on kettu's own screen; pupu's is kept for pupu); kettu's id. The watchdog gives up on a silent link after `dead_after`.
fn connect_kettu(
    h: &mut Harness<'_, App>,
    pupu: RigId,
    kettu: &Rig,
    dead_after: Duration,
) -> RigId {
    palette(h, "connect a rig");
    step_until(h, "the connect dialog", |a| a.connect_open());
    let ep = kettu.daemon.endpoints().clone();
    let addr = RigAddr::Endpoints {
        ctrl: ep.ctrl.clone(),
        data: ep.data.clone(),
    };
    let mut config = ac2_client::ClientConfig::new(ep, "ac2-ui test");
    config.dead_after = dead_after;
    let choice = Choice::Target(
        Box::new(Target {
            config,
            describe: "kettu".into(),
        }),
        Some(addr),
    );
    let ctx = h.ctx.clone();
    h.state_mut().connect_to(&ctx, choice);
    step_until(h, "kettu shown and ready", |a| {
        !a.connect_open() && a.state.active() != pupu && synced(a)
    });
    h.state().state.active()
}

/// `taker`'s daemon paired with rig `source`, by the command the app's pairing sends it
/// (`rig.peers`): the rig may take that rig's inputs. These daemons listen on this computer
/// only, so no link between them comes up and the source's half of a pairing (admitting
/// the taker's key) has no key to admit; the pairing itself is the daemon's, and the
/// address is where a networked rig would be.
fn pair(taker: &Rig, source: RigId, name: &str) -> R {
    use ac2_proto::model::RigPeer;
    taker.call(Command::RigPeers {
        peers: vec![RigPeer {
            rig: source,
            name: name.into(),
            ctrl: "tcp://127.0.0.1:7700".into(),
            data: "tcp://127.0.0.1:7701".into(),
            key: ac2_zmq::KeyPair::generate()?.public.to_z85(),
        }],
    })?;
    Ok(())
}

/// The mirror is in step with the daemon: what a rig with no pane of its own on screen
/// (nothing subscribed) can be waited for.
fn synced(a: &App) -> bool {
    matches!(a.state.rig.conn, ConnState::Connected { .. })
        && a.state.rig.mirror.as_ref().is_some_and(|m| m.synced())
}

fn connected(a: &App, id: RigId) -> bool {
    a.state
        .find_rig(id)
        .is_some_and(|r| matches!(r.conn, ConnState::Connected { .. }))
}

/// The drop-down from the palette, its first row taken.
fn pick_first_rig(h: &mut Harness<'_, App>) {
    open_rig_menu(h);
    h.key_press(Key::Home);
    h.key_press(Key::Enter);
}

/// *Rigs…* from the palette: the drop-down opens.
fn open_rig_menu(h: &mut Harness<'_, App>) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.event(Event::Text("rigs".into()));
    step_until(h, "palette typed", |a| {
        matches!(&a.state.overlay, Overlay::Palette(_))
    });
    h.key_press(Key::Enter);
    step_until(h, "the rigs drop-down", |a| {
        matches!(a.state.overlay, Overlay::RigMenu(_))
    });
}

/// Kettu goes away (its daemon shut down): its pane on pupu's screen says so, pupu goes on;
/// once it is back on the same address, it is connected anew without the operator doing
/// anything.
#[test]
fn a_rig_down_and_back() -> R {
    if !have_gpu("a_rig_down_and_back") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let keys = dir.path().join("keys");
    let pupu = named("pupu", &dir.path().join("pupu-id"))?;
    let kettu = named("kettu", &dir.path().join("kettu-id"))?;
    let (mut h, pupu_id) = on_pupu(&pupu, &keys);
    let kettu_id = connect_kettu(&mut h, pupu_id, &kettu, Duration::from_millis(600));
    pair(&pupu, kettu_id, "kettu")?;
    // Pupu shown, kettu's spectrum dropped on its focused pane.
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| a.state.active() == pupu_id);
    let p = h.state().state.layout.focus;
    step_until(&mut h, "kettu's rows listed", |a| {
        a.state.list_rows().iter().any(|(r, _)| *r == kettu_id)
    });
    drop_kettus(&mut h, kettu_id, MIC_FFT, p);
    h.state_mut().state.toasts.list.clear();
    let ep = kettu.daemon.endpoints().clone();
    drop(kettu);
    step_until(&mut h, "kettu down", |a| !connected(a, kettu_id));
    let away = ac2_scene::chrome::rig_pane_down("kettu", ac2_scene::chrome::RigDown::Unreachable);
    step_until(&mut h, "kettu's panes name it unreachable", |_| true);
    assert!(h.query_by_label(&away).is_some(), "{away}");
    assert!(connected(h.state(), pupu_id), "pupu is untouched");
    let id_file = dir.path().join("kettu-id");
    let _kettu = Rig::start_with(move |c| {
        c.name = "kettu".into();
        c.rig_id = ac2d::RigIdSource::File(id_file);
        c.listen = ac2d::Listen::Local {
            ctrl: ep.ctrl,
            data: ep.data,
        };
    })?;
    step_until(&mut h, "kettu back", |a| connected(a, kettu_id));
    step_until(&mut h, "kettu's panes back", |_| true);
    assert!(h.query_by_label(&away).is_none(), "kettu's panes are back");
    Ok(())
}

/// A tone on out 1 at `level` dBFS: in 1, the loop return, reads it.
fn tone(level: f64) -> GeneratorDesired {
    GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    }
}

/// The reading of the shown rig's Inputs-list row for input 1.
fn in1_reading(a: &App) -> Option<f64> {
    a.state
        .session_inputs()
        .into_iter()
        .find(|r| r.channel == 0)
        .and_then(|r| r.reading.text.replace('\u{2212}', "-").parse().ok())
}

/// In 1 of the rig shown reads `level` (a sine's level, as its generator played it).
fn in1_reads(level: f64) -> impl Fn(&App) -> bool {
    move |a| in1_reading(a).is_some_and(|v| (v - level).abs() < 0.3)
}

/// The Inputs list meters the rig shown, whichever was shown before and however the rig
/// was added: kettu's once Connect… shows it, pupu's again after the drop-down goes back,
/// both following their own daemon's levels (each rig's loop return reading its own
/// tone).
#[test]
fn the_inputs_meter_the_rig_shown() -> R {
    if !have_gpu("the_inputs_meter_the_rig_shown") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let keys = dir.path().join("keys");
    let pupu = named("pupu", &dir.path().join("pupu-id"))?;
    let kettu = named("kettu", &dir.path().join("kettu-id"))?;
    // Each change raises the tone: a meter rises at once but falls with its ballistics.
    let pupu_tone = pupu.cli.lease(tone(-33.0))?;
    let kettu_tone = kettu.cli.lease(tone(-28.0))?;
    let (mut h, pupu_id) = on_pupu(&pupu, &keys);
    step_until(&mut h, "pupu's in 1 metered", in1_reads(-33.0));
    connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(10));
    step_until(&mut h, "kettu's in 1 metered", in1_reads(-28.0));
    kettu.cli.block_on(kettu_tone.set(tone(-23.0)))?;
    step_until(&mut h, "kettu's in 1 following it", in1_reads(-23.0));

    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| a.state.active() == pupu_id);
    pupu.cli.block_on(pupu_tone.set(tone(-13.0)))?;
    step_until(&mut h, "pupu's in 1 following it again", in1_reads(-13.0));
    pupu.cli.block_on(pupu_tone.end())?;
    kettu.cli.block_on(kettu_tone.end())?;
    Ok(())
}

/// Kettu's measurement `id`'s row of the list dragged onto the middle of pane `p`.
fn drop_kettus(h: &mut Harness<'_, App>, kettu: RigId, id: MeasId, p: ac2_ui::state::PaneId) {
    use ac2_scene::meas_list::TreeKey;
    use ac2_ui::state::{DragItem, DragMsg, DropTarget, Msg};
    let a = h.state_mut();
    a.dispatch(Msg::RowDrag(DragMsg::Start(DragItem::Row {
        rig: kettu,
        key: TreeKey::Meas(id),
    })));
    a.dispatch(Msg::RowDrag(DragMsg::Over(Some(DropTarget {
        pane: p,
        edge: None,
    }))));
    a.dispatch(Msg::RowDrag(DragMsg::Drop));
    h.step();
}

/// A session saved on pupu with its measurement's own panes (`ThisRig`) and loaded on
/// kettu: choosing that measurement on kettu brings kettu's panes of it, on kettu's ids,
/// and none of pupu's stays beside them; back on pupu its own screen is back, with none of
/// kettu's panes.
#[test]
fn a_session_with_its_own_panes_loads_on_either_rig() -> R {
    if !have_gpu("a_session_with_its_own_panes_loads_on_either_rig") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let keys = dir.path().join("keys");
    let sessions = dir.path().join("sessions");
    let both = |name: &'static str| {
        let (sessions, id_file) = (sessions.clone(), dir.path().join(format!("{name}-id")));
        Rig::start_with(move |c| {
            c.name = name.into();
            c.rig_id = ac2d::RigIdSource::File(id_file);
            c.session_dir = sessions;
        })
    };
    let pupu = both("pupu")?;
    let kettu = both("kettu")?;
    let (mut h, pupu_id) = on_pupu(&pupu, &keys);
    let kettu_id = connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(10));
    pair(&pupu, kettu_id, "kettu")?;
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == pupu_id && live(a)
    });
    // Mic 1 FFT chosen on pupu and its panes edited (a split, it dropped on the new half):
    // its layout is stored on pupu.
    tab_to(&mut h, MIC_FFT);
    let sp = crate::common::pane(&h.state().state, PaneKind::Spectrum);
    h.state_mut().dispatch(ac2_ui::state::Msg::FocusPane(sp));
    h.key_press(Key::N);
    settle(&mut h, 2);
    drop_on_kind(&mut h, PaneKind::Spectrum, MIC_FFT);
    let stored = |rig: &Rig| {
        rig.cli.state().and_then(|s| {
            s.measurements
                .iter()
                .find(|m| m.id == MIC_FFT)
                .and_then(|m| m.layout.clone())
        })
    };
    step_until(&mut h, "pupu stores the layout", |_| {
        stored(&pupu).is_some()
    });
    let session = SessionRef::Name {
        name: "both rigs".into(),
    };
    pupu.call(Command::FileSave {
        session: session.clone(),
        overwrite: true,
    })?;
    kettu.call(Command::FileLoad {
        session,
        discard: true,
    })?;
    step_until(&mut h, "kettu has pupu's layout", |_| {
        stored(&kettu).is_some() && stored(&kettu) == stored(&pupu)
    });
    let own = |a: &App, rig: PaneRig| {
        a.state
            .layout
            .panes()
            .into_iter()
            .filter_map(|p| a.state.layout.view(p))
            .filter(|v| v.rig == rig && v.meas == Some(MIC_FFT))
            .count()
    };
    let pupus = own(h.state(), PaneRig::Active);
    assert!(pupus >= 2, "pupu's own panes of it: {pupus}");
    // On kettu: the same measurement chosen brings its panes, as kettu's.
    open_rig_menu(&mut h);
    h.key_press(Key::Home);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    step_until(&mut h, "kettu shown", |a| {
        a.state.active() == kettu_id && synced(a)
    });
    step_until(&mut h, "kettu has the session", |a| {
        a.state.meas(MIC_FFT).is_some_and(|m| m.layout.is_some())
    });
    tab_to(&mut h, MIC_FFT);
    step_until(&mut h, "kettu's own panes", |a| {
        own(a, PaneRig::Active) >= pupus && own(a, PaneRig::Rig(pupu_id)) == 0
    });
    // Back on pupu: its own screen, none of kettu's panes.
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| a.state.active() == pupu_id);
    let a = h.state();
    assert_eq!(own(a, PaneRig::Active), pupus);
    assert_eq!(own(a, PaneRig::Rig(kettu_id)), 0);
    Ok(())
}

/// A pane pinned to kettu; kettu closed (the pane reads *kettu not connected*), then kettu
/// reached again on another address: the pin finds it by its id and shows its measurement.
#[test]
fn a_pin_finds_its_rig_on_another_address() -> R {
    if !have_gpu("a_pin_finds_its_rig_on_another_address") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let keys = dir.path().join("keys");
    let kettu_file = dir.path().join("kettu-id");
    let pupu = named("pupu", &dir.path().join("pupu-id"))?;
    let kettu = named("kettu", &kettu_file)?;
    let (mut h, pupu_id) = on_pupu(&pupu, &keys);
    let kettu_id = connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(10));
    pair(&pupu, kettu_id, "kettu")?;
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == pupu_id && live(a)
    });
    let p = h.state().state.layout.focus;
    step_until(&mut h, "kettu's rows listed", |a| {
        a.state.list_rows().iter().any(|(r, _)| *r == kettu_id)
    });
    drop_kettus(&mut h, kettu_id, MIC_FFT, p);
    // Kettu shown and closed from the drop-down (its last row).
    open_rig_menu(&mut h);
    h.key_press(Key::Home);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    step_until(&mut h, "kettu shown", |a| a.state.active() == kettu_id);
    open_rig_menu(&mut h);
    h.key_press(Key::End);
    h.key_press(Key::Enter);
    step_until(&mut h, "kettu closed", |a| {
        a.state.active() == pupu_id && a.state.find_rig(kettu_id).is_none()
    });
    let away = ac2_scene::chrome::rig_pane_closed("kettu");
    step_until(&mut h, "the pane says so", |_| true);
    assert!(h.query_by_label(&away).is_some(), "{away}");
    drop(kettu);
    // The same rig (its id kept) on another port.
    let moved = named("kettu", &kettu_file)?;
    let again = connect_kettu(&mut h, pupu_id, &moved, Duration::from_secs(10));
    assert_eq!(again, kettu_id, "the rig keeps its id");
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown, the pane on kettu's measurement", |a| {
        a.state.active() == pupu_id
            && a.state
                .layout
                .view(p)
                .is_some_and(|v| v.rig == PaneRig::Rig(kettu_id) && v.meas == Some(MIC_FFT))
    });
    assert!(h.query_by_label(&away).is_none(), "kettu's pane is back");
    Ok(())
}

/// Connect… adds kettu beside pupu and shows it; pupu stays linked. The drop-down goes back
/// to pupu, kettu linked in the background, and lists both. Kettu is remembered by its
/// name and id.
#[test]
fn a_second_rig_connects_and_the_drop_down_switches() -> R {
    if !have_gpu("a_second_rig_connects_and_the_drop_down_switches") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let pupu = named("pupu", &dir.path().join("pupu-id"))?;
    let kettu = named("kettu", &dir.path().join("kettu-id"))?;
    let (mut h, pupu_id) = on_pupu(&pupu, &dir.path().join("keys"));
    let kettu_id = connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(10));
    assert_ne!(kettu_id, pupu_id);
    assert!(connected(h.state(), pupu_id), "pupu keeps its link");
    assert!(
        h.state()
            .state
            .prefs
            .rigs
            .iter()
            .any(|r| r.id == kettu_id && r.name == "kettu"),
        "kettu remembered"
    );

    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == pupu_id && live(a)
    });
    assert!(
        connected(h.state(), kettu_id),
        "kettu keeps its link in the background"
    );
    // The drop-down lists both rigs by name.
    open_rig_menu(&mut h);
    settle(&mut h, 2);
    for name in ["pupu", "kettu"] {
        assert!(
            h.query_all(egui_kittest::kittest::By::new().label_contains(name))
                .next()
                .is_some(),
            "{name} in the drop-down"
        );
    }
    h.key_press(Key::Escape);
    step_until(&mut h, "the drop-down closed", |a| {
        a.state.overlay == Overlay::None
    });
    Ok(())
}

/// Kettu connected beside pupu, the two daemons not paired: each rig shown lists itself
/// alone on its own screen, no pane of the other on it. Once pupu's daemon is paired with
/// kettu the list shows both, whichever of the two is shown, and kettu's row dragged onto
/// a pane of pupu's screen draws there. The pairing removed, that pane keeps what it holds
/// and says the rigs are not paired; paired again it draws.
#[test]
fn rigs_not_paired_share_neither_the_list_nor_panes() -> R {
    use ac2_scene::missing::{Missing, missing_text};
    if !have_gpu("rigs_not_paired_share_neither_the_list_nor_panes") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let pupu = named("pupu", &dir.path().join("pupu-id"))?;
    let kettu = named("kettu", &dir.path().join("kettu-id"))?;
    let (mut h, pupu_id) = on_pupu(&pupu, &dir.path().join("keys"));
    let kettu_id = connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(10));
    let listed = |a: &App| {
        let mut rigs: Vec<RigId> = a.state.list_rows().iter().map(|(r, _)| *r).collect();
        rigs.dedup();
        rigs
    };
    let pinned = |a: &App, rig: RigId| {
        a.state
            .layout
            .panes()
            .into_iter()
            .filter_map(|p| a.state.layout.view(p).copied())
            .filter(|v| v.rig == PaneRig::Rig(rig))
            .count()
    };
    let plate = missing_text(
        "kettu",
        Missing::NotPaired { shown: "pupu" },
        "the rig drop-down",
    );
    let plates = |h: &Harness<'_, App>| {
        h.query_all(egui_kittest::kittest::By::new().label(&plate))
            .count()
    };
    settle(&mut h, 2);
    assert_eq!(
        pinned(h.state(), pupu_id),
        0,
        "kettu's screen, no pane of pupu"
    );
    assert_eq!(listed(h.state()), [kettu_id], "kettu's list alone");

    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == pupu_id && live(a)
    });
    settle(&mut h, 2);
    assert_eq!(
        pinned(h.state(), kettu_id),
        0,
        "pupu's screen, no pane of kettu"
    );
    assert_eq!(listed(h.state()), [pupu_id], "pupu's list alone");
    let p = h.state().state.layout.focus;
    let before = *h.state().state.layout.view(p).ok_or("pane")?;
    drop_kettus(&mut h, kettu_id, MIC_FFT, p);
    assert_eq!(
        *h.state().state.layout.view(p).ok_or("pane")?,
        before,
        "not paired: kettu's row is no drop"
    );

    pair(&pupu, kettu_id, "kettu")?;
    step_until(&mut h, "kettu listed under pupu", |a| {
        listed(a) == [pupu_id, kettu_id]
    });
    drop_kettus(&mut h, kettu_id, MIC_FFT, p);
    let v = *h.state().state.layout.view(p).ok_or("pane")?;
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(kettu_id), Some(MIC_FFT)));
    settle(&mut h, 2);
    assert_eq!(
        plates(&h),
        0,
        "paired: kettu's curve draws on pupu's screen"
    );

    pupu.call(Command::RigPeers { peers: Vec::new() })?;
    step_until(&mut h, "pupu's list alone again", |a| {
        listed(a) == [pupu_id]
    });
    settle(&mut h, 2);
    assert_eq!(
        *h.state().state.layout.view(p).ok_or("pane")?,
        v,
        "the pane kept what it held"
    );
    assert_eq!(plates(&h), 1, "{plate}");

    pair(&pupu, kettu_id, "kettu")?;
    step_until(&mut h, "kettu listed under pupu", |a| {
        listed(a) == [pupu_id, kettu_id]
    });
    settle(&mut h, 2);
    assert_eq!(plates(&h), 0, "paired again: it draws");
    // Kettu shown lists pupu too: a pairing is the two daemons', whichever side takes.
    open_rig_menu(&mut h);
    h.key_press(Key::Home);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    step_until(&mut h, "kettu shown, pupu listed under it", |a| {
        a.state.active() == kettu_id && listed(a) == [kettu_id, pupu_id]
    });
    assert!(h.state().state.layout.view(p).is_none(), "on pupu's screen");
    Ok(())
}

/// Each rig armed in turn from the app, kettu fired: pupu shown, its top bar carries kettu's
/// chip; Shift+Esc on pupu stops both daemons' stimulus, kettu's too.
#[test]
fn shift_esc_stops_every_rig() -> R {
    if !have_gpu("shift_esc_stops_every_rig") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let pupu = named("pupu", &dir.path().join("pupu-id"))?;
    let kettu = named("kettu", &dir.path().join("kettu-id"))?;
    let (mut h, pupu_id) = on_pupu(&pupu, &dir.path().join("keys"));
    connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(10));
    let generator = |rig: &Rig| rig.cli.state().map(|s| s.generator.clone());
    let arm = |h: &mut Harness<'_, App>, rig: &Rig| {
        h.state_mut().state.rig.stimulus.level = Some(Dbfs(-24.0));
        h.state_mut().state.rig.stimulus.generator = Some(ac2_scene::stimulus::SignalKind::Pink);
        h.key_press(Key::Space);
        step_until(h, "armed", |_| generator(rig).is_some_and(|g| g.armed));
    };
    arm(&mut h, &kettu);
    h.key_press(Key::Enter);
    step_until(&mut h, "kettu firing", |_| {
        generator(&kettu).is_some_and(|g| g.firing)
    });
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == pupu_id && live(a)
    });
    arm(&mut h, &pupu);
    assert!(
        generator(&kettu).is_some_and(|g| g.armed),
        "kettu still armed"
    );
    // Kettu plays out of sight: the top bar says so beside pupu's own badge.
    let chip = "▶ kettu −24 dBFS";
    step_until(&mut h, "kettu's chip", |a| {
        a.state
            .live_elsewhere()
            .iter()
            .map(ac2_scene::stimulus::elsewhere_chip)
            .any(|c| c == chip)
    });
    settle(&mut h, 2);
    assert!(h.query_by_label(chip).is_some(), "the chip is drawn");
    h.key_press_modifiers(Modifiers::SHIFT, Key::Escape);
    let stopped =
        |rig: &Rig| generator(rig).is_some_and(|g| !g.armed && !g.firing && g.owner.is_none());
    step_until(&mut h, "both stopped", |_| {
        stopped(&pupu) && stopped(&kettu)
    });
    Ok(())
}

/// Pupu shown; the list shows kettu's rows under kettu's heading, its spectrum by kettu's
/// own name for it, and that row dragged onto a pane shows kettu's measurement there (not
/// pupu's of the same id), titled with kettu's name, pupu still shown.
#[test]
fn another_rigs_measurement_dropped_into_a_pane() -> R {
    use ac2_proto::model::MeasConfig;
    if !have_gpu("another_rigs_measurement_dropped_into_a_pane") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let pupu = named("pupu", &dir.path().join("pupu-id"))?;
    let kettu = named("kettu", &dir.path().join("kettu-id"))?;
    // Kettu's spectrum goes by another name than pupu's of the same id.
    let m = kettu
        .cli
        .state()
        .and_then(|s| s.measurements.iter().find(|m| m.id == MIC_FFT).cloned())
        .ok_or("kettu's spectrum")?;
    kettu.call(Command::MeasUpdate {
        meas: MIC_FFT,
        base: m.config_rev,
        config: MeasConfig {
            name: "Kettu FFT".into(),
            ..m.config.clone()
        },
    })?;
    let (mut h, pupu_id) = on_pupu(&pupu, &dir.path().join("keys"));
    let kettu_id = connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(10));
    pair(&pupu, kettu_id, "kettu")?;
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == pupu_id && live(a)
    });
    let p = h.state().state.layout.focus;
    step_until(&mut h, "kettu's rows listed", |a| {
        a.state
            .list_rows()
            .iter()
            .any(|(r, row)| *r == kettu_id && row.name.contains("Kettu FFT"))
    });
    settle(&mut h, 2);
    // Each rig's heading over its rows, kettu's measurement by its own name.
    for (what, label) in [("kettu's heading", "kettu"), ("its spectrum", "Kettu FFT")] {
        assert!(
            h.query_all(egui_kittest::kittest::By::new().label_contains(label))
                .next()
                .is_some(),
            "{what}"
        );
    }
    drop_kettus(&mut h, kettu_id, MIC_FFT, p);
    assert_eq!(h.state().state.active(), pupu_id, "the rig shown stays");
    let v = *h.state().state.layout.view(p).ok_or("pane")?;
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(kettu_id), Some(MIC_FFT)));
    assert_eq!(v.kind, PaneKind::Spectrum);
    settle(&mut h, 2);
    assert!(
        h.query_all_by_label_contains("kettu · ").next().is_some(),
        "the pane's title names kettu"
    );
    assert!(connected(h.state(), kettu_id));
    Ok(())
}

/// Connect a rig… to a daemon already in the list shows that rig, never a second row for
/// it: a second link to a daemon that does not answer would only retry beside the first.
#[test]
fn connecting_a_rig_already_listed_shows_it() -> R {
    if !have_gpu("connecting_a_rig_already_listed_shows_it") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let keys = dir.path().join("keys");
    let pupu = named("pupu", &dir.path().join("pupu-id"))?;
    let kettu = named("kettu", &dir.path().join("kettu-id"))?;
    let (mut h, pupu_id) = on_pupu(&pupu, &keys);
    let kettu_id = connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(5));
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| a.state.active() == pupu_id);
    let again = connect_kettu(&mut h, pupu_id, &kettu, Duration::from_secs(5));
    assert_eq!(again, kettu_id);
    assert_eq!(h.state().state.rigs().count(), 2, "kettu listed once");
    Ok(())
}
