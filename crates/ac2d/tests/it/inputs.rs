//! Declared input kinds and pairs (`docs/design/programme-meter.md` §1, §2): the pair rules
//! name the inputs they refuse, an SPL meter wants a microphone, a line calibration wants a
//! line input and lands in `cal.list`, and a preview's frames carry the declared kinds and
//! pairs so a meter page can draw them before a session exists.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_proto::frame::FrameData;
use ac2_proto::model::{
    BackendKind, CalRef, CalStatus, DeviceId, GeneratorDesired, GeneratorSettings, InputKind,
    InputSetup, Pair, PairSide, PreviewInput, Signal,
};
use ac2_proto::units::{Dbfs, Hz, Volts};
use ac2_proto::{Command, ErrorCode, ReplyBody, Subscription};
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(5);

fn start() -> Handle {
    init_log();
    Daemon::start(steady_config(realtime_rig(), inproc("inputs"))).unwrap()
}

fn fake_dev() -> DeviceId {
    DeviceId(ac2_audio::fake::FAKE_DEVICE_ID.into())
}

fn row(channel: u16, kind: InputKind) -> InputSetup {
    InputSetup {
        channel,
        kind,
        pair: None,
    }
}

fn pair(left: u16, right: u16) -> Pair {
    Pair {
        left,
        right,
        name: None,
    }
}

fn open_four(c: &mut Client) {
    let mut cfg = session(false);
    cfg.input_channels = vec![0, 1, 2, 3];
    c.ok(Command::SessionOpen { config: cfg });
}

fn declare(c: &mut Client, inputs: Vec<InputSetup>, pairs: Vec<Pair>) -> Result<ReplyBody, String> {
    c.call(Command::SessionInputs { inputs, pairs })
        .map_err(|e| {
            assert_eq!(e.code, ErrorCode::Invalid, "{}", e.msg);
            e.msg
        })
}

#[test]
fn pair_rules_name_the_inputs_they_refuse() {
    let h = start();
    let (mut c, _s) = connect(&h, &[]);
    open_four(&mut c);
    let rows = vec![
        row(0, InputKind::Line),
        row(1, InputKind::Line),
        row(2, InputKind::Digital),
        row(
            3,
            InputKind::Mic {
                mic: None,
                curve: ac2_proto::model::CurveChoice::NotChosen,
            },
        ),
    ];
    // Of one kind: a line and a digital input do not pair.
    let e = declare(&mut c, rows.clone(), vec![pair(1, 2)]).unwrap_err();
    assert_eq!(
        e,
        "inputs 2 and 3 are of different kinds (line input and digital input); a pair is two of one kind"
    );
    // A microphone pairs only with a microphone.
    let e = declare(&mut c, rows.clone(), vec![pair(3, 0)]).unwrap_err();
    assert_eq!(
        e,
        "inputs 4 and 1 are of different kinds (microphone and line input); a pair is two of one kind"
    );
    // One pair per input, and never with itself.
    let e = declare(&mut c, rows.clone(), vec![pair(0, 1), pair(1, 2)]).unwrap_err();
    assert_eq!(e, "input 2 is in two pairs");
    let e = declare(&mut c, rows.clone(), vec![pair(0, 0)]).unwrap_err();
    assert_eq!(e, "input 1 cannot be paired with itself");
    // Only inputs the session captures.
    let e = declare(&mut c, rows.clone(), vec![pair(0, 7)]).unwrap_err();
    assert_eq!(
        e,
        "input 8 is not captured by the session, so it cannot be paired"
    );
    // Nothing of the refused attempts stuck.
    let ReplyBody::Snapshot(snap) = c.ok(Command::StateSnapshot) else {
        panic!("state");
    };
    let st = snap.state;
    assert!(st.inputs.is_empty() && st.pairs.is_empty());
    // A valid pair lands, and the daemon marks the rows' sides.
    let ReplyBody::Inputs(all) = declare(
        &mut c,
        rows,
        vec![Pair {
            left: 0,
            right: 1,
            name: Some("Main".into()),
        }],
    )
    .unwrap() else {
        panic!("inputs");
    };
    assert_eq!(all.pairs.len(), 1);
    let side = |ch: u16| all.inputs.iter().find(|r| r.channel == ch).unwrap().pair;
    assert_eq!(side(0), Some(PairSide::Left));
    assert_eq!(side(1), Some(PairSide::Right));
    assert_eq!(side(2), None);
    // Replacing the pair list with an empty one unpairs them.
    let ReplyBody::Inputs(all) = declare(&mut c, Vec::new(), Vec::new()).unwrap() else {
        panic!("inputs");
    };
    assert!(all.pairs.is_empty());
    assert!(all.inputs.iter().all(|r| r.pair.is_none()));
    // Two mics pair, whatever their mic names: an undeclared input is a mic too.
    let mic = |name: Option<&str>| InputKind::Mic {
        mic: name.map(Into::into),
        curve: ac2_proto::model::CurveChoice::NotChosen,
    };
    let ReplyBody::Inputs(all) = declare(
        &mut c,
        vec![row(2, mic(Some("M30"))), row(3, mic(None))],
        vec![pair(2, 3)],
    )
    .unwrap() else {
        panic!("inputs");
    };
    assert_eq!(all.pairs.len(), 1);
    let side = |ch: u16| all.inputs.iter().find(|r| r.channel == ch).unwrap().pair;
    assert_eq!(side(2), Some(PairSide::Left));
    assert_eq!(side(3), Some(PairSide::Right));
}

#[test]
fn an_spl_meter_needs_a_microphone() {
    let h = start();
    let (mut c, _s) = connect(&h, &[]);
    open_four(&mut c);
    declare(&mut c, vec![row(1, InputKind::Line)], Vec::new()).unwrap();
    let e = c
        .call(Command::MeasCreate {
            config: spl("line", 1),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert_eq!(
        e.msg,
        "input 2 is a line input; an SPL meter needs a microphone"
    );
    // An undeclared input is a microphone, so the meter reads it (in dBFS until calibrated).
    assert!(matches!(
        c.ok(Command::MeasCreate {
            config: spl("mic", 0)
        }),
        ReplyBody::Measurement(_)
    ));
    // So does a sensitivity calibration on a line input: there is no capsule to calibrate.
    let e = c
        .call(Command::CalSpl {
            input: 1,
            mic: "M30".into(),
            calibrator_level: ac2_proto::units::DbSpl(94.0),
            calibrator_freq: ac2_proto::units::Hz(1000.0),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert_eq!(
        e.msg,
        "input 2 is a line input; a sensitivity calibration needs a microphone"
    );
}

/// `cmd` once the tone the generator just started is there and steady (about 5 s).
fn steady(c: &mut Client, cmd: Command) -> Result<ReplyBody, ac2_proto::ProtoError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        match c.call(cmd.clone()) {
            Err(e) if e.msg.contains("not steady") || e.msg.contains("no tone") => {}
            other => return other,
        }
        assert!(std::time::Instant::now() < deadline, "level never steady");
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[test]
fn a_line_calibration_wants_a_line_input_with_a_tone_on_it() {
    let h = start();
    let (mut c, _s) = connect(&h, &[]);
    open_four(&mut c);
    declare(
        &mut c,
        vec![row(1, InputKind::Line), row(2, InputKind::Digital)],
        Vec::new(),
    )
    .unwrap();
    let line = |channel| Command::CalLine {
        channel,
        volts: Volts(1.2277),
    };
    // A digital feed has no voltage: 0 dBFS is its full scale.
    let e = c.call(line(2)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(
        e.msg.starts_with("input 3 is a digital input;"),
        "{}",
        e.msg
    );
    // Nor has an undeclared (microphone) input.
    let e = c.call(line(0)).unwrap_err();
    assert!(e.msg.starts_with("input 1 is a microphone;"), "{}", e.msg);
    // Silence on the line input: the daemon measures the level itself and finds no tone.
    let e = c.call(line(1)).unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    assert!(e.msg.starts_with("no tone signal on input 2"), "{}", e.msg);
}

#[test]
fn slow_a_line_calibration_measures_the_tone_and_lands_in_the_list() {
    let h = start();
    let (mut c, _s) = connect(&h, &[]);
    open_four(&mut c);
    declare(&mut c, vec![row(1, InputKind::Line)], Vec::new()).unwrap();
    // A 1 kHz tone out of output 0 reaches input 1 through the fake rig's acoustic path,
    // ACOUSTIC_GAIN down: that is the level the daemon measures.
    let level = -12.0;
    let measured = level + 20.0 * f64::from(ACOUSTIC_GAIN).log10();
    let ReplyBody::Lease(lease) = c.ok(Command::GenAcquire { force: false }) else {
        panic!("lease");
    };
    c.ok(Command::GenSet {
        lease_token: lease.lease_token,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(1000.0) },
                level: Dbfs(level),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    // Millivolts typed as volts put full scale far outside any line input's range.
    let e = steady(
        &mut c,
        Command::CalLine {
            channel: 1,
            volts: Volts(0.0012277),
        },
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(e.msg.contains("dBu"), "{}", e.msg);
    // A +4 dBu tone (1.2277 V) measured at −18.02 dBFS: full scale is +22.02 dBu, 9.775 V.
    let v_fs = 1.2277 / 10f64.powf(measured / 20.0);
    let dbu_fs = 20.0 * (v_fs / 0.774_597).log10();
    let ReplyBody::LineCalibration(entry) = steady(
        &mut c,
        Command::CalLine {
            channel: 1,
            volts: Volts(1.2277),
        },
    )
    .unwrap() else {
        panic!("line calibration");
    };
    assert_eq!(entry.key.channel, 1);
    assert_eq!(entry.key.device, fake_dev());
    assert_eq!(entry.line.volts, Volts(1.2277));
    assert!(
        (entry.line.measured.0 - measured).abs() < 0.1,
        "{} vs {measured}",
        entry.line.measured.0
    );
    assert!(
        (entry.line.v_fs.0 - v_fs).abs() < 0.1,
        "{} vs {v_fs}",
        entry.line.v_fs.0
    );
    let ReplyBody::Calibrations {
        line_calibrations, ..
    } = c.ok(Command::CalList)
    else {
        panic!("list");
    };
    assert_eq!(line_calibrations, vec![entry.clone()]);
    // The state carries it, and a meter on the input would read dBu through it.
    let ReplyBody::Snapshot(snap) = c.ok(Command::StateSnapshot) else {
        panic!("state");
    };
    let st = snap.state;
    assert_eq!(st.line_calibrations, vec![entry.clone()]);
    match ac2_proto::cal::state_input_use(&st, 1).sensitivity.status() {
        CalStatus::Line { dbu_fs: d, .. } => {
            assert!((d.0 - dbu_fs).abs() < 0.1, "{} vs {dbu_fs}", d.0)
        }
        other => panic!("{other:?}"),
    }
    // Deleted by its own key; the input stays declared.
    c.ok(Command::CalDelete {
        key: CalRef::Line(entry.key.clone()),
    });
    let e = c
        .call(Command::CalDelete {
            key: CalRef::Line(entry.key),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let ReplyBody::Snapshot(snap) = c.ok(Command::StateSnapshot) else {
        panic!("state");
    };
    let st = snap.state;
    assert!(st.line_calibrations.is_empty());
    assert_eq!(
        st.inputs.iter().find(|r| r.channel == 1).map(|r| &r.kind),
        Some(&InputKind::Line)
    );
}

#[test]
fn preview_frames_carry_the_declared_kinds_and_pairs() {
    let h = start();
    let (mut c, s) = connect(&h, &[&Subscription::InputMeters.prefix()]);
    let inputs = vec![
        PreviewInput {
            channel: 0,
            kind: InputKind::Line,
        },
        PreviewInput {
            channel: 1,
            kind: InputKind::Line,
        },
        PreviewInput {
            channel: 2,
            kind: InputKind::Digital,
        },
    ];
    let pairs = vec![Pair {
        left: 0,
        right: 1,
        name: Some("Main".into()),
    }];
    // The pair rules hold for a preview too, against the device's channel count.
    let e = c
        .call(Command::SessionPreview {
            backend: BackendKind::Fake,
            device: fake_dev(),
            inputs: inputs.clone(),
            pairs: vec![pair(1, 2)],
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(
        e.msg.starts_with("inputs 2 and 3 are of different kinds"),
        "{}",
        e.msg
    );
    let e = c
        .call(Command::SessionPreview {
            backend: BackendKind::Fake,
            device: fake_dev(),
            inputs: inputs.clone(),
            pairs: vec![pair(0, 9)],
        })
        .unwrap_err();
    assert_eq!(
        e.msg,
        "input 10 is not captured by the session, so it cannot be paired"
    );
    c.ok(Command::SessionPreview {
        backend: BackendKind::Fake,
        device: fake_dev(),
        inputs: inputs.clone(),
        pairs: pairs.clone(),
    });
    let f = s
        .frame(T, |f| matches!(f.data, FrameData::PreviewLevels(_)))
        .expect("preview frame");
    let FrameData::PreviewLevels(p) = f.data else {
        unreachable!()
    };
    assert_eq!(p.meta.channels, vec![0, 1, 2, 3]);
    assert_eq!(p.meta.inputs, inputs);
    assert_eq!(p.meta.pairs, pairs);
    c.ok(Command::SessionPreviewStop);
}
