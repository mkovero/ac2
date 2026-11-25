//! Measurement-grade fractional resampling for ratios very near 1: re-timing a capture whose
//! crystal runs a few ppm off onto another clock's timeline. Pure math: no I/O, no threads.
//!
//! The caller says where each output falls in input-sample time — a [`Pos`] for the first
//! output and a [`Step`] per output — rather than giving a ratio, so the position never
//! drifts from what the caller (a clock servo) intends: position arithmetic is exact 64.64
//! fixed point.
//!
//! ```
//! use std::sync::Arc;
//! use ac2_resampler::{Kernel, Pos, Resampler, Step};
//!
//! let mut r = Resampler::new(Arc::new(Kernel::measurement()), 512);
//! r.push(&[0.25; 512]);
//! let step = Step::new(1.0 + 4e-6)?; // the input clock runs 4 ppm fast
//! let mut pos = Pos::new(r.kernel().history(), 0.5)?;
//! let mut out = vec![0.0; r.available(pos, step)];
//! pos = r.pull(pos, step, &mut out)?;
//! assert!(out.iter().all(|&y| y == 0.25));
//! # Ok::<(), ac2_resampler::Error>(())
//! ```
//!
//! # The measurement kernel
//!
//! [`Kernel::measurement`] is a Kaiser-windowed sinc with cutoff at Nyquist — the only cutoff
//! for which an integer delay is a unit impulse, so outputs on input samples are bit-exact.
//! What it must deliver, at 48 kHz for tones 20 Hz – 20 kHz, any offset in [0, 1) and steps
//! up to 1 ± 1e-3: error against the ideal re-timed sine (amplitude and phase together)
//! at or below −110 dB re the tone, passband within ±0.001 dB.
//!
//! The 20 kHz edge sets the length: the window's transition band must fit in the 4 kHz
//! between the passband edge and Nyquist (and its mirror above). Worst peak error, any
//! offset and step, measured against analytic sines:
//!
//! | half-length (taps) | beta | error, phases → ∞ |
//! |---|---|---|
//! | 24 (48) | 12   | −112 dB |
//! | 26 (52) | 13   | −121 dB |
//! | 28 (56) | 14.5 | −134 dB |
//! | 32 (64) | 16   | −142 dB (f32 floor) |
//!
//! Between tabulated phases the coefficients are interpolated linearly, whose error grows as
//! (ω/phases)²/8 — at 20 kHz −121 dB for 1024 phases, −131 dB for 2048. Cubic interpolation
//! would allow fewer phases but costs two more multiply-adds per tap; the table (2049 × 56
//! f64, 0.9 MB, shared between channels) is cheaper than that.
//!
//! So the preset is **half-length 28 (56 taps), beta 14.5, 2048 phases**: worst −131.6 dB
//! (near 16–20 kHz, every step), gain within 2e-6 dB, phase within 4e-6° (the
//! `slow_tones_meet_targets_20_hz_to_20_khz` test prints the table). Below 10 kHz the error
//! is about −141 dB, the floor set by f32: an exact copy of an f32-rounded sine already
//! errs by −150.5 dB, and the kernel sums 56 such rounded inputs. Between 20 kHz and Nyquist
//! the response falls off; a tone there is attenuated, never aliased far, since a ratio this
//! near 1 maps every frequency onto itself within 0.1 %.
#![forbid(unsafe_code)]

mod kernel;
mod pos;
mod resampler;

pub use kernel::{Kernel, KernelSpec};
pub use pos::{Pos, Step};
pub use resampler::Resampler;

/// Why a kernel, step, position or pull was refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Error {
    /// Half-length 0 or over 1024, phases not a power of two in 2..=65536, or beta outside
    /// 0..=50.
    KernelSpec(KernelSpec),
    /// A step outside 1 ± [`Step::MAX_DEVIATION`].
    StepOutOfRange { step: f64 },
    /// A fraction outside [0, 1).
    FractionOutOfRange { frac: f64 },
    /// The input pushed so far covers only `available` of the `requested` outputs.
    InputShort { requested: usize, available: usize },
    /// The first output needs input from before `oldest`, the oldest sample retained (or
    /// before index 0).
    BeforeHistory { pos: Pos, oldest: u64 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KernelSpec(s) => write!(
                f,
                "kernel spec refused: half-length {}, phases {}, beta {}",
                s.half_length, s.phases, s.beta
            ),
            Self::StepOutOfRange { step } => {
                write!(f, "step {step} is outside 1 ± {}", Step::MAX_DEVIATION)
            }
            Self::FractionOutOfRange { frac } => write!(f, "fraction {frac} is outside [0, 1)"),
            Self::InputShort {
                requested,
                available,
            } => {
                write!(f, "{requested} outputs requested, input covers {available}")
            }
            Self::BeforeHistory { pos, oldest } => write!(
                f,
                "position {}+{} needs input before the oldest retained sample {oldest}",
                pos.index(),
                pos.fraction()
            ),
        }
    }
}

impl std::error::Error for Error {}
