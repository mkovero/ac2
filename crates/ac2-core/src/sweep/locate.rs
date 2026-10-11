//! Locating the sweeps in the reference record and its level re the emitted sweep.

use std::f64::consts::LN_2;

use crate::generator::{EssInverse, dbfs_to_rms};

use super::spectral::{db10, power_spectrum, segment, taper};
use super::{HARMONIC_FS_FRACTION, MIN_REFERENCE_DB, SweepError, SweepSpec, SweepTiming};

/// Onsets (sample index of the first sweep sample) of `count` sweeps in `reference`, and the
/// strongest sweep's level re the emitted one, dB.
pub(super) fn locate(
    reference: &[f64],
    spec: &SweepSpec,
    timing: &SweepTiming,
    count: usize,
) -> Result<(Vec<usize>, f64), SweepError> {
    let fs = spec.sample_rate;
    let inv = EssInverse::new(&timing.emitted, fs, spec.level_dbfs)?;
    let m = timing.sweep_samples();
    // The matched-filter peak a unit loopback of the emitted sweep produces.
    let amp = dbfs_to_rms(spec.level_dbfs) * std::f64::consts::SQRT_2;
    let ideal: Vec<f64> = (0..m).map(|n| amp * timing.plan.sample(n)).collect();
    let unit_peak = inv
        .deconvolve(&ideal)
        .iter()
        .fold(0.0f64, |a, v| a.max(v.abs()));
    let y = inv.deconvolve(reference);
    let mut mag: Vec<f64> = y.iter().map(|v| v.abs()).collect();
    let period = timing.period_samples(fs);
    let mut peaks = Vec::new();
    for _ in 0..count {
        let (i, v) =
            mag.iter().enumerate().fold(
                (0, 0.0f64),
                |(bi, bv), (i, v)| if *v > bv { (i, *v) } else { (bi, bv) },
            );
        if v.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            break;
        }
        peaks.push((i, v));
        let lo = i.saturating_sub(period / 2);
        let hi = (i + period / 2).min(mag.len());
        mag[lo..hi].iter_mut().for_each(|x| *x = 0.0);
    }
    let strongest = peaks.iter().fold(0.0f64, |a, p| a.max(p.1));
    let level_db = if unit_peak > 0.0 && strongest > 0.0 {
        20.0 * (strongest / unit_peak).log10()
    } else {
        f64::NEG_INFINITY
    };
    if level_db < MIN_REFERENCE_DB {
        return Err(SweepError::NoReference { level_db });
    }
    let i_ref = peaks
        .iter()
        .fold((0, 0.0f64), |a, p| if p.1 > a.1 { *p } else { a })
        .0;
    let level_db =
        mid_band_gain_db(&y, i_ref, &inv.deconvolve(&ideal), spec, timing).unwrap_or(level_db);
    // A matched-filter peak far below the strongest is a sidelobe or a stray burst, not a
    // repeat of the same sweep.
    let mut onsets: Vec<usize> = peaks
        .iter()
        .filter(|p| p.1 >= strongest * 0.25 && p.0 >= m - 1)
        .map(|p| p.0 + 1 - m)
        .collect();
    onsets.sort_unstable();
    let whole = onsets
        .iter()
        .filter(|&&o| o + period <= reference.len())
        .count();
    if onsets.len() < count || whole < count {
        return Err(SweepError::Incomplete {
            found: whole.min(onsets.len()),
            expected: count,
        });
    }
    Ok((onsets, level_db))
}

/// The reference's gain re the emitted sweep over the log-middle half of the full-level band,
/// dB: the power ratio of the deconvolved reference to the deconvolved ideal loopback there,
/// each windowed around its peak, averaged per octave. The matched-filter peak weights frequencies linearly (a
/// sweep spends equal time per octave but the impulse's energy is spread per hertz), so its
/// ratio is the gain near the top of the band, where a converter's anti-alias roll-off sits;
/// the middle of the band is where the loopback's gain is the level it passes on.
/// None when the band holds no bin.
fn mid_band_gain_db(
    y: &[f64],
    i_y: usize,
    y_unit: &[f64],
    spec: &SweepSpec,
    timing: &SweepTiming,
) -> Option<f64> {
    let fs = spec.sample_rate;
    let l = timing.plan.rate_s;
    let f_full = timing.full_level_hz();
    let f_top = (spec.ess.end_hz * (-spec.ess.fade_out_s / l).exp()).min(HARMONIC_FS_FRACTION * fs);
    if f_top <= f_full {
        return None;
    }
    let r = f_top / f_full;
    let (lo, hi) = (f_full * r.powf(0.25), f_full * r.powf(0.75));
    // Long enough for several bins below the band, short enough that the window's quarter
    // before the peak stays clear of H2's impulse, L·ln 2 earlier.
    let w_s = (8.0 / lo).max(0.1).min(2.0 * l * LN_2);
    let nw = ((w_s * fs) as usize).next_power_of_two().max(64);
    let pre = nw / 4;
    let w = taper(nw, pre / 4, nw / 8);
    let i_u = y_unit
        .iter()
        .enumerate()
        .fold(
            (0, 0.0f64),
            |a, (i, v)| if v.abs() > a.1 { (i, v.abs()) } else { a },
        )
        .0;
    let py = power_spectrum(&segment(y, i_y as i64 - pre as i64, &w), nw);
    let pu = power_spectrum(&segment(y_unit, i_u as i64 - pre as i64, &w), nw);
    let bin = fs / nw as f64;
    // Per-bin power ratio weighted 1/f: equal weight per octave, like the sweep's own time.
    let (mut acc, mut wsum) = (0.0, 0.0);
    for (k, (a, b)) in py.iter().zip(&pu).enumerate() {
        let f = k as f64 * bin;
        if f >= lo && f <= hi && *b > 0.0 {
            acc += a / b / f;
            wsum += 1.0 / f;
        }
    }
    (wsum > 0.0 && acc > 0.0).then(|| db10(acc / wsum))
}
