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
//! values, so no lock or allocation sits on the per-block path in steady state. The
//! loudness sources ([`super::loudness`]) ride along on the same blocks and resets.

use std::sync::{Arc, Mutex};

use super::loudness::Loudness;
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
    /// Bumped when `inputs` or `pairs` change: the loudness sources follow both, and a
    /// count is cheaper to compare every frame than the declaration itself.
    declared: u64,
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

    /// Takes the session's declaration; unchanged rows leave the loudness sources alone.
    pub(crate) fn declare(&mut self, inputs: &[InputSetup], pairs: &[Pair]) {
        if self.inputs != inputs {
            self.inputs = inputs.to_vec();
            self.declared += 1;
        }
        if self.pairs != pairs {
            self.pairs = pairs.to_vec();
            self.declared += 1;
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

    /// Meters a block; returns its true peak (linear).
    fn push(&mut self, x: &[f32], over: f32, release: f32) -> f32 {
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
        tp
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
    /// Metered positions of the two sides.
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

/// Whose declaration of kinds and pairs a [`Programme`] meters by.
pub(crate) enum Declared {
    /// The session's, followed through the policy.
    Session,
    /// A preview's own, fixed for its life.
    Fixed {
        inputs: Vec<InputSetup>,
        pairs: Vec<Pair>,
    },
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
    declared_seen: u64,
    /// Each metered input's samples of the block being metered.
    chans: Vec<Vec<f32>>,
    /// Each metered input's true peak of that block (linear).
    block_tp: Vec<f32>,
    /// One past the last sample metered: a block not starting there follows a gap.
    end: Option<u64>,
    loudness: Loudness,
    seed: u64,
}

impl Programme {
    /// Meters of the inputs `c` captures, by `declared` kinds and pairs (the session's are
    /// refreshed from the policy at every frame). `seed` decorrelates the goniometers'
    /// sampling positions. Every loudness source starts afresh.
    pub(crate) fn new(c: Captured, declared: Declared, policy: SharedPolicy, seed: u64) -> Self {
        let follow_pairs = matches!(declared, Declared::Session);
        let (settings, resets_seen, declared_seen, (inputs, pairs)) = {
            let p = policy.lock().unwrap_or_else(|e| e.into_inner());
            let decl = match declared {
                Declared::Session => (p.inputs.clone(), p.pairs.clone()),
                Declared::Fixed { inputs, pairs } => (inputs, pairs),
            };
            (p.settings, p.reset_seq, p.declared, decl)
        };
        let fs = f64::from(c.sample_rate);
        let n = c.channels.len();
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
            declared_seen,
            chans: vec![Vec::new(); n],
            block_tp: vec![0.0; n],
            end: None,
            loudness: Loudness::new(fs),
            seed,
        };
        me.set_thresholds();
        me.set_pairs(&pairs);
        me.loudness
            .declare(&inputs, &pairs, &me.channels, crate::util::wall_ns());
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
                li,
                ri,
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
        // A gap, or a block the host flags as not following the last one: the K-weighting
        // state and the loudness windows must not run across it.
        if self
            .end
            .is_some_and(|e| e != b.start_sample || b.flags.breaks_continuity())
        {
            self.loudness.discontinuity();
        }
        self.end = Some(b.end_sample());
        let (over, release) = (self.over, self.release);
        for (m, &ch) in self.idx.iter().enumerate() {
            b.channel_into(ch, &mut self.chans[m]);
            self.block_tp[m] = self.inputs[m].push(&self.chans[m], over, release);
        }
        for p in &mut self.pairs {
            let (l, r) = (&self.chans[p.li], &self.chans[p.ri]);
            // The correlator sees every sample; only the picker's interval buffer is bounded.
            p.corr.process(l, r);
            let room = p.l.capacity() - p.l.len();
            let take = l.len().min(room);
            p.l.extend_from_slice(&l[..take]);
            p.r.extend_from_slice(&r[..take]);
        }
        self.loudness.push(&self.chans, &self.block_tp);
    }

    /// Reads the policy: settings, VU offers, the session's pairs and the resets since the
    /// last frame.
    fn sync(&mut self) {
        let mut pairs_changed = false;
        let mut redeclare = None;
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
                self.loudness.reset(channels, crate::util::wall_ns());
            }
            self.resets_seen = p.reset_seq;
            if self.follow_pairs && !self.pairs_match(&p.pairs) {
                pairs_changed = true;
            }
            if self.follow_pairs && p.declared != self.declared_seen {
                self.declared_seen = p.declared;
                redeclare = Some((p.inputs.clone(), p.pairs.clone()));
            }
        }
        if let Some((inputs, pairs)) = redeclare {
            self.loudness
                .declare(&inputs, &pairs, &self.channels, crate::util::wall_ns());
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

    /// The loudness of every source (docs/design/programme-meter.md §6.1): pairs first in
    /// `pair_levels` order, then unpaired line and digital inputs ascending by channel.
    fn loudness(&mut self) -> Vec<SourceLoudness> {
        self.loudness.readings()
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
            Declared::Session,
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

    fn line(channel: u16) -> InputSetup {
        InputSetup {
            channel,
            kind: ac2_proto::model::InputKind::Line,
            pair: None,
        }
    }

    /// A source that stays in the set keeps its meter across a change of the declaration;
    /// one that changed starts afresh; a mic has none.
    #[test]
    fn a_redeclaration_keeps_the_sources_that_did_not_change() {
        use ac2_proto::frame::LoudnessSource::{Input as In, Pair as P};
        let (mut m, p) = programme(LevelsSettings::default());
        let source = |r: &Readings| r.loudness.iter().map(|l| l.source).collect::<Vec<_>>();
        // Undeclared inputs are mics: no loudness.
        assert!(m.take(false).loudness.is_empty());
        p.lock().expect("policy").declare(&[line(0), line(1)], &[]);
        let mut start = 0;
        let run = |m: &mut Programme, start: &mut u64, blocks: u32| {
            for _ in 0..blocks {
                m.push(&block(*start, 480, 0.1, true));
                *start += 480;
            }
        };
        let r = m.take(false);
        assert_eq!(r.loudness.len(), 2, "declared, no block metered yet");
        assert_eq!(r.loudness[0].duration.0, 0.0);
        assert_eq!(r.loudness[0].max_true_peak.0, f64::NEG_INFINITY);
        run(&mut m, &mut start, 100);
        let r = m.take(false);
        assert_eq!(source(&r), [In { channel: 0 }, In { channel: 1 }]);
        assert!(
            (r.loudness[0].duration.0 - 1.0).abs() < 1e-9,
            "{:?}",
            r.loudness[0]
        );
        // Input 1 becomes a mic: input 0 keeps its second, input 1 has no source.
        let mut mic = line(1);
        mic.kind = ac2_proto::cal::default_kind();
        p.lock().expect("policy").declare(&[line(0), mic], &[]);
        run(&mut m, &mut start, 50);
        let r = m.take(false);
        assert_eq!(source(&r), [In { channel: 0 }]);
        assert!((r.loudness[0].duration.0 - 1.5).abs() < 1e-9);
        // Paired: a new source, input 0's own is dropped.
        let pair = Pair {
            left: 0,
            right: 1,
            name: None,
        };
        p.lock()
            .expect("policy")
            .declare(&[line(0), line(1)], &[pair]);
        assert_eq!(source(&m.take(false)), [P { left: 0, right: 1 }]);
        run(&mut m, &mut start, 50);
        let r = m.take(false);
        assert!((r.loudness[0].duration.0 - 0.5).abs() < 1e-9);
        // A gap in the sample index restarts the windows; integrated carries on.
        start += 48_000;
        run(&mut m, &mut start, 10);
        let r = m.take(false);
        let l = &r.loudness[0];
        assert_eq!(l.momentary.0, f64::NEG_INFINITY, "{l:?}");
        assert!(l.integrated.is_some(), "{l:?}");
        assert!((l.duration.0 - 0.6).abs() < 1e-9);
        assert!((l.max_true_peak.0 + 20.0).abs() < 0.2, "{l:?}");
        // Contiguous blocks the host flags as breaking continuity restart the windows too.
        for flag in [BlockFlags::XRUN, BlockFlags::DISCONTINUITY] {
            run(&mut m, &mut start, 50);
            let before = m.take(false).loudness[0];
            assert!(before.momentary.0.is_finite(), "{before:?}");
            let mut b = block(start, 480, 0.1, true);
            b.flags = flag;
            m.push(&b);
            start += 480;
            let l = m.take(false).loudness[0];
            assert_eq!(l.momentary.0, f64::NEG_INFINITY, "{flag:?}: {l:?}");
            assert_eq!(l.integrated, before.integrated, "{flag:?}");
        }
        // A contiguous unflagged run restarts nothing: short-term fills and stays.
        run(&mut m, &mut start, 310);
        let l = m.take(false).loudness[0];
        assert!((l.short_term.0 + 20.0).abs() < 0.1, "{l:?}");
        run(&mut m, &mut start, 1);
        let l = m.take(false).loudness[0];
        assert!((l.short_term.0 + 20.0).abs() < 0.1, "{l:?}");
        assert!((l.momentary.0 + 20.0).abs() < 0.1, "{l:?}");
    }

    /// A capture of device inputs 2, 3 and 5 (block channels 0, 1, 2): the pair 3+5 and the
    /// single line input 2 find their samples by metered position, not device number.
    #[test]
    fn a_sparse_capture_meters_each_source_on_its_own_inputs() {
        use ac2_proto::frame::LoudnessSource::{Input as In, Pair as P};
        let pair = Pair {
            left: 3,
            right: 5,
            name: None,
        };
        let p = policy(LevelsSettings::default(), Vec::new());
        p.lock()
            .expect("policy")
            .declare(&[line(2), line(3), line(5)], std::slice::from_ref(&pair));
        let mut m = Programme::new(
            Captured {
                idx: vec![0, 1, 2],
                channels: vec![2, 3, 5],
                device: DeviceId("fake".into()),
                sample_rate: 48_000,
            },
            Declared::Session,
            Arc::clone(&p),
            7,
        );
        // Inputs 2 and 3 carry the tone, input 5 (the pair's right) is silent.
        let fs = 48_000.0;
        let mut start = 0u64;
        let mut run = |m: &mut Programme, blocks: u32| {
            for _ in 0..blocks {
                let mut data = Vec::with_capacity(480 * 3);
                for i in 0..480u64 {
                    let t = (start + i) as f64 / fs;
                    let v = 0.1 * (2.0 * std::f64::consts::PI * 997.0 * t).sin() as f32;
                    data.extend_from_slice(&[v, v, 0.0]);
                }
                m.push(&Block::new(start, 480, 3, BlockFlags::NONE, 0, data));
                start += 480;
            }
        };
        run(&mut m, 100);
        let r = m.take(false);
        assert_eq!(r.pair_levels.len(), 1);
        assert_eq!((r.pair_levels[0].left, r.pair_levels[0].right), (3, 5));
        assert_eq!(r.pair_levels[0].quiet, Some(PairSide::Right));
        assert_eq!(r.pair_levels[0].correlation, None);
        assert!((r.true_peak[1] + 20.0).abs() < 0.2, "{:?}", r.true_peak);
        assert_eq!(r.true_peak[2], f32::NEG_INFINITY);
        let sources: Vec<_> = r.loudness.iter().map(|l| l.source).collect();
        assert_eq!(sources, [P { left: 3, right: 5 }, In { channel: 2 }]);
        // One side of a pair carries a −20 dBFS sine: its mean square is the single
        // channel's, 3 dB under the same tone on both sides.
        let (pl, il) = (r.loudness[0], r.loudness[1]);
        assert!((pl.max_true_peak.0 + 20.0).abs() < 0.2, "{pl:?}");
        assert!((pl.momentary.0 + 23.01).abs() < 0.1, "{pl:?}");
        assert!((il.momentary.0 + 23.01).abs() < 0.1, "{il:?}");
        assert!((il.max_true_peak.0 + 20.0).abs() < 0.2, "{il:?}");
        // A reset of input 5 restarts the pair, not input 2.
        std::thread::sleep(std::time::Duration::from_millis(2));
        p.lock().expect("policy").reset(vec![5]);
        m.take(false);
        run(&mut m, 10);
        let r = m.take(false);
        let (p2, i2) = (r.loudness[0], r.loudness[1]);
        assert!(p2.since.0 > pl.since.0, "{p2:?}");
        assert!(p2.duration.0 <= 0.1 + 1e-9, "{p2:?}");
        assert_eq!(p2.integrated, None, "{p2:?}");
        assert_eq!(i2.since, il.since);
        assert!((i2.duration.0 - 1.1).abs() < 1e-9, "{i2:?}");
        assert!(i2.integrated.is_some());
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
