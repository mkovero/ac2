//! Many rigs against two fake daemons: Connect… adds a rig and shows it, the drop-down
//! switches between them, a rig gone quiet greys out and comes back, and Shift+Esc stops
//! the stimulus each rig holds.

use super::*;
use ac2_proto::units::{Dbfs, RigId};
use ac2_ui::connect::Choice;
use ac2_ui::prefs::RigAddr;
use ac2_ui::state::PaneRig;

const PUPU: RigId = RigId(0x7075_7075);
const KETTU: RigId = RigId(0x6b65_7474);

/// The app on `pupu`, able to open the connect dialog (no network browse, an empty key
/// directory).
fn on_pupu(pupu: &common::Rig, keys: &std::path::Path) -> Harness<'static, App> {
    let mut opts = options(Some(pupu));
    opts.connect = Some(ac2_ui::app::ConnectSetup {
        key_dir: keys.to_path_buf(),
        client_name: "ac2-ui test".into(),
        discovery: false,
    });
    let mut h = harness(opts);
    step_until(&mut h, "pupu live", |a| live(a) && a.state.rig.id == PUPU);
    h
}

/// Connect a rig… from the palette, the dialog's choice `kettu` at its endpoints, shown once
/// its mirror is in step (the panes keep pupu's measurements, pinned to it by the switch).
/// The watchdog gives up on a silent link after `dead_after`.
fn connect_kettu(h: &mut Harness<'_, App>, kettu: &common::Rig, dead_after: Duration) {
    connect_kettu_until(h, kettu, dead_after, synced);
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

/// Each rig armed in turn, kettu fired: shown pupu's top bar carries kettu's chip; Shift+Esc
/// on pupu stops both, kettu's too.
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
    h.key_press(Key::Enter);
    step_until(&mut h, "kettu firing", |_| {
        kettu.fake.lock().state.generator.firing
    });
    pick_first_rig(&mut h);
    step_until(&mut h, "pupu shown", |a| {
        a.state.active() == PUPU && live(a)
    });
    arm(&mut h, &pupu.fake);
    assert!(kettu.fake.lock().state.generator.armed, "kettu still armed");
    // Kettu plays out of sight: the top bar says so beside pupu's own badge.
    let chip = "▶ kettu −24 dBFS";
    step_until(&mut h, "kettu's chip", |a| {
        a.state
            .live_elsewhere()
            .iter()
            .map(ac2_scene::stimulus::elsewhere_chip)
            .any(|c| c == chip)
    });
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "rig_stimulus_chip");
    assert!(h.query_by_label(chip).is_some(), "the chip is drawn");
    h.key_press_modifiers(Modifiers::SHIFT, Key::Escape);
    let stopped = |fake: &ac2_client::fake::FakeDaemon| {
        let s = fake.lock();
        !s.state.generator.armed && !s.state.generator.firing && s.state.generator.owner.is_none()
    };
    step_until(&mut h, "both stopped", |_| {
        stopped(&pupu.fake) && stopped(&kettu.fake)
    });
}

/// Kettu's measurement 3's row of the list dragged onto the middle of pane `p`.
fn drop_kettus(h: &mut Harness<'_, App>, p: ac2_ui::state::PaneId) {
    use ac2_scene::meas_list::TreeKey;
    use ac2_ui::state::{DragItem, DragMsg, DropTarget, Msg};
    let key = TreeKey::Meas(MeasId(3));
    let a = h.state_mut();
    a.dispatch(Msg::RowDrag(DragMsg::Start(DragItem::Row {
        rig: KETTU,
        key,
    })));
    a.dispatch(Msg::RowDrag(DragMsg::Over(Some(DropTarget {
        pane: p,
        edge: None,
    }))));
    a.dispatch(Msg::RowDrag(DragMsg::Drop));
    h.step();
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

/// Pupu shown; the list shows kettu's rows under kettu's heading, and kettu's spectrum
/// dragged onto a pane shows kettu's measurement 3 there (not pupu's of that id), titled with
/// kettu's name, pupu still shown.
#[test]
fn another_rigs_measurement_dropped_into_a_pane() {
    if !have_gpu("another_rigs_measurement_dropped_into_a_pane") {
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
    step_until(&mut h, "kettu's rows listed", |a| {
        a.state
            .list_rows()
            .iter()
            .any(|(r, row)| *r == KETTU && row.name.contains("Kettu FFT"))
    });
    h.step();
    // Each rig's heading over its rows, kettu's measurement by its own name.
    assert!(
        h.query_all(egui_kittest::kittest::By::new().label_contains("kettu"))
            .next()
            .is_some(),
        "kettu's heading"
    );
    assert!(
        h.query_all(egui_kittest::kittest::By::new().label_contains("Kettu FFT"))
            .next()
            .is_some(),
        "kettu's measurement listed"
    );
    h.state_mut().state.toasts.list.clear();
    snapshot(&mut h, "list_two_rigs");
    drop_kettus(&mut h, p);
    assert_eq!(h.state().state.active(), PUPU, "the rig shown stays");
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
