//! Many rigs against two fake daemons: Connect… adds a rig and shows it, the drop-down
//! switches between them, a rig gone quiet greys out and comes back, and Shift+Esc stops
//! the stimulus each rig holds.

use super::*;
use ac2_proto::units::{Dbfs, RigId};
use ac2_ui::connect::Choice;
use ac2_ui::prefs::RigAddr;
use ac2_ui::state::{PaneMenuRow, PaneRig};

const PUPU: RigId = RigId(0x7075_7075);
const KETTU: RigId = RigId(0x6b65_7474);

/// The app on `pupu`, able to open the connect dialog (no network browse, an empty key
/// directory).
fn on_pupu(pupu: &common::Rig, keys: &std::path::Path) -> Harness<'static, App> {
    let mut opts = options(Some(pupu));
    opts.connect = Some(ac2_ui::app::ConnectSetup {
        key_dir: keys.to_path_buf(),
        client_name: "ac2-ui test".into(),
        backends: vec![],
        discovery: false,
    });
    let mut h = harness(opts);
    step_until(&mut h, "pupu live", |a| live(a) && a.state.rig.id == PUPU);
    h
}

/// Connect a rig… from the palette, the dialog's choice `kettu` at its endpoints. The
/// watchdog gives up on a silent link after `dead_after`.
fn connect_kettu(h: &mut Harness<'_, App>, kettu: &common::Rig, dead_after: Duration) {
    connect_kettu_until(h, kettu, dead_after, live);
}

/// The mirror is in step with the daemon: what a rig with no pane of its own on screen
/// (nothing subscribed) can be waited for.
fn synced(a: &App) -> bool {
    matches!(a.state.rig.conn, ConnState::Connected { .. })
        && a.state.rig.mirror.as_ref().is_some_and(|m| m.synced())
}

/// [`connect_kettu`], kettu shown once `ready`.
fn connect_kettu_until(
    h: &mut Harness<'_, App>,
    kettu: &common::Rig,
    dead_after: Duration,
    ready: fn(&App) -> bool,
) {
    palette(h, "connect a rig");
    step_until(h, "the connect dialog", |a| a.connect_open());
    let ep = kettu.fake.endpoints();
    let addr = RigAddr::Endpoints {
        ctrl: ep.ctrl.clone(),
        data: ep.data.clone(),
    };
    let mut config = ClientConfig::new(ep, "ac2-ui test");
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
        !a.connect_open() && a.state.active() == KETTU && ready(a)
    });
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

/// Connect… adds kettu beside pupu and shows it; pupu stays linked. The drop-down goes back
/// to pupu, and kettu's own panes carry its name. Kettu is remembered by its address.
#[test]
fn a_second_rig_connects_and_the_drop_down_switches() {
    if !have_gpu("a_second_rig_connects_and_the_drop_down_switches") {
        return;
    }
    let keys = tempfile::tempdir().expect("key directory");
    let pupu = common::Rig::start_as(PUPU, "pupu");
    let kettu = common::Rig::start_as(KETTU, "kettu");
    let mut h = on_pupu(&pupu, keys.path());
    connect_kettu(&mut h, &kettu, Duration::from_secs(10));
    assert!(connected(h.state(), PUPU), "pupu keeps its link");
    assert!(
        h.state()
            .state
            .prefs
            .rigs
            .iter()
            .any(|r| r.id == KETTU && r.name == "kettu")
    );

    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == PUPU && live(a)
    });
    assert!(
        connected(h.state(), KETTU),
        "kettu keeps its link in the background"
    );
    // Kettu's panes stay beside pupu's, titled with its name; the drop-down lists both.
    open_rig_menu(&mut h);
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "rig_menu");
    h.key_press(Key::Escape);
    step_until(&mut h, "the drop-down closed", |a| {
        a.state.overlay == Overlay::None
    });
}

/// Kettu stops answering: its row and its panes say so, pupu goes on; once it answers
/// again, it is connected anew without the operator doing anything.
#[test]
fn a_rig_down_and_back() {
    if !have_gpu("a_rig_down_and_back") {
        return;
    }
    let keys = tempfile::tempdir().expect("key directory");
    let pupu = common::Rig::start_as(PUPU, "pupu");
    let kettu = common::Rig::start_as(KETTU, "kettu");
    let mut h = on_pupu(&pupu, keys.path());
    connect_kettu(&mut h, &kettu, Duration::from_millis(600));
    // Pupu shown, with kettu's panes beside its own.
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| a.state.active() == PUPU);
    kettu.fake.lock().ka_paused = true;
    step_until(&mut h, "kettu down", |a| !connected(a, KETTU));
    let away = ac2_scene::chrome::rig_pane_unreachable("kettu");
    // The link reconnects on its own and the paused keepalives drop it again: whenever it
    // is down as a frame is drawn, kettu's panes say so.
    assert!(
        h.query_by_label(&away).is_some() || connected(h.state(), KETTU),
        "kettu's panes name it unreachable"
    );
    assert!(connected(h.state(), PUPU), "pupu is untouched");
    kettu.fake.lock().ka_paused = false;
    step_until(&mut h, "kettu back", |a| connected(a, KETTU));
    assert!(h.query_by_label(&away).is_none(), "kettu's panes are back");
}

/// Each rig armed in turn; Shift+Esc on pupu stops both, kettu's too.
#[test]
fn shift_esc_stops_every_rig() {
    if !have_gpu("shift_esc_stops_every_rig") {
        return;
    }
    let keys = tempfile::tempdir().expect("key directory");
    let pupu = common::Rig::start_as(PUPU, "pupu");
    let kettu = common::Rig::start_as(KETTU, "kettu");
    let mut h = on_pupu(&pupu, keys.path());
    connect_kettu(&mut h, &kettu, Duration::from_secs(10));
    let arm = |h: &mut Harness<'_, App>, fake: &ac2_client::fake::FakeDaemon| {
        h.state_mut().state.rig.stimulus.level = Some(Dbfs(-24.0));
        h.key_press(Key::Space);
        step_until(h, "armed", |_| fake.lock().state.generator.armed);
    };
    arm(&mut h, &kettu.fake);
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == PUPU && live(a)
    });
    arm(&mut h, &pupu.fake);
    assert!(kettu.fake.lock().state.generator.armed, "kettu still armed");
    h.key_press_modifiers(Modifiers::SHIFT, Key::Escape);
    let stopped = |fake: &ac2_client::fake::FakeDaemon| {
        let s = fake.lock();
        !s.state.generator.armed && !s.state.generator.firing && s.state.generator.owner.is_none()
    };
    step_until(&mut h, "both stopped", |_| {
        stopped(&pupu.fake) && stopped(&kettu.fake)
    });
}

/// The readout of the shown rig's Inputs-list row for input 1.
fn in1_reading(a: &App) -> Option<String> {
    a.state
        .session_inputs()
        .into_iter()
        .find(|r| r.channel == 0)
        .map(|r| r.reading.text)
}

/// Both fakes meter their inputs, input 1 peaking at `peak` dBFS (its RMS 3 dB below).
fn metering(rig: &common::Rig, peak: f32) {
    let mut s = rig.fake.lock();
    s.set_input_peak(0, peak);
    s.meters = true;
}

/// The Inputs list meters the rig shown, whichever was shown before and however the rig
/// was added: kettu's once Connect… shows it, pupu's again after the drop-down goes back,
/// both following their own daemon's levels.
#[test]
fn the_inputs_meter_the_rig_shown() {
    if !have_gpu("the_inputs_meter_the_rig_shown") {
        return;
    }
    let keys = tempfile::tempdir().expect("key directory");
    let pupu = common::Rig::start_as(PUPU, "pupu");
    let kettu = common::Rig::start_as(KETTU, "kettu");
    metering(&pupu, -10.0);
    metering(&kettu, -20.0);
    let mut h = on_pupu(&pupu, keys.path());
    step_until(&mut h, "pupu's in 1 metered", |a| {
        in1_reading(a).as_deref() == Some("−13.0")
    });
    connect_kettu(&mut h, &kettu, Duration::from_secs(10));
    step_until(&mut h, "kettu's in 1 metered", |a| {
        in1_reading(a).as_deref() == Some("−23.0")
    });
    kettu.fake.lock().set_input_peak(0, -25.0);
    step_until(&mut h, "kettu's in 1 following it", |a| {
        in1_reading(a).as_deref() == Some("−28.0")
    });

    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| a.state.active() == PUPU);
    pupu.fake.lock().set_input_peak(0, -30.0);
    step_until(&mut h, "pupu's in 1 following it again", |a| {
        in1_reading(a).as_deref() == Some("−33.0")
    });
}

/// The focused pane's list opened from the palette (*Choose pane measurement…*).
fn open_pane_list(h: &mut Harness<'_, App>) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.event(Event::Text("choose pane measurement".into()));
    step_until(h, "palette typed", |a| {
        matches!(&a.state.overlay, Overlay::Palette(_))
    });
    h.key_press(Key::Enter);
    step_until(h, "the pane's list", |a| {
        matches!(a.state.overlay, Overlay::PaneMenu(_))
    });
}

/// ↓ in the open pane list until `row` is highlighted, then Enter.
fn take_row(h: &mut Harness<'_, App>, row: PaneMenuRow) {
    let at = |a: &App| {
        let Overlay::PaneMenu(m) = a.state.overlay else {
            return None;
        };
        Some((m.index, a.state.pane_menu_rows(m.pane)))
    };
    h.key_press(Key::Home);
    for _ in 0..40 {
        h.step();
        let (i, rows) = at(h.state()).expect("the list open");
        if rows.get(i).map(|(r, _)| *r) == Some(row) {
            h.key_press(Key::Enter);
            step_until(h, "the list closed", |a| a.state.overlay == Overlay::None);
            return;
        }
        h.key_press(Key::ArrowDown);
    }
    panic!("{row:?} not in the list: {:?}", at(h.state()));
}

/// Kettu's spectrum measurement goes by another name than pupu's of the same id.
fn rename_kettu_fft(kettu: &common::Rig) {
    use ac2_proto::{Change, Patch};
    let mut s = kettu.fake.lock();
    let mut m = s
        .state
        .measurements
        .iter()
        .find(|m| m.id == MeasId(3))
        .cloned()
        .expect("measurement 3");
    m.config.name = "Kettu FFT".into();
    s.commit(Change::Measurement(Patch::Set(m)));
}

/// Pupu shown; kettu's spectrum picked from the pane's list, under kettu's heading: the pane
/// shows kettu's measurement 3 (not pupu's of that id), titled with kettu's name.
#[test]
fn another_rigs_measurement_picked_into_a_pane() {
    if !have_gpu("another_rigs_measurement_picked_into_a_pane") {
        return;
    }
    let keys = tempfile::tempdir().expect("key directory");
    let pupu = common::Rig::start_as(PUPU, "pupu");
    let kettu = common::Rig::start_as(KETTU, "kettu");
    rename_kettu_fft(&kettu);
    let mut h = on_pupu(&pupu, keys.path());
    connect_kettu(&mut h, &kettu, Duration::from_secs(10));
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == PUPU && live(a)
    });
    let p = h.state().state.layout.focus;
    open_pane_list(&mut h);
    h.step();
    // Each rig's heading over its rows, kettu's measurement by its own name.
    assert!(h.query_by_label("kettu").is_some(), "kettu's heading");
    assert!(
        h.query_by_label_contains("Kettu FFT").is_some(),
        "kettu's measurement listed"
    );
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "pane_list_two_rigs");
    take_row(&mut h, PaneMenuRow::RigMeas(KETTU, MeasId(3)));
    let v = *h.state().state.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(KETTU), Some(MeasId(3))));
    assert_eq!(v.kind, PaneKind::Spectrum);
    step_until(&mut h, "kettu's title drawn", |_| true);
    assert!(
        h.query_all_by_label_contains("kettu · ").next().is_some(),
        "the pane's title names kettu"
    );
    assert!(connected(h.state(), KETTU));
}

/// A session saved on pupu with its measurement's own panes (`ThisRig`) and loaded on
/// kettu: choosing that measurement on kettu brings kettu's panes of it, on kettu's ids;
/// back on pupu its own come back beside kettu's, which stay kettu's.
#[test]
fn a_session_with_its_own_panes_loads_on_either_rig() {
    use ac2_proto::model::SessionRef;
    if !have_gpu("a_session_with_its_own_panes_loads_on_either_rig") {
        return;
    }
    let keys = tempfile::tempdir().expect("key directory");
    let dir = tempfile::tempdir().expect("session directory");
    let pupu = common::Rig::start_in(PUPU, "pupu", dir.path());
    let kettu = common::Rig::start_in(KETTU, "kettu", dir.path());
    let mut h = on_pupu(&pupu, keys.path());
    connect_kettu(&mut h, &kettu, Duration::from_secs(10));
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == PUPU && live(a)
    });
    // Mic 1 FFT chosen on pupu and its panes edited: its layout is stored on pupu.
    tab_to(&mut h, MeasId(3));
    h.key_press(Key::N);
    let stored = |fake: &ac2_client::fake::FakeDaemon| {
        fake.lock()
            .state
            .measurements
            .iter()
            .find(|m| m.id == MeasId(3))
            .and_then(|m| m.layout.clone())
    };
    step_until(&mut h, "pupu stores the layout", |_| {
        stored(&pupu.fake).is_some()
    });
    let cli = |fake: &ac2_client::fake::FakeDaemon, cmd: ac2_proto::Command| {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let c =
                ac2_client::Client::connect(ClientConfig::new(fake.endpoints(), "ac2-ui e2e cli"))
                    .await
                    .expect("connect");
            c.wait_synced(Duration::from_secs(10))
                .await
                .expect("synced");
            c.call(cmd).await.expect("call")
        })
    };
    let session = SessionRef::Name {
        name: "both rigs".into(),
    };
    cli(
        &pupu.fake,
        ac2_proto::Command::FileSave {
            session: session.clone(),
            overwrite: true,
        },
    );
    cli(
        &kettu.fake,
        ac2_proto::Command::FileLoad {
            session,
            discard: true,
        },
    );
    assert_eq!(stored(&kettu.fake), stored(&pupu.fake));
    let own = |a: &App, rig: PaneRig| {
        a.state
            .layout
            .panes()
            .into_iter()
            .filter_map(|p| a.state.layout.view(p))
            .filter(|v| v.rig == rig && v.meas == Some(MeasId(3)))
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
        a.state.active() == KETTU && synced(a)
    });
    step_until(&mut h, "kettu has the session", |a| {
        a.state.meas(MeasId(3)).is_some_and(|m| m.layout.is_some())
    });
    tab_to(&mut h, MeasId(3));
    step_until(&mut h, "kettu's own panes", |a| {
        own(a, PaneRig::Active) >= pupus && own(a, PaneRig::Rig(PUPU)) >= pupus
    });
    // Back on pupu: kettu's stay kettu's, pupu's are pupu's own again.
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| a.state.active() == PUPU);
    let a = h.state();
    assert!(own(a, PaneRig::Rig(KETTU)) >= pupus);
    assert!(own(a, PaneRig::Active) >= pupus);
}

/// A pane pinned to kettu; kettu closed (the pane reads *kettu not connected*), then kettu
/// reached again on another address: the pin finds it by its id and shows its measurement.
#[test]
fn a_pin_finds_its_rig_on_another_address() {
    if !have_gpu("a_pin_finds_its_rig_on_another_address") {
        return;
    }
    let keys = tempfile::tempdir().expect("key directory");
    let pupu = common::Rig::start_as(PUPU, "pupu");
    let kettu = common::Rig::start_as(KETTU, "kettu");
    let mut h = on_pupu(&pupu, keys.path());
    connect_kettu(&mut h, &kettu, Duration::from_secs(10));
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == PUPU && live(a)
    });
    let p = h.state().state.layout.focus;
    open_pane_list(&mut h);
    take_row(&mut h, PaneMenuRow::RigMeas(KETTU, MeasId(3)));
    // Kettu shown and closed from the drop-down (its last row).
    open_rig_menu(&mut h);
    h.key_press(Key::Home);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    step_until(&mut h, "kettu shown", |a| a.state.active() == KETTU);
    open_rig_menu(&mut h);
    h.key_press(Key::End);
    h.key_press(Key::Enter);
    step_until(&mut h, "kettu closed", |a| {
        a.state.active() == PUPU && a.state.find_rig(KETTU).is_none()
    });
    let away = ac2_scene::chrome::rig_pane_closed("kettu");
    step_until(&mut h, "the pane says so", |_| true);
    assert!(h.query_by_label(&away).is_some(), "{away}");
    drop(kettu);
    let moved = common::Rig::start_as(KETTU, "kettu");
    connect_kettu_until(&mut h, &moved, Duration::from_secs(10), synced);
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown, the pane on kettu's measurement", |a| {
        a.state.active() == PUPU
            && a.state
                .layout
                .view(p)
                .is_some_and(|v| v.rig == PaneRig::Rig(KETTU) && v.meas == Some(MeasId(3)))
    });
    assert!(h.query_by_label(&away).is_none(), "kettu's pane is back");
}
