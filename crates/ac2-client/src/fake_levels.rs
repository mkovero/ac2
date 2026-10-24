//! Programme meters of the fake daemon: `levels.set`/`levels.reset`, and the input-meter
//! and goniometer frames it publishes while a session or a preview is open, once a test
//! turns [`Shared::meters`] on.
//!
//! The figures are plausible, not measured: every input peaks at [`Shared::input_peak`]
//! (default [`DEFAULT_PEAK_DBFS`]) unless a test sets it, `ppm` is that peak, the true peak
//! sits 0.2 dB above it (the inter-sample overshoot a 4× interpolator finds on a sine), the
//! max holds the highest peak since the last reset, nothing is ever over. A pair of two
//! sounding inputs is mono (+1); one with a silent side names it `quiet`. The goniometer
//! draws a 1 kHz mono tone: a vertical line.

use std::collections::HashMap;

use ac2_proto::frame::ClipFlags;
use ac2_proto::frame::{
    Frame, FrameData, GONIO_POINTS, GonioFrame, GonioPair, PairLevels, PreviewLevelsFrame,
    PreviewLevelsMeta, SessionLevelsFrame, SessionLevelsMeta,
};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, Command, ErrorCode, ProtoError, ReplyBody};

use super::{Shared, conflict, err};

/// The peak of an input a test did not set.
pub const DEFAULT_PEAK_DBFS: f32 = -12.0;

/// How far above the sample peak the fake's true peak reads.
pub const TRUE_PEAK_OVER_PEAK_DB: f32 = 0.2;

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

    /// The programme columns of `channels`, the max holds advanced.
    fn programme(&mut self, channels: &[u16]) -> Programme {
        let mut p = Programme::default();
        for &ch in channels {
            let peak = self.peak_of(ch);
            let held = self.levels_max.entry(ch).or_insert(f32::NEG_INFINITY);
            *held = held.max(peak);
            p.peak.push(peak);
            p.rms.push(peak - 3.0);
            p.clip.push(ClipFlags::NONE);
            p.ppm.push(peak);
            p.true_peak.push(if peak.is_finite() {
                peak + TRUE_PEAK_OVER_PEAK_DB
            } else {
                peak
            });
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
            let p = self.programme(&channels);
            let pair_levels = self.pair_levels(&self.state.pairs.clone(), &channels);
            let gonio = self.gonio(&pair_levels);
            self.meter_seq += 1;
            let stamp = self.stamp(self.meter_seq, None);
            self.publish(&Frame {
                data: FrameData::SessionLevels(SessionLevelsFrame {
                    meta: SessionLevelsMeta {
                        channels,
                        ballistic: self.state.rig.levels.ballistic,
                        pair_levels,
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
            let p = self.programme(&channels);
            let pair_levels = self.pair_levels(&d.pairs, &channels);
            let gonio = self.gonio(&pair_levels);
            self.meter_seq += 1;
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
