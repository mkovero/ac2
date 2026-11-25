//! A transfer at the `bins` resolution sends each analysis stage's own bins on
//! `bins/<meas>/<stage>`: formed only while subscribed, stamped as the `tf` of the same emit,
//! equal to the `tf` column holding a bin alone, and sent once more at a stop.
#![allow(clippy::unwrap_used)]

use crate::common;
use crate::transfer::tf;

use std::collections::HashMap;
use std::time::Duration;

use ac2_audio::{FakeBackend, FakeDriver};
use ac2_proto::frame::{BinLayout, BinsFrame, Frame, FrameData};
use ac2_proto::model::{
    GeneratorDesired, GeneratorSettings, LfWindow, MeasKind, OwnedTraces, Resolution, Signal,
    TransferResolution,
};
use ac2_proto::units::{Dbfs, MeasId, Seconds};
use ac2_proto::{Command, ReplyBody, Subscription, Topic};
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(20);

pub(crate) fn stage(meas: u32, stage: u8) -> Topic {
    Topic::Bins {
        meas: MeasId(meas),
        stage,
    }
}

pub(crate) fn tf_topic(meas: u32) -> Topic {
    Topic::Data {
        meas: MeasId(meas),
        stream: ac2_proto::Stream::Tf,
    }
}

pub(crate) fn bins(f: &Frame) -> &BinsFrame {
    match &f.data {
        FrameData::Bins(b) => b,
        other => panic!("{other:?}"),
    }
}

struct Run {
    h: Handle,
    c: Client,
    sub: Sub,
    d: FakeDriver,
}

/// A stream on the fake rig with transfer 1 at `resolution`, started; pink noise playing
/// when `play`.
fn start(
    name: &str,
    resolution: TransferResolution,
    lf_window: LfWindow,
    play: bool,
    prefixes: &[&[u8]],
) -> (Run, FakeBackend) {
    init_log();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc(name));
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, prefixes);
    c.ok(Command::StreamOpen {
        config: stream(true),
    });
    let mut m = transfer("main");
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.resolution = resolution;
        config.lf_window = lf_window;
    }
    c.ok(Command::MeasCreate { config: m });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    if play {
        let tok = c.acquire();
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
    }
    let d = driver(&backend);
    (Run { h, c, sub, d }, backend)
}

/// The frames of `topics` covering the audio up to `end`, at config rev `rev` or later: the
/// newest emit's, one per topic.
pub(crate) fn covering(sub: &Sub, end: u64, rev: u64, topics: &[Topic]) -> HashMap<Topic, Frame> {
    let mut got = HashMap::new();
    while got.len() < topics.len() {
        let f = sub
            .frame(T, |f| {
                topics.contains(&f.topic())
                    && f.stamp.audio_sample.0 + 1 >= end
                    && f.stamp.config_rev.0 >= rev
            })
            .unwrap_or_else(|| panic!("frames up to {end}: have {:?}", got.keys()));
        got.insert(f.topic(), f);
    }
    got
}

/// A stage frame carries the stamp of the `tf` formed beside it, its grid aside.
pub(crate) fn same_emit(s: &Frame, tf: &Frame) {
    assert_eq!(s.stamp.audio_sample, tf.stamp.audio_sample, "{}", s.topic());
    assert_eq!(s.stamp.config_rev, tf.stamp.config_rev);
    assert_eq!(s.stamp.config_applied_at, tf.stamp.config_applied_at);
    assert_eq!(s.stamp.stream_epoch, tf.stamp.stream_epoch);
    assert_eq!(s.stamp.protection, tf.stamp.protection);
    assert_eq!(s.stamp.grid_id, None);
}

pub(crate) fn layout(f: &Frame) -> BinLayout {
    *tf(f).meta.bins.clone().expect("a layout at bins")
}

/// The 1/96 column nearest a frequency (the grid runs from 1 kHz · 2^(−672/96)).
pub(crate) fn column(hz: f64) -> usize {
    ((96.0 * (hz / 1000.0).log2()).round() as i64 + 672) as usize
}

/// Bin `bin` of a stage frame equals the 1/96 column holding it alone (columns there are
/// narrower than the bins), within the two frames' quantisation.
fn bin_equals_its_column(s: &Frame, tf_frame: &Frame, bin: u32) {
    let b = bins(s);
    let t = tf(tf_frame);
    let i = (bin - b.meta.first_bin) as usize;
    let hz = f64::from(bin) * b.meta.bin_hz.0;
    let c = column(hz);
    assert_eq!(
        t.validity[c],
        ac2_proto::frame::ValidityMask::NONE,
        "{hz} Hz"
    );
    let what = format!("stage {} bin {bin} ({hz:.2} Hz)", b.meta.stage);
    assert!(
        (b.mag[i] - t.mag[c]).abs() <= 0.011,
        "{what}: {} {}",
        b.mag[i],
        t.mag[c]
    );
    let dp = (b.phase[i] - t.phase[c]).rem_euclid(360.0);
    assert!(
        dp.min(360.0 - dp) <= 0.011,
        "{what}: {} {}",
        b.phase[i],
        t.phase[c]
    );
    assert!(
        (b.coh[i] - t.coh[c]).abs() <= 2e-4,
        "{what}: {} {}",
        b.coh[i],
        t.coh[c]
    );
}

/// Every stage's frame matches the layout its `tf` states.
fn matches_layout(s: &Frame, l: &BinLayout, tf_frame: &Frame) {
    let b = bins(s);
    let st = l.stages[usize::from(b.meta.stage)];
    assert_eq!(
        (b.meta.first_bin, b.mag.len() as u32),
        (st.first_bin, st.bins)
    );
    assert_eq!(b.meta.bin_hz, st.bin_hz);
    assert!(b.meta.blocks > 0 && b.meta.eff_avg > 0.0, "{:?}", b.meta);
    assert_eq!(b.meta.delay, tf(tf_frame).meta.delay);
    assert_eq!(b.meta.mic_curve, tf(tf_frame).meta.mic_curve);
    assert!(!b.meta.mic_curve);
}

#[test]
fn stages_arrive_while_subscribed_stamped_as_their_tf() {
    let (mut r, _b) = start(
        "bins-std",
        TransferResolution::Bins,
        LfWindow::Standard,
        true,
        &[b"d/1/tf", b"bins/1/0", b"bins/1/1"],
    );
    run(&mut r.d, 2.0);
    let end = end_sample(&r.d);
    let topics = [tf_topic(1), stage(1, 0), stage(1, 1)];
    let got = covering(&r.sub, end, 0, &topics);
    let t = &got[&tf_topic(1)];
    let l = layout(t);
    let counts: Vec<_> = l.stages.iter().map(|s| (s.first_bin, s.bins)).collect();
    assert_eq!(counts, [(88, 1756), (88, 189), (1, 207)]);
    assert_eq!(l.blends.len(), 2);
    for (s, bin) in [(0, 100), (1, 110)] {
        let f = &got[&stage(1, s)];
        same_emit(f, t);
        matches_layout(f, &l, t);
        assert_eq!(bins(f).meta.hop.0, [2048, 4096][usize::from(s)]);
        bin_equals_its_column(f, t, bin);
    }

    // Stage 2 was never formed while unsubscribed: its first frame is its topic's first.
    // The subscription lands asynchronously: audio keeps running until an emit sees it.
    r.sub.subscribe(Subscription::Topic(stage(1, 2)));
    let first = (0..100)
        .find_map(|_| {
            run(&mut r.d, 0.1);
            r.sub
                .frame(Duration::from_millis(100), |f| f.topic() == stage(1, 2))
        })
        .expect("stage 2");
    assert_eq!(first.stamp.seq, 1);
    run(&mut r.d, 0.3);
    let end = end_sample(&r.d);
    let got = covering(&r.sub, end, 0, &[tf_topic(1), stage(1, 2)]);
    let (f, t) = (&got[&stage(1, 2)], &got[&tf_topic(1)]);
    same_emit(f, t);
    matches_layout(f, &l, t);
    bin_equals_its_column(f, t, 100);
    r.h.shutdown();
}

/// The long window adds a fourth stage, a quarter of the third's bin width.
#[test]
fn the_long_window_ladder_has_four_stages() {
    let (mut r, _b) = start(
        "bins-long",
        TransferResolution::Bins,
        LfWindow::Long,
        true,
        &[b"d/1/tf", b"bins/1/3"],
    );
    run(&mut r.d, 7.0);
    let end = end_sample(&r.d);
    let got = covering(&r.sub, end, 0, &[tf_topic(1), stage(1, 3)]);
    let t = &got[&tf_topic(1)];
    let l = layout(t);
    let counts: Vec<_> = l.stages.iter().map(|s| (s.first_bin, s.bins)).collect();
    assert_eq!(counts, [(88, 1756), (88, 189), (88, 120), (1, 276)]);
    let f = &got[&stage(1, 3)];
    same_emit(f, t);
    matches_layout(f, &l, t);
    assert_eq!(bins(f).meta.hop.0, 12_288);
    assert!((bins(f).meta.bin_hz.0 - 48_000.0 / 196_608.0).abs() < 1e-12);
    bin_equals_its_column(f, t, 100);
    r.h.shutdown();
}

/// A delay 300 samples off is more than the full-rate stage's held blocks keep (113), less
/// than the deeper stages' (452, 1356): only stage 0 starts over and says so.
#[test]
fn a_partial_delay_restart_empties_only_the_reset_stages() {
    let (mut r, _b) = start(
        "bins-delay",
        TransferResolution::Bins,
        LfWindow::Standard,
        true,
        &[b"d/1/tf", b"bins/1/0", b"bins/1/1", b"bins/1/2"],
    );
    run(&mut r.d, 2.0);
    let rev = match r.c.ok(Command::DelaySet {
        meas: MeasId(1),
        delay: Seconds(300.0 / f64::from(FS)),
    }) {
        ReplyBody::Delay(d) => d.rev.0,
        other => panic!("{other:?}"),
    };
    // Less than one full-rate hop: no block after the splice yet.
    run(&mut r.d, 0.02);
    let end = end_sample(&r.d);
    let topics = [tf_topic(1), stage(1, 0), stage(1, 1), stage(1, 2)];
    let got = covering(&r.sub, end, rev, &topics);
    let t = &got[&tf_topic(1)];
    let blocks: Vec<u64> = (0..3)
        .map(|s| bins(&got[&stage(1, s)]).meta.blocks)
        .collect();
    assert_eq!(blocks[0], 0, "{blocks:?}");
    assert!(blocks[1] > 0 && blocks[2] > 0, "{blocks:?}");
    let s0 = bins(&got[&stage(1, 0)]);
    assert!(s0.mag.iter().all(|v| v.is_nan()) && s0.meta.eff_avg.is_nan());
    for s in 0..3 {
        let f = &got[&stage(1, s)];
        same_emit(f, t);
        assert_eq!(bins(f).meta.delay, tf(t).meta.delay);
        assert!((tf(t).meta.delay.0 - 300.0 / f64::from(FS)).abs() < 1e-12);
    }
    r.h.shutdown();
}

/// Without a reference every block is refused: a stage holds nothing, yet its frames keep
/// the `tf`'s advancing stamp, so a client does not take the stream for stopped.
#[test]
fn stages_keep_the_stamp_while_blocks_are_refused() {
    let (mut r, _b) = start(
        "bins-silent",
        TransferResolution::Bins,
        LfWindow::Standard,
        false,
        &[b"d/1/tf", b"bins/1/0"],
    );
    let mut last = 0;
    for _ in 0..2 {
        run(&mut r.d, 0.5);
        let end = end_sample(&r.d);
        let got = covering(&r.sub, end, 0, &[tf_topic(1), stage(1, 0)]);
        let f = &got[&stage(1, 0)];
        same_emit(f, &got[&tf_topic(1)]);
        let b = bins(f);
        assert_eq!(b.meta.blocks, 0);
        assert!(b.mag.iter().chain(&b.coh).all(|v| v.is_nan()));
        assert!(f.stamp.audio_sample.0 > last);
        last = f.stamp.audio_sample.0;
    }
    r.h.shutdown();
}

/// At 1/96 a transfer states no stages and forms none: switched to bins, a stage's first
/// frame is its topic's first. An SPL meter forms none either.
#[test]
fn only_a_transfer_at_bins_forms_stages() {
    let (mut r, _b) = start(
        "bins-none",
        Resolution::NinetySixth.into(),
        LfWindow::Standard,
        true,
        &[b"d/1/tf", b"bins/1/0", b"d/2/spl", b"bins/2/0"],
    );
    r.c.ok(Command::MeasCreate {
        config: spl("meter", 1),
    });
    r.c.ok(Command::MeasStart { meas: MeasId(2) });
    run(&mut r.d, 1.0);
    let end = end_sample(&r.d);
    let spl_topic = Topic::Data {
        meas: MeasId(2),
        stream: ac2_proto::Stream::Spl,
    };
    let mut seen = Vec::new();
    let mut have = (false, false);
    while !(have.0 && have.1) {
        let f = r
            .sub
            .frame(T, |f| f.stamp.audio_sample.0 + 1 >= end)
            .expect("frames");
        seen.push(f.topic());
        have.0 |= f.topic() == tf_topic(1);
        have.1 |= f.topic() == spl_topic;
        if f.topic() == tf_topic(1) {
            assert_eq!(tf(&f).meta.bins, None);
        }
    }
    assert!(
        !seen.iter().any(|t| matches!(t, Topic::Bins { .. })),
        "{seen:?}"
    );

    let mut m = transfer("main");
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.resolution = TransferResolution::Bins;
    }
    let rev = r.c.config_rev(MeasId(1)).0;
    r.c.update_ok(MeasId(1), m);
    run(&mut r.d, 0.5);
    let first = r
        .sub
        .frame(T, |f| matches!(f.topic(), Topic::Bins { .. }))
        .expect("a stage frame");
    assert_eq!(first.topic(), stage(1, 0));
    assert_eq!(first.stamp.seq, 1);
    assert!(first.stamp.config_rev.0 > rev);
    r.h.shutdown();
}

/// Waits for the keepalive on a fresh subscriber to `prefixes` plus `ka`: what came before it
/// is what the daemon's slots held.
fn slots(r: &Run, prefixes: &[&[u8]]) -> Vec<Frame> {
    let mut p = prefixes.to_vec();
    p.push(b"ka");
    let late = Sub::connect(r.h.context(), r.h.data_endpoint(), &p);
    let mut got = Vec::new();
    loop {
        let f = late.frame(T, |_| true).expect("the keepalive");
        if matches!(f.data, FrameData::Ka(_)) {
            return got;
        }
        got.push(f);
    }
}

const STAGES: [&[u8]; 3] = [b"bins/1/0", b"bins/1/1", b"bins/1/2"];

/// A stop sends every stage once more, subscribed or not, under the final `tf`'s stamp; a
/// stage zoomed into after the stop matches the curve. Deleting the measurement drops them.
#[test]
fn a_stop_leaves_every_stage_with_the_final_curve() {
    let (mut r, _b) = start(
        "bins-final",
        TransferResolution::Bins,
        LfWindow::Standard,
        true,
        &[b"d/1/tf"],
    );
    run(&mut r.d, 2.0);
    let end = end_sample(&r.d);
    covering(&r.sub, end, 0, &[tf_topic(1)]);
    r.c.ok(Command::MeasStop { meas: MeasId(1) });

    let mut p = STAGES.to_vec();
    p.push(b"d/1/tf");
    let got: HashMap<Topic, Frame> = slots(&r, &p).into_iter().map(|f| (f.topic(), f)).collect();
    let t = &got[&tf_topic(1)];
    let l = layout(t);
    assert_eq!(l.stages.len(), 3);
    for s in 0..3 {
        let f = got.get(&stage(1, s)).unwrap_or_else(|| panic!("stage {s}"));
        same_emit(f, t);
        matches_layout(f, &l, t);
    }
    bin_equals_its_column(&got[&stage(1, 1)], t, 110);

    r.c.ok(Command::MeasDelete {
        meas: MeasId(1),
        traces: OwnedTraces::Delete,
    });
    let left = slots(&r, &STAGES);
    assert!(
        left.is_empty(),
        "{:?}",
        left.iter().map(Frame::topic).collect::<Vec<_>>()
    );
    r.h.shutdown();
}

/// A stream's close drops its stage frames with its curves.
#[test]
fn a_stream_close_leaves_no_stage() {
    let (mut r, _b) = start(
        "bins-close",
        TransferResolution::Bins,
        LfWindow::Standard,
        true,
        &[b"d/1/tf", b"bins/1/0"],
    );
    run(&mut r.d, 1.0);
    let end = end_sample(&r.d);
    covering(&r.sub, end, 0, &[stage(1, 0)]);
    assert!(!slots(&r, &STAGES).is_empty());
    r.c.ok(Command::StreamClose);
    let left = slots(&r, &STAGES);
    assert!(
        left.is_empty(),
        "{:?}",
        left.iter().map(Frame::topic).collect::<Vec<_>>()
    );
    r.h.shutdown();
}
