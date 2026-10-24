//! Programme meters against the refgen vectors (`tools/refgen/sets/ppm.py`, parity) and
//! against the IEC 60268-10 burst table, the VU settling and the BS.1770 true-peak claims
//! (acceptance), as separate tests so a failure names the contract that broke.

use ac2_core::ppm::{QuasiPeak, QuasiPeakSpec, TRUE_PEAK_PHASES, TruePeak, Vu, dbtp};
use ac2_core::protection::peak_dbfs;
use ac2_testkit::{Comparison, DbTolerance, GoldenSet, Tolerance, assert_ok};

const FS: f64 = 48000.0;
const BURST_HZ: f64 = 5000.0;
const PARITY_DB: DbTolerance = DbTolerance {
    db: 0.05,
    floor_db: -80.0,
};

fn f32s(v: &[f64]) -> Vec<f32> {
    v.iter().map(|&x| x as f32).collect()
}

fn db(v: &[f32]) -> Vec<f64> {
    v.iter().map(|&x| peak_dbfs(f64::from(x))).collect()
}

/// The sequence of a type from the golden primitives: zeros with the table bursts (each a
/// prefix of the 100 ms burst) at their segment starts.
fn sequence(g: &GoldenSet, kind: &str) -> Vec<f32> {
    let burst = f32s(&g.f64("burst_100ms").expect("burst"));
    let n = g.scalar(&format!("{kind}_input_samples")).expect("len") as usize;
    let seg = g.f64(&format!("{kind}_segments")).expect("segments");
    let mut x = vec![0.0f32; n];
    for s in seg.chunks(2) {
        let (start, len) = (s[0] as usize, s[1] as usize);
        x[start..start + len].copy_from_slice(&burst[..len]);
    }
    x
}

fn block_maxima(spec: &QuasiPeakSpec, x: &[f32], block: usize) -> Vec<f32> {
    let mut d = QuasiPeak::new(FS, spec);
    x.chunks(block).map(|b| d.process(b)).collect()
}

fn sine(fs: f64, f0: f64, amp: f64, n: usize) -> Vec<f32> {
    (0..n)
        .map(|k| (amp * (std::f64::consts::TAU * f0 * k as f64 / fs).sin()) as f32)
        .collect()
}

/// Reading of a burst of `ms` relative to the steady tone, in dB, as the table defines it.
fn table_reading(spec: &QuasiPeakSpec, ms: f64) -> f64 {
    table_reading_at(FS, spec, ms)
}

fn table_reading_at(fs: f64, spec: &QuasiPeakSpec, ms: f64) -> f64 {
    let steady = QuasiPeak::new(fs, spec).process(&sine(fs, BURST_HZ, 1.0, (0.4 * fs) as usize));
    let nb = (ms * 1e-3 * fs).round() as usize;
    let burst = QuasiPeak::new(fs, spec).process(&sine(fs, BURST_HZ, 1.0, nb));
    20.0 * (f64::from(burst) / f64::from(steady)).log10()
}

fn parity(kind: &str, spec: &QuasiPeakSpec) {
    let g = GoldenSet::load("ppm_quasi_peak").expect("golden");
    let x = sequence(&g, kind);
    let got = block_maxima(spec, &x, 48);
    let want = g.f64(&format!("{kind}_block_max")).expect("block_max");
    assert_ok(Comparison::new(&format!("{kind} block maxima")).close_db(
        &want.iter().map(|&v| peak_dbfs(v)).collect::<Vec<_>>(),
        &db(&got),
        PARITY_DB,
    ));
    let rows = g.f64(&format!("{kind}_burst_ms")).expect("rows");
    let want = g.f64(&format!("{kind}_reading_db")).expect("reading_db");
    let got: Vec<f64> = rows.iter().map(|&ms| table_reading(spec, ms)).collect();
    assert_ok(
        Comparison::new(&format!("{kind} table readings")).close_f64(
            &want,
            &got,
            Tolerance::abs(0.05),
        ),
    );
}

#[test]
fn quasi_peak_type1_matches_golden() {
    parity("type1", &QuasiPeakSpec::TYPE1);
}

#[test]
fn quasi_peak_type2_matches_golden() {
    parity("type2", &QuasiPeakSpec::TYPE2);
}

const TABLE1: [(f64, f64); 5] = [
    (100.0, 0.0),
    (10.0, -1.0),
    (5.0, -2.0),
    (3.0, -4.0),
    (0.4, -15.0),
];
const TABLE2: [(f64, f64); 5] = [
    (100.0, 0.0),
    (10.0, -2.0),
    (5.0, -4.0),
    (1.5, -9.0),
    (0.5, -17.0),
];

/// The table rows literally: ±1 dB on 100, 10 and 5 ms, ±2 dB on the shorter bursts.
fn table(fs: f64, spec: &QuasiPeakSpec, rows: &[(f64, f64)]) {
    for &(ms, want) in rows {
        let got = table_reading_at(fs, spec, ms);
        let tol = if ms >= 5.0 { 1.0 } else { 2.0 };
        assert!(
            (got - want).abs() <= tol,
            "{ms} ms burst at {fs} Hz reads {got:.2} dB, table {want} ± {tol} dB"
        );
    }
}

#[test]
fn quasi_peak_type1_burst_table() {
    table(FS, &QuasiPeakSpec::TYPE1, &TABLE1);
}

#[test]
fn quasi_peak_type2_burst_table() {
    table(FS, &QuasiPeakSpec::TYPE2, &TABLE2);
}

/// The constants were fitted at 48 kHz; the per-sample coefficients follow the rate, so the
/// table must hold at the other device rates too.
#[test]
fn quasi_peak_burst_table_holds_at_other_rates() {
    for fs in [44100.0, 96000.0] {
        table(fs, &QuasiPeakSpec::TYPE1, &TABLE1);
        table(fs, &QuasiPeakSpec::TYPE2, &TABLE2);
    }
}

/// Time for the reading to fall `fall_db` after a steady tone stops.
fn return_time(spec: &QuasiPeakSpec, fall_db: f64) -> f64 {
    let mut d = QuasiPeak::new(FS, spec);
    let top = f64::from(d.process(&sine(FS, BURST_HZ, 1.0, (0.4 * FS) as usize)));
    let silence = [0.0f32; 48];
    let mut blocks = 0;
    while peak_dbfs(f64::from(d.reading())) > peak_dbfs(top) - fall_db {
        d.process(&silence);
        blocks += 1;
        assert!(blocks < 10_000, "never returned");
    }
    blocks as f64 * 48.0 / FS
}

#[test]
fn quasi_peak_return_times() {
    let t1 = return_time(&QuasiPeakSpec::TYPE1, 20.0);
    assert!(
        (t1 - 1.7).abs() <= 0.3,
        "Type I: 20 dB in {t1:.3} s, want 1.7 ± 0.3"
    );
    let t2 = return_time(&QuasiPeakSpec::TYPE2, 24.0);
    assert!(
        (t2 - 2.8).abs() <= 0.3,
        "Type II: 24 dB in {t2:.3} s, want 2.8 ± 0.3"
    );
}

#[test]
fn quasi_peak_steady_sine_reads_its_peak() {
    for spec in [&QuasiPeakSpec::TYPE1, &QuasiPeakSpec::TYPE2] {
        for fs in [44100.0, 48000.0, 96000.0] {
            for f0 in [1000.0, 5000.0, 10000.0] {
                let mut d = QuasiPeak::new(fs, spec);
                d.process(&sine(fs, f0, 0.5, (0.5 * fs) as usize));
                let got = peak_dbfs(f64::from(d.process(&sine(
                    fs,
                    f0,
                    0.5,
                    (0.1 * fs) as usize,
                ))));
                let want = peak_dbfs(0.5);
                assert!(
                    (got - want).abs() <= 0.1,
                    "{f0} Hz at {fs} Hz reads {got:.3} dBFS, peak {want:.3}"
                );
            }
        }
    }
}

#[test]
fn vu_step_matches_golden() {
    let g = GoldenSet::load("ppm_vu_step").expect("golden");
    let period = f32s(&g.f64("period").expect("period"));
    let want = g.f64("reading").expect("reading");
    let mut vu = Vu::new(FS);
    let block: Vec<f32> = period.iter().cycle().take(480).copied().collect();
    let got: Vec<f32> = (0..want.len()).map(|_| vu.process(&block)).collect();
    assert_ok(Comparison::new("VU step").close_db(
        &want.iter().map(|&v| peak_dbfs(v)).collect::<Vec<_>>(),
        &db(&got),
        PARITY_DB,
    ));
}

#[test]
fn vu_reaches_99_percent_in_300_ms_and_reads_rms() {
    let rms = 0.5 / std::f64::consts::SQRT_2;
    let mut vu = Vu::new(FS);
    let at_300ms = f64::from(vu.process(&sine(FS, 1000.0, 0.5, (0.3 * FS) as usize)));
    assert!(
        (at_300ms / rms - 0.99).abs() < 0.005,
        "VU at 300 ms reads {:.4} of the RMS, want 0.99",
        at_300ms / rms
    );
    let settled = f64::from(vu.process(&sine(FS, 1000.0, 0.5, (2.0 * FS) as usize)));
    assert!((peak_dbfs(settled) - peak_dbfs(rms)).abs() < 0.05);
}

#[test]
fn true_peak_matches_golden() {
    let g = GoldenSet::load("ppm_true_peak").expect("golden");
    let phases = g.f64("phases").expect("phases");
    for (m, row) in phases.chunks(4).enumerate() {
        for (p, &tap) in row.iter().enumerate() {
            assert_eq!(f64::from(TRUE_PEAK_PHASES[p][m]), tap, "phase {p} tap {m}");
        }
    }
    for name in ["sine_fs4", "sine_11025", "pink"] {
        let x = f32s(&g.f64(&format!("{name}_input")).expect("input"));
        let want = g
            .f64(&format!("{name}_block_true_peak"))
            .expect("true peak");
        let mut tp = TruePeak::new();
        let got: Vec<f64> = x.chunks(480).map(|b| f64::from(tp.process(b))).collect();
        assert_ok(Comparison::new(name).close_f64(&want, &got, Tolerance::abs(1e-4)));
    }
}

#[test]
fn true_peak_phases_have_unit_gain_and_history_spans_blocks() {
    // DC gain = the sum of all 48 taps = the 4× upsampling gain, within the ripple.
    let sum: f32 = TRUE_PEAK_PHASES.iter().flatten().sum();
    assert!((sum / 4.0 - 1.0).abs() < 0.02, "taps sum to {sum}, want 4");
    let x = sine(FS, 997.0, 0.7, 4800);
    let whole = TruePeak::new().process(&x);
    let mut tp = TruePeak::new();
    let split = x.chunks(37).map(|b| tp.process(b)).fold(0.0f32, f32::max);
    assert_eq!(whole, split, "block boundaries must not change the peak");
}

/// An fs/4 sine sampled 45° off its peaks: the samples read −3.01 dBFS, the interpolator
/// finds the peaks between them.
#[test]
fn true_peak_sees_between_the_samples() {
    let x: Vec<f32> = (0..4800)
        .map(|n| {
            (std::f64::consts::FRAC_PI_2 * n as f64 + std::f64::consts::FRAC_PI_4).sin() as f32
        })
        .collect();
    let sample_peak = x.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    assert!((dbtp(sample_peak) + 3.01).abs() < 0.01);
    let tp = TruePeak::new().process(&x);
    assert!(
        dbtp(tp).abs() < 0.1,
        "true peak {:.3} dBTP, want 0",
        dbtp(tp)
    );
    assert_eq!(dbtp(1.0), 0.0);
}
