//! What a delta carries of its frame's metadata: per kind the fields that move from frame to
//! frame. The rest of the metadata is the base's, so a change of it goes as a keyframe.

use serde::{Deserialize, Serialize, Serializer};

use crate::model::{AlignState, FreqRange};
use crate::units::Seconds;

use super::{FrameKind, FrameMeta, LevelsMark, SpecHarmonics};

/// A `tf` delta's: how full its averages are and where they still fill, the delay the
/// phase is referred to (a nudge moves it without a new configuration), and an alignment's
/// fit, which follows the curves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TfPerFrame {
    filled: f32,
    settling: Option<FreqRange>,
    delay: Seconds,
    nudged: Seconds,
    align: Option<Box<AlignState>>,
}

/// A `bins` delta's: how much its stage holds, and the delay as in its `tf`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BinsPerFrame {
    blocks: u64,
    eff_avg: f32,
    delay: Seconds,
    nudged: Seconds,
}

/// An `spl` delta's: its readings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SplPerFrame {
    level: f64,
    lmax: f64,
    lmin: f64,
    leq: f64,
    lpeak: f64,
    duration: Seconds,
}

/// A `spec` delta's: the age of each band of a multi-resolution spectrum (one per band of
/// `bands`, none without), and the generator's harmonics read from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpecPerFrame {
    ages: Vec<Option<Seconds>>,
    harmonics: Option<Box<SpecHarmonics>>,
}

/// A goniometer delta's: each pair's auto gain, dBFS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GonioPerFrame {
    gains: Vec<f32>,
}

/// A `source_levels` delta's: its timing mark.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourcePerFrame {
    mark: Option<LevelsMark>,
}

/// What a delta carries of the metadata, fixed per kind; every kind not listed nothing
/// (nil on the wire).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum PerFrame {
    Tf(TfPerFrame),
    Bins(BinsPerFrame),
    Spl(SplPerFrame),
    Spec(SpecPerFrame),
    Gonio(GonioPerFrame),
    Source(SourcePerFrame),
    Static,
}

impl Serialize for PerFrame {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Tf(p) => p.serialize(s),
            Self::Bins(p) => p.serialize(s),
            Self::Spl(p) => p.serialize(s),
            Self::Spec(p) => p.serialize(s),
            Self::Gonio(p) => p.serialize(s),
            Self::Source(p) => p.serialize(s),
            Self::Static => s.serialize_unit(),
        }
    }
}

/// Whether a delta of `kind` carries any of its metadata.
fn kind_has_per_frame(kind: FrameKind) -> bool {
    matches!(
        kind,
        FrameKind::Tf
            | FrameKind::Bins
            | FrameKind::Spl
            | FrameKind::Spec
            | FrameKind::StreamGonio
            | FrameKind::PreviewGonio
            | FrameKind::SourceLevels
    )
}

/// The per-frame part of `m`.
pub(super) fn of(m: &FrameMeta) -> PerFrame {
    match m {
        FrameMeta::Tf(t) => PerFrame::Tf(TfPerFrame {
            filled: t.filled,
            settling: t.settling,
            delay: t.delay,
            nudged: t.nudged,
            align: t.math.as_ref().and_then(|m| m.align.clone()),
        }),
        FrameMeta::Bins(b) => PerFrame::Bins(BinsPerFrame {
            blocks: b.blocks,
            eff_avg: b.eff_avg,
            delay: b.delay,
            nudged: b.nudged,
        }),
        FrameMeta::Spl(s) => PerFrame::Spl(SplPerFrame {
            level: s.level,
            lmax: s.lmax,
            lmin: s.lmin,
            leq: s.leq,
            lpeak: s.lpeak,
            duration: s.duration,
        }),
        FrameMeta::Spec(s) => PerFrame::Spec(SpecPerFrame {
            ages: s
                .bands
                .as_ref()
                .map(|b| b.bands.iter().map(|x| x.age).collect())
                .unwrap_or_default(),
            harmonics: s.harmonics.clone(),
        }),
        FrameMeta::StreamGonio(g) | FrameMeta::PreviewGonio(g) => PerFrame::Gonio(GonioPerFrame {
            gains: g.pairs.iter().map(|p| p.gain_dbfs).collect(),
        }),
        FrameMeta::SourceLevels(s) => PerFrame::Source(SourcePerFrame { mark: s.mark }),
        // Named one by one: a new kind of metadata has to say here whether any of it
        // moves from frame to frame.
        FrameMeta::Ir(_)
        | FrameMeta::Rta(_)
        | FrameMeta::Leq(_)
        | FrameMeta::BandLeq(_)
        | FrameMeta::Levels(_)
        | FrameMeta::StreamLevels(_)
        | FrameMeta::PreviewLevels(_)
        | FrameMeta::Timing(_)
        | FrameMeta::Ka(_) => PerFrame::Static,
    }
}

/// `base` with the per-frame fields `p`; `None` if `p` is not of `base`'s kind or does not
/// fit its static part (an alignment on a frame of no math, ages or gains of another count).
pub(super) fn with(base: &FrameMeta, p: &PerFrame) -> Option<FrameMeta> {
    let mut m = base.clone();
    match (&mut m, p) {
        (FrameMeta::Tf(t), PerFrame::Tf(p)) => {
            t.filled = p.filled;
            t.settling = p.settling;
            t.delay = p.delay;
            t.nudged = p.nudged;
            match (&mut t.math, &p.align) {
                (Some(math), a) => math.align.clone_from(a),
                (None, None) => {}
                (None, Some(_)) => return None,
            }
        }
        (FrameMeta::Bins(b), PerFrame::Bins(p)) => {
            b.blocks = p.blocks;
            b.eff_avg = p.eff_avg;
            b.delay = p.delay;
            b.nudged = p.nudged;
        }
        (FrameMeta::Spl(s), PerFrame::Spl(p)) => {
            s.level = p.level;
            s.lmax = p.lmax;
            s.lmin = p.lmin;
            s.leq = p.leq;
            s.lpeak = p.lpeak;
            s.duration = p.duration;
        }
        (FrameMeta::Spec(s), PerFrame::Spec(p)) => {
            match &mut s.bands {
                Some(b) if b.bands.len() == p.ages.len() => {
                    for (band, age) in b.bands.iter_mut().zip(&p.ages) {
                        band.age = *age;
                    }
                }
                None if p.ages.is_empty() => {}
                _ => return None,
            }
            s.harmonics.clone_from(&p.harmonics);
        }
        (FrameMeta::StreamGonio(g) | FrameMeta::PreviewGonio(g), PerFrame::Gonio(p)) => {
            if g.pairs.len() != p.gains.len() {
                return None;
            }
            for (pair, gain) in g.pairs.iter_mut().zip(&p.gains) {
                pair.gain_dbfs = *gain;
            }
        }
        (FrameMeta::SourceLevels(s), PerFrame::Source(p)) => s.mark = p.mark,
        (_, PerFrame::Static) if !kind_has_per_frame(base.kind()) => {}
        _ => return None,
    }
    Some(m)
}
