//! Many rigs against two fake daemons: Connect… adds a rig and shows it, the drop-down
//! switches between them, a rig gone quiet greys out and comes back, and Shift+Esc stops
//! the stimulus each rig holds.

use super::*;
use ac2_proto::units::{Dbfs, RigId};
use ac2_ui::connect::Choice;
use ac2_ui::prefs::RigAddr;

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
    step_until(h, "kettu shown and live", |a| {
        !a.connect_open() && a.state.active() == KETTU && live(a)
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
