//! The daemon link: one background thread with a small tokio runtime that owns the
//! [`Client`], the data subscription and the stimulus lease.
//!
//! The UI thread never awaits. It sends [`Request`]s and drains [`ConnEvent`]s once per
//! frame; the thread wakes the UI (`request_repaint`) whenever it sends something, so the
//! UI renders on change: a new frame `seq`, a mirror change, a reply — plus a 4 Hz refresh
//! while live data is shown, so frame ages and STALE keep counting when frames stop.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_client::{
    Client, ClientConfig, ClientError, Latest, MirrorView, OnDrop, StimulusLease, expect_body,
};
use ac2_proto::model::{
    DelayFinding, DelayPick, FinderBand, GeneratorDesired, GeneratorSettings, TraceData, TraceMeta,
};
use ac2_proto::units::{ClientId, MeasId, TraceId};
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
    /// A stored trace's data and grid (fetched once per trace).
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
        trace: TraceMeta,
    },
    Stimulus(StimEvent),
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
    /// `delay.find` (auto band) on `meas`, to insert `pick` from; the reducer decides what to
    /// insert once the finding is back.
    FindDelay { meas: MeasId, pick: DelayPick },
    /// `trace.capture` of `meas` into `slot`, deleting `replace` first.
    Capture {
        meas: MeasId,
        slot: u8,
        replace: Option<TraceId>,
    },
    /// Drop the connection and connect again now.
    Reconnect,
}

enum Ctl {
    Req(Request),
    Shutdown,
}

/// Wakes the UI thread.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Handle to the link thread. Dropping it stops the stimulus (if held), releases the lease
/// and joins the thread.
pub struct Conn {
    tx: mpsc::UnboundedSender<Ctl>,
    rx: std_mpsc::Receiver<ConnEvent>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn").finish_non_exhaustive()
    }
}

/// Retry delay after a failed connect.
pub const RETRY_EVERY: Duration = Duration::from_secs(2);
/// Deadline of one connect attempt (`hello` round trip).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Data drain period.
const POLL_EVERY: Duration = Duration::from_millis(8);
/// Refresh of ages / STALE while live data is shown and no new frame arrives.
const AGE_REFRESH: Duration = Duration::from_millis(250);
/// Re-send of an unchanged mirror view, for the keepalive age.
const MIRROR_REFRESH: Duration = Duration::from_millis(500);

impl Conn {
    pub fn start(target: Target, wake: Wake) -> std::io::Result<Self> {
        let (tx, ctl_rx) = mpsc::unbounded_channel();
        let (ev_tx, rx) = std_mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ac2-ui link".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("ac2-ui io")
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ev_tx.send(ConnEvent::Failed {
                            target: target.describe.clone(),
                            error: format!("runtime: {e}"),
                            retry_in: Duration::MAX,
                        });
                        wake();
                        return;
                    }
                };
                rt.block_on(run(target, ctl_rx, Out { tx: ev_tx, wake }));
            })?;
        Ok(Self {
            tx,
            rx,
            thread: Some(thread),
        })
    }

    pub fn send(&self, r: Request) {
        let _ = self.tx.send(Ctl::Req(r));
    }

    /// Everything reported since the last call.
    pub fn drain(&self) -> Vec<ConnEvent> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        let _ = self.tx.send(Ctl::Shutdown);
        if let Some(t) = self.thread.take() {
            // The stimulus stop + release round trip is bounded by the client's retry
            // policy; the daemon's lease expiry fades the output out regardless.
            let _ = t.join();
        }
    }
}

#[derive(Clone)]
struct Out {
    tx: std_mpsc::Sender<ConnEvent>,
    wake: Wake,
}

impl Out {
    fn send(&self, e: ConnEvent) {
        let _ = self.tx.send(e);
        (self.wake)();
    }
}

enum Next {
    Reconnect,
    Exit,
}

async fn run(target: Target, mut ctl: mpsc::UnboundedReceiver<Ctl>, out: Out) {
    loop {
        out.send(ConnEvent::Connecting {
            target: target.describe.clone(),
        });
        let attempt =
            tokio::time::timeout(CONNECT_TIMEOUT, Client::connect(target.config.clone())).await;
        let client = match attempt {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                if wait_retry(&target, &mut ctl, &out, describe_err(&e)).await {
                    continue;
                }
                return;
            }
            Err(_) => {
                if wait_retry(&target, &mut ctl, &out, "not responding".into()).await {
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
        match session(client, &mut ctl, &out).await {
            Next::Reconnect => continue,
            Next::Exit => return,
        }
    }
}

fn describe_err(e: &ClientError) -> String {
    match e {
        ClientError::Timeout { .. } => "not responding".into(),
        other => other.to_string(),
    }
}

/// Reports the failure and waits [`RETRY_EVERY`]; `false` when the UI is shutting down.
async fn wait_retry(
    target: &Target,
    ctl: &mut mpsc::UnboundedReceiver<Ctl>,
    out: &Out,
    error: String,
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
                Some(Ctl::Req(r)) => out.send(ConnEvent::Reply {
                    what: request_name(&r),
                    result: Err("not connected".into()),
                }),
            },
        }
    }
}

fn request_name(r: &Request) -> String {
    match r {
        Request::Call { what, .. } => what.clone(),
        Request::StimArm { .. } => "arm".into(),
        Request::StimSet(_) => "stimulus".into(),
        Request::StimStop => "stop".into(),
        Request::Capture { slot, .. } => format!("capture slot {slot}"),
        Request::FindDelay { .. } => "delay find".into(),
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
}

async fn session(client: Client, ctl: &mut mpsc::UnboundedReceiver<Ctl>, out: &Out) -> Next {
    if let Err(e) = client.subscribe(Subscription::AllData) {
        out.send(ConnEvent::Reply {
            what: "subscribe".into(),
            result: Err(e.to_string()),
        });
    }
    let (stim_tx, stim_rx) = mpsc::unbounded_channel();
    let stim = tokio::spawn(stimulus_task(client.clone(), stim_rx, out.clone()));
    let mut mirror = client.watch();
    let mut tick = tokio::time::interval(POLL_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut seqs: HashMap<String, u64> = HashMap::new();
    let mut responding = false;
    let mut last_push = Instant::now();
    let mut last_mirror = Instant::now();
    let mut fetched: HashSet<TraceId> = HashSet::new();
    let next = loop {
        tokio::select! {
            c = ctl.recv() => match c {
                None | Some(Ctl::Shutdown) => break Next::Exit,
                Some(Ctl::Req(Request::Reconnect)) => break Next::Reconnect,
                Some(Ctl::Req(r)) => handle(&client, r, &stim_tx, out),
            },
            changed = mirror.changed() => {
                if changed.is_err() {
                    break Next::Reconnect;
                }
                let v = mirror.borrow_and_update().clone();
                if let Some(st) = &v.state {
                    for t in &st.traces {
                        if fetched.insert(t.id) {
                            tokio::spawn(fetch_trace(client.clone(), t.id, t.grid_id, out.clone()));
                        }
                    }
                }
                last_mirror = Instant::now();
                out.send(ConnEvent::Mirror(v));
            },
            _ = tick.tick() => {
                // Keepalives stopping changes nothing in the mirror; re-send it so the UI
                // shows "not responding" even when no frames are flowing.
                if last_mirror.elapsed() >= MIRROR_REFRESH {
                    last_mirror = Instant::now();
                    out.send(ConnEvent::Mirror(client.view()));
                }
                let Ok(latest) = client.latest() else { continue };
                let fresh = latest.frames.iter().any(|(k, f)| seqs.get(k) != Some(&f.frame.stamp.seq))
                    || latest.frames.len() != seqs.len()
                    || latest.responding != responding;
                let refresh = !latest.frames.is_empty() && last_push.elapsed() >= AGE_REFRESH;
                if !(fresh || refresh) {
                    continue;
                }
                seqs = latest.frames.iter().map(|(k, f)| (k.clone(), f.frame.stamp.seq)).collect();
                responding = latest.responding;
                let mut grids = BTreeMap::new();
                for id in latest.grid_ids() {
                    if let Ok(g) = client.grid(id).await {
                        grids.insert(id, g);
                    }
                }
                last_push = Instant::now();
                out.send(ConnEvent::Data(Arc::new(DataSnapshot { latest, grids, drained: last_push })));
            },
        }
    };
    // Closing the channel makes the stimulus task stop the output and release the lease.
    drop(stim_tx);
    let _ = tokio::time::timeout(Duration::from_secs(5), stim).await;
    next
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
        Request::FindDelay { meas, pick } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let r = c
                    .call(Command::DelayFind {
                        meas,
                        band: FinderBand::Auto,
                        observation: None,
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
            replace,
        } => {
            let (c, o) = (client.clone(), out.clone());
            tokio::spawn(async move {
                let r = capture(&c, meas, slot, replace).await;
                match r {
                    Ok(trace) => o.send(ConnEvent::Captured { slot, trace }),
                    Err(e) => o.send(ConnEvent::Reply {
                        what: format!("capture slot {slot}"),
                        result: Err(e.to_string()),
                    }),
                }
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
        Request::Reconnect => {}
    }
}

async fn capture(
    c: &Client,
    meas: MeasId,
    slot: u8,
    replace: Option<TraceId>,
) -> Result<TraceMeta, ClientError> {
    if let Some(old) = replace {
        // The slot's previous trace may already be gone; the capture still proceeds.
        let _ = c.call(Command::TraceDelete { trace: old }).await;
    }
    let r = c
        .call(Command::TraceCapture {
            meas,
            name: format!("slot {slot}"),
        })
        .await?;
    expect_body!("trace.capture", r, ReplyBody::Trace(t) => t)
}

async fn fetch_trace(c: Client, id: TraceId, grid: GridId, out: Out) {
    let data = match c.call(Command::TraceGet { trace: id }).await {
        Ok(ReplyBody::TraceData(d)) => d,
        // Not every daemon build serves trace data yet; the trace stays listed without a
        // curve.
        _ => return,
    };
    if let Ok(g) = c.grid(grid).await {
        out.send(ConnEvent::Trace(Arc::new(data), g));
    }
}

/// Owns the lease. Ops run strictly in order, so a stop can never overtake an arm.
async fn stimulus_task(client: Client, mut ops: mpsc::UnboundedReceiver<StimOp>, out: Out) {
    let mut lease: Option<StimulusLease> = None;
    let mut check = tokio::time::interval(Duration::from_millis(100));
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
                };
                out.send(ConnEvent::Stimulus(ev));
            }
            _ = check.tick() => {
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
