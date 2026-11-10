//! The `ac2d` processes the app starts: this computer's when none serves, the simulated rig,
//! and what their ends mean for the rigs they were started for ([`crate::spawn`]).

use super::*;
use crate::spawn::{self, Ended, Kind, Spawned};
use ac2_client::endpoint::LockHolder;
use ac2_client::{ClientConfig, Endpoints};
use ac2_proto::model::BackendKind;

/// How long the endpoints file of a daemon holding the lock may take to appear: it is
/// written once that daemon has bound its sockets.
const HOLDER_WAIT: Duration = Duration::from_secs(10);

/// The children the app started and what waits on them.
#[derive(Debug, Default)]
pub(super) struct Daemons {
    /// By the slot of the rig each was started for: owned, or detached and watched until its
    /// rig's link connects (until then its end says whether it started).
    children: Vec<Watched>,
    /// Rigs whose `ac2d` lost the lock, since when: the holder's endpoints file is read
    /// until it says where that one listens.
    lost: Vec<(u32, Instant)>,
    /// Rigs started on a named backend (`ac2-ui --backend`): the daemon reached must run on
    /// it, judged once its device list arrives.
    wanted: Vec<Wanted>,
    /// Where owned children go when the window closes; `None`: stopped then and there.
    at_exit: Option<spawn::AtExit>,
    /// Why `--smoke` fails, when a daemon did not start or was refused.
    pub(super) smoke_failure: Option<String>,
    /// The `ac2d` to start in place of the one beside the app ([`App::use_ac2d`]).
    exe: Option<std::path::PathBuf>,
}

/// An app dropped without closing its window (a test harness, a panic unwinding) still
/// stops what it owns and waits for it: a child left running would hold its executable and
/// its sockets past the app.
impl Drop for Daemons {
    fn drop(&mut self) {
        let owned: Vec<Spawned> = std::mem::take(&mut self.children)
            .into_iter()
            .map(|c| c.child)
            .filter(Spawned::owned)
            .collect();
        spawn::stop_all(owned);
    }
}

#[derive(Debug)]
struct Watched {
    slot: u32,
    child: Spawned,
    /// The rig's link connected while it ran.
    served: bool,
}

#[derive(Debug)]
struct Wanted {
    slot: u32,
    backend: BackendKind,
    asked: bool,
}

impl Daemons {
    pub(super) fn new(at_exit: Option<spawn::AtExit>) -> Self {
        let mut d = Self::default();
        d.at_exit = at_exit;
        d
    }

    /// The owned child of the rig in `slot`, given up to be stopped after its link. A
    /// detached one stays running and is no longer watched.
    pub(super) fn release(&mut self, slot: u32) -> Option<Spawned> {
        self.lost.retain(|l| l.0 != slot);
        self.wanted.retain(|w| w.slot != slot);
        let i = self.children.iter().position(|c| c.slot == slot)?;
        let c = self.children.remove(i);
        c.child.owned().then_some(c.child)
    }

    /// The window closes and every link is closed: the owned children stop, after the
    /// window is gone when the caller takes them.
    pub(super) fn at_exit(&mut self) {
        let owned: Vec<Spawned> = std::mem::take(&mut self.children)
            .into_iter()
            .map(|c| c.child)
            .filter(Spawned::owned)
            .collect();
        match &self.at_exit {
            Some(a) => a.hand_over(owned),
            None => spawn::stop_all(owned),
        }
    }
}

impl App {
    /// Starts `exe` wherever the app would start the `ac2d` beside it: for a test whose own
    /// binary has no `ac2d` beside it.
    pub fn use_ac2d(&mut self, exe: std::path::PathBuf) {
        self.daemons.exe = Some(exe);
    }

    fn ac2d(&self) -> Result<std::path::PathBuf, String> {
        match &self.daemons.exe {
            Some(p) => Ok(p.clone()),
            None => spawn::find_ac2d(),
        }
    }

    fn client_name(&self) -> String {
        self.connect_setup
            .as_ref()
            .map_or_else(|| "ac2-ui".to_owned(), |c| c.client_name.clone())
    }

    /// This computer's `ac2d` for the rig in `slot`: its default endpoints, reached at once,
    /// and the bundled `ac2d` started beside the link unless Settings › Connection says not
    /// to. Which of them serves is the daemon's instance lock's to decide.
    pub(super) fn start_local(
        &mut self,
        slot: u32,
        backend: Option<BackendKind>,
    ) -> Result<Target, String> {
        let prefs = self.state.prefs.daemon;
        if prefs.start_local {
            let exe = self.ac2d()?;
            let owned = prefs.stop_on_quit || spawn::stop_forced();
            let child = spawn::spawn_local(&exe, backend, owned)?;
            self.daemons.children.push(Watched {
                slot,
                child,
                served: false,
            });
        }
        if let Some(backend) = backend {
            self.daemons.wanted.push(Wanted {
                slot,
                backend,
                asked: false,
            });
        }
        Ok(Target {
            config: ClientConfig::local(self.client_name()),
            describe: "local daemon".into(),
        })
    }

    /// The simulated rig for the rig in `slot`, started, set up measuring and owned. One per
    /// app.
    pub(super) fn start_simulated(&mut self, slot: u32) -> Result<Target, String> {
        if self
            .daemons
            .children
            .iter()
            .any(|c| c.child.kind() == Kind::Simulated)
        {
            return Err("Simulated rig is already a rig here".into());
        }
        let exe = self.ac2d()?;
        let (child, endpoints) = spawn::spawn_simulated(&exe)?;
        let config = ClientConfig::new(endpoints, self.client_name());
        if let Err(e) = crate::sim::set_up_demo(config.clone()) {
            child.release().finish_in_time();
            return Err(e);
        }
        self.daemons.children.push(Watched {
            slot,
            child,
            served: false,
        });
        Ok(Target {
            config,
            describe: "simulated rig".into(),
        })
    }

    /// Each frame: the children that ended, the lock holder looked for, the backend asked
    /// for judged.
    pub(super) fn watch_daemons(&mut self, ctx: &egui::Context) {
        let connected = |st: &AppState, slot: u32| {
            st.rig_in_slot(slot)
                .is_some_and(|r| matches!(r.conn, crate::state::ConnState::Connected { .. }))
        };
        let mut ends = Vec::new();
        for c in &mut self.daemons.children {
            c.served |= connected(&self.state, c.slot);
        }
        self.daemons.children.retain_mut(|c| {
            if let Some(st) = c.child.exited() {
                ends.push((c.slot, spawn::ended(st, c.served, c.child.log())));
                return false;
            }
            // A detached child serving is this computer's daemon, not the app's to watch.
            c.child.owned() || !c.served
        });
        for (slot, e) in ends {
            match e {
                Ended::LostLock => self.daemons.lost.push((slot, Instant::now())),
                Ended::AtStart(e) => self.daemon_failed(slot, e),
                Ended::Stopped(e) => {
                    tracing::warn!("ac2-ui: {e}");
                    self.state.fault(e);
                }
            }
        }
        for (slot, since) in std::mem::take(&mut self.daemons.lost) {
            match LockHolder::read() {
                Some(LockHolder::Local(ep)) => {
                    if ep != Endpoints::local() {
                        self.retarget(ctx, slot, ep);
                    }
                }
                Some(LockHolder::Network(ep)) => self.network_holder(slot, &ep),
                None if since.elapsed() > HOLDER_WAIT => self.state.fault(format!(
                    "another ac2d is running for this user, but {} does not say where it \
                     listens",
                    ac2_client::endpoint::endpoints_file().display()
                )),
                None => self.daemons.lost.push((slot, since)),
            }
        }
        self.judge_backends();
    }

    /// The daemon of a rig started on a named backend must run on it: its device list
    /// (default backend first) is asked for once connected, and another backend is refused,
    /// never used in its place.
    fn judge_backends(&mut self) {
        let mut refused = Vec::new();
        for w in &mut self.daemons.wanted {
            let Some(r) = self.state.rig_in_slot(w.slot) else {
                continue;
            };
            if !matches!(r.conn, crate::state::ConnState::Connected { .. }) {
                continue;
            }
            if !w.asked
                && let Some(c) = &r.link
            {
                c.send(crate::conn::Request::Devices);
                w.asked = true;
            }
            if let Some(have) = r.devices.as_ref().and_then(|d| d.first()).map(|b| b.kind) {
                refused.push((w.slot, (have != w.backend).then_some((have, w.backend))));
            }
        }
        for (slot, verdict) in refused {
            self.daemons.wanted.retain(|w| w.slot != slot);
            if let Some((have, want)) = verdict {
                if let Some(r) = self.state.rig_in_slot_mut(slot) {
                    r.devices = None;
                }
                self.daemon_failed(
                    slot,
                    format!(
                        "ac2d on this computer runs on {}, not {}",
                        spawn::backend_arg(have),
                        spawn::backend_arg(want)
                    ),
                );
            }
        }
    }

    /// The lock holder serves in network mode: a client needs its pinned key, so it is
    /// reached as a network rig, paired first. The dialog opens with its address typed in.
    fn network_holder(&mut self, slot: u32, ep: &Endpoints) {
        let address = ep
            .ctrl
            .strip_prefix("tcp://")
            .unwrap_or(&ep.ctrl)
            .to_owned();
        self.daemon_failed(
            slot,
            format!(
                "another ac2d is running for this user, in network mode on {}: pair with it, \
                 then connect to {address}",
                ep.ctrl
            ),
        );
        if let Some(d) = &mut self.connect {
            d.address = address;
        }
    }

    /// The rig in `slot` reaches the lock holder at `ep` instead of the default endpoints.
    fn retarget(&mut self, ctx: &egui::Context, slot: u32, ep: Endpoints) {
        let describe = format!("daemon at {}", ep.ctrl);
        let mut config = ClientConfig::new(ep, self.client_name());
        config.received_bytes = Arc::clone(&self.health.received);
        let wake = crate::pace::link_wake(ctx.clone(), Arc::clone(&self.pacer));
        let c = match Conn::start(
            Target {
                config,
                describe: describe.clone(),
            },
            wake,
        ) {
            Ok(c) => c,
            Err(e) => return self.daemon_failed(slot, format!("link thread: {e}")),
        };
        if let Some(r) = self.state.rig_in_slot_mut(slot) {
            if let Some(old) = r.link.replace(c) {
                self.closers.push(std::thread::spawn(move || {
                    old.close();
                }));
            }
            r.conn = crate::state::ConnState::Connecting { target: describe };
        }
    }

    /// The rig in `slot` has no daemon: its link closed, and the connect dialog opens with
    /// `error` (the operator chooses again; never a silent switch to another daemon).
    pub(super) fn daemon_failed(&mut self, slot: u32, error: String) {
        if self.smoke {
            self.daemons.smoke_failure = Some(error.clone());
        }
        if let Some(r) = self.state.rig_in_slot_mut(slot) {
            if let Some(c) = r.link.take() {
                self.closers.push(std::thread::spawn(move || {
                    c.close();
                }));
            }
            let target = match &r.conn {
                crate::state::ConnState::Connecting { target }
                | crate::state::ConnState::Connected { target, .. }
                | crate::state::ConnState::Failed { target, .. } => target.clone(),
            };
            r.conn = crate::state::ConnState::Failed {
                target,
                error: error.clone(),
            };
        }
        match &self.connect_setup {
            Some(c) if self.connect.is_none() => {
                let mut d = c.dialog();
                d.closable = self.state.rigs().any(|r| r.link.is_some());
                d.error = Some(error);
                self.connect = Some(d);
            }
            Some(_) => dialog_error(&mut self.connect, error),
            None => self.state.fault(error),
        }
    }
}
