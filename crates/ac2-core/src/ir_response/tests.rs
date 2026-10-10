//! Analytic models: an impulse (flat, linear phase), an RBJ peaking biquad (its transfer
//! function evaluated in closed form), a direct sound plus one reflection (a comb that a
//! gate removes), a band-limited fractional delay (the peak located between samples).

use std::f64::consts::PI;

use super::*;

const FS: f64 = 48_000.0;

fn grid() -> LogGrid {
    LogGrid {
        ppo: 48,
        k_min: -240,
        k_max: 239,
    }
}

fn spec(window: IrWindow, align: Align) -> IrSpec {
    IrSpec {
        grid: grid(),
        window,
        align,
    }
}

fn impulse(len: usize, at: usize) -> Vec<f64> {
    let mut h = vec![0.0; len];
    h[at] = 1.0;
    h
}

fn wrap(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// RBJ cookbook peaking EQ: (b, a) normalised to a0 = 1.
fn peaking(f0: f64, gain_db: f64, q: f64) -> ([f64; 3], [f64; 3]) {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * f0 / FS;
    let alpha = w0.sin() / (2.0 * q);
    let a0 = 1.0 + alpha / a;
    (
        [
            (1.0 + alpha * a) / a0,
            -2.0 * w0.cos() / a0,
            (1.0 - alpha * a) / a0,
        ],
        [1.0, -2.0 * w0.cos() / a0, (1.0 - alpha / a) / a0],
    )
}

fn filter(b: [f64; 3], a: [f64; 3], x: &[f64]) -> Vec<f64> {
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    x.iter()
        .map(|&x0| {
            let y0 = b[0] * x0 + b[1] * x1 + b[2] * x2 - a[1] * y1 - a[2] * y2;
            (x2, x1, y2, y1) = (x1, x0, y1, y0);
            y0
        })
        .collect()
}

fn biquad_at(b: [f64; 3], a: [f64; 3], f: f64) -> Complex64 {
    let z1 = Complex64::from_polar(1.0, -2.0 * PI * f / FS);
    let z2 = z1 * z1;
    (b[0] + b[1] * z1 + b[2] * z2) / (a[0] + a[1] * z1 + a[2] * z2)
}

#[test]
fn an_impulse_is_flat_with_its_delay_removed_at_the_peak() {
    let h = impulse(4096, 300);
    let r = analyse(&h, FS, &spec(IrWindow::Whole, Align::Peak)).expect("response");
    assert!((r.peak_s * FS - 300.0).abs() < 1e-6, "{}", r.peak_s * FS);
    assert_eq!(r.delay_s, r.peak_s);
    for (i, f) in r.frequencies.iter().enumerate() {
        if *f > 0.45 * FS {
            continue;
        }
        assert!(
            r.magnitude_db[i].abs() < 1e-6,
            "{f} Hz: {}",
            r.magnitude_db[i]
        );
        assert!(r.phase_deg[i].abs() < 1e-3, "{f} Hz: {}", r.phase_deg[i]);
    }
    // Above Nyquist nothing is reported.
    let last = r
        .frequencies
        .iter()
        .position(|f| *f > 0.5 * FS)
        .expect("above");
    assert!(r.magnitude_db[last].is_nan());
}

#[test]
fn kept_at_the_file_start_the_phase_is_the_delay() {
    let h = impulse(4096, 300);
    let r = analyse(&h, FS, &spec(IrWindow::Whole, Align::Start)).expect("response");
    assert_eq!(r.delay_s, 0.0);
    for (i, f) in r.frequencies.iter().enumerate() {
        if *f > 10_000.0 {
            break;
        }
        // A column's complex mean of a linear phase is the phase at the column's arithmetic
        // centre (its edges are geometric about `f`).
        let (lo, hi) = grid().edges(i);
        let want = wrap(-360.0 * 0.5 * (lo + hi) * 300.0 / FS);
        assert!(
            wrap(r.phase_deg[i] - want).abs() < 0.01,
            "{f} Hz: {} vs {want}",
            r.phase_deg[i]
        );
    }
}

#[test]
fn a_biquad_reads_its_closed_form_response() {
    let (b, a) = peaking(1000.0, 6.0, 2.0);
    let lead = 64;
    let h = filter(b, a, &impulse(32_768, lead));
    let r = analyse(&h, FS, &spec(IrWindow::Whole, Align::Start)).expect("response");
    for (i, f) in r.frequencies.iter().enumerate() {
        if !(40.0..=15_000.0).contains(f) {
            continue;
        }
        let want =
            biquad_at(b, a, *f) * Complex64::from_polar(1.0, -2.0 * PI * f * lead as f64 / FS);
        let m = 20.0 * want.norm().log10();
        let p = want.arg().to_degrees();
        assert!(
            (r.magnitude_db[i] - m).abs() < 0.02,
            "{f} Hz: {} vs {m}",
            r.magnitude_db[i]
        );
        assert!(
            wrap(r.phase_deg[i] - p).abs() < 0.5,
            "{f} Hz: {} vs {p}",
            r.phase_deg[i]
        );
    }
    // The boost itself.
    let k = r
        .frequencies
        .iter()
        .position(|f| (*f - 1000.0).abs() < 1e-6)
        .expect("1 kHz");
    assert!(
        (r.magnitude_db[k] - 6.0).abs() < 0.02,
        "{}",
        r.magnitude_db[k]
    );
}

#[test]
fn a_gate_keeps_a_reflection_out_and_declares_its_cell() {
    // Direct sound at 10 ms, a −6 dB reflection 20 ms later.
    let mut h = impulse(48_000, 480);
    h[480 + 960] = 0.5;
    let whole = analyse(&h, FS, &spec(IrWindow::Whole, Align::Peak)).expect("whole");
    let ripple = |r: &IrResponse, band: std::ops::RangeInclusive<f64>| {
        r.frequencies
            .iter()
            .zip(&r.magnitude_db)
            .filter(|(f, _)| band.contains(*f))
            .fold((f64::MAX, f64::MIN), |(lo, hi), (_, m)| {
                (lo.min(*m), hi.max(*m))
            })
    };
    // Where the 1/48-octave columns are narrower than the comb's 50 Hz period it shows.
    let (lo, hi) = ripple(&whole, 200.0..=600.0);
    assert!(hi > 3.0 && lo < -4.0, "comb {lo} … {hi}");
    let gated = analyse(
        &h,
        FS,
        &spec(
            IrWindow::Gate {
                pre_s: 0.001,
                post_s: 0.01,
            },
            Align::Peak,
        ),
    )
    .expect("gated");
    let (lo, hi) = ripple(&gated, 1000.0..=10_000.0);
    assert!(lo > -0.01 && hi < 0.01, "gated {lo} … {hi}");
    assert!((gated.window_s.0 - 0.009).abs() < 1e-9);
    assert!((gated.window_s.1 - 0.011).abs() < 1e-9);
    assert!((gated.cell_hz - FS / 528.0).abs() < 1e-9);
    // Unresolved from the bottom of the grid up to κ·cell.
    let edge = crate::resolution::resolved_from_hz(gated.cell_hz, 48);
    let (lo, hi) = gated.unresolved[0];
    assert!(
        lo < 32.0 && (hi / edge - 1.0).abs() < 0.03,
        "{lo} … {hi} vs {edge}"
    );
    // The whole file's cell is one hertz: everything above 31 Hz is resolved there.
    assert!((whole.cell_hz - 1.0).abs() < 1e-12);
}

#[test]
fn the_peak_is_found_between_samples() {
    // A band-limited impulse 0.3 sample after sample 1000 (Hann-windowed sinc).
    let at = 1000.3;
    let h: Vec<f64> = (0..4096)
        .map(|i| {
            let x = i as f64 - at;
            let w = if x.abs() < 400.0 {
                0.5 + 0.5 * (PI * x / 400.0).cos()
            } else {
                0.0
            };
            let s = if x == 0.0 {
                1.0
            } else {
                (PI * x).sin() / (PI * x)
            };
            0.9 * s * w
        })
        .collect();
    let r = analyse(&h, FS, &spec(IrWindow::Whole, Align::Peak)).expect("response");
    assert!((r.peak_s * FS - at).abs() < 0.01, "{}", r.peak_s * FS);
    // Its delay removed, the phase is flat in band.
    for (i, f) in r.frequencies.iter().enumerate() {
        if (100.0..=10_000.0).contains(f) {
            assert!(r.phase_deg[i].abs() < 1.0, "{f} Hz: {}", r.phase_deg[i]);
        }
    }
}

#[test]
fn the_envelope_peaks_at_the_arrival() {
    let h = impulse(2048, 100);
    let r = analyse(&h, FS, &spec(IrWindow::Whole, Align::Peak)).expect("response");
    assert_eq!(r.etc_db.len(), 2048);
    let top = r
        .etc_db
        .iter()
        .enumerate()
        .fold(0, |b, (i, v)| if *v > r.etc_db[b] { i } else { b });
    assert_eq!(top, 100);
    assert!(r.etc_db[100].abs() < 1e-9);
}

#[test]
fn refusals() {
    let s = spec(IrWindow::Whole, Align::Peak);
    assert_eq!(analyse(&[1.0], FS, &s), Err(IrResponseError::TooShort));
    assert_eq!(analyse(&[0.0; 8], FS, &s), Err(IrResponseError::Silent));
    assert_eq!(
        analyse(&[1.0, f64::NAN], FS, &s),
        Err(IrResponseError::NonFinite)
    );
    assert_eq!(
        analyse(&[1.0, 0.0], 0.0, &s),
        Err(IrResponseError::BadSampleRate)
    );
    let g = spec(
        IrWindow::Gate {
            pre_s: 0.0,
            post_s: 0.0,
        },
        Align::Peak,
    );
    assert_eq!(analyse(&[1.0, 0.0], FS, &g), Err(IrResponseError::BadGate));
}
