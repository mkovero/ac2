//! Deconvolution, windowing and harmonic separation of a recorded sweep.

use std::f64::consts::{LN_2, TAU};

use num_complex::Complex64;

use crate::ir_view::ImpulseResponse;

use super::locate::locate;
use super::spectral::{
    band, band_power, column_mean, db10, fft_forward, fft_inverse, fine_peak, high_passed,
    mean_power, noise_power, power_spectrum, segment, taper, wrap_deg,
};
use super::*;

/// The spectra one harmonic column is read from: the shared window's or an order's own.
struct Windowed<'a> {
    /// Window length, s: its resolution cell is `1/window_s`.
    window_s: f64,
    /// Lowest fundamental this window resolves (and the sweep plays at full level), Hz.
    f_lo: f64,
    /// Narrowest band a point averages over: [`DISTORTION_MIN_CELLS`] of this window.
    min_hz: f64,
    /// Bin spacing of the spectra, Hz.
    bin: f64,
    pk: &'a [f64],
    p1: &'a [f64],
    noise: &'a [f64],
    /// Noise in the impulse-response gaps beside this order's window (none where no whole
    /// window fits): see [`GAP_NOISE_FACTOR`].
    gaps: &'a [Vec<f64>],
}

/// An order's own longest window ([`LfHarmonics::Fine`]), with the fundamental and the
/// noise in the same window.
struct OwnWindow {
    window_s: f64,
    f_lo: f64,
    min_hz: f64,
    bin: f64,
    pk: Vec<f64>,
    p1: Vec<f64>,
    noise: Vec<f64>,
    gaps: Vec<Vec<f64>>,
}

impl OwnWindow {
    fn view(&self) -> Windowed<'_> {
        Windowed {
            window_s: self.window_s,
            f_lo: self.f_lo,
            min_hz: self.min_hz,
            bin: self.bin,
            pk: &self.pk,
            p1: &self.p1,
            noise: &self.noise,
            gaps: &self.gaps,
        }
    }
}

/// Power spectra of windows `w` (rise `pre_n`, length `w.len()`) centred in the gaps on either
/// side of order `k`'s window: between it and order k+1's, and between it and order k−1's (the
/// linear response's for H2), every order's window placed alike around its `t(k)`. A gap
/// without room for a whole window is skipped: the neighbours' band-limited impulses ring
/// into it.
fn gap_spectra(
    src: &[f64],
    t: impl Fn(u8) -> i64,
    k: u8,
    pre_n: usize,
    w: &[f64],
    nw: usize,
) -> Vec<Vec<f64>> {
    let (pre, len) = (pre_n as i64, w.len() as i64);
    let post = len - pre;
    [(k + 1, k), (k, k - 1)]
        .into_iter()
        .filter_map(|(early, late)| {
            let (lo, hi) = (t(early) + post, t(late) - pre);
            (hi - lo >= len).then(|| power_spectrum(&segment(src, (lo + hi - len) / 2, w), nw))
        })
        .collect()
}

/// Analyses a recording of `repeats` sweeps (each followed by its post-roll of silence) on
/// the reference and the measurement input, sample-aligned (one capture clock).
pub fn analyse_recording(
    spec: &SweepSpec,
    reference: &[f64],
    measurement: &[f64],
    repeats: usize,
) -> Result<SweepAnalysis, SweepError> {
    let timing = SweepTiming::new(spec)?;
    if repeats == 0 || reference.len() != measurement.len() || reference.is_empty() {
        return Err(SweepError::BadRecording);
    }
    let fs = spec.sample_rate;
    let clipped = reference.iter().chain(measurement).any(|v| v.abs() >= CLIP);
    let (onsets, reference_db) = locate(reference, spec, &timing, repeats)?;
    let m = timing.sweep_samples();
    let pre_roll = (PRE_ROLL_S * fs).round() as usize;
    let post_roll = timing.post_roll_samples(fs);
    let cuts: Vec<(usize, usize)> = onsets
        .iter()
        .map(|&o| (o.saturating_sub(pre_roll), o + m + post_roll))
        .collect();
    let longest = cuts.iter().map(|(a, b)| b - a).max().unwrap_or(1);
    let n = 2 * longest.next_power_of_two();

    // Complex mean of the per-repeat spectral divisions.
    let mut acc = vec![Complex64::new(0.0, 0.0); n / 2 + 1];
    for &(a, b) in &cuts {
        let r = fft_forward(&reference[a..b], n);
        let mm = fft_forward(&measurement[a..b], n);
        let peak = r.iter().fold(0.0f64, |p, z| p.max(z.norm_sqr()));
        let eps = REGULARISATION * peak;
        for ((h, rz), mz) in acc.iter_mut().zip(&r).zip(&mm) {
            *h += mz * rz.conj() / (rz.norm_sqr() + eps);
        }
    }
    let scale = 1.0 / cuts.len() as f64;
    acc.iter_mut().for_each(|z| *z *= scale);
    // Lowest fundamental analysed for distortion.
    let f_lo = timing.full_level_hz().max(2.0 / timing.window_s());
    let h_harm = fft_inverse(
        high_passed(&acc, fs / n as f64, HARMONIC_HIGH_PASS * f_lo),
        n,
    );
    let acc_lf = (spec.lf_harmonics == LfHarmonics::Fine).then(|| acc.clone());
    let h = fft_inverse(acc, n);

    // Arrival: the strongest sample from slightly before the reference to half the
    // post-roll after it.
    let at = |i: i64| h[i.rem_euclid(n as i64) as usize];
    let earliest = -((EARLIEST_ARRIVAL_S * fs).round() as i64);
    let latest = (post_roll / 2) as i64;
    let (d, peak) = (earliest..latest).fold((0i64, 0.0f64), |(bi, bv), i| {
        let v = at(i).abs();
        if v > bv { (i, v) } else { (bi, bv) }
    });
    if peak.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Err(SweepError::NoSignal);
    }
    // The arrival `D = d + phi`: windows stay anchored on the whole sample `d`, times and the
    // phase are referred to `D`.
    let phi = fine_peak(at, d);

    let l = timing.plan.rate_s;
    let k_max = spec.max_order;
    let pre_n = ((timing.pre_s * fs).round() as usize).max(1);
    let post_n = ((timing.post_s * fs).round() as usize).max(1);
    let w_len = pre_n + post_n;
    let w = taper(w_len, pre_n, (post_n / 5).max(1));
    let nw = 4 * w_len.next_power_of_two();
    let bin_w = fs / nw as f64;
    let t_k = |k: u8| d - (l * f64::from(k).ln() * fs).round() as i64;

    // Fundamental (distortion reference) and every order in the same window; the harmonics
    // from the response without its content below the analysed band (see `high_passed`).
    let spectra: Vec<Vec<f64>> = (1..=k_max)
        .map(|k| {
            let src = if k == 1 { &h } else { &h_harm };
            power_spectrum(&segment(src, t_k(k) - pre_n as i64, &w), nw)
        })
        .collect();
    // Noise: windows like the harmonic ones, tiled back from just before the end of the
    // post-roll (which every repeat's record covers fully) over its second half, where the
    // system's own decay has ended. With the window at most MAX_WINDOW_S and the post-roll at
    // least MIN_POST_ROLL_S, at least four fit.
    let region = |roll_s: f64| {
        let end = ((roll_s - NOISE_MARGIN_S) * fs).floor() as i64;
        (end, (NOISE_REGION_START * roll_s * fs).ceil() as i64)
    };
    let (noise_end, noise_first) = region(timing.shared_roll_s);
    let noise_start = noise_end - w_len as i64;
    let noise_windows = usize::try_from((noise_end - noise_first) / w_len as i64)
        .unwrap_or(0)
        .max(1);
    let noise = noise_power(&h_harm, noise_end, noise_windows, &w, nw);
    let gaps: Vec<Vec<Vec<f64>>> = (2..=k_max)
        .map(|k| gap_spectra(&h_harm, t_k, k, pre_n, &w, nw))
        .collect();

    let freqs = spec.grid.frequencies();
    let (f1, f2) = (spec.ess.start_hz, spec.ess.end_hz);
    let f_top = (f2 * (-spec.ess.fade_out_s / l).exp()).min(HARMONIC_FS_FRACTION * fs);
    let mut harmonics: Vec<HarmonicCurve> = (2..=k_max)
        .map(|order| HarmonicCurve {
            order,
            level_db: vec![f64::NAN; freqs.len()],
            floor_db: vec![f64::NAN; freqs.len()],
        })
        .collect();
    let min_hz = DISTORTION_MIN_CELLS / timing.window_s();
    let shared = |k: u8| Windowed {
        window_s: w_len as f64 / fs,
        f_lo,
        min_hz,
        bin: bin_w,
        pk: &spectra[usize::from(k) - 1],
        p1: &spectra[0],
        noise: &noise,
        gaps: &gaps[usize::from(k) - 2],
    };
    // `LfHarmonics::Fine`: per order, its longest window, the harmonic from a response
    // high-passed below that window's own lowest fundamental.
    let own: Vec<Option<OwnWindow>> = (2..=k_max)
        .map(|k| {
            let acc = acc_lf.as_ref()?;
            let (pre, post) = lf_window(l, k);
            let (pre_n, post_n) = ((pre * fs).round() as usize, (post * fs).round() as usize);
            let w_len = pre_n + post_n;
            let (end, first) = region(timing.post_roll_s);
            let count = usize::try_from((end - first) / w_len as i64).unwrap_or(0);
            if count == 0 {
                return None;
            }
            let w = taper(w_len, pre_n, (post_n / 5).max(1));
            let nw = 4 * w_len.next_power_of_two();
            let ws = w_len as f64 / fs;
            let f_lo = timing.full_level_hz().max(2.0 / ws);
            let hk = fft_inverse(
                high_passed(acc, fs / n as f64, HARMONIC_HIGH_PASS * f_lo),
                n,
            );
            Some(OwnWindow {
                window_s: ws,
                f_lo,
                min_hz: DISTORTION_MIN_CELLS / ws,
                bin: fs / nw as f64,
                pk: power_spectrum(&segment(&hk, t_k(k) - pre_n as i64, &w), nw),
                p1: power_spectrum(&segment(&h, t_k(1) - pre_n as i64, &w), nw),
                noise: noise_power(&hk, end, count, &w, nw),
                gaps: gap_spectra(&hk, t_k, k, pre_n, &w, nw),
            })
        })
        .collect();
    let f_lo_any = own.iter().flatten().map(|x| x.f_lo).fold(f_lo, f64::min);
    let mut thd_db = vec![f64::NAN; freqs.len()];
    let mut thd_floor_db = vec![f64::NAN; freqs.len()];
    // Per column, the widest cell (on the fundamental's axis) of a window a reported order
    // was read from.
    let mut harmonic_cell = vec![None::<f64>; freqs.len()];
    let half = 2f64.powf(DISTORTION_BAND_OCT / 2.0);
    for (i, &f) in freqs.iter().enumerate() {
        if f < f_lo_any {
            continue;
        }
        let (mut sum, mut floor_sum, mut any) = (0.0, 0.0, false);
        for hc in &mut harmonics {
            let k = f64::from(hc.order);
            let kf = k * f;
            // Its own window where the shared one's band had to be widened to `min_hz`.
            let widened = min_hz > kf * (half - 1.0 / half);
            let x = match &own[usize::from(hc.order) - 2] {
                Some(o) if widened => o.view(),
                _ => shared(hc.order),
            };
            if f < x.f_lo {
                continue;
            }
            let (lo, hi) = band(kf, DISTORTION_BAND_OCT, x.min_hz);
            // The whole band below the fade-out: dividing by a reference faded there while
            // the harmonic was driven by the fundamental at full level reads it too high.
            if hi > f_top {
                continue;
            }
            let pk = mean_power(x.pk, x.bin, lo, hi, kf);
            // The fundamentals whose harmonic falls in that band: the window's resolution
            // widens a low harmonic's band to `min_hz`, which spans k times fewer hertz of
            // fundamental. Under a roll-off the fundamental's power slopes steeply there
            // (6 dB over 10–40 Hz at 25 Hz behind a 20 Hz high-pass), so averaged over a
            // band of its own it would not be the gain those harmonics were driven with.
            let p1 = mean_power(x.p1, x.bin, lo / k, hi / k, f);
            if p1.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
                continue;
            }
            let floor = |s: &[f64]| band_power(s, x.bin, kf, FLOOR_BAND_OCT, 2.0 * x.min_hz);
            let post = floor(x.noise);
            let nk = x
                .gaps
                .iter()
                .map(|g| floor(g))
                .filter(|&ng| ng > GAP_NOISE_FACTOR * post)
                .fold(post, f64::max);
            let cell = 1.0 / (k * x.window_s);
            harmonic_cell[i] = Some(harmonic_cell[i].map_or(cell, |c: f64| c.max(cell)));
            hc.level_db[i] = db10(pk / p1);
            hc.floor_db[i] = db10(nk / p1);
            sum += pk / p1;
            floor_sum += nk / p1;
            any = true;
        }
        if any {
            thd_db[i] = db10(sum);
            thd_floor_db[i] = db10(floor_sum);
        }
    }

    // Linear response: from just after H2's window to the gate (default: the noise window).
    let lin_pre = ((PRE_FRACTION * l * LN_2 * fs).round() as usize).max(1);
    let lin_end = match spec.gate_s {
        Some(g) => (d + (g * fs).round() as i64).min(noise_start),
        None => noise_start,
    };
    let lin_end = lin_end.max(d + 2);
    let lin_post = (lin_end - d) as usize;
    let lin_len = lin_pre + lin_post;
    let lw = taper(lin_len, lin_pre, (lin_post / 10).max(1));
    let nl = 2 * lin_len.next_power_of_two();
    let s1 = fft_forward(&segment(&h, d - lin_pre as i64, &lw), nl);
    let bin_l = fs / nl as f64;
    // Refer the phase to the arrival: the segment starts `lin_pre + phi` samples before it.
    let lead = lin_pre as f64 + phi;
    let rot = |k: usize| {
        let z = s1[k];
        z * Complex64::from_polar(1.0, TAU * k as f64 * lead / nl as f64)
    };
    let mut magnitude_db = vec![f64::NAN; freqs.len()];
    let mut phase_deg = vec![f64::NAN; freqs.len()];
    for (i, &f) in freqs.iter().enumerate() {
        if f < f1 || f > f2 {
            continue;
        }
        let (e_lo, e_hi) = spec.grid.edges(i);
        let (a, b) = (e_lo / bin_l, e_hi / bin_l);
        if b + 1.0 >= s1.len() as f64 {
            continue;
        }
        let p = column_mean(|k| s1[k].norm_sqr(), a, b);
        let z = column_mean(rot, a, b);
        if p > 0.0 {
            magnitude_db[i] = db10(p);
            phase_deg[i] = wrap_deg(z.arg().to_degrees());
        }
    }

    // One cell of the linear window wherever the response is reported.
    let lin_cell = fs / lin_len as f64;
    let unresolved_response = crate::resolution::unresolved_ranges(&spec.grid, |f| {
        (f1..=f2).contains(&f).then_some(lin_cell)
    });

    // IR from the highest order's window to the end of the linear window.
    let ir_start = t_k(k_max) - pre_n as i64;
    let span: Vec<f64> = (ir_start..lin_end).map(at).collect();
    let env = ImpulseResponse {
        samples: span.clone(),
        sample_rate: fs,
        inserted_delay_s: 0.0,
    }
    .envelope();
    let bucket = span.len().div_ceil(IR_POINTS).max(1);
    let ir: Vec<f64> = span
        .chunks(bucket)
        .map(|c| {
            c.iter()
                .fold(0.0f64, |a, &v| if v.abs() > a.abs() { v } else { a })
        })
        .collect();
    let ir_etc_db: Vec<f64> = env
        .chunks(bucket)
        .map(|c| {
            let m = c.iter().fold(0.0f64, |a, &v| a.max(v));
            if m > 0.0 {
                (20.0 * m.log10()).max(crate::ir_view::FLOOR_DB)
            } else {
                crate::ir_view::FLOOR_DB
            }
        })
        .collect();

    // Room parameters: from half-way to H2's impulse (at most 100 ms before the arrival, room
    // for the band filters' pre-ringing) to the end of the silence.
    let room_start = d - ((0.5 * l * LN_2).min(ROOM_PRE_S) * fs).round() as i64;
    let room_end = (((timing.shared_roll_s - NOISE_MARGIN_S) * fs).floor() as i64).max(d + 2);
    let room_ir: Vec<f64> = (room_start..room_end).map(at).collect();
    let excited = (
        timing.full_level_hz(),
        f2 * (-spec.ess.fade_out_s / l).exp(),
    );
    let re_arrival = |i: i64| ((i - d) as f64 - phi) / fs;
    let room = crate::room::analyse(&room_ir, fs, re_arrival(room_start), excited);

    Ok(SweepAnalysis {
        sample_rate: fs,
        rate_s: l,
        duration_s: timing.plan.duration_s(),
        repeats: cuts.len(),
        arrival_s: (d as f64 + phi) / fs,
        reference_db,
        first_onset_s: onsets[0] as f64 / fs,
        harmonic_window_s: (pre_n as f64 / fs, post_n as f64 / fs),
        linear_window_s: (lin_pre as f64 / fs, lin_post as f64 / fs),
        noise_window_s: re_arrival(noise_end - (noise_windows * w_len) as i64),
        frequencies: freqs,
        magnitude_db,
        phase_deg,
        harmonics,
        thd_db,
        thd_floor_db,
        ir,
        ir_etc_db,
        ir_t0_s: re_arrival(ir_start),
        ir_dt_s: bucket as f64 / fs,
        clipped,
        room,
        room_end_s: re_arrival(room_end),
        unresolved_response,
        unresolved_harmonics: crate::resolution::unresolved_columns(&spec.grid, |i| {
            harmonic_cell[i]
        }),
    })
}
