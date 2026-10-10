//! Checks and small helpers shared by the command handlers: limits, error builders, measurement and sweep validation.

use ac2_core::generator::GeneratorError;
use ac2_proto::grid::GridDef;
use ac2_proto::model::{InputSetup, MeasConfig, MeasKind, Measurement, SpecAveraging, SweepConfig};
use ac2_proto::units::{MeasId, Rev};
use ac2_proto::{ErrorCode, ErrorDetail, ProtoError};

use crate::conv;
use crate::jobs::{self, SmoothingChange};
use crate::util::{perr, perr_detail};

use super::maths;

pub(super) const MAX_DELAY_S: f64 = 10.0;

/// A delay in samples at `fs`, fractions kept. Snapped to a millionth of a sample: a delay
/// given in seconds or built from fractional steps lands within float rounding of the value
/// meant, and a whole-sample delay must stay exactly whole (the engine then applies no phase
/// rotation at all). A millionth of a sample is 0.0002° at 20 kHz and 48 kHz.
pub(super) fn delay_samples(seconds: f64, fs: f64) -> f64 {
    (seconds * fs * 1e6).round() / 1e6
}
/// Largest difference between the fast and slow input mean squares a calibration accepts,
/// dB.
pub(super) const MAX_CAL_UNSETTLED_DB: f64 = 0.05;

/// Refuses a `delay.find` the finder could not run as asked: band edges or observation out
/// of range for `fs`. Sub-band observations are the operator's 2 / 4 / 8 s choice (D2).
pub(super) fn check_find(
    band: conv::FindBand,
    observation: Option<f64>,
    fs: f64,
) -> Result<(), ProtoError> {
    use ac2_core::delay::{Band, BandClass, FinderConfig};
    let inv = |m: String| Err(perr(ErrorCode::Invalid, m));
    let core_band = match band {
        conv::FindBand::Auto => Band::FullRange,
        conv::FindBand::Band(b) => b,
    };
    let mut cfg = FinderConfig::new(fs, core_band);
    cfg.observation_s = observation;
    if let Err(e) = cfg.validate() {
        return inv(format!("delay finder: {e}"));
    }
    if let Some(s) = observation {
        if s > jobs::finder::MAX_OBSERVATION_S {
            return inv(format!(
                "observation {s} s is longer than {} s",
                jobs::finder::MAX_OBSERVATION_S
            ));
        }
        let sub = match band {
            conv::FindBand::Auto => false,
            conv::FindBand::Band(b) => b.class() == BandClass::Sub,
        };
        if sub && ![2.0, 4.0, 8.0].contains(&s) {
            return inv(format!(
                "sub-band observation must be 2, 4 or 8 s, not {s} s"
            ));
        }
    }
    Ok(())
}

/// `current` with `rows` replacing the rows of their channels, sorted by channel.
pub(super) fn upsert_inputs(current: &[InputSetup], rows: Vec<InputSetup>) -> Vec<InputSetup> {
    let mut out: Vec<InputSetup> = current
        .iter()
        .filter(|c| rows.iter().all(|r| r.channel != c.channel))
        .cloned()
        .collect();
    out.extend(rows);
    out.sort_by_key(|i| i.channel);
    out
}

pub(super) fn mutation_conflict(rev: Rev) -> ProtoError {
    perr_detail(
        ErrorCode::Conflict,
        format!("state has moved on to rev {}", rev.0),
        ErrorDetail::Conflict { rev },
    )
}

pub(super) fn not_found(meas: MeasId) -> ProtoError {
    perr(ErrorCode::NotFound, format!("no measurement {}", meas.0))
}

pub(super) fn lease_required() -> ProtoError {
    perr(
        ErrorCode::LeaseRequired,
        "this command needs the stimulus lease; the token is missing, stale or expired",
    )
}

pub(super) fn gen_err(e: GeneratorError) -> ProtoError {
    let code = match e {
        GeneratorError::WouldClip { .. } | GeneratorError::AboveCeiling { .. } => {
            ErrorCode::Refused
        }
        _ => ErrorCode::Invalid,
    };
    perr(code, e.to_string())
}

/// Checks a measurement configuration without a session.
pub(super) fn validate_meas(c: &MeasConfig) -> Result<(), ProtoError> {
    let inv = |m: &str| Err(perr(ErrorCode::Invalid, m.to_owned()));
    match &c.kind {
        MeasKind::Transfer { config } => {
            if config.reference_input == config.measurement_input {
                return inv("reference and measurement must be different inputs");
            }
            if conv::tf_averaging(config.averaging).is_none() {
                return inv("invalid averaging");
            }
            if conv::depth(config.depth).is_none() {
                return inv("fast_lf max_settle_s must be a positive time");
            }
        }
        MeasKind::Spectrum { config } => {
            if !config.fft_len.is_power_of_two() || !(64..=65536).contains(&config.fft_len) {
                return inv("fft_len must be a power of two in 64..=65536");
            }
            if conv::spec_averaging(config.averaging).is_none() {
                return inv("invalid averaging");
            }
            if let SpecAveraging::Fifo { frames } = config.averaging {
                let bins = u64::from(config.fft_len) / 2 + 1;
                let most = SpecAveraging::MAX_SPECTRUM_FIFO_VALUES / bins;
                if u64::from(frames) > most {
                    return inv(&format!(
                        "a {}-point spectrum averages at most {most} FIFO frames \
                         (use exponential averaging for longer)",
                        config.fft_len
                    ));
                }
            }
        }
        MeasKind::Rta { config } => {
            if !(config.f_lo.0.is_finite()
                && config.f_hi.0.is_finite()
                && config.f_lo.0 > 0.0
                && config.f_hi.0 > config.f_lo.0)
            {
                return inv("f_lo must be positive and below f_hi");
            }
            if conv::spec_averaging(config.averaging).is_none() {
                return inv("invalid averaging");
            }
            if let SpecAveraging::Fifo { frames } = config.averaging
                && frames > SpecAveraging::MAX_RTA_FIFO_FRAMES
            {
                return inv(&format!(
                    "an RTA averages at most {} FIFO frames (use exponential averaging for \
                     longer)",
                    SpecAveraging::MAX_RTA_FIFO_FRAMES
                ));
            }
        }
        MeasKind::Spl { config } => {
            config.check().map_err(|m| perr(ErrorCode::Invalid, m))?;
        }
        MeasKind::Math { config } => maths::validate(config)?,
        MeasKind::Sweep { config } => validate_sweep(config)?,
    }
    Ok(())
}

/// Checks of a sweep measurement's settings that need no session (the session's inputs,
/// outputs and rate, and the ceiling, are checked at every run).
fn validate_sweep(c: &SweepConfig) -> Result<(), ProtoError> {
    let inv = |m: String| Err(perr(ErrorCode::Invalid, m));
    if c.reference_input == c.measurement_input {
        return inv("the reference and the measurement are the same input".into());
    }
    if c.outputs.is_empty() {
        return inv("no output channels".into());
    }
    for (i, o) in c.outputs.iter().enumerate() {
        if c.outputs[..i].contains(o) {
            return inv(format!("output {o} listed twice"));
        }
    }
    if !(1..=SweepConfig::MAX_REPEATS).contains(&c.repeats) {
        return inv(format!("repeats must be 1 … {}", SweepConfig::MAX_REPEATS));
    }
    if !(c.level.0.is_finite() && c.level.0 <= 0.0) {
        return inv("the level must be a finite dBFS value ≤ 0".into());
    }
    if c.gate.is_some_and(|g| !(g.0.is_finite() && g.0 > 0.0)) {
        return inv("the gate must be a positive time".into());
    }
    if c.tail
        .is_some_and(|t| !(t.0.is_finite() && t.0 <= SweepConfig::MAX_TAIL.0))
    {
        return inv(format!(
            "the silence after each sweep is at most {} s",
            SweepConfig::MAX_TAIL.0
        ));
    }
    let s = c.sweep;
    let runs_up =
        s.start.0.is_finite() && s.end.0.is_finite() && s.start.0 > 0.0 && s.end.0 > s.start.0;
    let lasts = s.duration.0.is_finite() && s.duration.0 > 0.0;
    if !(runs_up && lasts) {
        return inv(
            "the sweep must run up from a positive start frequency, for a positive time".into(),
        );
    }
    Ok(())
}

/// The new configuration when `new` is `old` on the same input (an SPL meter): its
/// weightings and Leq windows change in place — the meter runs every weighting all along, so
/// its interval, its log and its windows carry on.
pub(super) fn spl_in_place(old: &MeasKind, new: &MeasKind) -> Option<ac2_proto::model::SplConfig> {
    match (old, new) {
        (MeasKind::Spl { config: a }, MeasKind::Spl { config: b }) if a.input == b.input => {
            Some(b.clone())
        }
        _ => None,
    }
}

/// The new smoothing when `new` is `old` with only the display smoothing changed (a
/// transfer or spectrum measurement); such an update is applied in place instead of
/// restarting the job.
pub(super) fn smoothing_only(old: &MeasKind, new: &MeasKind) -> Option<SmoothingChange> {
    match (old, new) {
        (MeasKind::Transfer { config: a }, MeasKind::Transfer { config: b })
            if ac2_proto::model::TransferConfig {
                smoothing: b.smoothing,
                ..a.clone()
            } == *b =>
        {
            Some(SmoothingChange::Transfer(b.smoothing))
        }
        (MeasKind::Spectrum { config: a }, MeasKind::Spectrum { config: b })
            if ac2_proto::model::SpectrumConfig {
                smoothing: b.smoothing,
                ..a.clone()
            } == *b =>
        {
            Some(SmoothingChange::Spectrum(b.smoothing))
        }
        (MeasKind::Math { config: a }, MeasKind::Math { config: b })
            if ac2_proto::model::MathConfig {
                smoothing: b.smoothing,
                ..a.clone()
            } == *b =>
        {
            Some(SmoothingChange::Transfer(b.smoothing))
        }
        _ => None,
    }
}

/// Refuses the job commands for a sweep measurement: it has no job, `sweep.run` plays it.
pub(super) fn not_a_sweep(m: &Measurement) -> Result<(), ProtoError> {
    if m.config.kind.is_job() {
        Ok(())
    } else {
        Err(perr(
            ErrorCode::Invalid,
            format!(
                "{} is a sweep measurement: it has no job to start, stop or reset; \
                 sweep.run plays it",
                m.config.name
            ),
        ))
    }
}

/// Grid of a transfer measurement; known without a session.
pub(super) fn static_grid(kind: &MeasKind) -> Option<GridDef> {
    match kind {
        MeasKind::Transfer { config } => Some(GridDef::Log {
            ppo: config.grid().ppo,
            k_min: config.grid().k_min,
            k_max: config.grid().k_max,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ac2_proto::model::{BandFraction, RtaConfig, SpectrumConfig};
    use ac2_proto::units::Seconds;

    use super::*;

    fn meas(kind: MeasKind) -> MeasConfig {
        MeasConfig {
            name: "m".into(),
            kind,
        }
    }

    #[test]
    fn spectrum_and_rta_averaging_is_bounded() {
        let spec = |fft_len, averaging| {
            validate_meas(&meas(MeasKind::Spectrum {
                config: SpectrumConfig {
                    fft_len,
                    averaging,
                    ..SpectrumConfig::on_input(0)
                },
            }))
        };
        let fifo = |frames| SpecAveraging::Fifo { frames };
        assert!(spec(65_536, fifo(511)).is_ok());
        assert!(spec(65_536, fifo(512)).is_err());
        assert!(spec(4096, fifo(8188)).is_ok());
        assert!(spec(4096, fifo(0)).is_err());
        let exp = |s| SpecAveraging::Exponential {
            time_constant: Seconds(s),
        };
        assert!(spec(65_536, exp(600.0)).is_ok());
        assert!(spec(65_536, exp(0.0)).is_err());
        let rta = |averaging| {
            validate_meas(&meas(MeasKind::Rta {
                config: RtaConfig {
                    averaging,
                    ..RtaConfig::on_input(0, BandFraction::Third)
                },
            }))
        };
        assert!(rta(fifo(SpecAveraging::MAX_RTA_FIFO_FRAMES)).is_ok());
        assert!(rta(fifo(SpecAveraging::MAX_RTA_FIFO_FRAMES + 1)).is_err());
        assert!(rta(exp(-1.0)).is_err());
    }
}
