//! A remote input's missing samples in the SPL meter: lost time, never silence.

use std::sync::{Arc, Mutex};

use ac2_audio::BlockFlags;
use ac2_proto::model::{LeqConfig, PeakWeighting, SplConfig, TimeWeighting, Weighting};
use ac2_proto::units::{MeasId, Rev, SessionEpoch};

use super::{LeqSetup, Spl};
use crate::calstore::InputCal;
use crate::fanout::Block;
use crate::jobs::{Analysis, JobCtx};
use crate::leq_log::{self, LeqLog};

const FS: u32 = 48_000;
const BLOCK: u32 = 480;

fn spl(log: &leq_log::SharedLog) -> Spl {
    let (to_control, _) = std::sync::mpsc::channel();
    Spl::new(
        JobCtx {
            meas: MeasId(1),
            cfg: SplConfig {
                cue: None,
                bands: None,
                input: 0,
                weighting: Weighting::Z,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::Z,
                leq: LeqConfig::default_windows(),
                position: None,
            },
            config_rev: Rev(1),
            epoch: SessionEpoch(1),
        },
        FS,
        0,
        InputCal::none(),
        LeqSetup {
            log: Arc::clone(log),
            to_control,
            judgements: Vec::new(),
            peak_judgements: [ac2_proto::model::LeqJudgement::NoLimit; 2],
            local: crate::config::LocalClock::Host,
        },
    )
    .expect("spl")
}

/// A 997 Hz sine at `amp` from sample `start`; the second half of every other block never
/// arrived (zeros, marked missing).
fn block(start: u64, amp: f32, gap: bool) -> Block {
    let mut data: Vec<f32> = (0..BLOCK)
        .map(|i| {
            let t = (start + u64::from(i)) as f64 / f64::from(FS);
            amp * (2.0 * std::f64::consts::PI * 997.0 * t).sin() as f32
        })
        .collect();
    let mut b = Block::new(start, BLOCK, 1, BlockFlags::NONE, 0, Vec::new());
    if gap {
        for f in BLOCK / 2..BLOCK {
            data[f as usize] = 0.0;
            b.marks.gap(0, f);
        }
    }
    b.data = data;
    b.compute_stats();
    b
}

#[test]
fn missing_remote_samples_are_lost_time_not_silence() {
    let log: leq_log::SharedLog = Arc::new(Mutex::new(LeqLog::default()));
    let mut s = spl(&log);
    let blocks = 3 * FS / BLOCK;
    for n in 0..blocks {
        let start = u64::from(n * BLOCK);
        let mut b = block(start, 0.1, n % 2 == 1);
        b.wall_ns = 1_000_000_000 + start * 1_000_000_000 / u64::from(FS);
        s.push(&b);
    }
    let rows = leq_log::lock(&log).rows();
    assert!(rows.len() >= 2, "{rows:?}");
    for r in &rows[1..] {
        // A quarter of each second went missing; what was measured reads the sine's level.
        assert!((r.measured.0 - 0.75).abs() < 0.01, "{r:?}");
        assert!((r.lzeq.0 + 20.0).abs() < 0.2, "{r:?}");
    }
}
