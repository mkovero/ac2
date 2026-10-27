//! The daemon link: one background thread with a small tokio runtime that owns the
//! [`Client`], the data subscription and the stimulus lease.
//!
//! The UI thread never awaits. It sends [`Request`]s and drains [`ConnEvent`]s once per
//! frame. What supersedes itself (the mirror, the newest frames) waits in a slot holding
//! only the newest, so a UI that stops drawing (minimised, occluded) never piles up
//! snapshots; replies and other events queue in order. The thread wakes the UI
//! (`request_repaint`) only for what changes the picture: new frame content, a stale or
//! liveness flip, a mirror change beyond a keepalive, a reply. Frames are drained when they
//! arrive, at most twice per display period ([`PollClock`]), and polled slower when idle or
//! when the UI is not drawing. Ages between pushes are the UI's to count
//! ([`DataSnapshot::aged`]).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_client::{
    Client, ClientConfig, ClientError, Latest, MirrorView, OnDrop, StimulusLease, expect_body,
};
use ac2_proto::model::{
    BackendInfo, BackendKind, DelayFinding, DelayPick, DeviceId, FinderBand, GeneratorDesired,
    GeneratorSettings, ImportFormat, ImportRole, InputSetup, LoopbackDetection, MeasConfig,
    Measurement, OwnedTraces, Preview, SessionConfig, Smoothing, SplHistory, TraceData, TraceMeta,
    TraceMicCurve, TraceSource,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{ClientId, MeasId, RigId, Seconds, TraceId};
use ac2_proto::{Command, GridDef, GridId, ReplyBody, Subscription};
use tokio::sync::mpsc;

mod pacing;
mod request;
mod stimulus;

use pacing::*;
use request::*;
use stimulus::*;

pub use pacing::{mirror_differs, same_picture};
pub use request::{export_format, write_new_file};

/// Where to connect.
#[derive(Clone, Debug)]
pub struct Target {
    /// Endpoints, CURVE and the name sent in `hello`.
    pub config: ClientConfig,
    /// `local daemon`, `daemon at host:port`.
    pub describe: String,
}

/// Newest frames per topic with their grids, as drained at `drained`.
#[derive(Clone, Debug)]
pub struct DataSnapshot {
    pub latest: Latest,
    pub grids: BTreeMap<GridId, Arc<GridDef>>,
    pub drained: Instant,
}

impl DataSnapshot {
    /// The same frames with their ages as of `now`: the link pushes a snapshot when frame
    /// content or a STALE flag changes, and in between the ages shown keep counting.
    pub fn aged(&self, now: Instant) -> Self {
        let dt = now.saturating_duration_since(self.drained);
        let mut latest = self.latest.clone();
        for f in latest.frames.values_mut() {
            f.since_new += dt;
            f.age = f.age.map(|a| a + dt.as_secs_f64());
        }
        Self {
            latest,
            grids: self.grids.clone(),
            drained: now.max(self.drained),
        }
    }
}

/// What the link reports.
#[derive(Clone, Debug)]
pub enum ConnEvent {
    Connecting {
        target: String,
    },
    Connected {
        target: String,
        server: String,
        client_id: ClientId,
        /// The daemon's identity and display name, from its hello reply.
        rig_id: RigId,
        name: String,
    },
    Failed {
        target: String,
        error: String,
        retry_in: Duration,
    },
    Mirror(Arc<MirrorView>),
    Data(Arc<DataSnapshot>),
    /// A stored trace's data and grid (fetched once per trace, again when its display
    /// smoothing changes).
    Trace(Arc<TraceData>, Arc<GridDef>),
    /// A command finished; `Err` carries the daemon's (or the transport's) message.
    Reply {
        what: String,
        result: Result<(), String>,
    },
    /// A command was refused as made on a configuration (or the rig settings) another
    /// client changed first: the daemon's `conflict`.
    Conflict {
        what: String,
        error: String,
    },
    /// A command succeeded with warnings the operator should hear about (sent before its
    /// `Reply`).
    Warnings(Vec<ac2_proto::Warning>),
    /// A `delay.find` answered; `pick` is what the operator asked to insert.
    DelayFound {
        meas: MeasId,
        pick: DelayPick,
        finding: Box<DelayFinding>,
    },
    /// A capture into a slot finished.
    Captured {
        slot: u8,
        trace: Box<TraceMeta>,
    },
    Stimulus(StimEvent),
    /// `session.devices` answered (or could not be asked).
    Devices(Result<Vec<BackendInfo>, String>),
    /// `session.preview` of `backend`/`device` answered.
    Preview {
        backend: BackendKind,
        device: DeviceId,
        result: Result<Preview, String>,
    },
    /// `session.detect_loopback` answered (or could not run).
    LoopbackDetected(Result<LoopbackDetection, String>),
    /// The session dialog's session is open (and its mic names set).
    SessionOpened,
    /// A measurement was created (and, unless a reply says otherwise, started).
    MeasCreated(Box<Measurement>),
    /// `server.info` (`what: None`), or a `server.*` change named `what`, answered with the
    /// daemon's server info or why not.
    Server {
        what: Option<String>,
        result: Result<ac2_proto::model::ServerInfo, String>,
    },
    /// The Leq history of SPL meter `meas` as the daemon rebuilt it from the meter's log
    /// ([`Request::LeqBackfill`] number `ask`).
    LeqBackfill {
        meas: MeasId,
        ask: u64,
        result: Result<Box<SplHistory>, String>,
    },
    /// A span of an SPL meter's log at a resolution, for a history strip looking back
    /// ([`Request::LeqSpan`] number `ask`).
    LeqSpan {
        meas: MeasId,
        ask: u64,
        result: Result<Box<SplHistory>, String>,
    },
    /// A span of a band log read back ([`Request::BandLogGet`]).
    BandLog {
        ask: crate::leq_dialog::Ask,
        result: Result<Box<ac2_proto::model::SplBandLog>, String>,
    },
    /// `spl.band_transfer` answered with the meter that took the transfer, or why not.
    BandTransfer(Result<Box<Measurement>, String>),
    /// A [`Request::Sessions`] job ended.
    Sessions(Box<crate::sessions::SessionOutcome>),
    /// A session file moving: `uploading fri.ac2session: 1.2 of 3.4 MB`.
    SessionProgress(String),
    /// `file.list` answered.
    SessionList(Result<Vec<ac2_proto::model::SessionFile>, String>),
}

/// Stimulus lease outcomes.
#[derive(Clone, Debug, PartialEq)]
pub enum StimEvent {
    /// Lease held and the generator armed (not emitting).
    Armed,
    /// A `gen.set` was accepted (fire, level change).
    Set { firing: bool },
    /// Output stopped and the lease released (or no lease was held and `gen.stop` was sent).
    Stopped,
    /// The daemon refused a refresh: the lease expired or was taken over.
    Lost(String),
    /// An arm / set / stop failed.
    Failed(String),
    /// `ir.capture` started this run.
    SweepStarted(Box<ac2_proto::model::SweepRun>),
}

/// What the UI asks for.
#[derive(Clone, Debug)]
pub enum Request {
    /// A command whose reply only matters as success / failure.
    Call { cmd: Command, what: String },
    /// Acquire the lease (taking it over with `force`) and arm with `settings`.
    StimArm {
        settings: GeneratorSettings,
        force: bool,
    },
    /// `gen.set` under the held lease.
    StimSet(GeneratorDesired),
    /// Stop and release; without a lease, `gen.stop` (any client may stop the output).
    StimStop,
    /// `sweep.run` of sweep measurement `meas` under the held lease (the generator armed
    /// with its sweep); `update` first stores the settings it is run with (a level or
    /// outputs changed while armed), so the run plays what the top bar said.
    Sweep {
        meas: MeasId,
        /// The measurement's name, for messages.
        label: String,
        /// With the `config_rev` it was made on.
        update: Option<(ac2_proto::units::Rev, Box<ac2_proto::model::MeasConfig>)>,
    },
    /// `delay.find` on `meas` in `band` over `observation`, to insert `pick` from; the
    /// reducer decides what to insert once the finding is back.
    FindDelay {
        meas: MeasId,
        pick: DelayPick,
        band: FinderBand,
        observation: Option<Seconds>,
    },
    /// `trace.capture` of `meas` into `slot` as `name`, deleting `replace` first.
    Capture {
        meas: MeasId,
        slot: u8,
        name: String,
        replace: Option<TraceId>,
    },
    /// Read a local file and `trace.import` it (`ir`: how a WAV impulse response is read;
    /// `None` for a text file). The client reads it, so a remote daemon gets its content.
    Import {
        path: std::path::PathBuf,
        role: ImportRole,
        /// Where it is listed.
        owner: ac2_proto::model::TraceOwner,
        ir: Option<ac2_proto::model::IrImport>,
    },
    /// Read a local mic-curve file and `cal.curve_import` it as a curve of `mic` (the label
    /// from the file), setting `input`'s mic name when given.
    ImportCurve {
        path: std::path::PathBuf,
        mic: String,
        input: Option<u16>,
    },
    /// `trace.export` of `trace` (named `name`) as ac2 CSV, written to `path`, or into it
    /// under the daemon's suggested file name when `path` is a folder.
    ExportTrace {
        trace: TraceId,
        name: String,
        path: std::path::PathBuf,
    },
    /// `session.devices`, for the session dialog.
    Devices,
    /// `session.open`, then `session.inputs` with the mic names; [`ConnEvent::SessionOpened`]
    /// once both succeeded.
    OpenSession {
        config: SessionConfig,
        inputs: Vec<InputSetup>,
        pairs: Vec<ac2_proto::model::Pair>,
        what: String,
    },
    /// Open or renew the capture-only preview of a device, with what its inputs are
    /// declared to be (its frames then say what each column is).
    Preview {
        backend: BackendKind,
        device: DeviceId,
        inputs: Vec<ac2_proto::model::PreviewInput>,
        pairs: Vec<ac2_proto::model::Pair>,
    },
    /// Close the preview.
    PreviewStop,
    /// Subscribe to the input meters (`session/`), or stop.
    Meters(bool),
    /// `session.detect_loopback` under the stimulus lease (taken for it when not held).
    DetectLoopback(crate::session_dialog::DetectRequest),
    /// `meas.create` then `meas.start` (as `ac2 meas new --start`).
    CreateMeas { config: MeasConfig },
    /// `spl.history_get` of SPL meter `meas` over the history the strip keeps
    /// ([`ConnEvent::LeqBackfill`]).
    LeqBackfill { meas: MeasId, ask: u64 },
    /// `spl.history_get` of a range of SPL meter `meas`'s log at `step_s` seconds a point
    /// ([`ConnEvent::LeqSpan`]).
    LeqSpan {
        meas: MeasId,
        from: ac2_proto::units::WallNs,
        to: ac2_proto::units::WallNs,
        step_s: u32,
        ask: u64,
    },
    /// Every row of one of SPL meter `info.meas`'s logs (`spl.log_get`, paged), written as
    /// the log CSV to `path` on this computer, its folder made first; a reply toast says
    /// how many seconds went where.
    ExportLeqLog {
        info: ac2_proto::spl_log::SplLogInfo,
        log: ac2_proto::model::SplLogWhich,
        path: std::path::PathBuf,
    },
    /// `spl.band_log_get` of a span, its average alone ([`ConnEvent::BandLog`]).
    BandLogGet(crate::leq_dialog::Ask),
    /// `spl.band_transfer` into `meas` ([`ConnEvent::BandTransfer`]).
    BandTransfer {
        meas: MeasId,
        sources: Box<crate::leq_dialog::Sources>,
        /// The place the limits are for.
        place: String,
    },
    /// The measurement streams to receive: what the visible panes draw and what the
    /// reducer folds (an IR nobody shows is never computed, since the daemon derives it
    /// only for subscribers). Kept across reconnects.
    Topics(HashSet<Topic>),
    /// How often new frames may reach the UI: what the visible panes can show.
    DisplayPeriod(Duration),
    /// Drop the connection and connect again now.
    Reconnect,
    /// `server.info` ([`ConnEvent::Server`]).
    ServerInfo,
    /// `server.authorize` / `server.revoke`; answered with the new info
    /// ([`ConnEvent::Server`] carrying `what`).
    ServerCall { cmd: Command, what: String },
    /// A Sessions-dialog job ([`ConnEvent::SessionProgress`] while it moves a file,
    /// [`ConnEvent::Sessions`] when it ends).
    Sessions(crate::sessions::SessionJob),
    /// `file.list` ([`ConnEvent::SessionList`]).
    SessionList,
}

// One message per operator action, moved once through the channel: boxing the request
// would add an allocation per action and buy nothing.
#[allow(clippy::large_enum_variant)]
enum Ctl {
    Req(Request),
    Shutdown,
}

/// Wakes the UI thread.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// What the link has reported and the UI not yet taken.
#[derive(Default)]
struct Inbox {
    /// Ordered events (connection, replies, stimulus): each one matters.
    events: Vec<ConnEvent>,
    /// The newest mirror view.
    mirror: Option<Arc<MirrorView>>,
    /// The newest frames.
    data: Option<Arc<DataSnapshot>>,
    /// `leq` frames a newer snapshot replaced before the UI took them: the Leq history folds
    /// every second, so these are delivered (in order, before the newest) and never dropped
    /// while the UI is not drawing. Each holds only `leq` topics.
    leq: VecDeque<Arc<DataSnapshot>>,
    /// When the UI was woken without draining since: another wake adds nothing, and one
    /// long unanswered means the UI is not drawing (minimised, occluded).
    woken: Option<Instant>,
}

impl Inbox {
    /// Everything, in delivery order; the UI is awake again.
    fn take(&mut self) -> Vec<ConnEvent> {
        self.woken = None;
        let mut out = std::mem::take(&mut self.events);
        out.extend(self.leq.drain(..).map(ConnEvent::Data));
        out.extend(self.mirror.take().map(ConnEvent::Mirror));
        out.extend(self.data.take().map(ConnEvent::Data));
        out
    }
}

/// Most `leq` backlog snapshots kept for a UI that is not drawing: the Leq history's span
/// at one a second, beyond which the history is rebuilt from the meter's log anyway.
const LEQ_BACKLOG: usize = ac2_scene::leq::HISTORY_S as usize;

type Shared = Arc<Mutex<Inbox>>;

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, Inbox> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Handle to the link thread. Dropping it stops the stimulus (if held) and releases the
/// lease, waiting at most [`QUIT_GRACE`] for that.
pub struct Conn {
    tx: mpsc::UnboundedSender<Ctl>,
    inbox: Shared,
    thread: Option<JoinHandle<()>>,
    /// Signalled (or dropped) when the link thread is done.
    done: std_mpsc::Receiver<()>,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn").finish_non_exhaustive()
    }
}

/// Longest a quit waits for the stimulus stop and lease release (decision K6). Past it the
/// app exits anyway: an unreachable daemon must not hold the window open, and its lease
/// expiry (1.5 s) fades the output out regardless.
pub const QUIT_GRACE: Duration = Duration::from_secs(1);

/// Retry delay after a failed connect.
pub const RETRY_EVERY: Duration = Duration::from_secs(2);
/// Deadline of one connect attempt (`hello` round trip).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Display period until the UI says otherwise: 30 frames a second.
pub const DISPLAY_PERIOD: Duration = Duration::from_millis(33);
/// Data poll period while no frame content changes or the UI is not drawing: still often
/// enough to see a STALE flag flip or a measurement start within a quarter second.
const IDLE_POLL: Duration = Duration::from_millis(250);
/// A UI that has not drained this long after a wake is not drawing (minimised, occluded):
/// the link polls at [`IDLE_POLL`] until it drains again.
const UI_AWAY: Duration = Duration::from_secs(1);
/// How long after the last new frame the link keeps polling at the display period.
const ACTIVE_HOLD: Duration = Duration::from_secs(1);

impl Conn {
    pub fn start(target: Target, wake: Wake) -> std::io::Result<Self> {
        let (tx, ctl_rx) = mpsc::unbounded_channel();
        let inbox = Shared::default();
        let out = Out {
            inbox: inbox.clone(),
            wake,
        };
        let (done_tx, done) = std_mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ac2-ui link".into())
            .spawn(move || {
                // Dropped on every exit path, panics included: the closer stops waiting.
                let _done = done_tx;
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("ac2-ui io")
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        out.send(ConnEvent::Failed {
                            target: target.describe.clone(),
                            error: format!("runtime: {e}"),
                            retry_in: Duration::MAX,
                        });
                        return;
                    }
                };
                rt.block_on(run(target, ctl_rx, out));
            })?;
        Ok(Self {
            tx,
            inbox,
            thread: Some(thread),
            done,
        })
    }

    pub fn send(&self, r: Request) {
        let _ = self.tx.send(Ctl::Req(r));
    }

    /// Everything reported since the last call: the ordered events, then the `leq` frames
    /// that would otherwise be lost, then the newest mirror and the newest frames.
    pub fn drain(&self) -> Vec<ConnEvent> {
        lock(&self.inbox).take()
    }
}

impl Conn {
    /// Stops the stimulus (if held), releases the lease and ends the link, waiting at most
    /// [`QUIT_GRACE`]. `true` when the link finished in time; otherwise its thread is left
    /// to the process exit and the daemon's lease expiry stops the output.
    pub fn close(mut self) -> bool {
        self.shutdown()
    }

    fn shutdown(&mut self) -> bool {
        let _ = self.tx.send(Ctl::Shutdown);
        self.finish(Instant::now() + QUIT_GRACE)
    }

    /// Waits until `deadline` for the link to end after its shutdown was sent.
    fn finish(&mut self, deadline: Instant) -> bool {
        let Some(t) = self.thread.take() else {
            return true;
        };
        let left = deadline.saturating_duration_since(Instant::now());
        match self.done.recv_timeout(left) {
            Err(std_mpsc::RecvTimeoutError::Timeout) => false,
            Ok(()) | Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                let _ = t.join();
                true
            }
        }
    }
}

/// Closes every link at once: each stops the stimulus it holds and releases its lease in
/// parallel, all within one [`QUIT_GRACE`], so quitting with several rigs waits no longer
/// than with one. `true` when every link finished in time.
pub fn close_all(conns: Vec<Conn>) -> bool {
    for c in &conns {
        let _ = c.tx.send(Ctl::Shutdown);
    }
    let deadline = Instant::now() + QUIT_GRACE;
    let mut all = true;
    for mut c in conns {
        all &= c.finish(deadline);
    }
    all
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
struct Out {
    inbox: Shared,
    wake: Wake,
}

impl Out {
    fn wake_once(&self, mut i: std::sync::MutexGuard<'_, Inbox>) {
        if i.woken.is_none() {
            i.woken = Some(Instant::now());
            drop(i);
            (self.wake)();
        }
    }

    fn send(&self, e: ConnEvent) {
        let mut i = lock(&self.inbox);
        if matches!(e, ConnEvent::Connecting { .. } | ConnEvent::Failed { .. }) {
            // What the previous connection left untaken describes a daemon that is gone.
            i.mirror = None;
            i.data = None;
            i.leq.clear();
        }
        i.events.push(e);
        self.wake_once(i);
    }

    /// The newest mirror view; the UI is woken only when `wake` (a change it shows).
    fn mirror(&self, v: Arc<MirrorView>, wake: bool) {
        let mut i = lock(&self.inbox);
        i.mirror = Some(v);
        if wake {
            self.wake_once(i);
        }
    }

    /// The newest frames; the UI is woken only when `wake` (a change it shows). A snapshot
    /// the UI has not taken keeps its `leq` frames that this one does not carry.
    fn data(&self, d: Arc<DataSnapshot>, wake: bool) {
        let mut i = lock(&self.inbox);
        if let Some(old) = i.data.take()
            && let Some(lost) = leq_only(&old, &d)
        {
            if i.leq.len() >= LEQ_BACKLOG {
                i.leq.pop_front();
            }
            i.leq.push_back(Arc::new(lost));
        }
        i.data = Some(d);
        if wake {
            self.wake_once(i);
        }
    }

    /// Wakes the UI for what it already holds.
    fn wake(&self) {
        self.wake_once(lock(&self.inbox));
    }

    /// The UI has not drained for [`UI_AWAY`] since it was woken: it is not drawing.
    fn ui_behind(&self) -> bool {
        lock(&self.inbox)
            .woken
            .is_some_and(|t| t.elapsed() >= UI_AWAY)
    }
}

enum Next {
    Reconnect,
    Exit,
}

/// What the UI asked to receive; kept across reconnects and applied to each connection.
#[derive(Debug)]
struct Wants {
    /// The input meters (`session/`).
    meters: bool,
    topics: HashSet<Topic>,
    period: Duration,
}

async fn run(target: Target, mut ctl: mpsc::UnboundedReceiver<Ctl>, out: Out) {
    let mut wants = Wants {
        meters: false,
        topics: HashSet::new(),
        period: DISPLAY_PERIOD,
    };
    loop {
        out.send(ConnEvent::Connecting {
            target: target.describe.clone(),
        });
        let attempt =
            tokio::time::timeout(CONNECT_TIMEOUT, Client::connect(target.config.clone())).await;
        let client = match attempt {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                let why = describe_err(&e, &target);
                if wait_retry(&target, &mut ctl, &out, why, &mut wants).await {
                    continue;
                }
                return;
            }
            Err(_) => {
                let why = not_responding(&target);
                if wait_retry(&target, &mut ctl, &out, why, &mut wants).await {
                    continue;
                }
                return;
            }
        };
        let w = client.welcome();
        out.send(ConnEvent::Connected {
            target: target.describe.clone(),
            server: w.server,
            client_id: w.client_id,
            rig_id: w.rig_id,
            name: w.name,
        });
        match session(client, &mut ctl, &out, &mut wants).await {
            Next::Reconnect => continue,
            Next::Exit => return,
        }
    }
}

/// Reports the failure and waits [`RETRY_EVERY`]; `false` when the UI is shutting down.
async fn wait_retry(
    target: &Target,
    ctl: &mut mpsc::UnboundedReceiver<Ctl>,
    out: &Out,
    error: String,
    wants: &mut Wants,
) -> bool {
    out.send(ConnEvent::Failed {
        target: target.describe.clone(),
        error,
        retry_in: RETRY_EVERY,
    });
    let deadline = tokio::time::sleep(RETRY_EVERY);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return true,
            c = ctl.recv() => match c {
                None | Some(Ctl::Shutdown) => return false,
                Some(Ctl::Req(Request::Reconnect)) => return true,
                Some(Ctl::Req(Request::StimStop)) => {
                    // Nothing to stop through: no connection, so no lease of ours either.
                    out.send(ConnEvent::Stimulus(StimEvent::Stopped));
                }
                Some(Ctl::Req(Request::Devices)) => {
                    out.send(ConnEvent::Devices(Err("not connected".into())));
                }
                Some(Ctl::Req(Request::Preview { backend, device, .. })) => {
                    out.send(ConnEvent::Preview {
                        backend,
                        device,
                        result: Err("not connected".into()),
                    });
                }
                Some(Ctl::Req(Request::DetectLoopback(_))) => {
                    out.send(ConnEvent::LoopbackDetected(Err("not connected".into())));
                }
                Some(Ctl::Req(Request::ServerInfo)) => out.send(ConnEvent::Server {
                    what: None,
                    result: Err("not connected".into()),
                }),
                Some(Ctl::Req(Request::ServerCall { what, .. })) => out.send(ConnEvent::Server {
                    what: Some(what),
                    result: Err("not connected".into()),
                }),
                // Applied on the next connection.
                Some(Ctl::Req(Request::Meters(on))) => wants.meters = on,
                Some(Ctl::Req(Request::Topics(t))) => wants.topics = t,
                Some(Ctl::Req(Request::DisplayPeriod(p))) => wants.period = p,
                // Asked again once connected: the history follows the daemon's log.
                Some(Ctl::Req(Request::LeqBackfill { .. })) => {}
                Some(Ctl::Req(Request::LeqSpan { meas, ask, .. })) => out.send(ConnEvent::LeqSpan {
                    meas,
                    ask,
                    result: Err("not connected".into()),
                }),
                Some(Ctl::Req(Request::BandLogGet(ask))) => out.send(ConnEvent::BandLog {
                    ask,
                    result: Err("not connected".into()),
                }),
                Some(Ctl::Req(Request::BandTransfer { .. })) => {
                    out.send(ConnEvent::BandTransfer(Err("not connected".into())));
                }
                Some(Ctl::Req(Request::PreviewStop)) => {}
                Some(Ctl::Req(r)) => out.send(ConnEvent::Reply {
                    what: request_name(&r),
                    result: Err("not connected".into()),
                }),
            },
        }
    }
}

/// A link problem that is not a command's failure, as an error toast.
fn tracing_free_note(out: &Out, what: &str, e: &str) {
    out.send(ConnEvent::Reply {
        what: what.into(),
        result: Err(e.into()),
    });
}

async fn session(
    client: Client,
    ctl: &mut mpsc::UnboundedReceiver<Ctl>,
    out: &Out,
    wants: &mut Wants,
) -> Next {
    let mut subscribed: HashSet<Topic> = HashSet::new();
    resubscribe(&client, &mut subscribed, &wants.topics, out);
    if wants.meters {
        let _ = client.subscribe(Subscription::InputMeters);
    }
    let (stim_tx, stim_rx) = mpsc::unbounded_channel();
    let stim = tokio::spawn(stimulus_task(client.clone(), stim_rx, out.clone()));
    let (preview_tx, preview_rx) = mpsc::unbounded_channel();
    let preview = tokio::spawn(preview_task(client.clone(), preview_rx, out.clone()));
    let mut mirror = client.watch();
    let mut shown_mirror: Option<Arc<MirrorView>> = None;
    let mut poll = DataPoll::default();
    let mut clock = PollClock::new(tokio::time::Instant::now());
    // Served columns carry the trace's display smoothing and mic curve, and an EQ curve its
    // set's rev: a new setting means new data.
    type Shown = (Option<Smoothing>, Option<Box<TraceMicCurve>>, Option<u32>);
    let mut fetched: HashMap<TraceId, Shown> = HashMap::new();
    let next = loop {
        tokio::select! {
            c = ctl.recv() => match c {
                None | Some(Ctl::Shutdown) => break Next::Exit,
                Some(Ctl::Req(Request::Reconnect)) => break Next::Reconnect,
                Some(Ctl::Req(Request::Meters(on))) => {
                    if on != wants.meters {
                        wants.meters = on;
                        let r = if on {
                            client.subscribe(Subscription::InputMeters)
                        } else {
                            client.unsubscribe(Subscription::InputMeters)
                        };
                        if let Err(e) = r {
                            tracing_free_note(out, "meters", &e.to_string());
                        }
                    }
                }
                Some(Ctl::Req(Request::Topics(t))) => {
                    wants.topics = t;
                    resubscribe(&client, &mut subscribed, &wants.topics, out);
                    // A new stream's first frame is worth showing at the display rate.
                    clock.poll_now(tokio::time::Instant::now());
                }
                Some(Ctl::Req(Request::DisplayPeriod(p))) => wants.period = p,
                Some(Ctl::Req(r @ (Request::Preview { .. } | Request::PreviewStop))) => {
                    let _ = preview_tx.send(r);
                }
                Some(Ctl::Req(r)) => handle(&client, r, &stim_tx, out),
            },
            silent = client.await_dead() => {
                // The transport may still look connected (a half-open TCP connection after
                // the link dropped), so libzmq would never reconnect by itself: drop the
                // client, which closes all three sockets, and connect anew.
                tracing_free_note(
                    out,
                    "daemon",
                    &format!(
                        "no keepalive for {}, reconnecting",
                        ac2_scene::format::age(silent.as_secs_f64())
                    ),
                );
                break Next::Reconnect;
            },
            changed = mirror.changed() => {
                if changed.is_err() {
                    break Next::Reconnect;
                }
                let v = mirror.borrow_and_update().clone();
                if let Some(st) = &v.state {
                    for t in &st.traces {
                        // An EQ set's filter curve is recomputed on every edit of the set.
                        let eq_rev = match t.source {
                            TraceSource::Eq { rev, .. } => Some(rev),
                            _ => None,
                        };
                        let shown = (t.edit.smoothing, t.mic_curve.clone(), eq_rev);
                        if fetched.insert(t.id, shown.clone()) != Some(shown) {
                            tokio::spawn(fetch_trace(client.clone(), t.id, t.grid_id, out.clone()));
                        }
                    }
                }
                // Keepalives arrive four times a second and change only their own time:
                // the UI takes those with its next pass, and liveness flips wake it (the
                // data poll sees `responding` change).
                let wake = shown_mirror.as_ref().is_none_or(|s| mirror_differs(s, &v));
                if wake {
                    shown_mirror = Some(v.clone());
                }
                out.mirror(v, wake);
            },
            () = tokio::time::sleep_until(clock.next) => {
                let active = poll.poll(&client, out).await;
                clock.polled(tokio::time::Instant::now(), active, out.ui_behind(), wants.period);
            },
            () = client.data_changed(), if clock.watch => {
                clock.arrived(wants.period);
            },
        }
    };
    // The preview task ends with its channel; the daemon expires a preview nobody renews.
    drop(preview_tx);
    preview.abort();
    // Closing the channel makes the stimulus task stop the output and release the lease.
    drop(stim_tx);
    let _ = tokio::time::timeout(QUIT_GRACE, stim).await;
    next
}

/// Subscribes to the topics in `want` not yet in `have` and drops the rest.
fn resubscribe(client: &Client, have: &mut HashSet<Topic>, want: &HashSet<Topic>, out: &Out) {
    let gone: Vec<Topic> = have.difference(want).copied().collect();
    for t in gone {
        have.remove(&t);
        if let Err(e) = client.unsubscribe(Subscription::Topic(t)) {
            tracing_free_note(out, "unsubscribe", &e.to_string());
        }
    }
    for t in want {
        if !have.contains(t) {
            match client.subscribe(Subscription::Topic(*t)) {
                Ok(()) => {
                    have.insert(*t);
                }
                Err(e) => tracing_free_note(out, "subscribe", &e.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests;
