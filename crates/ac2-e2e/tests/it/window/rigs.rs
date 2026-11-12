//! Many rigs against two real daemons: a rig gone quiet greys out and comes back, the Inputs
//! list meters the rig shown, a session's own panes load on either rig, a pin finds its
//! rig on another address.

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
/// its mirror is in step (the panes keep pupu's measurements, pinned to it by the switch);
/// kettu's id. The watchdog gives up on a silent link after `dead_after`.
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
    let away = ac2_scene::chrome::rig_pane_unreachable("kettu");
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
/// kettu: choosing that measurement on kettu brings kettu's panes of it, on kettu's ids;
/// back on pupu its own come back beside kettu's, which stay kettu's.
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
        own(a, PaneRig::Active) >= pupus && own(a, PaneRig::Rig(pupu_id)) >= pupus
    });
    // Back on pupu: kettu's stay kettu's, pupu's are pupu's own again.
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| a.state.active() == pupu_id);
    let a = h.state();
    assert!(own(a, PaneRig::Rig(kettu_id)) >= pupus);
    assert!(own(a, PaneRig::Active) >= pupus);
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
