//! The daemon link: one background thread with a small tokio runtime that owns the
//! [`Client`], the data subscription and the stimulus lease.
//!
//! The UI thread never awaits. It sends [`Request`]s and drains [`ConnEvent`]s once per
//! frame. What supersedes itself (the mirror, the newest frames) waits in a slot holding
//! only the newest, so a UI that stops drawing (minimised, occluded) never piles up
//! snapshots; replies and other events queue in order. The thread wakes the UI
//! (`request_repaint`) only for what changes the picture: new frame content, a stale or
//! liveness flip, a mirror change beyond a keepalive, a reply. Frames are polled at the
//! display period while they flow and slower when idle or when the UI is not drawing, so
//! data never repaints faster than the display period. Ages between pushes are the UI's
//! to count ([`DataSnapshot::aged`]).

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
    Measurement, Preview, SessionConfig, Smoothing, SplHistory, TraceData, TraceMeta,
    TraceMicCurve,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{ClientId, MeasId, Seconds, TraceId};
use ac2_proto::{Command, GridDef, GridId, ReplyBody, Subscription};
use tokio::sync::mpsc;

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
    /// The session dialog's session is open (and its mic names set); `transfers` are the
    /// measurements it offers to create.
    SessionOpened {
        transfers: Vec<MeasConfig>,
    },
    /// A measurement was created (and, unless a reply says otherwise, started).
    MeasCreated(Box<Measurement>),
    /// The Leq history of SPL meter `meas` as the daemon rebuilt it from the meter's log
    /// ([`Request::LeqBackfill`] number `ask`).
    LeqBackfill {
        meas: MeasId,
        ask: u64,
        result: Result<Box<SplHistory>, String>,
    },
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
    /// `ir.capture` under the held lease (the generator armed with the sweep).
    Sweep {
        request: ac2_proto::model::SweepRequest,
        name: String,
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
    /// Read a local file and `trace.import` it.
    Import {
        path: std::path::PathBuf,
        role: ImportRole,
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
    /// carries `transfers` back once both succeeded.
    OpenSession {
        config: SessionConfig,
        inputs: Vec<InputSetup>,
        transfers: Vec<MeasConfig>,
        what: String,
    },
    /// Open or renew the capture-only preview of a device.
    Preview {
        backend: BackendKind,
        device: DeviceId,
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
    /// The measurement streams to receive: what the visible panes draw and what the
    /// reducer folds (an IR nobody shows is never computed, since the daemon derives it
    /// only for subscribers). Kept across reconnects.
    Topics(HashSet<Topic>),
    /// How often new frames may reach the UI: what the visible panes can show.
    DisplayPeriod(Duration),
    /// Drop the connection and connect again now.
    Reconnect,
}

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
        let Some(t) = self.thread.take() else {
            return true;
        };
        let _ = self.tx.send(Ctl::Shutdown);
        match self.done.recv_timeout(QUIT_GRACE) {
            Err(std_mpsc::RecvTimeoutError::Timeout) => false,
            Ok(()) | Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                let _ = t.join();
                true
            }
        }
    }
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

/// `old`'s `leq` frames that `new` replaces with a newer one or drops, as a snapshot of
/// their own; `None` when every one of them is still in `new`.
fn leq_only(old: &DataSnapshot, new: &DataSnapshot) -> Option<DataSnapshot> {
    let frames: BTreeMap<Arc<str>, ac2_client::TopicFrame> = old
        .latest
        .frames
        .iter()
        .filter(|(k, f)| {
            matches!(
                f.topic,
                Topic::Data {
                    stream: Stream::Leq,
                    ..
                }
            ) && new
                .latest
                .frames
                .get(*k)
                .is_none_or(|n| n.frame.stamp.seq != f.frame.stamp.seq)
        })
        .map(|(k, f)| (k.clone(), f.clone()))
        .collect();
    (!frames.is_empty()).then(|| DataSnapshot {
        latest: Latest {
            frames,
            ..old.latest.clone()
        },
        grids: BTreeMap::new(),
        drained: old.drained,
    })
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
        });
        match session(client, &mut ctl, &out, &mut wants).await {
            Next::Reconnect => continue,
            Next::Exit => return,
        }
    }
}

fn describe_err(e: &ClientError, target: &Target) -> String {
    match e {
        ClientError::Timeout { .. } => not_responding(target),
        other => other.to_string(),
    }
}

/// "not responding", with what to check on a daemon on another host (its firewall, and
/// whether it authorized this client's key).
fn not_responding(target: &Target) -> String {
    format!("not responding{}", target.config.not_responding_hints())
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
                Some(Ctl::Req(Request::Preview { backend, device })) => {
                    out.send(ConnEvent::Preview {
                        backend,
                        device,
                        result: Err("not connected".into()),
                    });
                }
                Some(Ctl::Req(Request::DetectLoopback(_))) => {
                    out.send(ConnEvent::LoopbackDetected(Err("not connected".into())));
                }
                // Applied on the next connection.
                Some(Ctl::Req(Request::Meters(on))) => wants.meters = on,
                Some(Ctl::Req(Request::Topics(t))) => wants.topics = t,
                Some(Ctl::Req(Request::DisplayPeriod(p))) => wants.period = p,
                // Asked again once connected: the history follows the daemon's log.
                Some(Ctl::Req(Request::LeqBackfill { .. })) => {}
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

fn request_name(r: &Request) -> String {
    match r {
        Request::Call { what, .. } => what.clone(),
        Request::StimArm { .. } => "arm".into(),
        Request::StimSet(_) => "stimulus".into(),
        Request::StimStop => "stop".into(),
        Request::Sweep { name, .. } => format!("sweep {name}"),
        Request::Capture { slot, .. } => format!("capture slot {slot}"),
        Request::Import { path, .. } => format!("import {}", path.display()),
        Request::ImportCurve { path, .. } => format!("import curve {}", path.display()),
        Request::ExportTrace { name, .. } => format!("export {name}"),
        Request::FindDelay { .. } => "delay find".into(),
        Request::Devices => "list devices".into(),
        Request::OpenSession { what, .. } => what.clone(),
        Request::Preview { .. } => "device meters".into(),
        Request::PreviewStop => "close device meters".into(),
        Request::Meters(_) => "meters".into(),
        Request::DetectLoopback(_) => "detect loopback".into(),
        Request::CreateMeas { config } => format!("new measurement {}", config.name),
        Request::LeqBackfill { .. } => "SPL history".into(),
        Request::Topics(_) => "subscribe".into(),
        Request::DisplayPeriod(_) => "display rate".into(),
        Request::Reconnect => "reconnect".into(),
    }
}

enum StimOp {
    Arm {
        settings: GeneratorSettings,
        force: bool,
    },
    Set(GeneratorDesired),
    Stop,
    Detect(crate::session_dialog::DetectRequest),
    Sweep {
        request: ac2_proto::model::SweepRequest,
        name: String,
    },
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
    let mut next_poll = tokio::time::Instant::now();
    let mut last_poll = next_poll;
    // While polling at the idle period, a frame arriving cuts the wait short (the display
    // period still bounds how often the UI is fed); while frames flow, the display period
    // paces the drains and nothing else wakes the link.
    let mut idle = false;
    // Served columns carry the trace's display smoothing and mic curve: a new setting means
    // new data.
    let mut fetched: HashMap<TraceId, (Option<Smoothing>, Option<Box<TraceMicCurve>>)> =
        HashMap::new();
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
                    next_poll = tokio::time::Instant::now();
                }
                Some(Ctl::Req(Request::DisplayPeriod(p))) => wants.period = p,
                Some(Ctl::Req(r @ (Request::Preview { .. } | Request::PreviewStop))) => {
                    let _ = preview_tx.send(r);
                }
                Some(Ctl::Req(r)) => handle(&client, r, &stim_tx, out),
            },
            changed = mirror.changed() => {
                if changed.is_err() {
                    break Next::Reconnect;
                }
                let v = mirror.borrow_and_update().clone();
                if let Some(st) = &v.state {
                    for t in &st.traces {
                        let shown = (t.edit.smoothing, t.mic_curve.clone());
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
            () = tokio::time::sleep_until(next_poll) => {
                let active = poll.poll(&client, out).await;
                let every = if active && !out.ui_behind() { wants.period } else { IDLE_POLL };
                last_poll = tokio::time::Instant::now();
                next_poll = last_poll + every;
                idle = every == IDLE_POLL && !out.ui_behind();
            },
            () = client.data_changed(), if idle => {
                idle = false;
                next_poll = next_poll.min(last_poll + wants.period);
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

/// Whether two mirror views differ in anything the UI shows, keepalive time aside. The
/// daemon's silence is judged from `responding` flips, which wake the UI on their own.
pub fn mirror_differs(a: &MirrorView, b: &MirrorView) -> bool {
    let state = match (&a.state, &b.state) {
        (Some(x), Some(y)) => !Arc::ptr_eq(x, y),
        (None, None) => false,
        _ => true,
    };
    state
        || a.phase != b.phase
        || a.incarnation != b.incarnation
        || a.session_epoch != b.session_epoch
        || a.rev != b.rev
        || a.generator != b.generator
        || a.timing != b.timing
        || a.clock_offset_ns.is_some() != b.clock_offset_ns.is_some()
        || a.snapshots != b.snapshots
        || a.since_requests != b.since_requests
        || a.incarnation_changes != b.incarnation_changes
        || a.client_id != b.client_id
}

/// Whether two frames of a topic draw the same: equal content, protection, grid and
/// configuration. A meter in steady silence sends such frames; showing them changes no
/// pixel.
pub fn same_picture(a: &ac2_proto::Frame, b: &ac2_proto::Frame) -> bool {
    a.stamp.protection == b.stamp.protection
        && a.stamp.grid_id == b.stamp.grid_id
        && a.stamp.config_rev == b.stamp.config_rev
        && a.data == b.data
}

/// Input meters with a moving bar (an input with signal, or a change of state) redraw at
/// most this often: a bar reads as moving at ten steps a second.
const METER_PERIOD: Duration = Duration::from_millis(100);
/// Input meters whose only change is the readout of a silent input (its noise floor)
/// redraw at most this often: a number that changes faster cannot be read anyway.
const METER_QUIET_PERIOD: Duration = Duration::from_millis(500);

/// The meter states of an input-levels frame, per channel; `None` for any other frame.
fn meter_states(f: &ac2_proto::Frame) -> Option<Vec<ac2_scene::meter::MeterState>> {
    use ac2_proto::FrameData;
    let (peak, rms, clip) = match &f.data {
        FrameData::SessionLevels(l) => (&l.peak, &l.rms, &l.clip),
        FrameData::PreviewLevels(l) => (&l.peak, &l.rms, &l.clip),
        _ => return None,
    };
    Some(
        peak.iter()
            .zip(rms)
            .zip(clip)
            .map(|((p, r), c)| {
                ac2_scene::meter::MeterReading::new(*p, *r, *c != ac2_proto::frame::ClipFlags::NONE)
                    .state
            })
            .collect(),
    )
}

/// How soon a change from `prev` to `new` on an input-meter topic needs drawing; `None`
/// for any other topic, which is drawn at once.
fn meter_period(prev: &ac2_proto::Frame, new: &ac2_proto::Frame) -> Option<Duration> {
    use ac2_scene::meter::MeterState;
    let (a, b) = (meter_states(prev)?, meter_states(new)?);
    let quiet = a == b
        && b.iter()
            .all(|s| matches!(s, MeterState::Silent | MeterState::NoData));
    Some(if quiet {
        METER_QUIET_PERIOD
    } else {
        METER_PERIOD
    })
}

/// The data poll: what the UI was last given, per topic.
#[derive(Default)]
struct DataPoll {
    seen: HashMap<Arc<str>, (u64, bool, Arc<ac2_proto::Frame>)>,
    responding: bool,
    /// When a frame with a new `seq` last arrived.
    last_new: Option<Instant>,
    /// When the UI was last woken for data.
    last_wake: Option<Instant>,
    /// An input-meter change handed over without a wake, and how soon it needs drawing.
    meter_due: Option<Duration>,
}

impl DataPoll {
    /// Drains the client's newest frames and hands them to the UI: waking it when the
    /// picture changes (new content, a topic coming or going, a STALE or liveness flip;
    /// input meters at their own rate), quietly when only `seq` and ages moved. `true`
    /// while frames are flowing.
    ///
    /// Polled: the client keeps only the newest frame per topic, so a poll at the display
    /// period loses nothing the screen could show.
    async fn poll(&mut self, client: &Client, out: &Out) -> bool {
        let Ok(latest) = client.latest() else {
            return false;
        };
        let mut new_seq = false;
        let mut shows =
            latest.responding != self.responding || latest.frames.len() != self.seen.len();
        for (k, f) in &latest.frames {
            match self.seen.get(k) {
                Some((seq, stale, prev)) => {
                    shows |= *stale != f.stale;
                    if *seq == f.frame.stamp.seq {
                        continue;
                    }
                    new_seq = true;
                    if same_picture(prev, &f.frame) {
                        continue;
                    }
                    match meter_period(prev, &f.frame) {
                        Some(p) => self.meter_due = Some(self.meter_due.map_or(p, |d| d.min(p))),
                        None => shows = true,
                    }
                }
                None => {
                    new_seq = true;
                    shows = true;
                }
            }
        }
        let now = Instant::now();
        if new_seq {
            self.last_new = Some(now);
        }
        let meters = self.meter_due.is_some_and(|due| {
            self.last_wake
                .is_none_or(|t| now.saturating_duration_since(t) >= due)
        });
        let wake = shows || meters;
        if wake {
            self.last_wake = Some(now);
            self.meter_due = None;
        }
        if shows || new_seq {
            self.seen = latest
                .frames
                .iter()
                .map(|(k, f)| (k.clone(), (f.frame.stamp.seq, f.stale, f.frame.clone())))
                .collect();
            self.responding = latest.responding;
            let mut grids = BTreeMap::new();
            for id in latest.grid_ids() {
                if let Ok(g) = client.grid(id).await {
                    grids.insert(id, g);
                }
            }
            let snapshot = DataSnapshot {
                latest,
                grids,
                drained: Instant::now(),
            };
            out.data(Arc::new(snapshot), wake);
        } else if wake {
            out.wake();
        }
        self.last_new
            .is_some_and(|t| now.saturating_duration_since(t) < ACTIVE_HOLD)
    }
}

fn handle(client: &Client, r: Request, stim: &mpsc::UnboundedSender<StimOp>, out: &Out) {
    match r {
        Request::Call { cmd, what } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = c.call(cmd).await.map(drop).map_err(|e| e.to_string());
                o.send(ConnEvent::Reply { what, result });
            });
        }
        Request::FindDelay {
            meas,
            pick,
            band,
            observation,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let r = c
                    .call(Command::DelayFind {
                        meas,
                        band,
                        observation,
                    })
                    .await
                    .and_then(|r| expect_body!("delay.find", r, ReplyBody::DelayFinding(f) => f));
                match r {
                    Ok(finding) => o.send(ConnEvent::DelayFound {
                        meas,
                        pick,
                        finding: Box::new(finding),
                    }),
                    Err(e) => o.send(ConnEvent::Reply {
                        what: "delay find".into(),
                        result: Err(e.to_string()),
                    }),
                }
            });
        }
        Request::Capture {
            meas,
            slot,
            name,
            replace,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let r = capture(&c, meas, slot, name, replace).await;
                match r {
                    Ok(trace) => o.send(ConnEvent::Captured {
                        slot,
                        trace: Box::new(trace),
                    }),
                    Err(e) => o.send(ConnEvent::Reply {
                        what: format!("capture slot {slot}"),
                        result: Err(e.to_string()),
                    }),
                }
            });
        }
        Request::ImportCurve { path, mic, input } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let what = format!("import curve {}", path.display());
                let result = import_curve(&c, &path, &mic, input).await;
                o.send(ConnEvent::Reply {
                    what: match &result {
                        Ok(label) => {
                            format!("curve {} imported", ac2_scene::cal::curve_name(&mic, label))
                        }
                        Err(_) => what,
                    },
                    result: result.map(|_| ()),
                });
            });
        }
        Request::Import { path, role } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let what = format!("import {}", path.display());
                let result = import(&c, &path, role).await.map(|t| {
                    o.send(ConnEvent::Reply {
                        what: format!("{} imported", t.edit.name),
                        result: Ok(()),
                    });
                });
                if let Err(e) = result {
                    o.send(ConnEvent::Reply {
                        what,
                        result: Err(e),
                    });
                }
            });
        }
        Request::ExportTrace { trace, name, path } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = export_trace(&c, trace, path).await;
                o.send(ConnEvent::Reply {
                    what: match &result {
                        Ok((p, n)) => format!("{name} exported to {} ({n} bytes)", p.display()),
                        Err(_) => format!("export {name}"),
                    },
                    result: result.map(|_| ()),
                });
            });
        }
        Request::Devices => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let r = c
                    .call(Command::SessionDevices)
                    .await
                    .and_then(|r| expect_body!("session.devices", r, ReplyBody::Backends(d) => d))
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::Devices(r));
            });
        }
        Request::OpenSession {
            config,
            inputs,
            transfers,
            what,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                if let Err(e) = c.call(Command::SessionOpen { config }).await {
                    o.send(ConnEvent::Reply {
                        what,
                        result: Err(e.to_string()),
                    });
                    return;
                }
                if !inputs.is_empty()
                    && let Err(e) = c.call(Command::SessionInputs { inputs }).await
                {
                    o.send(ConnEvent::Reply {
                        what: format!("{what}, but the mic names were not set"),
                        result: Err(e.to_string()),
                    });
                    return;
                }
                o.send(ConnEvent::Reply {
                    what,
                    result: Ok(()),
                });
                o.send(ConnEvent::SessionOpened { transfers });
            });
        }
        // Ordered through `preview_task`.
        Request::Preview { .. } | Request::PreviewStop | Request::Meters(_) => {}
        Request::DetectLoopback(d) => {
            let _ = stim.send(StimOp::Detect(d));
        }
        Request::CreateMeas { config } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let name = config.name.clone();
                let created = c
                    .call(Command::MeasCreate { config })
                    .await
                    .and_then(|r| expect_body!("meas.create", r, ReplyBody::Measurement(m) => m));
                let m = match created {
                    Ok(m) => m,
                    Err(e) => {
                        o.send(ConnEvent::Reply {
                            what: format!("new measurement {name}"),
                            result: Err(e.to_string()),
                        });
                        return;
                    }
                };
                let id = m.id;
                o.send(ConnEvent::MeasCreated(Box::new(m)));
                let (what, result) = match c.call(Command::MeasStart { meas: id }).await {
                    Ok(_) => (format!("{name} created and started"), Ok(())),
                    Err(e) => (name, Err(format!("created but not started: {e}"))),
                };
                o.send(ConnEvent::Reply { what, result });
            });
        }
        Request::StimArm { settings, force } => {
            let _ = stim.send(StimOp::Arm { settings, force });
        }
        Request::StimSet(d) => {
            let _ = stim.send(StimOp::Set(d));
        }
        Request::StimStop => {
            let _ = stim.send(StimOp::Stop);
        }
        Request::Sweep { request, name } => {
            let _ = stim.send(StimOp::Sweep { request, name });
        }
        Request::LeqBackfill { meas, ask } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let result = c
                    .call(Command::SplHistoryGet {
                        meas,
                        seconds: ac2_scene::leq::HISTORY_S as u32,
                    })
                    .await
                    .and_then(|r| expect_body!("spl.history_get", r, ReplyBody::SplHistory(h) => h))
                    .map_err(|e| e.to_string());
                o.send(ConnEvent::LeqBackfill { meas, ask, result });
            });
        }
        // Applied by the session loop.
        Request::Topics(_) | Request::DisplayPeriod(_) | Request::Reconnect => {}
    }
}

async fn capture(
    c: &Client,
    meas: MeasId,
    slot: u8,
    name: String,
    replace: Option<TraceId>,
) -> Result<TraceMeta, ClientError> {
    if let Some(old) = replace {
        // The slot's previous trace may already be gone; the capture still proceeds.
        let _ = c.call(Command::TraceDelete { trace: old }).await;
    }
    let r = c
        .call(Command::TraceCapture {
            meas,
            name,
            slot: Some(slot),
        })
        .await?;
    expect_body!("trace.capture", r, ReplyBody::Trace(t) => t)
}

async fn import(c: &Client, path: &std::path::Path, role: ImportRole) -> Result<TraceMeta, String> {
    let content = tokio::task::spawn_blocking({
        let p = path.to_owned();
        move || std::fs::read(&p)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("cannot read: {e}"))?;
    let file_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let r = c
        .call(Command::TraceImport {
            file_name,
            format: ImportFormat::Auto,
            role,
            content: ac2_proto::units::Blob(content),
        })
        .await
        .map_err(|e| e.to_string())?;
    expect_body!("trace.import", r, ReplyBody::Trace(t) => t).map_err(|e| e.to_string())
}

/// `trace.export`s `trace` as ac2 CSV and writes it to `path` (into it, under the daemon's
/// suggested name, when `path` is a folder); returns where it went and its size.
async fn export_trace(
    c: &Client,
    trace: TraceId,
    path: std::path::PathBuf,
) -> Result<(std::path::PathBuf, usize), String> {
    let r = c
        .call(Command::TraceExport {
            trace,
            format: ac2_proto::model::ExportFormat::Ac2Csv,
        })
        .await
        .map_err(|e| e.to_string())?;
    let (file_name, content) = expect_body!(
        "trace.export", r, ReplyBody::Export { file_name, content } => (file_name, content)
    )
    .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let path = if path.is_dir() {
            path.join(file_name)
        } else {
            path
        };
        // Said back absolute: a relative path is relative to where the app was started.
        let path = std::path::absolute(&path).unwrap_or(path);
        std::fs::write(&path, &content.0)
            .map(|()| (path.clone(), content.0.len()))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Reads `path` and imports it into the mic library; returns the curve's label.
async fn import_curve(
    c: &Client,
    path: &std::path::Path,
    mic: &str,
    input: Option<u16>,
) -> Result<String, String> {
    let content = tokio::task::spawn_blocking({
        let p = path.to_owned();
        move || std::fs::read(&p)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("cannot read: {e}"))?;
    let file_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let r = c
        .call(Command::CalCurveImport {
            mic: mic.to_owned(),
            label: None,
            file_name,
            content: ac2_proto::units::Blob(content),
            input,
        })
        .await
        .map_err(|e| e.to_string())?;
    let m =
        expect_body!("cal.curve_import", r, ReplyBody::Mic(m) => m).map_err(|e| e.to_string())?;
    // The curve just imported is the newest of the mic's.
    Ok(m.curves
        .iter()
        .max_by_key(|c| c.imported_at)
        .map(|c| c.label.clone())
        .unwrap_or_default())
}

async fn fetch_trace(c: Client, id: TraceId, grid: GridId, out: Out) {
    let data = match c.call(Command::TraceGet { trace: id }).await {
        Ok(ReplyBody::TraceData(d)) => *d,
        // Not every daemon build serves trace data yet; the trace stays listed without a
        // curve.
        _ => return,
    };
    if let Ok(g) = c.grid(grid).await {
        out.send(ConnEvent::Trace(Arc::new(data), g));
    }
}

/// Opens, renews and stops the device preview strictly in the order the reducer asked:
/// closing the session dialog and opening it again sends a stop and then a preview, and a
/// stop that overtook its preview would close the preview just asked for, leaving the
/// meters blank until the next renewal.
async fn preview_task(client: Client, mut reqs: mpsc::UnboundedReceiver<Request>, out: Out) {
    while let Some(r) = reqs.recv().await {
        match r {
            Request::Preview { backend, device } => {
                let result = client
                    .call(Command::SessionPreview {
                        backend,
                        device: device.clone(),
                    })
                    .await
                    .and_then(|r| expect_body!("session.preview", r, ReplyBody::Preview(p) => p))
                    .map_err(|e| e.to_string());
                out.send(ConnEvent::Preview {
                    backend,
                    device,
                    result,
                });
            }
            // A preview the daemon already closed (session open, expiry) is fine.
            _ => {
                let _ = client.call(Command::SessionPreviewStop).await;
            }
        }
    }
}

/// Owns the lease. Ops run strictly in order, so a stop can never overtake an arm.
async fn stimulus_task(client: Client, mut ops: mpsc::UnboundedReceiver<StimOp>, out: Out) {
    let mut lease: Option<StimulusLease> = None;
    let mut check = tokio::time::interval(Duration::from_millis(100));
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            op = ops.recv() => {
                let Some(op) = op else { break };
                let ev = match op {
                    StimOp::Arm { settings, force } => arm(&client, &mut lease, settings, force).await,
                    StimOp::Set(d) => match &lease {
                        Some(l) => {
                            let firing = d.firing;
                            match l.set(d).await {
                                Ok(_) => StimEvent::Set { firing },
                                Err(e) => StimEvent::Failed(e.to_string()),
                            }
                        }
                        None => StimEvent::Failed("no stimulus lease held".into()),
                    },
                    StimOp::Stop => stop(&client, &mut lease).await,
                    StimOp::Sweep { request, name } => match &lease {
                        Some(l) => match client
                            .call(Command::IrCapture {
                                lease_token: l.token(),
                                request: Box::new(request),
                                name,
                            })
                            .await
                            .and_then(|r| expect_body!("ir.capture", r, ReplyBody::Sweep(s) => s))
                        {
                            Ok(run) => StimEvent::SweepStarted(Box::new(run)),
                            Err(e) => StimEvent::Failed(e.to_string()),
                        },
                        None => StimEvent::Failed("no stimulus lease held".into()),
                    },
                    StimOp::Detect(d) => {
                        let r = detect(&client, &mut lease, d).await;
                        out.send(ConnEvent::LoopbackDetected(r));
                        continue;
                    }
                };
                out.send(ConnEvent::Stimulus(ev));
            }
            // Only a held lease can be lost.
            _ = check.tick(), if lease.is_some() => {
                if let Some(lost) = lease.as_ref().and_then(StimulusLease::lost) {
                    lease = None;
                    let ac2_client::LeaseLost::Refused { msg, .. } = lost;
                    out.send(ConnEvent::Stimulus(StimEvent::Lost(msg)));
                }
            }
        }
    }
    if let Some(l) = lease.take() {
        let _ = l.end().await;
    }
}

async fn arm(
    client: &Client,
    lease: &mut Option<StimulusLease>,
    settings: GeneratorSettings,
    force: bool,
) -> StimEvent {
    if lease.is_none() {
        match client.acquire_lease(force, OnDrop::StopAndRelease).await {
            Ok(l) => *lease = Some(l),
            Err(e) => return StimEvent::Failed(e.to_string()),
        }
    }
    let Some(l) = lease.as_ref() else {
        return StimEvent::Failed("no stimulus lease held".into());
    };
    match l
        .set(GeneratorDesired {
            settings,
            armed: true,
            firing: false,
        })
        .await
    {
        Ok(_) => StimEvent::Armed,
        Err(e) => StimEvent::Failed(e.to_string()),
    }
}

/// Runs a loopback detection under the lease: the held one, else one taken for it (never by
/// force) and released after.
async fn detect(
    client: &Client,
    lease: &mut Option<StimulusLease>,
    d: crate::session_dialog::DetectRequest,
) -> Result<LoopbackDetection, String> {
    let taken = if lease.is_none() {
        Some(
            client
                .acquire_lease(false, OnDrop::StopAndRelease)
                .await
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let token = match (&taken, lease.as_ref()) {
        (Some(l), _) | (None, Some(l)) => l.token(),
        (None, None) => return Err("no stimulus lease".into()),
    };
    let r = client
        .call(Command::SessionDetectLoopback {
            lease_token: token,
            backend: d.backend,
            device: d.device,
            output: d.output,
            level: Some(d.level),
        })
        .await
        .and_then(
            |r| expect_body!("session.detect_loopback", r, ReplyBody::LoopbackDetection(x) => x),
        )
        .map_err(|e| e.to_string());
    if let Some(l) = taken {
        let _ = l.end().await;
    }
    r
}

async fn stop(client: &Client, lease: &mut Option<StimulusLease>) -> StimEvent {
    let r = match lease.take() {
        Some(l) => l.end().await,
        None => client.call(Command::GenStop).await.map(drop),
    };
    match r {
        Ok(()) => StimEvent::Stopped,
        Err(e) => StimEvent::Failed(format!("stop: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ac2_client::TopicFrame;
    use ac2_proto::frame::{ClipFlags, LevelsMeta, SessionLevelsFrame};
    use ac2_proto::units::MeasId;
    use ac2_proto::{Frame, FrameData};

    use super::*;

    fn out() -> (Out, Shared, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let inbox = Shared::default();
        let out = Out {
            inbox: inbox.clone(),
            wake: Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }),
        };
        (out, inbox, wakes)
    }

    /// A frame on `topic` with sequence number `seq` (the body is display material only).
    fn frame(topic: Topic, seq: u64) -> TopicFrame {
        let mut stamp = ac2_proto::samples::stamp(None);
        stamp.seq = seq;
        TopicFrame {
            topic,
            frame: Arc::new(Frame {
                stamp,
                data: FrameData::SessionLevels(SessionLevelsFrame {
                    meta: LevelsMeta { channels: vec![0] },
                    peak: vec![-20.0],
                    rms: vec![-30.0],
                    clip: vec![ClipFlags::NONE],
                }),
            }),
            received: Instant::now(),
            since_new: Duration::ZERO,
            age: Some(0.5),
            stale: false,
        }
    }

    fn snapshot(frames: &[TopicFrame]) -> Arc<DataSnapshot> {
        Arc::new(DataSnapshot {
            latest: Latest {
                frames: frames
                    .iter()
                    .map(|f| (f.topic.to_string().into(), f.clone()))
                    .collect(),
                ..Latest::default()
            },
            grids: BTreeMap::new(),
            drained: Instant::now(),
        })
    }

    const LEQ: Topic = Topic::Data {
        meas: MeasId(4),
        stream: Stream::Leq,
    };
    const TF: Topic = Topic::Data {
        meas: MeasId(1),
        stream: Stream::Tf,
    };

    fn data_of(e: &ConnEvent) -> Option<&DataSnapshot> {
        match e {
            ConnEvent::Data(d) => Some(d),
            _ => None,
        }
    }

    /// A UI that is not drawing holds one snapshot, not a queue of them; the `leq` frames a
    /// newer snapshot replaced still reach it, in order, before the newest; the UI is woken
    /// once until it drains.
    #[test]
    fn an_undrained_ui_holds_the_newest_frames_and_every_leq_second() {
        let (out, inbox, wakes) = out();
        for s in 1..=100 {
            // The TF every poll, the Leq once in ten.
            out.data(snapshot(&[frame(TF, s), frame(LEQ, s.div_ceil(10))]), true);
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        let got = lock(&inbox).take();
        let leq_seqs: Vec<u64> = got
            .iter()
            .filter_map(data_of)
            .filter_map(|d| d.latest.get(&LEQ).map(|f| f.frame.stamp.seq))
            .collect();
        assert_eq!(leq_seqs, (1..=10).collect::<Vec<u64>>());
        // The backlog carries only `leq` topics; the last one is the whole newest snapshot.
        let (last, backlog) = got.split_last().expect("events");
        assert!(
            backlog
                .iter()
                .filter_map(data_of)
                .all(|d| d.latest.get(&TF).is_none())
        );
        assert_eq!(
            data_of(last)
                .and_then(|d| d.latest.get(&TF))
                .map(|f| f.frame.stamp.seq),
            Some(100)
        );
        out.data(snapshot(&[frame(TF, 101)]), true);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    /// Quiet updates (keepalive time, unchanged frames) wait for the UI's next pass without
    /// waking it; a new connection discards what the old one left untaken.
    #[test]
    fn quiet_updates_do_not_wake_and_reconnects_discard() {
        let (out, inbox, wakes) = out();
        out.data(snapshot(&[frame(TF, 1)]), false);
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        assert!(lock(&inbox).data.is_some());
        out.send(ConnEvent::Connecting { target: "t".into() });
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        let i = lock(&inbox);
        assert!(i.data.is_none() && i.mirror.is_none() && i.leq.is_empty());
        assert_eq!(i.events.len(), 1);
    }

    #[test]
    fn ages_count_between_snapshots() {
        let s = snapshot(&[frame(TF, 1)]);
        let later = s.drained + Duration::from_millis(1500);
        let a = s.aged(later);
        let f = a.latest.get(&TF).expect("the TF");
        assert_eq!(f.since_new, Duration::from_millis(1500));
        assert!((f.age.expect("an age") - 2.0).abs() < 1e-9);
        assert_eq!(a.drained, later);
    }

    #[test]
    fn same_picture_ignores_seq_and_time_only() {
        let a = frame(TF, 1);
        let mut b = frame(TF, 2);
        assert!(same_picture(&a.frame, &b.frame));
        Arc::make_mut(&mut b.frame).stamp.protection = ac2_proto::frame::ProtectionFlags::CLIP;
        assert!(!same_picture(&a.frame, &b.frame));
    }
}
