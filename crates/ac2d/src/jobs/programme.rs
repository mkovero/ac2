//! Programme meters of the inputs a stream captures: the ballistic readings of
//! `rig.levels`, the true peak, the max holds and the over counts per input, and the
//! correlation and goniometer points per declared pair. They run beside the peak/RMS
//! meters, on the thread that already holds the blocks (the fan-out for a session, the
//! preview's own thread), so they cost no copy of the audio.
//!
//! Every ballistic runs all the time: a change of `rig.levels.ballistic` reads a settled
//! detector at once instead of a meter charging from zero. What the operator chose, and
//! what a reset asks for, reach the meters through a [`SharedPolicy`] the control thread
//! refreshes and the meters read once per frame; the blocks themselves see only cached
//! values, so no lock or allocation sits on the per-block path in steady state.

use std::sync::{Arc, Mutex};

use ac2_core::ppm::{QuasiPeak, QuasiPeakSpec, TruePeak, Vu, dbtp};
use ac2_core::protection::peak_dbfs;
use ac2_core::stereo::{Correlator, GonioPicker, Side};
use ac2_proto::cal;
use ac2_proto::frame::{GONIO_POINTS, GonioFrame, GonioPair, PairLevels, SourceLoudness};
use ac2_proto::model::{
    Ballistic, DeviceId, InputSetup, LevelsSettings, LineCalEntry, Pair, PairSide,
};

use crate::fanout::Block;

const _: () = assert!(ac2_core::stereo::GONIO_POINTS == GONIO_POINTS);

/// Resets kept for consumers that have not seen them yet; a reset is an operator's key
/// press, so a bound this generous is never reached between two frames.
const RESET_LOG: usize = 64;

/// A tone must fall this far below the over threshold before the next over counts: a
/// sustained clip is one over, not one per block.
const OVER_RELEASE_DB: f64 = 1.0;

/// What the control thread tells the meters: the rig's settings, what decides whether an
/// input may show a VU, the session's pairs, and the resets asked for.
#[derive(Debug, Default)]
pub(crate) struct LevelsPolicy {
    pub(crate) settings: LevelsSettings,
    pub(crate) inputs: Vec<InputSetup>,
    pub(crate) line_calibrations: Vec<LineCalEntry>,
    /// The session's declared pairs; the preview keeps its own declaration.
    pub(crate) pairs: Vec<Pair>,
    /// `(seq, channels)` of each reset; empty `channels` = every input.
    resets: Vec<(u64, Vec<u16>)>,
    reset_seq: u64,
}

impl LevelsPolicy {
    /// Asks every consumer to clear the max holds and over counts of `channels`.
    pub(crate) fn reset(&mut self, channels: Vec<u16>) {
        self.reset_seq += 1;
        self.resets.push((self.reset_seq, channels));
        if self.resets.len() > RESET_LOG {
            self.resets.remove(0);
        }
    }
}

pub(crate) type SharedPolicy = Arc<Mutex<LevelsPolicy>>;

/// One input's detectors and holds.
struct Input {
    true_peak: TruePeak,
    type1: QuasiPeak,
    type2: QuasiPeak,
    vu: Vu,
    /// Per ballistic ([`Ballistic::ALL`] order): the reading of the interval (linear).
    interval: [f32; 4],
    /// Per ballistic: the highest reading since the last reset (linear).
    hold: [f32; 4],
    overs: u32,
    /// The true peak fell below the threshold's release since the last over.
    armed: bool,
    vu_offered: bool,
}

impl Input {
    fn new(sample_rate: f64) -> Self {
        Self {
            true_peak: TruePeak::new(),
            type1: QuasiPeak::new(sample_rate, &QuasiPeakSpec::TYPE1),
            type2: QuasiPeak::new(sample_rate, &QuasiPeakSpec::TYPE2),
            vu: Vu::new(sample_rate),
            interval: [0.0; 4],
            hold: [0.0; 4],
            overs: 0,
            armed: true,
            vu_offered: false,
        }
    }

    fn push(&mut self, x: &[f32], over: f32, release: f32) {
        let tp = self.true_peak.process(x);
        let readings = [
            tp,
            self.type1.process(x),
            self.type2.process(x),
            self.vu.process(x),
        ];
        for (slot, r) in readings.into_iter().enumerate() {
            self.interval[slot] = self.interval[slot].max(r);
            self.hold[slot] = self.hold[slot].max(r);
        }
        if tp > over {
            if self.armed {
                self.overs = self.overs.saturating_add(1);
                self.armed = false;
            }
        } else if tp < release {
            self.armed = true;
        }
    }

    /// Clears the holds and the count. A tone still above the threshold at the reset is the
    /// same over, so the arming follows the release rule, not the reset.
    fn reset(&mut self) {
        self.hold = [0.0; 4];
        self.overs = 0;
    }
}

struct PairState {
    left: u16,
    right: u16,
    li: usize,
    ri: usize,
    corr: Correlator,
    gonio: GonioPicker,
    /// The interval's samples, for the picker; bounded by its capacity.
    l: Vec<f32>,
    r: Vec<f32>,
}

/// Which block channels a [`Programme`] meters, and as which device inputs.
pub(crate) struct Captured {
    /// Block channel index per metered input.
    pub(crate) idx: Vec<usize>,
    /// Device input per metered input.
    pub(crate) channels: Vec<u16>,
    pub(crate) device: DeviceId,
    pub(crate) sample_rate: u32,
}

/// Programme meters of the block channels `idx` carrying device inputs `channels`.
pub(crate) struct Programme {
    idx: Vec<usize>,
    channels: Vec<u16>,
    device: DeviceId,
    sample_rate: f64,
    inputs: Vec<Input>,
    pairs: Vec<PairState>,
    /// The session's pairs follow the policy; a preview's are declared once.
    follow_pairs: bool,
    policy: SharedPolicy,
    settings: LevelsSettings,
    /// Linear true peak of an over, and of its release.
    over: f32,
    release: f32,
    resets_seen: u64,
    scratch: Vec<f32>,
    /// The pair's right channel while `scratch` holds the left.
    scratch_r: Vec<f32>,
    seed: u64,
}

impl Programme {
    /// Meters of the inputs `c` captures; `pairs` as declared when `follow_pairs` is off,
    /// else refreshed from the policy at every frame. `seed` decorrelates the goniometers'
    /// sampling positions.
    pub(crate) fn new(
        c: Captured,
        pairs: &[Pair],
        follow_pairs: bool,
        policy: SharedPolicy,
        seed: u64,
    ) -> Self {
        let (settings, resets_seen, policy_pairs) = {
            let p = policy.lock().unwrap_or_else(|e| e.into_inner());
            (p.settings, p.reset_seq, p.pairs.clone())
        };
        let fs = f64::from(c.sample_rate);
        let mut me = Self {
            inputs: c.channels.iter().map(|_| Input::new(fs)).collect(),
            idx: c.idx,
            channels: c.channels,
            device: c.device,
            sample_rate: fs,
            pairs: Vec::new(),
            follow_pairs,
            policy,
            settings,
            over: 0.0,
            release: 0.0,
            resets_seen,
            scratch: Vec::new(),
            scratch_r: Vec::new(),
            seed,
        };
        me.set_thresholds();
        let declared = if follow_pairs { &policy_pairs } else { pairs };
        me.set_pairs(declared);
        me
    }

    fn set_thresholds(&mut self) {
        let t = self.settings.over_threshold.0;
        self.over = 10f64.powf(t / 20.0) as f32;
        self.release = 10f64.powf((t - OVER_RELEASE_DB) / 20.0) as f32;
    }

    /// Keeps the pairs whose both inputs are captured, in `pairs` order.
    fn set_pairs(&mut self, pairs: &[Pair]) {
        let window = f64::from(self.settings.correlation_window.seconds());
        // The picker wants the interval's samples; one second holds thirty intervals.
        let cap = self.sample_rate as usize;
        self.pairs.clear();
        for p in pairs {
            let side = |ch: u16| self.channels.iter().position(|&c| c == ch);
            let (Some(li), Some(ri)) = (side(p.left), side(p.right)) else {
                continue;
            };
            self.seed = self
                .seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            self.pairs.push(PairState {
                left: p.left,
                right: p.right,
                li: self.idx[li],
                ri: self.idx[ri],
                corr: Correlator::new(self.sample_rate, window),
                gonio: GonioPicker::new(self.sample_rate, self.seed),
                l: Vec::with_capacity(cap),
                r: Vec::with_capacity(cap),
            });
        }
    }

    fn pairs_match(&self, pairs: &[Pair]) -> bool {
        let mut mine = self.pairs.iter();
        for p in pairs {
            let captured = self.channels.contains(&p.left) && self.channels.contains(&p.right);
            if !captured {
                continue;
            }
            match mine.next() {
                Some(s) if s.left == p.left && s.right == p.right => {}
                _ => return false,
            }
        }
        mine.next().is_none()
    }

    pub(crate) fn push(&mut self, b: &Block) {
        let (over, release) = (self.over, self.release);
        for (m, &ch) in self.idx.iter().enumerate() {
            b.channel_into(ch, &mut self.scratch);
            self.inputs[m].push(&self.scratch, over, release);
        }
        for p in &mut self.pairs {
            b.channel_into(p.li, &mut self.scratch);
            b.channel_into(p.ri, &mut self.scratch_r);
            // The correlator sees every sample; only the picker's interval buffer is bounded.
            p.corr.process(&self.scratch, &self.scratch_r);
            let room = p.l.capacity() - p.l.len();
            let take = self.scratch.len().min(room);
            p.l.extend_from_slice(&self.scratch[..take]);
            p.r.extend_from_slice(&self.scratch_r[..take]);
        }
    }

    /// Reads the policy: settings, VU offers, the session's pairs and the resets since the
    /// last frame.
    fn sync(&mut self) {
        let mut pairs_changed = false;
        {
            let p = self.policy.lock().unwrap_or_else(|e| e.into_inner());
            let window_changed = p.settings.correlation_window != self.settings.correlation_window;
            self.settings = p.settings;
            if window_changed {
                let w = f64::from(self.settings.correlation_window.seconds());
                for s in &mut self.pairs {
                    s.corr.set_window(w);
                }
            }
            for (i, &ch) in self.channels.iter().enumerate() {
                self.inputs[i].vu_offered =
                    cal::offers_vu(&p.inputs, &p.line_calibrations, &self.device, ch);
            }
            for (seq, channels) in &p.resets {
                if *seq <= self.resets_seen {
                    continue;
                }
                for (i, &ch) in self.channels.iter().enumerate() {
                    if channels.is_empty() || channels.contains(&ch) {
                        self.inputs[i].reset();
                    }
                }
            }
            self.resets_seen = p.reset_seq;
            if self.follow_pairs && !self.pairs_match(&p.pairs) {
                pairs_changed = true;
            }
        }
        self.set_thresholds();
        if pairs_changed {
            let pairs = {
                let p = self.policy.lock().unwrap_or_else(|e| e.into_inner());
                p.pairs.clone()
            };
            self.set_pairs(&pairs);
        }
    }

    /// The interval's readings, then a new interval. `points`: the goniometer frame too
    /// (someone receives it).
    pub(crate) fn take(&mut self, points: bool) -> Readings {
        self.sync();
        let chosen = self.settings.ballistic;
        let gate = self.settings.correlation_gate.0;
        let mut out = Readings {
            ballistic: chosen,
            ppm: Vec::with_capacity(self.inputs.len()),
            true_peak: Vec::with_capacity(self.inputs.len()),
            max: Vec::with_capacity(self.inputs.len()),
            overs: Vec::with_capacity(self.inputs.len()),
            pair_levels: Vec::with_capacity(self.pairs.len()),
            loudness: self.loudness(),
            gonio: points.then(|| GonioFrame {
                pairs: Vec::with_capacity(self.pairs.len()),
            }),
        };
        for i in &mut self.inputs {
            let b = if chosen == Ballistic::Vu && !i.vu_offered {
                Ballistic::TruePeak
            } else {
                chosen
            };
            let k = slot(b);
            out.ppm.push(peak_dbfs(f64::from(i.interval[k])) as f32);
            out.true_peak
                .push(dbtp(i.interval[slot(Ballistic::TruePeak)]));
            out.max.push(peak_dbfs(f64::from(i.hold[k])) as f32);
            out.overs.push(i.overs);
            i.interval = [0.0; 4];
        }
        for p in &mut self.pairs {
            let c = p.corr.reading(gate);
            out.pair_levels.push(PairLevels {
                left: p.left,
                right: p.right,
                correlation: c.r,
                quiet: c.quiet.map(|s| match s {
                    Side::Left => PairSide::Left,
                    Side::Right => PairSide::Right,
                }),
            });
            if let Some(g) = &mut out.gonio {
                let mut pts = [[0i8; 2]; GONIO_POINTS];
                let gain_dbfs = p.gonio.pick(&p.l, &p.r, &mut pts);
                g.pairs.push(GonioPair {
                    left: p.left,
                    right: p.right,
                    gain_dbfs,
                    points: pts.to_vec(),
                });
            }
            p.l.clear();
            p.r.clear();
        }
        out
    }

    /// The loudness of every source this interval (docs/design/programme-meter.md §6.1):
    /// pairs first in `pair_levels` order, then unpaired line and digital inputs ascending
    /// by channel. The BS.1770 meters are not wired in yet, so there are no sources.
    fn loudness(&mut self) -> Vec<SourceLoudness> {
        Vec::new()
    }
}

fn slot(b: Ballistic) -> usize {
    Ballistic::ALL
        .iter()
        .position(|&x| x == b)
        .unwrap_or_default()
}

/// One interval's programme readings.
pub(crate) struct Readings {
    pub(crate) ballistic: Ballistic,
    pub(crate) ppm: Vec<f32>,
    pub(crate) true_peak: Vec<f32>,
    pub(crate) max: Vec<f32>,
    pub(crate) overs: Vec<u32>,
    pub(crate) pair_levels: Vec<PairLevels>,
    pub(crate) loudness: Vec<SourceLoudness>,
    pub(crate) gonio: Option<GonioFrame>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_audio::BlockFlags;
    use ac2_proto::model::CorrelationWindow;
    use ac2_proto::units::{Dbfs, Dbtp};

    fn policy(settings: LevelsSettings, pairs: Vec<Pair>) -> SharedPolicy {
        Arc::new(Mutex::new(LevelsPolicy {
            settings,
            pairs,
            ..LevelsPolicy::default()
        }))
    }

    /// Two channels: a 997 Hz sine at `amp` on both (mono) when `mono`, else the right
    /// silent.
    fn block(start: u64, frames: u32, amp: f32, mono: bool) -> Block {
        let fs = 48_000.0;
        let mut data = Vec::with_capacity(frames as usize * 2);
        for i in 0..frames {
            let t = (start + u64::from(i)) as f64 / fs;
            let v = amp * (2.0 * std::f64::consts::PI * 997.0 * t).sin() as f32;
            data.push(v);
            data.push(if mono { v } else { 0.0 });
        }
        Block::new(start, frames, 2, BlockFlags::NONE, 0, data)
    }

    fn programme(settings: LevelsSettings) -> (Programme, SharedPolicy) {
        let pairs = vec![Pair {
            left: 0,
            right: 1,
            name: None,
        }];
        let p = policy(settings, pairs.clone());
        let m = Programme::new(
            Captured {
                idx: vec![0, 1],
                channels: vec![0, 1],
                device: DeviceId("fake".into()),
                sample_rate: 48_000,
            },
            &pairs,
            true,
            Arc::clone(&p),
            7,
        );
        (m, p)
    }

    /// A sustained tone above the threshold is one over, counted once; the max holds it.
    #[test]
    fn a_sustained_over_counts_once_and_a_reset_clears_it() {
        let settings = LevelsSettings {
            over_threshold: Dbtp(-30.0),
            ..LevelsSettings::default()
        };
        let (mut m, p) = programme(settings);
        let mut start = 0;
        for _ in 0..60 {
            m.push(&block(start, 512, 0.1, true));
            start += 512;
        }
        let r = m.take(true);
        assert_eq!(r.overs, [1, 1]);
        assert!((r.true_peak[0] + 20.0).abs() < 0.2, "{:?}", r.true_peak);
        assert!((r.max[0] + 20.0).abs() < 0.2, "{:?}", r.max);
        assert_eq!(r.ballistic, Ballistic::TruePeak);
        // Mono: full correlation, a vertical figure.
        assert_eq!(r.pair_levels.len(), 1);
        assert!(r.pair_levels[0].correlation.expect("mono correlates") > 0.99);
        let g = r.gonio.expect("points asked for");
        assert_eq!(g.pairs[0].points.len(), GONIO_POINTS);
        assert!(g.pairs[0].points.iter().all(|p| p[0] == 0));
        assert!(g.pairs[0].points.iter().any(|p| p[1].abs() > 100));
        // Silence: no new over, the holds stay. The first silent interval still reads the
        // interpolator's tail of the tone; the second is quiet.
        for _ in 0..30 {
            m.push(&block(start, 512, 0.0, true));
            start += 512;
        }
        m.take(false);
        for _ in 0..30 {
            m.push(&block(start, 512, 0.0, true));
            start += 512;
        }
        let r = m.take(false);
        assert_eq!(r.overs, [1, 1]);
        assert!((r.max[0] + 20.0).abs() < 0.2);
        assert_eq!(r.ppm[0], f32::NEG_INFINITY);
        p.lock().expect("policy").reset(vec![0]);
        let r = m.take(false);
        assert_eq!(r.overs, [0, 1]);
        assert_eq!(r.max[0], f32::NEG_INFINITY);
        // Back above: a second over, after the release.
        m.push(&block(start, 512, 0.1, true));
        let r = m.take(false);
        assert_eq!(r.overs, [1, 2]);
    }

    /// A silent right side reads quiet, never a correlation.
    #[test]
    fn a_quiet_side_is_named_and_the_window_follows_the_rig() {
        let (mut m, p) = programme(LevelsSettings::default());
        let mut start = 0;
        for _ in 0..30 {
            m.push(&block(start, 512, 0.1, false));
            start += 512;
        }
        let r = m.take(false);
        assert_eq!(r.pair_levels[0].quiet, Some(PairSide::Right));
        assert_eq!(r.pair_levels[0].correlation, None);
        {
            let mut g = p.lock().expect("policy");
            g.settings.correlation_window = CorrelationWindow::Short;
            g.settings.ballistic = Ballistic::Vu;
            g.settings.correlation_gate = Dbfs(-200.0);
        }
        m.push(&block(start, 512, 0.1, false));
        let r = m.take(false);
        // VU chosen but not offered on an input without a line calibration: true peak.
        assert_eq!(r.ballistic, Ballistic::Vu);
        assert!((r.ppm[0] + 20.0).abs() < 0.2, "{:?}", r.ppm);
        // Silence is below any gate.
        assert_eq!(r.pair_levels[0].quiet, Some(PairSide::Right));
        assert_eq!(m.settings.correlation_window, CorrelationWindow::Short);
    }
}
