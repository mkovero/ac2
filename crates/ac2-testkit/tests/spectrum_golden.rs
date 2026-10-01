//! End-to-end harness check: recompute the Hann amplitude spectrum and PSD of the
//! `spectrum_hann_tone_noise` golden set in Rust and compare against refgen's output.
//!
//! This is a self-contained reference computation for the harness, not ac2-core's DSP.

use ac2_testkit::compare::{amplitude_to_db, power_to_db};
use ac2_testkit::{GoldenSet, Tolerance};
use realfft::RealFftPlanner;

const SET: &str = "spectrum_hann_tone_noise";

fn param_f64(set: &GoldenSet, name: &str) -> f64 {
    set.parameter(name)
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("{SET}: numeric parameter {name} missing"))
}

/// Periodic Hann, w[i] = 0.5 − 0.5·cos(2πi/N): periodic so that a bin-centred tone spans
/// an integer number of window periods and the coherent gain is exactly 1/2.
fn periodic_hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())
        .collect()
}

struct Spectra {
    freq_hz: Vec<f64>,
    amplitude_rms: Vec<f64>,
    psd: Vec<f64>,
}

fn spectra(x: &[f64], w: &[f64], fs: f64) -> Spectra {
    let n = x.len();
    let mut input: Vec<f64> = x.iter().zip(w).map(|(x, w)| x * w).collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut out = fft.make_output_vec();
    fft.process(&mut input, &mut out)
        .expect("fft lengths match");

    let s1: f64 = w.iter().sum();
    let s2: f64 = w.iter().map(|w| w * w).sum();
    let nyquist = n / 2;
    // DC and Nyquist have no negative-frequency image, so they are not doubled.
    let fold = |k: usize| -> f64 {
        if k == 0 || (n.is_multiple_of(2) && k == nyquist) {
            1.0
        } else {
            2.0
        }
    };
    Spectra {
        freq_hz: (0..out.len()).map(|k| k as f64 * fs / n as f64).collect(),
        amplitude_rms: out
            .iter()
            .enumerate()
            .map(|(k, c)| fold(k).sqrt() * c.norm() / s1)
            .collect(),
        psd: out
            .iter()
            .enumerate()
            .map(|(k, c)| fold(k) * c.norm_sqr() / (fs * s2))
            .collect(),
    }
}

#[test]
fn hann_amplitude_spectrum_and_psd_match_refgen() {
    let set = GoldenSet::load(SET).unwrap_or_else(|e| panic!("{e}"));
    let fs = param_f64(&set, "fs_hz");
    let x = set.f64("x").expect("x");
    assert_eq!(x.len() as f64, param_f64(&set, "n"));

    let w = periodic_hann(x.len());
    set.assert_f64("window", &w);

    let s = spectra(&x, &w, fs);
    set.assert_f64("freq_hz", &s.freq_hz);
    set.assert_f64("amplitude_rms", &s.amplitude_rms);
    set.assert_f64("psd", &s.psd);

    let amp_dbfs: Vec<f64> = s
        .amplitude_rms
        .iter()
        .map(|a| amplitude_to_db(a * std::f64::consts::SQRT_2))
        .collect();
    set.assert_f64("amplitude_dbfs", &amp_dbfs);
    let psd_db: Vec<f64> = s.psd.iter().map(|&p| power_to_db(p)).collect();
    set.assert_f64("psd_db", &psd_db);

    // The bin-centred tone reads its RMS (decision 4a: 0 dBFS = full-scale sine).
    let k = param_f64(&set, "tone_bin") as usize;
    let tone_rms = set.scalar("tone_rms_expected_fs").expect("scalar");
    let noise_rms = param_f64(&set, "noise_rms_fs");
    ac2_testkit::assert_ok(ac2_testkit::Comparison::new("tone bin RMS").close_f64(
        &[tone_rms],
        &[s.amplitude_rms[k]],
        // Noise adds at most a few noise-RMS-per-bin units to the tone bin.
        Tolerance::abs(10.0 * noise_rms * (1.5 / x.len() as f64).sqrt()),
    ));
}

#[test]
fn harness_detects_a_wrong_normalisation() {
    let set = GoldenSet::load(SET).unwrap_or_else(|e| panic!("{e}"));
    let fs = param_f64(&set, "fs_hz");
    let x = set.f64("x").expect("x");
    let w = periodic_hann(x.len());
    let s = spectra(&x, &w, fs);
    // Doubling DC and Nyquist like interior bins is a classic one-sided mistake.
    let mut wrong = s.psd.clone();
    wrong[0] *= 2.0;
    let last = wrong.len() - 1;
    wrong[last] *= 2.0;
    let mismatch = set
        .compare_f64("psd", &wrong)
        .expect("comparable")
        .expect_err("doubled DC/Nyquist must be caught");
    assert_eq!(mismatch.failed, 2);
    let msg = mismatch.to_string();
    assert!(msg.contains("[0] at 0 Hz"), "{msg}");
    assert!(msg.contains(&format!("[{last}] at 24000 Hz")), "{msg}");
}
