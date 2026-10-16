//! OSC cues end to end on the fake rig: a UDP receiver on the loopback interface is the
//! rig's OSC target. The target is rev-guarded, `osc.test` arrives, every cue is sent on a
//! change and again every second, a meter that cannot judge (not running, no session,
//! uncalibrated limit) is `fault`, a deleted measurement sends `ok`; with a calibrated tone
//! over a limit, `over` carries the level and turning it down sends `ok`.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, ClientError, Endpoints, OnDrop};
use ac2_proto::model::{
    CueState, CueSubject, GeneratorDesired, GeneratorSettings, LeqConfig, LeqWindow, MeasConfig,
    MeasKind, OscArg, OscCue, OscMessage, OscPath, OwnedTraces, PeakWeighting, Signal, SplConfig,
    TimeWeighting, Weighting,
};
use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, MeasId, Rev, Seconds};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ReplyBody};
use ac2d::{AutosaveConfig, Daemon, DaemonConfig, Handle};
use common::*;
use rosc::{OscPacket, OscType};

const WAIT: Duration = Duration::from_secs(20);

async fn connect(h: &Handle) -> Client {
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    let c = Client::connect(ClientConfig::new(ep, "ac2d-osc-test"))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c
}

/// The OSC target: a socket on the loopback interface.
struct Receiver(UdpSocket);

impl Receiver {
    fn new() -> Self {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        Self(s)
    }

    fn addr(&self) -> SocketAddr {
        self.0.local_addr().unwrap()
    }

    /// The next message to `path` (others are skipped) and when it arrived.
    fn until(&self, path: &str) -> (rosc::OscMessage, Instant) {
        let deadline = Instant::now() + WAIT;
        let mut buf = [0u8; 1024];
        loop {
            assert!(Instant::now() < deadline, "no {path} within {WAIT:?}");
            let Ok(n) = self.0.recv(&mut buf) else {
                continue;
            };
            if let OscPacket::Message(m) = rosc::decoder::decode_udp(&buf[..n]).unwrap().1
                && m.addr == path
            {
                return (m, Instant::now());
            }
        }
    }

    /// Every message's path that arrives within `d`.
    fn during(&self, d: Duration) -> Vec<String> {
        let end = Instant::now() + d;
        let mut buf = [0u8; 1024];
        let mut out = Vec::new();
        while Instant::now() < end {
            if let Ok(n) = self.0.recv(&mut buf)
                && let OscPacket::Message(m) = rosc::decoder::decode_udp(&buf[..n]).unwrap().1
            {
                out.push(m.addr);
            }
        }
        out
    }
}

fn msg(path: &str, append_level: bool) -> OscMessage {
    OscMessage {
        path: OscPath::new(path).unwrap(),
        args: vec![],
        append_level,
    }
}

/// `/<name>/<state>` for every state, the level appended.
fn cue(name: &str) -> Option<Box<OscCue>> {
    let m = |s: &str| Some(msg(&format!("/{name}/{s}"), true));
    Some(Box::new(OscCue {
        ok: m("ok"),
        near: m("near"),
        over: m("over"),
        fault: m("fault"),
    }))
}

fn window(limit: Option<f64>, name: &str) -> LeqWindow {
    LeqWindow {
        cue: cue(name),
        duration: Seconds(1.0),
        weighting: Weighting::A,
        limit: limit.map(DbSpl),
        warn_margin: Db(3.0),
    }
}

fn meter(name: &str, windows: Vec<LeqWindow>, meter_cue: Option<Box<OscCue>>) -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: name.into(),
        kind: MeasKind::Spl {
            config: SplConfig {
                cue: meter_cue,
                bands: None,
                input: 1,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
                leq: LeqConfig {
                    windows,
                    horizon: Seconds(2.0),
                    peaks: Default::default(),
                },
                position: None,
            },
        },
    }
}

async fn create(c: &Client, config: MeasConfig) -> MeasId {
    match c.call(Command::MeasCreate { config }).await.unwrap() {
        ReplyBody::Measurement(m) => m.id,
        other => panic!("{other:?}"),
    }
}

fn rig_rev(c: &Client) -> Rev {
    c.view().state.clone().unwrap().rig.rev
}

async fn set_target(c: &Client, addr: SocketAddr) {
    c.call(Command::OscTarget {
        base: rig_rev(c),
        addr: Some(addr),
    })
    .await
    .unwrap();
    let deadline = Instant::now() + WAIT;
    while c.view().state.clone().unwrap().rig.osc_target != Some(addr) {
        assert!(Instant::now() < deadline, "target never in the state");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until the rig entity lists `meas`'s window cue in `state`.
async fn rig_cue_until(c: &Client, meas: MeasId, state: CueState) {
    let deadline = Instant::now() + WAIT;
    loop {
        let s = c.view().state.clone().unwrap();
        if s.rig.cues.iter().any(|r| {
            r.meas == meas && matches!(r.subject, CueSubject::Window { .. }) && r.state == state
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "rig cues never had {meas:?} {state:?}: {:?}",
            s.rig.cues
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cues_follow_the_meters_and_repeat() {
    init_log();
    let h = Daemon::start(steady_config(realtime_rig(), local_tcp())).unwrap();
    let c = connect(&h).await;
    let rx = Receiver::new();

    // The target: set on the rig's rev, a stale rev is a conflict naming the current one.
    let stale = rig_rev(&c);
    set_target(&c, rx.addr()).await;
    let e = c
        .call(Command::OscTarget {
            base: stale,
            addr: None,
        })
        .await
        .unwrap_err();
    match e {
        ClientError::Daemon(p) => {
            assert_eq!(p.code, ErrorCode::Conflict);
            assert_eq!(p.detail, Some(ErrorDetail::Conflict { rev: rig_rev(&c) }));
        }
        other => panic!("{other:?}"),
    }

    // A test message arrives as given, 0 dB appended.
    c.call(Command::OscTest {
        msg: OscMessage {
            path: OscPath::new("/test").unwrap(),
            args: vec![OscArg::Int(7), OscArg::Str("hi".into())],
            append_level: true,
        },
    })
    .await
    .unwrap();
    let (m, _) = rx.until("/test");
    assert_eq!(
        m.args,
        [
            OscType::Int(7),
            OscType::String("hi".into()),
            OscType::Float(0.0)
        ]
    );

    // A meter with no session cannot judge: fault, with no level.
    let a = create(&c, meter("A", vec![window(Some(90.0), "a")], None)).await;
    let (m, _) = rx.until("/a/fault");
    assert!(m.args.is_empty(), "{:?}", m.args);

    // Running uncalibrated, its limit is not judged: still fault. A meter without a limit
    // is ok once its first second is judged.
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    let b = create(&c, meter("B", vec![window(None, "b")], None)).await;
    c.call(Command::MeasStart { meas: a }).await.unwrap();
    c.call(Command::MeasStart { meas: b }).await.unwrap();
    rx.until("/b/ok");
    rig_cue_until(&c, b, CueState::Ok).await;
    rig_cue_until(&c, a, CueState::Fault).await;
    rx.until("/a/fault");

    // Sent again every second.
    let (_, t0) = rx.until("/b/ok");
    let (_, t1) = rx.until("/b/ok");
    let gap = t1 - t0;
    assert!(
        gap > Duration::from_millis(500) && gap < Duration::from_millis(1600),
        "{gap:?}"
    );

    // Deleted: ok once, then nothing more for it.
    c.call(Command::MeasDelete {
        meas: a,
        traces: OwnedTraces::Keep,
    })
    .await
    .unwrap();
    rx.until("/a/ok");

    // The session closed: the rig is blind.
    c.call(Command::SessionClose).await.unwrap();
    rx.until("/b/fault");
    rig_cue_until(&c, b, CueState::Fault).await;
    assert!(c.view().state.clone().unwrap().rig.osc.sent > 0);

    // A clean shutdown leaves every light at ok.
    drop(h);
    rx.until("/b/ok");
}

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

async fn calibrate(c: &Client) {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        match c
            .call(Command::CalSpl {
                input: 1,
                mic: "M30".into(),
                calibrator_level: DbSpl(94.0),
                calibrator_freq: Hz(1000.0),
            })
            .await
        {
            Ok(ReplyBody::Calibration(_)) => return,
            // The tone takes a moment to reach the input and settle.
            Err(ClientError::Daemon(p))
                if p.msg.contains("not steady") || p.msg.contains("no calibrator signal") => {}
            other => panic!("{other:?}"),
        }
        assert!(Instant::now() < deadline, "calibrator never steady");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn level_of(m: &rosc::OscMessage) -> f32 {
    match m.args.last() {
        Some(OscType::Float(l)) => *l,
        other => panic!("no level: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_a_tone_over_the_limit_lights_over_and_back_ok() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = steady_config(realtime_rig(), local_tcp());
    cfg.cal_store = Some(dir.path().join("calibrations.json"));
    let h = Daemon::start(cfg).unwrap();
    let c = connect(&h).await;
    let rx = Receiver::new();
    set_target(&c, rx.addr()).await;
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    let m = create(
        &c,
        meter("FOH", vec![window(Some(90.0), "w")], cue("meter")),
    )
    .await;
    c.call(Command::MeasStart { meas: m }).await.unwrap();
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    // The tone reaches input 1 at −26 dBFS, calibrated as 94 dB SPL: over the 90 dB limit.
    lease.set(tone(-20.0)).await.unwrap();
    calibrate(&c).await;
    let (w, _) = rx.until("/w/over");
    assert!((level_of(&w) - 94.0).abs() < 0.5, "{:?}", w.args);
    let (mc, _) = rx.until("/meter/over");
    assert!((level_of(&mc) - 94.0).abs() < 0.5, "{:?}", mc.args);
    rig_cue_until(&c, m, CueState::Over).await;

    // 20 dB down (74 dB SPL): the 1 s window is back under at once, so ok.
    lease.set(tone(-40.0)).await.unwrap();
    let (w, _) = rx.until("/w/ok");
    assert!(level_of(&w) < 87.0, "{:?}", w.args);
    rx.until("/meter/ok");
    rig_cue_until(&c, m, CueState::Ok).await;
    drop(lease);
    drop(h);
}

/// A daemon keeping its rig settings, sessions, rig logs and autosave in `dir`.
fn rig_config(dir: &std::path::Path) -> DaemonConfig {
    let mut cfg = steady_config(realtime_rig(), local_tcp());
    cfg.session_dir = dir.join("sessions");
    cfg.rig_settings = Some(dir.join("rig.json"));
    cfg.rig_logs = Some(dir.join("rig-logs"));
    cfg.autosave = Some(AutosaveConfig {
        dir: dir.join("autosave"),
        restore: true,
    });
    cfg
}

fn rig_scope(config: MeasConfig) -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Rig,
        ..config
    }
}

/// A cued meter made the rig's outlives a load: its cue stays in the table and keeps being
/// sent, with no `ok` (the removal message) on the way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rig_meters_cue_survives_a_load() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let h = Daemon::start(rig_config(dir.path())).unwrap();
    let c = connect(&h).await;
    let rx = Receiver::new();
    set_target(&c, rx.addr()).await;
    c.call(Command::FileSave {
        session: ac2_proto::model::SessionRef::Name {
            name: "empty".into(),
        },
        overwrite: true,
    })
    .await
    .unwrap();

    // Uncalibrated, the limit cannot be judged: fault, which the load must not clear.
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    let config = meter("FOH", vec![window(Some(90.0), "w")], None);
    let a = create(&c, config.clone()).await;
    c.call(Command::MeasStart { meas: a }).await.unwrap();
    rx.until("/w/fault");
    meas_update(&c, a, rig_scope(config)).await.unwrap();

    c.call(Command::FileLoad {
        session: ac2_proto::model::SessionRef::Name {
            name: "empty".into(),
        },
        discard: true,
    })
    .await
    .unwrap();
    let seen = rx.during(Duration::from_millis(2500));
    assert!(!seen.iter().any(|p| p == "/w/ok"), "{seen:?}");
    assert!(
        seen.iter().filter(|p| *p == "/w/fault").count() >= 2,
        "{seen:?}"
    );
    let st = c.snapshot().await.unwrap().state;
    assert!(st.measurements.iter().any(|m| m.id == a));
    assert!(
        st.rig
            .cues
            .iter()
            .any(|r| r.meas == a && r.state == CueState::Fault),
        "{:?}",
        st.rig.cues
    );
    drop(c);
    drop(h);
}

/// With the monitor at start on, a restarted daemon's cues reach their live states with no
/// client: the target and the meter come back from the rig settings, the audio reopens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn monitor_at_start_cues_with_no_client() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let rx = Receiver::new();
    {
        let h = Daemon::start(rig_config(dir.path())).unwrap();
        let c = connect(&h).await;
        set_target(&c, rx.addr()).await;
        let m = create(&c, rig_scope(meter("Stage", vec![window(None, "m")], None))).await;
        c.call(Command::MeasStart { meas: m }).await.unwrap();
        c.call(Command::RigMonitorStart {
            base: rig_rev(&c),
            on: true,
        })
        .await
        .unwrap();
        c.call(Command::SessionOpen {
            config: session(false),
        })
        .await
        .unwrap();
        rx.until("/m/ok");
        drop(c);
        drop(h);
    }
    // Whatever the first daemon sent, up to its clean shutdown's ok.
    rx.during(Duration::from_millis(300));

    let h = Daemon::start(rig_config(dir.path())).unwrap();
    let mut seen = Vec::new();
    let deadline = Instant::now() + WAIT;
    while !seen.iter().any(|p| p == "/m/ok") {
        assert!(Instant::now() < deadline, "never live: {seen:?}");
        seen.extend(rx.during(Duration::from_millis(200)));
    }
    // Before its first judged second the meter says nothing, or fault while the audio is
    // not open yet; nothing else.
    let first_ok = seen.iter().position(|p| p == "/m/ok").unwrap();
    assert!(seen[..first_ok].iter().all(|p| p == "/m/fault"), "{seen:?}");
    drop(h);
}
