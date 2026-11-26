//! The servo's settings and the checks that refuse a configuration it cannot run.

use super::filter::{Filter, Model, RATE, THETA, WANDER};

/// Noise figures and loop settings; [`Default`] gives the figures measured on the rigs (see
/// the module docs for where each comes from).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServoConfig {
    /// White part of the stamp error: the sd of a one-second mean of stamps, seconds.
    pub stamp_white_s: f64,
    /// Stamp-path wander: stationary sd, seconds.
    pub wander_sd_s: f64,
    /// Stamp-path wander: correlation time, seconds.
    pub wander_tau_s: f64,
    /// Crystal rate random walk: sd after one hour, ppm.
    pub rate_walk_ppm_per_h: f64,
    /// A stamp whose innovation lies beyond this many sd counts as far as one this many sd
    /// out (Huber): its measurement variance is inflated, not dropped, so one wild stamp
    /// moves little while a real step in the stamps is still followed over a few stamps.
    pub stamp_gate: f64,
    /// Audio path random walk: sd after one hour, seconds.
    pub path_walk_s_per_h: f64,
    /// Fast path jitter: white, per audio observation, seconds. Added to each observation's
    /// own `sigma_s` (which is the delay tracker's uncertainty only) as the air itself moves.
    pub path_jitter_s: f64,
    /// Time constant with which handed-out positions close on the estimate, seconds.
    pub steer_s: f64,
    /// Audio innovation beyond this many sd re-initialises the bias instead of moving the
    /// phase.
    pub audio_gate: f64,
    /// Audio counts as contributing this long after its last accepted observation, seconds.
    pub audio_hold_s: f64,
    /// The status says [`Mode::Coasting`](super::Mode::Coasting) once the newest stamp is
    /// this old, seconds.
    pub coast_after_s: f64,
    /// Acquired when the phase sd is below this, seconds …
    pub acquire_phase_s: f64,
    /// … the rate sd below this, ppm …
    pub acquire_rate_ppm: f64,
    /// … and stamps have spanned at least this long since the (re)start, seconds.
    pub acquire_min_s: f64,
}

impl Default for ServoConfig {
    fn default() -> Self {
        Self {
            stamp_white_s: 5e-6,
            wander_sd_s: 20e-6,
            wander_tau_s: 100.0,
            rate_walk_ppm_per_h: 0.1,
            stamp_gate: 4.0,
            path_walk_s_per_h: 50e-6,
            path_jitter_s: 5e-6,
            steer_s: 60.0,
            audio_gate: 5.0,
            audio_hold_s: 5.0,
            coast_after_s: 3.0,
            acquire_phase_s: 100e-6,
            acquire_rate_ppm: 1.0,
            acquire_min_s: 1.0,
        }
    }
}

/// Why a [`ServoConfig`] (or the nominal rate) was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadConfig {
    /// Zero, or above [`Servo::MAX_NOMINAL_RATE`](super::Servo::MAX_NOMINAL_RATE).
    NominalRate,
    StampWhite,
    WanderSd,
    WanderTau,
    RateWalk,
    StampGate,
    PathWalk,
    PathJitter,
    Steer,
    AudioGate,
    AudioHold,
    CoastAfter,
    AcquirePhase,
    AcquireRate,
    AcquireMin,
    /// The stamps as configured never bring the phase sd under `acquire_phase_s`: the
    /// servo would stay [`Acquiring`](super::Mode::Acquiring) for ever.
    AcquirePhaseUnreachable,
    /// The same for the rate and `acquire_rate_ppm`.
    AcquireRateUnreachable,
}

impl std::fmt::Display for BadConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NominalRate => write!(
                f,
                "nominal rate must be from 1 to {} Hz",
                super::Servo::MAX_NOMINAL_RATE
            ),
            Self::AcquirePhaseUnreachable => write!(
                f,
                "with this stamp noise the phase sd never falls under acquire_phase_s"
            ),
            Self::AcquireRateUnreachable => write!(
                f,
                "with this stamp noise the rate sd never falls under acquire_rate_ppm"
            ),
            other => write!(f, "servo setting {other:?} must be finite and positive"),
        }
    }
}

impl std::error::Error for BadConfig {}

impl ServoConfig {
    /// For a device on another host, stamped through both hosts' system clocks: one stamp a
    /// second, each the mean of a second of the remote host's capture times placed on this
    /// host's sample timeline.
    ///
    /// - `wander_sd_s` 33 µs: the wander of both stamp paths and of the two system clocks
    ///   against each other add as independent errors, √(15.5² + 27.8² + 9²) µs (the
    ///   hardware-PTP and software-PTP hosts' stamp wander, and their system clocks' 9 µs sd
    ///   apart over an hour).
    /// - `stamp_white_s` 7 µs: each host's one-second white part (5 µs) once per host, √2 × 5.
    /// - `acquire_min_s` 0.9 s: two one-second stamps span 1 s give or take one remote period,
    ///   so at 1 s exactly half the acquisitions would wait for a third stamp.
    ///
    /// The rest are the defaults. The larger wander slows acquisition: the rate's sd falls
    /// about as σ√(2(1 − e^(−T/τ)))/T over a span T of stamps (σ the wander, τ its
    /// correlation time), under 1 ppm (acquired) after 23 s of stamps a second apart instead of the
    /// default's 11 s at ten a second.
    pub fn cross_rig() -> Self {
        Self {
            wander_sd_s: 33e-6,
            stamp_white_s: 7e-6,
            acquire_min_s: 0.9,
            ..Self::default()
        }
    }

    pub(super) fn check(&self) -> Result<(), BadConfig> {
        use BadConfig as B;
        for (v, e) in [
            (self.stamp_white_s, B::StampWhite),
            (self.wander_sd_s, B::WanderSd),
            (self.wander_tau_s, B::WanderTau),
            (self.rate_walk_ppm_per_h, B::RateWalk),
            (self.stamp_gate, B::StampGate),
            (self.path_walk_s_per_h, B::PathWalk),
            (self.path_jitter_s, B::PathJitter),
            (self.steer_s, B::Steer),
            (self.audio_gate, B::AudioGate),
            (self.audio_hold_s, B::AudioHold),
            (self.coast_after_s, B::CoastAfter),
            (self.acquire_phase_s, B::AcquirePhase),
            (self.acquire_rate_ppm, B::AcquireRate),
            (self.acquire_min_s, B::AcquireMin),
        ] {
            if !(v.is_finite() && v > 0.0) {
                return Err(e);
            }
        }
        let (phase, rate) = self.steady_sd();
        if phase >= self.acquire_phase_s {
            return Err(B::AcquirePhaseUnreachable);
        }
        if rate * 1e6 >= self.acquire_rate_ppm {
            return Err(B::AcquireRateUnreachable);
        }
        Ok(())
    }

    /// The filter's process noise in device samples at nominal rate `fs`; at `fs` = 1 in
    /// seconds, which is how the acquisition check runs: the filter is linear in its units.
    pub(super) fn model(&self, fs: f64) -> Model {
        Model {
            q_rate: (self.rate_walk_ppm_per_h * 1e-6 * fs).powi(2) / 3600.0,
            wander_var: (self.wander_sd_s * fs).powi(2),
            wander_tau: self.wander_tau_s,
            q_bias: (self.path_walk_s_per_h * fs).powi(2) / 3600.0,
        }
    }

    /// The phase sd (seconds) and rate sd (fraction) the stamps alone settle to, right after
    /// a stamp: the stamp white noise taken as one-second means, so the figure does not
    /// depend on how densely the caller stamps. The best the acquisition bounds can ever
    /// see.
    pub(super) fn steady_sd(&self) -> (f64, f64) {
        let model = self.model(1.0);
        let mut f = Filter {
            x: [0.0; 4],
            p: [[0.0; 4]; 4],
        };
        f.p[THETA][THETA] = model.wander_var + 1.0;
        f.p[THETA][WANDER] = -model.wander_var;
        f.p[WANDER][THETA] = -model.wander_var;
        f.p[WANDER][WANDER] = model.wander_var;
        f.p[RATE][RATE] = 1e-4;
        let h = [1.0, 0.0, 1.0, 0.0];
        let r = self.stamp_white_s.powi(2);
        let mut last = f64::INFINITY;
        // The Riccati recursion converges geometrically; a few correlation times or rate
        // walk time constants settle it, and the cap only bounds pathological settings.
        for _ in 0..20_000 {
            f.predict(1.0, &model);
            f.update(&h, 0.0, r);
            let now = f.p[THETA][THETA];
            if (now - last).abs() <= 1e-9 * now {
                break;
            }
            last = now;
        }
        (f.p[THETA][THETA].sqrt(), f.p[RATE][RATE].sqrt())
    }
}
