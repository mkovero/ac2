//! Conversions between wire types (`ac2-proto`) and the DSP / audio crates' own types. The
//! protocol mirrors them deliberately (so a change there cannot silently change the wire);
//! this module is the one boundary where they meet.

use ac2_audio as audio;
use ac2_core as core;
use ac2_proto::model as pm;
use ac2_proto::units::{Samples, WallNs};

pub(crate) fn band_fraction(f: pm::BandFraction) -> core::rta::BandFraction {
    match f {
        pm::BandFraction::Octave => core::rta::BandFraction::Octave,
        pm::BandFraction::Third => core::rta::BandFraction::Third,
        pm::BandFraction::Sixth => core::rta::BandFraction::Sixth,
        pm::BandFraction::Twelfth => core::rta::BandFraction::Twelfth,
        pm::BandFraction::TwentyFourth => core::rta::BandFraction::TwentyFourth,
    }
}

pub(crate) fn smoothing_fraction(f: pm::SmoothingFraction) -> core::smoothing::SmoothingFraction {
    use core::smoothing::SmoothingFraction as F;
    match f {
        pm::SmoothingFraction::Third => F::Third,
        pm::SmoothingFraction::Sixth => F::Sixth,
        pm::SmoothingFraction::Twelfth => F::Twelfth,
        pm::SmoothingFraction::TwentyFourth => F::TwentyFourth,
        pm::SmoothingFraction::FortyEighth => F::FortyEighth,
    }
}

pub(crate) fn smoothing(
    s: pm::Smoothing,
) -> (
    core::smoothing::SmoothingFraction,
    core::smoothing::SmoothingMode,
) {
    use core::smoothing::SmoothingMode as M;
    let f = smoothing_fraction(s.fraction);
    let m = match s.mode {
        pm::SmoothingMode::Magnitude => M::Magnitude,
        pm::SmoothingMode::MagnitudePhase => M::MagnitudePhase,
    };
    (f, m)
}

pub(crate) fn weighting(w: pm::Weighting) -> core::weighting::Weighting {
    match w {
        pm::Weighting::A => core::weighting::Weighting::A,
        pm::Weighting::C => core::weighting::Weighting::C,
        pm::Weighting::Z => core::weighting::Weighting::Z,
    }
}

pub(crate) fn time_weighting(t: pm::TimeWeighting) -> core::spl::TimeWeighting {
    match t {
        pm::TimeWeighting::Fast => core::spl::TimeWeighting::Fast,
        pm::TimeWeighting::Slow => core::spl::TimeWeighting::Slow,
        pm::TimeWeighting::Impulse => core::spl::TimeWeighting::Impulse,
    }
}

pub(crate) fn peak_weighting(p: pm::PeakWeighting) -> core::spl::PeakWeighting {
    match p {
        pm::PeakWeighting::C => core::spl::PeakWeighting::C,
        pm::PeakWeighting::Z => core::spl::PeakWeighting::Z,
    }
}

pub(crate) fn window(w: pm::Window) -> core::window::Window {
    match w {
        pm::Window::Hann => core::window::Window::Hann,
        pm::Window::BlackmanHarris4 => core::window::Window::BlackmanHarris4,
        pm::Window::FlatTop => core::window::Window::FlatTop,
        pm::Window::Rectangular => core::window::Window::Rectangular,
    }
}

/// TF averaging; `None` when the values are out of range.
pub(crate) fn tf_averaging(a: pm::TfAveraging) -> Option<core::mtw::Averaging> {
    match a {
        pm::TfAveraging::Fifo { blocks } if blocks >= 1 => {
            Some(core::mtw::Averaging::Fifo { blocks })
        }
        pm::TfAveraging::Exponential { time_constant }
            if time_constant.0.is_finite() && time_constant.0 > 0.0 =>
        {
            Some(core::mtw::Averaging::Exponential {
                time_constant_s: time_constant.0,
            })
        }
        _ => None,
    }
}

/// Spectrum averaging; `None` when the values are out of range.
pub(crate) fn spec_averaging(a: pm::SpecAveraging) -> Option<core::spectrum::Averaging> {
    match a {
        pm::SpecAveraging::Off => Some(core::spectrum::Averaging::Off),
        pm::SpecAveraging::Fifo { frames } if frames >= 1 => {
            Some(core::spectrum::Averaging::Fifo {
                frames: frames as usize,
            })
        }
        pm::SpecAveraging::Exponential { time_constant }
            if time_constant.0.is_finite() && time_constant.0 > 0.0 =>
        {
            Some(core::spectrum::Averaging::Exponential {
                time_constant_s: time_constant.0,
            })
        }
        _ => None,
    }
}

pub(crate) fn band_limit(b: Option<pm::BandLimit>) -> core::generator::BandLimit {
    match b {
        None => core::generator::BandLimit::NONE,
        Some(b) => core::generator::BandLimit {
            highpass_hz: b.highpass.map(|h| h.0),
            lowpass_hz: b.lowpass.map(|h| h.0),
            order: match b.order {
                pm::FilterOrder::Second => core::generator::FilterOrder::Second,
                pm::FilterOrder::Fourth => core::generator::FilterOrder::Fourth,
            },
        },
    }
}

pub(crate) fn ess(s: pm::EssSpec) -> core::generator::EssConfig {
    core::generator::EssConfig {
        start_hz: s.start.0,
        end_hz: s.end.0,
        duration_s: s.duration.0,
        fade_in_s: s.fade_in.0,
        fade_out_s: s.fade_out.0,
    }
}

/// Generator signal; `None` for a negative or oversized period.
pub(crate) fn signal(s: pm::Signal) -> Option<core::generator::Signal> {
    use core::generator::Signal as S;
    Some(match s {
        pm::Signal::White => S::White,
        pm::Signal::Pink => S::Pink,
        pm::Signal::PeriodicPink { period } => S::PeriodicPink {
            period: usize::try_from(period.0).ok()?,
        },
        pm::Signal::Sine { freq } => S::Sine { freq_hz: freq.0 },
        pm::Signal::Ess { sweep } => S::Ess(ess(sweep)),
    })
}

pub(crate) fn device_selector(d: &pm::DeviceSelector) -> audio::DeviceSelector {
    match d {
        pm::DeviceSelector::Default => audio::DeviceSelector::Default,
        pm::DeviceSelector::Id { id } => audio::DeviceSelector::Id(audio::DeviceId(id.0.clone())),
    }
}

pub(crate) fn clock(c: audio::ClockRelation) -> pm::ClockRelation {
    match c {
        audio::ClockRelation::SingleCallback => pm::ClockRelation::SingleCallback,
        audio::ClockRelation::SameDeviceSeparateCallbacks => {
            pm::ClockRelation::SameDeviceSeparateCallbacks
        }
        audio::ClockRelation::Unknown => pm::ClockRelation::Unknown,
    }
}

fn direction(d: &audio::DirectionCaps) -> pm::DirectionInfo {
    pm::DirectionInfo {
        max_channels: d.max_channels,
        rates_hz: d
            .rates
            .iter()
            .map(|r| pm::RangeU32 {
                min: r.min,
                max: r.max,
            })
            .collect(),
        buffer_frames: d.buffer_frames.map(|b| pm::RangeU32 {
            min: b.min,
            max: b.max,
        }),
        default_rate_hz: d.default_rate,
        default_buffer_frames: d.default_buffer,
        channel_names: d.channel_names.clone(),
    }
}

pub(crate) fn backend_kind(k: audio::BackendKind) -> pm::BackendKind {
    match k {
        audio::BackendKind::Jack => pm::BackendKind::Jack,
        audio::BackendKind::Cpal => pm::BackendKind::Cpal,
        audio::BackendKind::Fake => pm::BackendKind::Fake,
        audio::BackendKind::Replay => pm::BackendKind::Replay,
    }
}

pub(crate) fn device_info(c: &audio::DeviceCaps) -> pm::DeviceInfo {
    pm::DeviceInfo {
        backend: backend_kind(c.backend),
        host: c.host.clone(),
        id: pm::DeviceId(c.id.0.clone()),
        name: c.name.clone(),
        input: c.input.as_ref().map(direction),
        output: c.output.as_ref().map(direction),
        duplex_clock: clock(c.duplex_clock),
        index: match c.index {
            audio::IndexExactness::Exact => pm::IndexExactness::Exact,
            audio::IndexExactness::Estimated => pm::IndexExactness::Estimated,
        },
        notes: c.notes.clone(),
    }
}

pub(crate) fn timing_state(s: core::timing::TimingState) -> pm::TimingState {
    use core::timing::TimingState as T;
    match s {
        T::NoStimulus => pm::TimingState::NoStimulus,
        T::Acquiring => pm::TimingState::Acquiring,
        T::Locked { offset } => pm::TimingState::Locked {
            offset: Samples(offset),
        },
        T::Jumped { from, to } => pm::TimingState::Jumped {
            from: Samples(from),
            to: Samples(to),
        },
        T::Lost => pm::TimingState::Lost,
    }
}

pub(crate) fn last_lock(l: core::timing::LastLock, at: WallNs) -> pm::LastLock {
    pm::LastLock {
        epoch: l.epoch,
        offset: Samples(l.offset),
        at_sample: ac2_proto::units::SampleIndex(l.at_capture_sample),
        at,
    }
}

/// MTW depth policy; `None` when the span cap is not a positive finite time.
pub(crate) fn depth(d: pm::DepthPolicy) -> Option<core::mtw::DepthPolicy> {
    match d {
        pm::DepthPolicy::EqualConfidence => Some(core::mtw::DepthPolicy::EqualConfidence),
        pm::DepthPolicy::FastLf { max_settle_s }
            if max_settle_s.0.is_finite() && max_settle_s.0 > 0.0 =>
        {
            Some(core::mtw::DepthPolicy::FastLf {
                max_settle_s: max_settle_s.0,
            })
        }
        pm::DepthPolicy::FastLf { .. } => None,
    }
}

/// A requested finder band: one band, or auto (full → mid → sub).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FindBand {
    Auto,
    Band(core::delay::Band),
}

pub(crate) fn finder_band(b: pm::FinderBand) -> FindBand {
    use core::delay::Band;
    match b {
        pm::FinderBand::Full => FindBand::Band(Band::FullRange),
        pm::FinderBand::Mid => FindBand::Band(Band::Mid),
        pm::FinderBand::Sub => FindBand::Band(Band::Sub),
        pm::FinderBand::Custom { lo_hz, hi_hz } => FindBand::Band(Band::Custom {
            lo_hz: lo_hz.0,
            hi_hz: hi_hz.0,
        }),
        pm::FinderBand::Auto => FindBand::Auto,
    }
}

fn delay_band(b: core::delay::Band) -> pm::DelayBand {
    use core::delay::Band;
    match b {
        Band::FullRange => pm::DelayBand::Full,
        Band::Mid => pm::DelayBand::Mid,
        Band::Sub => pm::DelayBand::Sub,
        Band::Custom { lo_hz, hi_hz } => pm::DelayBand::Custom {
            lo_hz: ac2_proto::units::Hz(lo_hz),
            hi_hz: ac2_proto::units::Hz(hi_hz),
        },
    }
}

fn arrival(a: &core::delay::Arrival, fs: f64) -> pm::DelayArrival {
    use ac2_proto::units::{Db, Degrees, Seconds};
    pm::DelayArrival {
        delay: Seconds(a.delay_frac / fs),
        delay_samples: a.delay_frac,
        level: Db(a.level_db),
        phase: Degrees(a.phase_deg),
        uncertainty_samples: a.uncertainty,
        misfit: a.misfit,
        refined: a.refined,
    }
}

fn no_estimate_reason(r: core::delay::NoEstimateReason) -> pm::NoEstimateReason {
    use core::delay::NoEstimateReason as R;
    match r {
        R::NoReference => pm::NoEstimateReason::NoReference,
        R::NoSignal => pm::NoEstimateReason::NoSignal,
        R::ObservationTooShort => pm::NoEstimateReason::ObservationTooShort,
        R::InsufficientOverlap => pm::NoEstimateReason::InsufficientOverlap,
        R::InsufficientExcitation => pm::NoEstimateReason::InsufficientExcitation,
        R::PeriodicExcitation { period } => pm::NoEstimateReason::PeriodicExcitation {
            period: Samples(i64::try_from(period).unwrap_or(i64::MAX)),
        },
        R::LowPsr => pm::NoEstimateReason::LowPsr,
        R::LowPrecision => pm::NoEstimateReason::LowPrecision,
        R::PeakAtSearchEdge => pm::NoEstimateReason::PeakAtSearchEdge,
        R::LowBandSnr => pm::NoEstimateReason::LowBandSnr,
    }
}

fn ambiguity_reason(r: core::delay::AmbiguityReason) -> pm::AmbiguityReason {
    use core::delay::AmbiguityReason as R;
    match r {
        R::BorderlineLevel => pm::AmbiguityReason::BorderlineLevel,
        R::CloseArrivals => pm::AmbiguityReason::CloseArrivals,
        R::MergedLobe => pm::AmbiguityReason::MergedLobe,
        R::OutsideRefinement => pm::AmbiguityReason::OutsideRefinement,
    }
}

/// Wire form of a finder result at sample rate `fs`. Values the finder did not reach before
/// refusing are NaN in the core record and nil on the wire.
pub(crate) fn delay_finding(
    r: &core::delay::FinderResult,
    fs: f64,
    found_at: WallNs,
) -> pm::DelayFinding {
    use ac2_proto::units::{Db, Seconds};
    use core::delay::Outcome;
    let finite = |v: f64| v.is_finite().then_some(v);
    let outcome = match &r.outcome {
        Outcome::Accepted { first, strongest } => pm::DelayOutcome::Accepted {
            first: arrival(first, fs),
            strongest: arrival(strongest, fs),
        },
        Outcome::Ambiguous {
            reasons,
            ranked,
            strongest,
        } => pm::DelayOutcome::Ambiguous {
            reasons: reasons.iter().copied().map(ambiguity_reason).collect(),
            ranked: ranked.iter().map(|a| arrival(a, fs)).collect(),
            strongest: arrival(strongest, fs),
        },
        Outcome::NoEstimate { reasons } => pm::DelayOutcome::NoEstimate {
            reasons: reasons.iter().copied().map(no_estimate_reason).collect(),
        },
    };
    let c = &r.confidence;
    pm::DelayFinding {
        outcome,
        confidence: pm::DelayConfidence {
            psr_db: finite(c.psr_db).map(Db),
            psr_acq_db: finite(c.psr_acq_db).map(Db),
            band_snr_db: finite(c.band_snr_db).map(Db),
            excited_fraction: finite(c.excited_fraction),
            uncertainty_samples: r.pick().and_then(|a| finite(a.uncertainty)),
            pulse_width_samples: finite(c.pulse_width),
            period: c
                .period
                .map(|p| Samples(i64::try_from(p).unwrap_or(i64::MAX))),
        },
        band: delay_band(r.band),
        observation: Seconds((r.meas_window.end - r.meas_window.start) as f64 / fs),
        candidates: r
            .candidates
            .iter()
            .take(pm::MAX_FINDING_CANDIDATES)
            .map(|a| arrival(a, fs))
            .collect(),
        found_at,
    }
}
