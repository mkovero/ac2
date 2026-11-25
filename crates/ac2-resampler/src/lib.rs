//! Measurement-grade fractional resampling for ratios very near 1: re-timing a capture whose
//! crystal runs a few ppm off onto another clock's timeline. Pure math: no I/O, no threads.
//!
//! The caller says where each output falls in input-sample time — a [`Pos`] for the first
//! output and a [`Step`] per output — rather than giving a ratio. Position arithmetic is
//! 64.64 fixed point: a position holds 2⁻⁶⁴ of a sample at any index and stepping it is
//! integer addition, so `n` steps land exactly on `pos + n·step` however the pulls are split.
//! What the step itself resolves depends on how it is built: [`Step::new`] keeps its `f64`
//! exactly (2⁻⁵² near 1), [`Step::from_deviation`] resolves 2⁻⁶⁴.
//!
//! One thread, not the audio callback, pushes input and pulls output; see [`Resampler`].
//!
//! ```
//! use std::sync::Arc;
//! use ac2_resampler::{Kernel, Pos, Resampler, Step};
//!
//! let mut r = Resampler::new(Arc::new(Kernel::measurement()), 512, 0)?;
//! r.push(&[0.25; 512]);
//! let step = Step::new(1.0 + 4e-6)?; // the input clock runs 4 ppm fast
//! let mut pos = Pos::new(r.kernel().history(), 0.5)?;
//! let mut out = vec![0.0; r.available(pos, step)?];
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
//! between the passband edge and Nyquist (and its mirror above). Worst peak error over
//! 10–20 kHz in 100 Hz steps, eight offsets, steps 1 and 1 ± 1e-3, against analytic sines,
//! with 2¹⁴ phases so that interpolation (below) plays no part
//! (`slow_design_table_half_length_and_beta` prints it):
//!
//! | half-length (taps) | beta | worst error |
//! |---|---|---|
//! | 24 (48) | 12   | −111.7 dB at 20.0 kHz |
//! | 26 (52) | 13   | −120.8 dB at 19.3 kHz |
//! | 28 (56) | 14.5 | −133.0 dB at 19.7 kHz |
//! | 32 (64) | 16   | −140.7 dB at 19.9 kHz, the f32 floor |
//!
//! Between tabulated phases the coefficients are interpolated linearly, whose error grows as
//! (ω/phases)²/8 — at 20 kHz −121.8 dB for 1024 phases, −133.8 dB for 2048. Cubic
//! interpolation would allow fewer phases but costs two more multiply-adds per tap; the table
//! (2049 × 56 f64, 0.9 MB, shared between channels) is cheaper than that.
//!
//! So the preset is **half-length 28 (56 taps), beta 14.5, 2048 phases**. At 2048 phases the
//! interpolation error near 20 kHz is as large as the window's own, and the two add: the
//! worst is −127.7 dB at 19.7 kHz, where the window alone gives −133.0 dB. Over 20 Hz – 20
//! kHz and every step, gain is within 2.9e-6 dB and phase within 6.1e-6°
//! (`slow_tones_meet_targets_20_hz_to_20_khz` prints the table; the error ripples with
//! frequency at a period of a few hundred hertz, so its grid is 100 Hz from 10 kHz up).
//!
//! Up to 8.1 kHz the error is −139.1 dB or better, near the floor f32 data sets: each input
//! already errs by up to half an f32 step, 2⁻²⁵ (−150.5 dB), the kernel sums 56 of them, and
//! the output is rounded to f32 again; at 1 kHz and below that floor measures −140.9 dB
//! (`f32_floor_through_the_tap_sum`). Between 20 kHz and Nyquist the response falls off; a
//! tone there is attenuated, never aliased far, since a ratio this near 1 maps every
//! frequency onto itself within 0.1 %.
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
    /// The spec's table would take `bytes`, over [`Kernel::MAX_TABLE_BYTES`].
    TableTooLarge { spec: KernelSpec, bytes: usize },
    /// `max_block + max_backlog + taps` overflows or exceeds [`Resampler::MAX_RETAINED`].
    RetentionTooLarge {
        max_block: usize,
        max_backlog: usize,
    },
    /// A step outside 1 ± [`Step::MAX_DEVIATION`].
    StepOutOfRange { step: f64 },
    /// A deviation from 1 outside ±[`Step::MAX_DEVIATION`].
    DeviationOutOfRange { deviation: f64 },
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
            Self::TableTooLarge { spec, bytes } => write!(
                f,
                "kernel table of {bytes} bytes (half-length {}, phases {}) is over {}",
                spec.half_length,
                spec.phases,
                Kernel::MAX_TABLE_BYTES
            ),
            Self::RetentionTooLarge {
                max_block,
                max_backlog,
            } => write!(
                f,
                "block {max_block} plus backlog {max_backlog} retains over {} samples",
                Resampler::MAX_RETAINED
            ),
            Self::DeviationOutOfRange { deviation } => {
                write!(
                    f,
                    "deviation {deviation} is outside ±{}",
                    Step::MAX_DEVIATION
                )
            }
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
