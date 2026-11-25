//! The arrays of the programme-meter kinds (`stream_levels`, `source_levels`,
//! `preview_levels`): per input the meters, per pair the correlation, per loudness source its
//! loudness. Every reading is an array, so the metadata holds only what stands until the
//! configuration changes and a frame goes as a delta on the one before.

use crate::model::{Ballistic, PairSide};
use crate::units::{Db, Dbtp, Lufs, Seconds};

use super::schema::desc;
use super::{ArrayDesc, ArrayName, ClipFlags, DecodeError, PairLevels, SourceLoudness, Unit};

/// The unit of `ppm` and `max`: dBTP where every input's column is a true peak (the
/// true-peak detector, and under a loudness ballistic, where the bars read the sources'
/// loudness and each input keeps its true peak), dBFS under the other detectors.
pub(super) fn peak_unit(b: Ballistic) -> Unit {
    if b == Ballistic::TruePeak || b.is_loudness() {
        Unit::Dbtp
    } else {
        Unit::Dbfs
    }
}

/// The values of one levels frame, borrowed for encoding.
pub(super) struct LevelsRef<'a> {
    pub(super) ballistic: Ballistic,
    pub(super) peak: &'a [f32],
    pub(super) rms: &'a [f32],
    pub(super) clip: &'a [ClipFlags],
    pub(super) ppm: &'a [f32],
    pub(super) true_peak: &'a [f32],
    pub(super) max: &'a [f32],
    pub(super) overs: &'a [u32],
    pub(super) pair_levels: &'a [PairLevels],
    pub(super) loudness: &'a [SourceLoudness],
}

/// An array's values for encoding.
pub(super) enum Values<'a> {
    F(&'a [f32]),
    Owned(Vec<f32>),
    U(Vec<u32>),
}

fn side_code(s: Option<PairSide>) -> u32 {
    match s {
        None => 0,
        Some(PairSide::Left) => 1,
        Some(PairSide::Right) => 2,
    }
}

/// A loudness figure in f32; `None` as NaN.
fn lufs(x: Option<f64>) -> f32 {
    x.map_or(f32::NAN, |v| v as f32)
}

/// Every array of a levels frame, in wire order.
pub(super) fn cols<'a>(f: &LevelsRef<'a>) -> Vec<(ArrayDesc, Values<'a>)> {
    use super::flags::MaskBits;
    let peak_unit = peak_unit(f.ballistic);
    let src = |g: fn(&SourceLoudness) -> f32| Values::Owned(f.loudness.iter().map(g).collect());
    vec![
        (desc(ArrayName::Peak, Unit::Dbfs), Values::F(f.peak)),
        (desc(ArrayName::Rms, Unit::Dbfs), Values::F(f.rms)),
        (
            desc(ArrayName::Clip, Unit::Bitmask),
            Values::U(f.clip.iter().map(ClipFlags::to_u32).collect()),
        ),
        (desc(ArrayName::Ppm, peak_unit), Values::F(f.ppm)),
        (
            desc(ArrayName::TruePeak, Unit::Dbtp),
            Values::F(f.true_peak),
        ),
        (desc(ArrayName::Max, peak_unit), Values::F(f.max)),
        (
            desc(ArrayName::Overs, Unit::Count),
            Values::U(f.overs.to_vec()),
        ),
        (
            desc(ArrayName::PairCorr, Unit::Correlation),
            Values::Owned(
                f.pair_levels
                    .iter()
                    .map(|p| p.correlation.unwrap_or(f32::NAN))
                    .collect(),
            ),
        ),
        (
            desc(ArrayName::PairQuiet, Unit::Side),
            Values::U(f.pair_levels.iter().map(|p| side_code(p.quiet)).collect()),
        ),
        (
            desc(ArrayName::LufsM, Unit::Lufs),
            src(|l| l.momentary.0 as f32),
        ),
        (
            desc(ArrayName::LufsS, Unit::Lufs),
            src(|l| l.short_term.0 as f32),
        ),
        (
            desc(ArrayName::LufsMMax, Unit::Lufs),
            src(|l| l.max_momentary.0 as f32),
        ),
        (
            desc(ArrayName::LufsSMax, Unit::Lufs),
            src(|l| l.max_short_term.0 as f32),
        ),
        (
            desc(ArrayName::LufsI, Unit::Lufs),
            src(|l| lufs(l.integrated.map(|x| x.0))),
        ),
        (
            desc(ArrayName::Lra, Unit::Lu),
            src(|l| lufs(l.range.map(|x| x.0))),
        ),
        (
            desc(ArrayName::TpMax, Unit::Dbtp),
            src(|l| l.max_true_peak.0 as f32),
        ),
        (
            desc(ArrayName::LoudS, Unit::Seconds),
            src(|l| l.duration.0 as f32),
        ),
    ]
}

/// The decoded arrays of a levels frame.
pub(super) struct Levels {
    pub(super) peak: Vec<f32>,
    pub(super) rms: Vec<f32>,
    pub(super) clip: Vec<ClipFlags>,
    pub(super) ppm: Vec<f32>,
    pub(super) true_peak: Vec<f32>,
    pub(super) max: Vec<f32>,
    pub(super) overs: Vec<u32>,
    pub(super) pair_levels: Vec<PairLevels>,
    pub(super) loudness: Vec<SourceLoudness>,
}

/// What [`super::decode`] reads arrays with.
pub(super) trait Read {
    fn f32(&mut self, name: ArrayName, unit: Unit) -> Result<Vec<f32>, DecodeError>;
    fn u32s(&mut self, name: ArrayName, unit: Unit) -> Result<Vec<u32>, DecodeError>;
    fn clip(&mut self) -> Result<Vec<ClipFlags>, DecodeError>;
}

fn side(code: u32) -> Result<Option<PairSide>, DecodeError> {
    match code {
        0 => Ok(None),
        1 => Ok(Some(PairSide::Left)),
        2 => Ok(Some(PairSide::Right)),
        _ => Err(DecodeError::Schema(format!("pair_quiet: side {code}"))),
    }
}

/// A NaN figure as none.
fn opt(x: f32) -> Option<f64> {
    (!x.is_nan()).then_some(f64::from(x))
}

/// Every array of a levels frame; their lengths were checked against the metadata.
pub(super) fn read(a: &mut impl Read, ballistic: Ballistic) -> Result<Levels, DecodeError> {
    let peak_unit = peak_unit(ballistic);
    let peak = a.f32(ArrayName::Peak, Unit::Dbfs)?;
    let rms = a.f32(ArrayName::Rms, Unit::Dbfs)?;
    let clip = a.clip()?;
    let ppm = a.f32(ArrayName::Ppm, peak_unit)?;
    let true_peak = a.f32(ArrayName::TruePeak, Unit::Dbtp)?;
    let max = a.f32(ArrayName::Max, peak_unit)?;
    let overs = a.u32s(ArrayName::Overs, Unit::Count)?;
    let corr = a.f32(ArrayName::PairCorr, Unit::Correlation)?;
    let quiet = a.u32s(ArrayName::PairQuiet, Unit::Side)?;
    let pair_levels = corr
        .iter()
        .zip(&quiet)
        .map(|(&c, &q)| {
            Ok(PairLevels {
                correlation: (!c.is_nan()).then_some(c),
                quiet: side(q)?,
            })
        })
        .collect::<Result<_, DecodeError>>()?;
    let m = a.f32(ArrayName::LufsM, Unit::Lufs)?;
    let s = a.f32(ArrayName::LufsS, Unit::Lufs)?;
    let m_max = a.f32(ArrayName::LufsMMax, Unit::Lufs)?;
    let s_max = a.f32(ArrayName::LufsSMax, Unit::Lufs)?;
    let i = a.f32(ArrayName::LufsI, Unit::Lufs)?;
    let lra = a.f32(ArrayName::Lra, Unit::Lu)?;
    let tp = a.f32(ArrayName::TpMax, Unit::Dbtp)?;
    let dur = a.f32(ArrayName::LoudS, Unit::Seconds)?;
    let loudness = (0..m.len())
        .map(|k| SourceLoudness {
            momentary: Lufs(f64::from(m[k])),
            short_term: Lufs(f64::from(s[k])),
            integrated: opt(i[k]).map(Lufs),
            range: opt(lra[k]).map(Db),
            max_momentary: Lufs(f64::from(m_max[k])),
            max_short_term: Lufs(f64::from(s_max[k])),
            max_true_peak: Dbtp(f64::from(tp[k])),
            duration: Seconds(f64::from(dur[k])),
        })
        .collect();
    Ok(Levels {
        peak,
        rms,
        clip,
        ppm,
        true_peak,
        max,
        overs,
        pair_levels,
        loudness,
    })
}
