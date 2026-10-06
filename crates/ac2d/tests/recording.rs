//! Raw capture files: record the simulated rig while a transfer function, a spectrum, an
//! RTA and an SPL meter run, replay the file, and the replayed analyses agree with the live
//! ones; the samples of a recording made from the replay are the recorded samples, bit for
//! bit. The sidecar holds the discontinuity and the configuration change at their samples;
//! bounds, shutdown and a daemon that died while recording all leave a finished file.
//!
//! The RTA averages one value per publish interval, and publish intervals follow wall time:
//! live and replayed, it averages the same samples over other spans. Its comparison
//! therefore uses a steady tone, whose band levels do not depend on the span, rather than
//! noise, whose do.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakeFault, FakePath};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_proto::Change;
use ac2_proto::frame::FrameData;
use ac2_proto::model::{
    BackendKind, BandFraction, DiscontinuityCause, GeneratorDesired, GeneratorSettings, MeasConfig,
    MeasKind, RecordRequest, RecordingEnd, RecordingRef, RecordingRun, RecordingStatus, ReplayPace,
    RtaConfig, Signal, SpectrumConfig, State, TraceData,
};
use ac2_proto::units::{Dbfs, MeasId, Seconds, SessionEpoch, TraceId};
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2_traces::raw::{self, TimelineChange, WavReader};
use ac2d::Daemon;
use common::*;

const T: Duration = Duration::from_secs(20);
/// Live audio recorded.
const SECONDS: f64 = 3.0;
/// The rig loses this many frames before block 200.
const LOST: u32 = 1000;
const XRUN_BLOCK: u64 = 200;

/// The common rig with an xrun that loses frames.
fn xrun_rig() -> FakeBackend {
    FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 4,
        outputs: 2,
        drive: FakeDrive::Manual,
        seed: 7,
        paths: vec![
            FakePath::loopback(0, 0, LOOP_DELAY),
            FakePath::acoustic(0, 1, LOOP_DELAY + ACOUSTIC_DELAY, vec![ACOUSTIC_GAIN], 1e-5),
        ],
        faults: vec![FakeFault::Xrun {
            at_block: XRUN_BLOCK,
            lost_frames: LOST,
        }],
        ..FakeConfig::default()
    })
    .unwrap()
}

fn state(c: &mut Client) -> State {
    match c.ok(Command::StateSnapshot) {
        ReplyBody::Snapshot(s) => s.state,
        other => panic!("{other:?}"),
    }
}

fn run_of(r: ReplyBody) -> RecordingRun {
    match r {
        ReplyBody::Recording(r) => r,
        other => panic!("{other:?}"),
    }
}

fn capture(c: &mut Client, meas: u32, name: &str) -> TraceData {
    let id: TraceId = match c.ok(Command::TraceCapture {
        meas: MeasId(meas),
        name: name.into(),
        slot: None,
    }) {
        ReplyBody::Trace(t) => t.id,
        other => panic!("{other:?}"),
    };
    match c.ok(Command::TraceGet { trace: id }) {
        ReplyBody::TraceData(d) => *d,
        other => panic!("{other:?}"),
    }
}

fn record(inputs: Vec<u16>, name: &str, max_s: f64) -> Command {
    Command::RecStart {
        request: RecordRequest {
            inputs,
            name: Some(name.into()),
            max_duration: Seconds(max_s),
            max_bytes: None,
        },
    }
}

/// One past the newest session sample of the xrun rig.
fn session_end(d: &ac2_audio::FakeDriver) -> u64 {
    end_sample(d)
        + if d.blocks() > XRUN_BLOCK {
            u64::from(LOST)
        } else {
            0
        }
}

/// Waits until the fan-out of session `epoch` has handed on everything up to `end`
/// (keepalive sample).
fn wait_handed_on(ka: &Sub, epoch: SessionEpoch, end: u64) {
    ka.frame(T, |f| {
        matches!(f.data, FrameData::Ka(_))
            && f.stamp.session_epoch == epoch
            && f.stamp.audio_sample.0 + 1 >= end
    })
    .unwrap_or_else(|| panic!("audio up to {end} never arrived"));
}

/// The newest SPL frame covering `end`.
fn spl_level(sub: &Sub, end: u64) -> f64 {
    let f = sub
        .frame(T, |f| {
            matches!(f.data, FrameData::Spl(_)) && f.stamp.audio_sample.0 + 1 >= end
        })
        .expect("spl frame covering the audio");
    match f.data {
        FrameData::Spl(s) => s.meta.level,
        _ => unreachable!(),
    }
}

fn assert_close(what: &str, a: &[f32], b: &[f32], tol: f32) {
    assert_eq!(a.len(), b.len(), "{what}: lengths");
    let mut compared = 0;
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if x.is_nan() || y.is_nan() {
            assert_eq!(x.is_nan(), y.is_nan(), "{what}[{i}]: {x} vs {y}");
            continue;
        }
        assert!((x - y).abs() <= tol, "{what}[{i}]: live {x}, replay {y}");
        compared += 1;
    }
    assert!(
        compared > a.len() / 4,
        "{what}: only {compared} values compared"
    );
}

/// Samples of `file` by session sample, from its sidecar's discontinuities.
fn by_session_sample(dir: &Path, name: &str) -> (Vec<(u64, Vec<f32>)>, u16) {
    let s = raw::read_sidecar(dir, name).unwrap();
    let mut r = WavReader::open(&raw::audio_path(dir, name)).unwrap();
    let ch = r.info().channels;
    let mut all = vec![0.0f32; r.info().frames as usize * usize::from(ch)];
    let mut got = 0;
    while got < r.info().frames as usize {
        let n = r.read(&mut all[got * usize::from(ch)..]).unwrap();
        assert!(n > 0);
        got += n;
    }
    let rows = (0..r.info().frames)
        .map(|f| {
            let i = f as usize * usize::from(ch);
            (
                raw::session_sample_of(&s, f),
                all[i..i + usize::from(ch)].to_vec(),
            )
        })
        .collect();
    (rows, ch)
}

#[test]
fn replayed_recording_reproduces_the_live_analyses() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = xrun_rig();
    let mut cfg = config(backend.clone(), inproc("rec"));
    cfg.lease_expiry = Duration::from_secs(120);
    cfg.recording_dir = Some(dir.path().to_owned());
    let h = Daemon::start(cfg).unwrap();
    // The SPL meter's frames, read as they come so none is lost to the queue limit; the
    // other results are captured on request.
    let (mut c, sub) = connect(&h, &[b"d/4/spl"]);
    let ka = Sub::connect(h.context(), h.data_endpoint(), &[b"ka"]);

    // Nothing to record without a session.
    let e = c.call(record(vec![0, 1], "x", 10.0)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{e:?}");

    let live_epoch = match c.ok(Command::SessionOpen {
        config: session(true),
    }) {
        ReplyBody::Session(s) => s.epoch,
        other => panic!("{other:?}"),
    };
    c.ok(Command::MeasCreate {
        config: transfer("main"),
    });
    c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "spec".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(1),
            },
        },
    });
    c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "rta".into(),
            kind: MeasKind::Rta {
                config: RtaConfig::on_input(1, BandFraction::Third),
            },
        },
    });
    c.ok(Command::MeasCreate {
        config: spl("spl", 1),
    });
    for m in 1..=4 {
        c.ok(Command::MeasStart { meas: MeasId(m) });
    }
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });

    // Inputs the session does not capture, and bad bounds, are refused.
    for bad in [
        record(vec![2], "x", 10.0),
        record(vec![0, 0], "x", 10.0),
        record(vec![0], "x", 0.0),
        record(vec![0], "../x", 10.0),
    ] {
        let e = c.call(bad).unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    }
    let started = run_of(c.ok(record(vec![0, 1], "take 1", 60.0)));
    assert!(started.active());
    assert_eq!(started.inputs, vec![0, 1]);
    let e = c.call(record(vec![0], "take 2", 10.0)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "one recording at a time");

    // The live audio: the recorder and the analyses see the same blocks from sample 0.
    let mut d = driver(&backend);
    let mut done = 0.0;
    let mut live_spl = f64::NAN;
    while done < SECONDS {
        run(&mut d, 0.25);
        done += 0.25;
        wait_handed_on(&ka, live_epoch, session_end(&d));
        live_spl = spl_level(&sub, session_end(&d));
        if (done - 1.0f64).abs() < 1e-9 {
            // A configuration change in the middle: it lands in the timeline.
            c.ok(Command::MeasCreate {
                config: MeasConfig {
                    name: "late".into(),
                    kind: MeasKind::Spectrum {
                        config: SpectrumConfig::on_input(0),
                    },
                },
            });
        }
        c.ok(Command::GenRefresh { lease_token: tok });
    }
    let live_end = session_end(&d);
    let live_tf = capture(&mut c, 1, "live tf");
    let live_spec = capture(&mut c, 2, "live spec");

    let done_run = run_of(c.ok(Command::RecStop));
    assert_eq!(
        done_run.status,
        RecordingStatus::Ended {
            reason: RecordingEnd::Stopped
        }
    );
    let frames = d.blocks() * u64::from(BLOCK);
    assert_eq!(
        done_run.frames, frames,
        "every captured frame is in the file"
    );
    assert_eq!(done_run.discontinuities, 1, "the xrun, and nothing else");
    assert_eq!(done_run.bytes, raw::wav::file_bytes(2, frames));
    assert_eq!(
        std::fs::metadata(&done_run.path).unwrap().len(),
        done_run.bytes
    );

    // The sidecar.
    let s = raw::read_sidecar(dir.path(), "take 1").unwrap();
    assert_eq!(s.start.session_sample.0, 0);
    let end = s.end.as_ref().expect("finished");
    assert_eq!(end.frames, frames);
    assert_eq!(end.reason, RecordingEnd::Stopped);
    assert_eq!(end.at.session_sample.0, live_end);
    assert_eq!(s.device.backend, BackendKind::Fake);
    assert_eq!(s.audio.sample_rate, FS);
    assert!(
        s.audio.channels[0]
            .roles
            .contains(&raw::ChannelRole::Loopback)
    );
    assert!(
        s.audio.channels[1]
            .roles
            .contains(&raw::ChannelRole::Measured {
                measurement: "main".into()
            })
    );
    assert_eq!(s.initial.measurements.len(), 4);
    assert!(s.initial.generator.firing);
    let gap = &s.discontinuities[0];
    let xrun_frame = XRUN_BLOCK * u64::from(BLOCK);
    assert_eq!(gap.frame, xrun_frame);
    assert_eq!(gap.session_sample.0, xrun_frame + u64::from(LOST));
    assert_eq!(gap.lost_frames, u64::from(LOST));
    assert!(gap.causes.contains(&DiscontinuityCause::Xrun), "{gap:?}");
    let late = s
        .timeline
        .iter()
        .find(|t| {
            matches!(&t.change, TimelineChange::Measurement(p)
                if matches!(&**p, ac2_proto::Patch::Set(m) if m.config.name == "late"))
        })
        .expect("the change is in the timeline");
    let one_s = u64::from(FS);
    assert!(
        late.at_sample.0 >= one_s && late.at_sample.0 <= one_s + 4096,
        "{late:?}"
    );
    assert!(late.at_sample.0 < xrun_frame);
    assert_eq!(
        late.frame, late.at_sample.0,
        "before the gap, frames are samples"
    );

    match c.ok(Command::RecList) {
        ReplyBody::Recordings(l) => {
            assert_eq!(l.len(), 1);
            assert_eq!(l[0].name, "take 1");
            assert_eq!(l[0].frames, frames);
            assert_eq!(l[0].end, Some(RecordingEnd::Stopped));
        }
        other => panic!("{other:?}"),
    }

    // Replay, as fast as the analyses take it: the same measurements run on the file.
    let session = match c.ok(Command::SessionReplay {
        recording: RecordingRef::Name {
            name: "take 1".into(),
        },
        pace: ReplayPace::Fast,
    }) {
        ReplyBody::Session(s) => s,
        other => panic!("{other:?}"),
    };
    let open = session.open.clone().expect("open");
    assert_eq!(open.backend, BackendKind::Replay);
    assert_eq!(open.replay.as_ref().map(|r| r.frames), Some(frames));
    assert_eq!(open.replay.as_ref().map(|r| r.end_sample.0), Some(live_end));
    assert_eq!(open.config.output_channels, 0);
    let g = state(&mut c).generator;
    assert!(!g.armed && !g.firing, "a replay plays nothing");
    wait_handed_on(&ka, session.epoch, live_end);
    let replay_spl = spl_level(&sub, live_end);
    let rep_tf = capture(&mut c, 1, "replay tf");
    let rep_spec = capture(&mut c, 2, "replay spec");

    // Same samples, same blocks, same resets: the transfer function, the spectrum and the
    // SPL meter agree to within rounding (the RTA: `replayed_rta_reads_a_steady_tone_alike`).
    assert_close("tf mag", &live_tf.mag_db, &rep_tf.mag_db, 0.01);
    assert_close(
        "tf phase",
        live_tf.phase_deg.as_deref().unwrap(),
        rep_tf.phase_deg.as_deref().unwrap(),
        0.1,
    );
    assert_close(
        "tf coherence",
        live_tf.coherence.as_deref().unwrap(),
        rep_tf.coherence.as_deref().unwrap(),
        1e-4,
    );
    assert_close("spectrum", &live_spec.mag_db, &rep_spec.mag_db, 0.01);

    assert!(
        (live_spl - replay_spl).abs() <= 0.01,
        "spl live {live_spl}, replay {replay_spl}"
    );

    // Recording the replay (real time) gives back the recorded samples, bit for bit.
    let again_epoch = match c.ok(Command::SessionReplay {
        recording: RecordingRef::Path {
            path: raw::sidecar_path(dir.path(), "take 1")
                .to_string_lossy()
                .into_owned(),
        },
        pace: ReplayPace::Realtime,
    }) {
        ReplyBody::Session(s) => s.epoch,
        other => panic!("{other:?}"),
    };
    c.ok(record(vec![1, 0], "again", 60.0));
    wait_handed_on(&ka, again_epoch, live_end);
    let again = run_of(c.ok(Command::RecStop));
    assert!(again.frames > 0);
    let (orig, _) = by_session_sample(dir.path(), "take 1");
    let (copy, ch) = by_session_sample(dir.path(), "again");
    assert_eq!(ch, 2);
    let first = copy[0].0;
    let start = orig.iter().position(|(s, _)| *s == first).expect("aligned");
    assert_eq!(
        copy.len(),
        orig.len() - start,
        "the copy runs to the end of the recording"
    );
    for ((s1, a), (s2, b)) in orig[start..].iter().zip(&copy) {
        assert_eq!(s1, s2);
        // The copy recorded input 1 first.
        assert_eq!(a[1].to_bits(), b[0].to_bits(), "sample {s1}");
        assert_eq!(a[0].to_bits(), b[1].to_bits(), "sample {s1}");
    }
    let s2 = raw::read_sidecar(dir.path(), "again").unwrap();
    if first <= xrun_frame {
        assert_eq!(s2.discontinuities.len(), 1, "the replay replays the xrun");
        assert_eq!(s2.discontinuities[0].session_sample, gap.session_sample);
    }
    assert_eq!(s2.device.backend, BackendKind::Replay);

    // A replayed session never had an input the recording lacks.
    c.ok(Command::SessionClose);
    h.shutdown();
}

/// The third-octave RTA of a recorded 1 kHz tone reads the same live and replayed. A steady
/// tone's band levels do not depend on which span of it an interval averages (to within
/// the part of a period an interval cuts off: < 0.05 dB at 1 kHz over the shortest live
/// interval), so the comparison checks the replayed samples, not how either run's
/// publish intervals fell.
#[test]
fn replayed_rta_reads_a_steady_tone_alike() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc("rec-rta"));
    cfg.lease_expiry = Duration::from_secs(120);
    cfg.recording_dir = Some(dir.path().to_owned());
    let h = Daemon::start(cfg).unwrap();
    let (mut c, _sub) = connect(&h, &[]);
    let ka = Sub::connect(h.context(), h.data_endpoint(), &[b"ka"]);
    let live_epoch = match c.ok(Command::SessionOpen {
        config: session(false),
    }) {
        ReplyBody::Session(s) => s.epoch,
        other => panic!("{other:?}"),
    };
    c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "rta".into(),
            kind: MeasKind::Rta {
                config: RtaConfig::on_input(1, BandFraction::Third),
            },
        },
    });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let tok = match c.ok(Command::GenAcquire { force: false }) {
        ReplyBody::Lease(l) => l.lease_token,
        other => panic!("{other:?}"),
    };
    c.ok(Command::GenSet {
        lease_token: tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine {
                    freq: ac2_proto::units::Hz(1000.0),
                },
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    let mut d = driver(&backend);
    // The tone settles (fade-in, the filter bank's transients) before the recording starts.
    run(&mut d, 0.5);
    wait_handed_on(&ka, live_epoch, end_sample(&d));
    c.ok(record(vec![1], "tone", 60.0));
    let mut done = 0.0;
    while done < 3.0 {
        run(&mut d, 0.25);
        done += 0.25;
        wait_handed_on(&ka, live_epoch, end_sample(&d));
        c.ok(Command::GenRefresh { lease_token: tok });
    }
    let live = capture(&mut c, 1, "live rta");
    c.ok(Command::RecStop);

    let epoch = match c.ok(Command::SessionReplay {
        recording: RecordingRef::Name {
            name: "tone".into(),
        },
        pace: ReplayPace::Fast,
    }) {
        ReplyBody::Session(s) => s.epoch,
        other => panic!("{other:?}"),
    };
    let s = raw::read_sidecar(dir.path(), "tone").unwrap();
    let end = s.end.as_ref().map(|e| e.frames).unwrap();
    wait_handed_on(&ka, epoch, end);
    let replayed = capture(&mut c, 1, "replay rta");

    // Every band within 30 dB of the tone's: the tone itself and its filters' skirts. The
    // bands below hold only the path's noise floor, a random signal again.
    let peak = live
        .mag_db
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(f32::NEG_INFINITY, f32::max);
    assert!((peak + 26.0).abs() < 1.0, "the tone reads {peak} dB");
    let near: Vec<usize> = (0..live.mag_db.len())
        .filter(|&i| live.mag_db[i] > peak - 30.0)
        .collect();
    assert!(near.len() >= 3, "{near:?}");
    for i in near {
        let (a, b) = (live.mag_db[i], replayed.mag_db[i]);
        assert!((a - b).abs() <= 0.05, "band {i}: live {a}, replay {b}");
    }
    h.shutdown();
}

#[test]
fn bounds_shutdown_and_a_dead_daemon_leave_finished_files() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc("rec-bounds"));
    cfg.recording_dir = Some(dir.path().to_owned());
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"evt"]);
    c.ok(Command::SessionOpen {
        config: session(false),
    });

    // The duration bound ends it by itself.
    c.ok(record(vec![1], "short", 0.5));
    let mut d = driver(&backend);
    run(&mut d, 1.0);
    let ended = sub
        .event(
            T,
            |e| matches!(&e.change, Change::Recording(r) if !r.active()),
        )
        .expect("the recording ends");
    let Change::Recording(r) = ended.change else {
        unreachable!()
    };
    assert_eq!(
        r.status,
        RecordingStatus::Ended {
            reason: RecordingEnd::DurationLimit
        }
    );
    assert_eq!(r.frames, u64::from(FS) / 2);
    let info = WavReader::open(Path::new(&r.path)).unwrap().info();
    assert_eq!(info.frames, u64::from(FS) / 2);
    let e = c.call(Command::RecStop).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);

    // A size bound below a second of audio is refused; a name in use is refused.
    let e = c
        .call(Command::RecStart {
            request: RecordRequest {
                inputs: vec![0],
                name: None,
                max_duration: Seconds(10.0),
                max_bytes: Some(1000),
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    let e = c.call(record(vec![0], "short", 10.0)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);

    // A shutdown while recording finishes the file and says why.
    let rec = run_of(c.ok(Command::RecStart {
        request: RecordRequest {
            inputs: vec![0, 1],
            name: None,
            max_duration: Seconds(60.0),
            max_bytes: Some(1 << 30),
        },
    }));
    assert!(rec.name.starts_with("rec-"), "{}", rec.name);
    run_of_state_progress(&mut c, &mut d);
    h.shutdown();
    let s = raw::read_sidecar(dir.path(), &rec.name).unwrap();
    let end = s.end.expect("finished at shutdown");
    assert_eq!(end.reason, RecordingEnd::DaemonShutdown);
    assert!(end.frames > 0);
    assert_eq!(
        WavReader::open(&raw::audio_path(dir.path(), &rec.name))
            .unwrap()
            .info()
            .frames,
        end.frames
    );

    // A daemon that died while recording left a sidecar without an end; the next start
    // finishes it as interrupted and says so.
    let mut s = raw::read_sidecar(dir.path(), &rec.name).unwrap();
    s.end = None;
    s.audio.file = "dead.wav".into();
    raw::write_sidecar(dir.path(), "dead", &s).unwrap();
    std::fs::copy(
        raw::audio_path(dir.path(), &rec.name),
        raw::audio_path(dir.path(), "dead"),
    )
    .unwrap();
    let f = std::fs::File::options()
        .write(true)
        .open(raw::audio_path(dir.path(), "dead"))
        .unwrap();
    f.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    drop(f);
    let mut cfg = config(manual_rig(), inproc("rec-recover"));
    cfg.recording_dir = Some(dir.path().to_owned());
    let h = Daemon::start(cfg).unwrap();
    let (mut c, _sub) = connect(&h, &[]);
    let st = state(&mut c);
    let r = st.recording.expect("the interrupted recording is shown");
    assert_eq!(r.name, "dead");
    assert_eq!(
        r.status,
        RecordingStatus::Ended {
            reason: RecordingEnd::Interrupted
        }
    );
    let s = raw::read_sidecar(dir.path(), "dead").unwrap();
    assert_eq!(s.end.map(|e| e.reason), Some(RecordingEnd::Interrupted));
    // Replaying needs the inputs it recorded; a live session cannot be opened as a replay.
    let e = c
        .call(Command::SessionOpen {
            config: ac2_proto::model::SessionConfig {
                backend: Some(BackendKind::Replay),
                ..session(false)
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    let e = c
        .call(Command::SessionReplay {
            recording: RecordingRef::Name {
                name: "nothing".into(),
            },
            pace: ReplayPace::Fast,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    h.shutdown();
}

/// Runs audio until the recording has reported frames.
fn run_of_state_progress(c: &mut Client, d: &mut ac2_audio::FakeDriver) {
    let deadline = Instant::now() + T;
    loop {
        run(d, 1.2);
        if state(c).recording.is_some_and(|r| r.frames > 0) {
            return;
        }
        assert!(Instant::now() < deadline, "no progress");
    }
}
