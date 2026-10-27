//! An app showing a measurement keeps showing it while the daemon loses it for a while: a
//! session reopened and salvaged (the measurements gone, then recreated by name), or the
//! daemon restarted. Only an operator action brings a measurement's own panes: the
//! selection falling back to another measurement while the shown one is gone must not.

use super::harness::{DEADLINE, Driver, NAME, R};
use ac2_client::{Client, ClientConfig, Endpoints};
use ac2_proto::model::{MeasConfig, MeasKind, OwnedTraces};
use ac2_proto::units::MeasId;
use ac2_proto::{Command, ReplyBody};
use ac2_ui::embedded::{demo_measurement, demo_session};
use ac2_ui::forms::FormKind;
use ac2_ui::prefs::{PanePrefs, PanesPrefs, UiPrefs};
use ac2_ui::state::{AppState, Msg, Overlay, PaneId, PaneKind, PaneModes, PaneNode};
use std::path::Path;
use std::time::{Duration, Instant};

const SPL: &str = "FOH SPL";
const TF: &str = "TF 1";

/// Any free port.
fn any_port() -> Endpoints {
    Endpoints {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    }
}

/// A daemon on the simulated rig that autosaves to `dir` (and comes up with what it saved
/// there when `restore`), serving on `ep`.
fn daemon(dir: &Path, ep: &Endpoints, restore: bool) -> R<(ac2d::Handle, Endpoints)> {
    let listen = ac2d::Listen::Local {
        ctrl: ep.ctrl.clone(),
        data: ep.data.clone(),
    };
    let audio = ac2d::backend(ac2d::BackendChoice::Fake)?;
    let mut config = ac2d::DaemonConfig::new(audio, listen, -10.0);
    config.session_dir = dir.join("sessions");
    config.autosave = Some(ac2d::AutosaveConfig {
        dir: dir.join("autosave"),
        restore,
    });
    let handle = ac2d::Daemon::start(config)?;
    let ep = Endpoints {
        ctrl: handle.ctrl_endpoint().to_owned(),
        data: handle.data_endpoint().to_owned(),
    };
    Ok((handle, ep))
}

/// Commands as `ac2` sends them: a client of its own.
struct Cli {
    rt: tokio::runtime::Runtime,
    c: Client,
}

impl Cli {
    fn connect(ep: &Endpoints) -> R<Self> {
        let rt = tokio::runtime::Runtime::new()?;
        let c = rt.block_on(async {
            let c = Client::connect(ClientConfig::new(ep.clone(), "ac2-ui e2e cli")).await?;
            c.wait_synced(Duration::from_secs(10)).await?;
            Ok::<_, Box<dyn std::error::Error>>(c)
        })?;
        Ok(Self { rt, c })
    }

    fn call(&self, cmd: Command) -> R<ReplyBody> {
        Ok(self.rt.block_on(self.c.call(cmd))?)
    }
}

/// The kiosk's preferences: one SPL pane on `FOH SPL`, full screen.
fn kiosk_prefs() -> UiPrefs {
    let mut p = UiPrefs::default();
    p.layout.panes = Some(PanesPrefs {
        root: PaneNode::Leaf(PaneId(1)),
        focus: PaneId(1),
        views: vec![PanePrefs {
            id: PaneId(1),
            kind: PaneKind::Spl,
            measurement: Some(SPL.to_owned()),
            meters: None,
            modes: PaneModes::default(),
            levels: Default::default(),
            plates: Default::default(),
        }],
    });
    p.layout.fullscreen = true;
    p.bar_ease = false;
    p.spec_ease = false;
    p
}

fn meas_named(s: &AppState, name: &str) -> Option<MeasId> {
    s.measurements()
        .iter()
        .find(|m| m.config.name == name)
        .map(|m| m.id)
}

/// The kiosk's panes as it saves them: still the one SPL pane on `FOH SPL`.
fn saved_kiosk(s: &AppState) -> Result<(), String> {
    let saved = s.layout_prefs().panes.ok_or("no panes saved")?;
    let views: Vec<(PaneKind, Option<String>)> = saved
        .views
        .iter()
        .map(|v| (v.kind, v.measurement.clone()))
        .collect();
    if views == [(PaneKind::Spl, Some(SPL.to_owned()))] {
        Ok(())
    } else {
        Err(format!("the kiosk saves {views:?}"))
    }
}

/// The kiosk on screen: one SPL pane showing `FOH SPL`, selected.
fn shown_kiosk(s: &AppState) -> Result<(), String> {
    saved_kiosk(s)?;
    let leaves = s.layout.root.leaves();
    let [p] = leaves[..] else {
        return Err(format!("{} panes", leaves.len()));
    };
    let kind = s.layout.kind(p);
    let shown = s.pane_meas(p).map(|m| m.config.name.clone());
    if kind != PaneKind::Spl || shown.as_deref() != Some(SPL) {
        return Err(format!("the pane is {kind:?} on {shown:?}"));
    }
    let sel = s.selected_meas().map(|m| m.config.name.clone());
    if sel.as_deref() != Some(SPL) {
        return Err(format!("{sel:?} selected"));
    }
    Ok(())
}

/// Pumps both apps until `cond` holds on the kiosk, checking after every message that the
/// kiosk never saved anything but its own pane.
fn until_kiosk(
    desk: &mut Driver,
    kiosk: &mut Driver,
    what: &str,
    cond: impl Fn(&AppState) -> bool,
) -> R {
    let end = Instant::now() + DEADLINE;
    loop {
        desk.pump();
        for e in kiosk.conn.drain() {
            kiosk.send(Msg::Conn(kiosk.st.active(), Box::new(e)));
            saved_kiosk(&kiosk.st).map_err(|e| format!("waiting for {what}: {e}"))?;
        }
        if cond(&kiosk.st) {
            return Ok(());
        }
        if Instant::now() > end {
            return Err(format!("timed out waiting for {what}").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `TF 1` given its own panes again from the desk (selected there, a pane split), and
/// the kiosk has seen it.
fn desk_edits_tf(desk: &mut Driver, kiosk: &mut Driver) -> R {
    let tf = meas_named(&desk.st, TF).ok_or("TF 1 on the desk")?;
    let before = desk.st.meas(tf).and_then(|m| m.layout.clone());
    desk.send(Msg::SelectMeas(tf));
    desk.key("N");
    let edited = desk.st.pane_layout();
    assert_ne!(Some(&edited), before.as_ref());
    until_kiosk(desk, kiosk, "TF 1's new panes", |s| {
        meas_named(s, TF)
            .is_some_and(|id| s.meas(id).and_then(|m| m.layout.as_ref()) == Some(&edited))
    })
}

/// `FOH SPL` (first) and `TF 1` with three panes of its own (from the desk app), and the
/// kiosk showing `FOH SPL`.
fn rig(ep: &Endpoints, cli: &Cli) -> R<(Driver, Driver)> {
    cli.call(Command::SessionOpen {
        config: demo_session(),
    })?;
    let mut desk = Driver::connect(ClientConfig::new(ep.clone(), NAME), "local daemon")?;
    desk.synced()?;
    desk.until("the session", |s| s.open_session().is_some())?;
    desk.key("Ctrl+K");
    desk.send(Msg::Text("new spl".into()));
    desk.key("Enter");
    desk.until(
        "the SPL dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spl),
    )?;
    desk.key("Enter");
    desk.until("the SPL meter", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
    })?;
    let spl = desk.st.measurements()[0].clone();
    cli.call(Command::MeasUpdate {
        meas: spl.id,
        base: spl.config_rev,
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: SPL.into(),
            kind: spl.config.kind.clone(),
        },
    })?;
    let mut tf = demo_measurement();
    tf.name = TF.into();
    cli.call(Command::MeasCreate { config: tf })?;
    desk.until("both", |s| {
        meas_named(s, SPL).is_some() && meas_named(s, TF).is_some()
    })?;
    let tf = meas_named(&desk.st, TF).ok_or("TF 1")?;
    assert!(spl.id < tf, "FOH SPL first");
    desk.send(Msg::SelectMeas(tf));
    desk.key("N");
    desk.key("N");
    desk.until("TF 1's own panes", |s| {
        s.meas(tf)
            .and_then(|m| m.layout.as_ref())
            .is_some_and(|l| l.check().is_ok())
    })?;

    let mut kiosk = Driver::connect(
        ClientConfig::new(ep.clone(), "ac2-ui kiosk"),
        "local daemon",
    )?;
    kiosk.st.set_prefs(kiosk_prefs());
    kiosk.synced()?;
    until_kiosk(&mut desk, &mut kiosk, "the kiosk on FOH SPL", |s| {
        shown_kiosk(s).is_ok()
    })?;
    Ok((desk, kiosk))
}

/// `ac2 session salvage` as it remakes the measurements: each created, in order (`FOH SPL`
/// first), its layout set by a command of its own after. The kiosk has seen them all
/// before `TF 1`'s panes come, as a kiosk pumping its link between the commands does.
fn salvage(
    cli: &Cli,
    desk: &mut Driver,
    kiosk: &mut Driver,
    saved: &[ac2_proto::model::Measurement],
) -> R {
    for m in saved {
        cli.call(Command::MeasCreate {
            config: m.config.clone(),
        })?;
    }
    until_kiosk(desk, kiosk, "the measurements made again", |s| {
        saved
            .iter()
            .all(|m| meas_named(s, &m.config.name).is_some())
    })?;
    let tf = saved.iter().find(|m| m.config.name == TF).ok_or("TF 1")?;
    let layout = tf.layout.clone().ok_or("TF 1's panes")?;
    let id = meas_named(&kiosk.st, TF).ok_or("TF 1 again")?;
    cli.call(Command::MeasLayout {
        meas: id,
        layout: Some(layout.clone()),
    })?;
    until_kiosk(desk, kiosk, "TF 1's panes again", |s| {
        s.meas(id).and_then(|m| m.layout.as_ref()) == Some(&layout)
    })?;
    for m in saved {
        let id = meas_named(&kiosk.st, &m.config.name).ok_or("made again")?;
        cli.call(Command::MeasStart { meas: id })?;
    }
    until_kiosk(desk, kiosk, "the kiosk on FOH SPL", |s| {
        shown_kiosk(s).is_ok()
    })
}

fn saved_measurements(s: &AppState) -> Vec<ac2_proto::model::Measurement> {
    s.measurements().into_iter().cloned().collect()
}

/// `ac2 session open` with the measurements gone meanwhile, then `ac2 session salvage`
/// making them again by name. The kiosk shows `FOH SPL` throughout and never takes
/// `TF 1`'s panes, not even when the desk edits them after.
#[test]
fn a_salvaged_session_keeps_the_kiosk_on_its_meter() -> R {
    let dir = tempfile::tempdir()?;
    let (handle, ep) = daemon(dir.path(), &any_port(), true)?;
    let cli = Cli::connect(&ep)?;
    let (mut desk, mut kiosk) = rig(&ep, &cli)?;
    let saved = saved_measurements(&kiosk.st);

    cli.call(Command::SessionOpen {
        config: demo_session(),
    })?;
    for m in &saved {
        cli.call(Command::MeasDelete {
            meas: m.id,
            traces: OwnedTraces::Delete,
        })?;
    }
    until_kiosk(&mut desk, &mut kiosk, "no measurements", |s| {
        s.measurements().is_empty()
    })?;
    salvage(&cli, &mut desk, &mut kiosk, &saved)?;
    desk_edits_tf(&mut desk, &mut kiosk)?;
    shown_kiosk(&kiosk.st)?;

    drop((desk, kiosk, cli));
    handle.shutdown();
    Ok(())
}

/// A daemon swapped in on the same address with nothing in it, `ac2 session open`, then
/// `ac2 session salvage`: the measurements come back under their names and ids. The kiosk
/// is back on `FOH SPL` and never takes `TF 1`'s panes.
#[test]
fn a_swapped_daemon_salvaged_keeps_the_kiosk_on_its_meter() -> R {
    let dir = tempfile::tempdir()?;
    let (handle, ep) = daemon(dir.path(), &any_port(), true)?;
    let cli = Cli::connect(&ep)?;
    let (mut desk, mut kiosk) = rig(&ep, &cli)?;
    let saved = saved_measurements(&kiosk.st);
    drop(cli);
    let incarnation = kiosk.st.rig.mirror.as_ref().and_then(|m| m.incarnation);
    handle.shutdown();

    let fresh = tempfile::tempdir()?;
    let (handle, _) = daemon(fresh.path(), &ep, false)?;
    until_kiosk(&mut desk, &mut kiosk, "the new daemon", |s| {
        s.rig
            .mirror
            .as_ref()
            .is_some_and(|m| m.synced() && m.incarnation != incarnation)
    })?;
    desk.until("the desk on the new daemon", |s| {
        s.rig
            .mirror
            .as_ref()
            .is_some_and(|m| m.synced() && m.incarnation != incarnation)
    })?;
    let cli = Cli::connect(&ep)?;
    cli.call(Command::SessionOpen {
        config: demo_session(),
    })?;
    salvage(&cli, &mut desk, &mut kiosk, &saved)?;
    let ids: Vec<MeasId> = kiosk.st.measurements().iter().map(|m| m.id).collect();
    let old: Vec<MeasId> = saved.iter().map(|m| m.id).collect();
    assert_eq!(ids, old, "the same ids");
    desk_edits_tf(&mut desk, &mut kiosk)?;
    shown_kiosk(&kiosk.st)?;

    drop((desk, kiosk, cli));
    handle.shutdown();
    Ok(())
}

/// The daemon restarted on the same address, coming up with the same measurements from its
/// autosave: the kiosk reconnects to `FOH SPL` and never takes `TF 1`'s panes.
#[test]
fn a_restarted_daemon_keeps_the_kiosk_on_its_meter() -> R {
    let dir = tempfile::tempdir()?;
    let (handle, ep) = daemon(dir.path(), &any_port(), true)?;
    let cli = Cli::connect(&ep)?;
    let (mut desk, mut kiosk) = rig(&ep, &cli)?;
    drop(cli);
    desk.until("autosaved", |s| {
        s.daemon().is_some_and(|x| {
            x.autosave.state == ac2_proto::model::AutosaveState::Saved
                && x.autosave.saved_at.is_some()
        })
    })?;
    let incarnation = kiosk.st.rig.mirror.as_ref().and_then(|m| m.incarnation);
    handle.shutdown();

    let (handle, _) = daemon(dir.path(), &ep, true)?;
    until_kiosk(&mut desk, &mut kiosk, "the restarted daemon", |s| {
        s.rig
            .mirror
            .as_ref()
            .is_some_and(|m| m.synced() && m.incarnation != incarnation)
            && meas_named(s, SPL).is_some()
            && meas_named(s, TF).is_some()
    })?;
    until_kiosk(&mut desk, &mut kiosk, "the kiosk back on FOH SPL", |s| {
        shown_kiosk(s).is_ok()
    })?;
    desk.until("the desk reconnected", |s| {
        s.rig
            .mirror
            .as_ref()
            .is_some_and(|m| m.synced() && m.incarnation != incarnation)
    })?;
    desk_edits_tf(&mut desk, &mut kiosk)?;
    shown_kiosk(&kiosk.st)?;

    drop((desk, kiosk));
    handle.shutdown();
    Ok(())
}

/// The desk lays out `FOH SPL`'s own panes (its row chosen there, a pane split, a view
/// stepped) while the kiosk shows it: the kiosk keeps its one SPL pane on `FOH SPL`, as
/// nobody chose that measurement's row on the kiosk.
#[test]
fn a_desk_laying_out_the_kiosk_meter_keeps_the_kiosk_on_its_meter() -> R {
    let dir = tempfile::tempdir()?;
    let (handle, ep) = daemon(dir.path(), &any_port(), true)?;
    let cli = Cli::connect(&ep)?;
    let (mut desk, mut kiosk) = rig(&ep, &cli)?;
    let spl = meas_named(&desk.st, SPL).ok_or("FOH SPL on the desk")?;
    desk.send(Msg::SelectMeas(spl));
    for key in ["N", "G"] {
        desk.key(key);
        let edited = desk.st.pane_layout();
        until_kiosk(&mut desk, &mut kiosk, "FOH SPL's new panes", |s| {
            meas_named(s, SPL)
                .is_some_and(|id| s.meas(id).and_then(|m| m.layout.as_ref()) == Some(&edited))
        })?;
        shown_kiosk(&kiosk.st)?;
    }

    drop((desk, kiosk, cli));
    handle.shutdown();
    Ok(())
}
