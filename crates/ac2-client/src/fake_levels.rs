//! Programme meters of the fake daemon: `levels.set`/`levels.reset`, and the input-meter
//! and goniometer frames it publishes while a session or a preview is open, once a test
//! turns [`Shared::meters`] on.
//!
//! The figures are plausible, not measured: every input peaks at [`Shared::input_peak`]
//! (default [`DEFAULT_PEAK_DBFS`]) unless a test sets it, the true peak sits 0.2 dB above it
//! (the inter-sample overshoot a 4× interpolator finds on a sine), `ppm` is what the rig's
//! ballistic reads (the true peak for true peak and for VU where VU is not offered, the
//! peak otherwise), the max holds the
//! highest `ppm` since the last reset, nothing is ever over. A pair of two
//! sounding inputs is mono (+1); one with a silent side names it `quiet`. The goniometer
//! draws a 1 kHz mono tone: a vertical line.
//!
//! Loudness follows the source rule of `ac2_proto::cal::loudness_sources` and reads what a
//! steady 1 kHz tone at each input's `rms` gives under BS.1770: 0 dBFS is the RMS of a
//! full-scale sine (mean square 1/2) and the K-weighting gains +0.691 dB at 1 kHz, which
//! cancels the formula's −0.691, so a source reads `10 log₁₀(Σ 10^(rms/10)) − 3.01` LUFS
//! (a −18 dBFS stereo tone reads −18 LUFS, the same tone on one input −21). Each frame
//! counts 100 ms of programme (its `audio_sample` step): momentary and its max read −∞
//! for the first 400 ms of a source or after a reset, short-term for the first 3 s;
//! integrated waits for the first 400 ms block, the range (0 LU, a steady tone) for the
//! first 3 s window, both only above the −70 LUFS absolute gate.

use std::collections::HashMap;

use ac2_proto::frame::ClipFlags;
use ac2_proto::frame::{
    Frame, FrameData, GONIO_POINTS, GonioFrame, GonioPair, LoudnessSource, PairLevels,
    PreviewLevelsFrame, PreviewLevelsMeta, SessionLevelsFrame, SessionLevelsMeta, SourceLoudness,
};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, Command, ErrorCode, ProtoError, ReplyBody};

use super::{Shared, conflict, err};

/// The peak of an input a test did not set.
pub const DEFAULT_PEAK_DBFS: f32 = -12.0;

/// How far above the sample peak the fake's true peak reads.
pub const TRUE_PEAK_OVER_PEAK_DB: f32 = 0.2;

/// Programme time one fake meter frame stands for, s (its `audio_sample` step at 48 kHz).
pub const FRAME_SECONDS: f64 = 0.1;

/// Which meters a loudness source belongs to: a preview's sources never carry into the
/// session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Meter {
    Session,
    Preview,
}

/// A loudness source's start and last reset, as meter frame sequence numbers.
#[derive(Debug, Clone, Copy)]
pub struct SourceStart {
    /// The first frame the source was metered in: momentary and short-term need windows
    /// wholly after it, and a reset does not move it.
    pub created: u64,
    /// The first frame after the last reset (or `created`).
    pub reset: u64,
    /// The fake's clock at that reset.
    pub since: WallNs,
}

/// A preview's declaration, for its frames.
#[derive(Debug, Clone)]
pub struct PreviewDecl {
    pub backend: BackendKind,
    pub device: DeviceId,
    pub channels: u16,
    pub inputs: Vec<PreviewInput>,
    pub pairs: Vec<Pair>,
}

impl Shared {
    pub(super) fn levels_cmd(&mut self, c: Command) -> Result<ReplyBody, ProtoError> {
        match c {
            Command::LevelsSet {
                base,
                ballistic,
                correlation_window,
                over_threshold,
                correlation_gate,
            } => {
                let mut rig = self.state.rig.clone();
                if base != rig.rev {
                    return Err(conflict(rig.rev));
                }
                let now = rig.levels;
                let levels = LevelsSettings {
                    ballistic: ballistic.unwrap_or(now.ballistic),
                    correlation_window: correlation_window.unwrap_or(now.correlation_window),
                    over_threshold: over_threshold.unwrap_or(now.over_threshold),
                    correlation_gate: correlation_gate.unwrap_or(now.correlation_gate),
                };
                levels.check().map_err(|e| err(ErrorCode::Invalid, e))?;
                rig.levels = levels;
                rig.rev = Rev(self.rev.0 + 1);
                self.commit(Change::Rig(rig));
                Ok(ReplyBody::Ack { rev: self.rev })
            }
            Command::LevelsReset { channels } => {
                let next = self.meter_seq + 1;
                let since = WallNs(self.now_ns());
                for ((_, src), st) in &mut self.loudness_starts {
                    if channels.is_empty() || channels.iter().any(|&ch| src.has(ch)) {
                        st.reset = next;
                        st.since = since;
                    }
                }
                if channels.is_empty() {
                    self.levels_max.clear();
                } else {
                    for ch in channels {
                        self.levels_max.remove(&ch);
                    }
                }
                Ok(ReplyBody::Ack { rev: self.rev })
            }
            _ => unreachable!("levels_cmd is called with levels commands only"),
        }
    }

    fn peak_of(&self, ch: u16) -> f32 {
        self.input_peak
            .get(&ch)
            .copied()
            .unwrap_or(DEFAULT_PEAK_DBFS)
    }

    /// The programme columns of `channels` on `device`, the max holds advanced.
    fn programme(&mut self, channels: &[u16], device: &DeviceId) -> Programme {
        let ballistic = self.state.rig.levels.ballistic;
        let mut p = Programme::default();
        for &ch in channels {
            let peak = self.peak_of(ch);
            let true_peak = if peak.is_finite() {
                peak + TRUE_PEAK_OVER_PEAK_DB
            } else {
                peak
            };
            let reads_true_peak = ballistic == Ballistic::TruePeak
                || (ballistic == Ballistic::Vu
                    && !ac2_proto::cal::offers_vu(
                        &self.state.inputs,
                        &self.state.line_calibrations,
                        device,
                        ch,
                    ));
            let ppm = if reads_true_peak { true_peak } else { peak };
            let held = self.levels_max.entry(ch).or_insert(f32::NEG_INFINITY);
            *held = held.max(ppm);
            p.peak.push(peak);
            p.rms.push(peak - 3.0);
            p.clip.push(ClipFlags::NONE);
            p.ppm.push(ppm);
            p.true_peak.push(true_peak);
            p.max.push(*held);
            p.overs.push(0);
        }
        p
    }

    fn pair_levels(&self, pairs: &[Pair], captured: &[u16]) -> Vec<PairLevels> {
        pairs
            .iter()
            .filter(|p| captured.contains(&p.left) && captured.contains(&p.right))
            .map(|p| {
                let gate = self.state.rig.levels.correlation_gate.0 as f32;
                let quiet = if self.peak_of(p.left) < gate {
                    Some(PairSide::Left)
                } else if self.peak_of(p.right) < gate {
                    Some(PairSide::Right)
                } else {
                    None
                };
                PairLevels {
                    left: p.left,
                    right: p.right,
                    correlation: quiet.is_none().then_some(1.0),
                    quiet,
                }
            })
            .collect()
    }

    /// Drops `meter`'s loudness sources: they start afresh when next metered.
    pub(super) fn forget_loudness(&mut self, meter: Meter) {
        self.loudness_starts.retain(|(m, _), _| *m != meter);
    }

    /// The loudness of every source of `meter` this frame (`seq`); see the module docs.
    fn loudness(
        &mut self,
        meter: Meter,
        seq: u64,
        kind: impl Fn(u16) -> InputKind,
        pairs: &[Pair],
        captured: &[u16],
    ) -> Vec<SourceLoudness> {
        let sources = ac2_proto::cal::loudness_sources(kind, pairs, captured);
        let now = WallNs(self.now_ns());
        // A source no longer declared is dropped, so a re-declared one starts afresh.
        self.loudness_starts
            .retain(|(m, s), _| *m != meter || sources.contains(s));
        sources
            .into_iter()
            .map(|source| {
                let st = *self
                    .loudness_starts
                    .entry((meter, source))
                    .or_insert(SourceStart {
                        created: seq,
                        reset: seq,
                        since: now,
                    });
                self.source_loudness(source, seq, st)
            })
            .collect()
    }

    fn source_loudness(&self, source: LoudnessSource, seq: u64, st: SourceStart) -> SourceLoudness {
        let inputs: &[u16] = match &source {
            LoudnessSource::Pair { left, right } => &[*left, *right],
            LoudnessSource::Input { channel } => std::slice::from_ref(channel),
        };
        let energy: f64 = inputs
            .iter()
            .map(|&ch| 10f64.powf(f64::from(self.peak_of(ch) - 3.0) / 10.0))
            .sum();
        let level = 10.0 * energy.log10() - 10.0 * 2f64.log10();
        let tp = inputs
            .iter()
            .map(|&ch| {
                let p = self.peak_of(ch);
                if p.is_finite() {
                    p + TRUE_PEAK_OVER_PEAK_DB
                } else {
                    p
                }
            })
            .fold(f32::NEG_INFINITY, f32::max);
        let frames = |from: u64| (seq + 1).saturating_sub(from) as f64 * FRAME_SECONDS;
        let (alive, metered) = (frames(st.created), frames(st.reset));
        let after = |t: f64, window: f64| {
            if t >= window - 1e-9 {
                level
            } else {
                f64::NEG_INFINITY
            }
        };
        let gated = level > -70.0;
        SourceLoudness {
            source,
            momentary: Lufs(after(alive, 0.4)),
            short_term: Lufs(after(alive, 3.0)),
            integrated: (gated && metered >= 0.4 - 1e-9).then_some(Lufs(level)),
            range: (gated && metered >= 3.0 - 1e-9).then_some(Db(0.0)),
            max_momentary: Lufs(after(metered, 0.4)),
            max_short_term: Lufs(after(metered, 3.0)),
            max_true_peak: Dbtp(f64::from(tp)),
            duration: Seconds(metered),
            since: st.since,
        }
    }

    fn gonio(&self, pairs: &[PairLevels]) -> GonioFrame {
        GonioFrame {
            pairs: pairs
                .iter()
                .map(|p| GonioPair {
                    left: p.left,
                    right: p.right,
                    gain_dbfs: self.peak_of(p.left) + 3.0,
                    points: (0..GONIO_POINTS)
                        .map(|i| {
                            // Twenty-one cycles of 1 kHz across the frame's 1/48 s.
                            let ph = i as f32 / GONIO_POINTS as f32 * 21.0 * std::f32::consts::TAU;
                            [0, (127.0 * ph.sin()).round() as i8]
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Publishes the session's and the preview's meter frames, if either is open.
    pub(super) fn publish_levels(&mut self) {
        if let Some(open) = self.state.session.open.clone() {
            let channels = open.config.input_channels.clone();
            let p = self.programme(&channels, &open.input_device);
            let pairs = self.state.pairs.clone();
            let pair_levels = self.pair_levels(&pairs, &channels);
            let gonio = self.gonio(&pair_levels);
            self.meter_seq += 1;
            let inputs = self.state.inputs.clone();
            let loudness = self.loudness(
                Meter::Session,
                self.meter_seq,
                |ch| ac2_proto::cal::input_kind(&inputs, ch),
                &pairs,
                &channels,
            );
            let stamp = self.stamp(self.meter_seq, None);
            self.publish(&Frame {
                data: FrameData::SessionLevels(SessionLevelsFrame {
                    meta: SessionLevelsMeta {
                        channels,
                        ballistic: self.state.rig.levels.ballistic,
                        pair_levels,
                        loudness,
                    },
                    peak: p.peak,
                    rms: p.rms,
                    clip: p.clip,
                    ppm: p.ppm,
                    true_peak: p.true_peak,
                    max: p.max,
                    overs: p.overs,
                }),
                stamp,
            });
            if !gonio.pairs.is_empty() {
                self.publish(&Frame {
                    data: FrameData::SessionGonio(gonio),
                    stamp,
                });
            }
        }
        if let Some(d) = self.preview_decl.clone() {
            let channels: Vec<u16> = (0..d.channels).collect();
            let p = self.programme(&channels, &d.device);
            let pair_levels = self.pair_levels(&d.pairs, &channels);
            let gonio = self.gonio(&pair_levels);
            self.meter_seq += 1;
            let kind = |ch: u16| {
                d.inputs
                    .iter()
                    .find(|i| i.channel == ch)
                    .map_or_else(ac2_proto::cal::default_kind, |i| i.kind.clone())
            };
            let loudness = self.loudness(Meter::Preview, self.meter_seq, kind, &d.pairs, &channels);
            let stamp = self.stamp(self.meter_seq, None);
            self.publish(&Frame {
                data: FrameData::PreviewLevels(PreviewLevelsFrame {
                    meta: PreviewLevelsMeta {
                        backend: d.backend,
                        device: d.device,
                        channels,
                        inputs: d.inputs,
                        pairs: d.pairs,
                        ballistic: self.state.rig.levels.ballistic,
                        pair_levels,
                        loudness,
                    },
                    peak: p.peak,
                    rms: p.rms,
                    clip: p.clip,
                    ppm: p.ppm,
                    true_peak: p.true_peak,
                    max: p.max,
                    overs: p.overs,
                }),
                stamp,
            });
            if !gonio.pairs.is_empty() {
                self.publish(&Frame {
                    data: FrameData::PreviewGonio(gonio),
                    stamp,
                });
            }
        }
    }
}

/// The columns of one meter frame.
#[derive(Default)]
struct Programme {
    peak: Vec<f32>,
    rms: Vec<f32>,
    clip: Vec<ClipFlags>,
    ppm: Vec<f32>,
    true_peak: Vec<f32>,
    max: Vec<f32>,
    overs: Vec<u32>,
}

/// Test helpers.
impl Shared {
    /// Makes input `ch` read `peak_dbfs` (`f32::NEG_INFINITY`: silent) in every frame.
    pub fn set_input_peak(&mut self, ch: u16, peak_dbfs: f32) {
        self.input_peak.insert(ch, peak_dbfs);
    }

    /// The max holds, by input, as the next frame carries them.
    pub fn levels_max(&self) -> &HashMap<u16, f32> {
        &self.levels_max
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::fake::FakeOptions;

    fn line(_: u16) -> InputKind {
        InputKind::Line
    }

    #[test]
    fn loudness_follows_the_windows_and_the_reset() {
        let mut s = Shared::new(FakeOptions::default(), 1);
        s.set_input_peak(1, f32::NEG_INFINITY);
        let pairs = [Pair {
            left: 0,
            right: 1,
            name: None,
        }];
        let captured = [0, 1, 2];
        let first = s.loudness(Meter::Session, 1, line, &pairs, &captured);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].source, LoudnessSource::Pair { left: 0, right: 1 });
        assert_eq!(first[1].source, LoudnessSource::Input { channel: 2 });
        // 100 ms in: no 400 ms window yet, but the true peak is metered.
        assert_eq!(first[0].momentary.0, f64::NEG_INFINITY);
        assert_eq!(first[0].integrated, None);
        assert!((first[0].max_true_peak.0 - -11.8).abs() < 1e-5);
        // 3 s in: a −15 dBFS RMS tone on one side of a pair reads −18.01 LUFS, the same
        // on its own input too (one channel, weight 1).
        let l = s.loudness(Meter::Session, 30, line, &pairs, &captured);
        let want = -15.0 - 10.0 * 2f64.log10();
        for x in &l {
            assert!((x.momentary.0 - want).abs() < 1e-9, "{x:?}");
            assert!((x.short_term.0 - want).abs() < 1e-9, "{x:?}");
            assert!((x.integrated.unwrap().0 - want).abs() < 1e-9);
            assert_eq!(x.range, Some(Db(0.0)));
            assert!((x.duration.0 - 3.0).abs() < 1e-9);
        }
        // A reset of input 3 restarts its maxima and integration, not its momentary.
        s.meter_seq = 30;
        s.levels_cmd(Command::LevelsReset { channels: vec![2] })
            .unwrap();
        let l = s.loudness(Meter::Session, 31, line, &pairs, &captured);
        assert!((l[0].duration.0 - 3.1).abs() < 1e-9, "pair untouched");
        assert!((l[1].duration.0 - 0.1).abs() < 1e-9);
        assert_eq!(l[1].integrated, None);
        assert_eq!(l[1].max_momentary.0, f64::NEG_INFINITY);
        assert!(
            l[1].momentary.0.is_finite(),
            "signal state survives a reset"
        );
        // A mic gets no source; neither does the half of a pair whose partner is not
        // captured.
        let none = s.loudness(
            Meter::Session,
            32,
            |ch| {
                if ch == 2 {
                    ac2_proto::cal::default_kind()
                } else {
                    InputKind::Line
                }
            },
            &pairs,
            &[0, 2],
        );
        assert!(none.is_empty(), "{none:?}");
    }
}
