//! The kernel-length table in the crate docs: worst error per half-length and beta with
//! phases fine enough (2¹⁴: interpolation error under −170 dB at 20 kHz) that only the window
//! design shows, and the preset's 2048 phases beside it.

use std::sync::Arc;

use ac2_resampler::{Kernel, KernelSpec};

use crate::quality::{sweep, top_octave};

#[test]
fn slow_design_table_half_length_and_beta() {
    // The transition band and the passband ripple both peak in the top octave.
    let freqs: Vec<f64> = top_octave().collect();
    let steps = [1.0, 1.0 + 1e-3, 1.0 - 1e-3];
    let mut rows = Vec::new();
    for (half_length, beta, phases) in [
        (24, 12.0, 1 << 14),
        (26, 13.0, 1 << 14),
        (28, 14.5, 1 << 14),
        (32, 16.0, 1 << 14),
        (28, 14.5, 2048),
    ] {
        let spec = KernelSpec {
            half_length,
            phases,
            beta,
        };
        let w = sweep(
            &Arc::new(Kernel::new(spec).unwrap()),
            &freqs,
            &steps,
            8,
            1024,
        );
        rows.push((spec, w.error_db, w.at_hz));
    }
    // Linear interpolation between phases errs by up to (ω/phases)²/8 of the tone.
    let w = 2.0 * std::f64::consts::PI * 20_000.0 / 48_000.0;
    for phases in [1024.0, 2048.0] {
        let db = 20.0 * ((w / phases).powi(2) / 8.0).log10();
        println!("(ω/phases)²/8 at 20 kHz, {phases} phases: {db:.1} dB");
    }
    println!("| half-length (taps) | beta | phases | worst error |");
    for (s, db, hz) in &rows {
        println!(
            "| {} ({}) | {} | {} | {db:.1} dB at {hz:.0} Hz |",
            s.half_length,
            2 * s.half_length,
            s.beta,
            s.phases
        );
    }
}
