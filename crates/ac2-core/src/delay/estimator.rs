//! Regularised H1 tiles, the pulse model, excitation and periodicity checks, band SNR
//! (design Q1 §4, §7, §10.1).

use std::f64::consts::{LN_2, PI};
use std::sync::Arc;

use num_complex::Complex64;
use realfft::RealFftPlanner;
use rustfft::FftPlanner;

use super::Estimator;
use crate::window::Window;

const ZERO: Complex64 = Complex64::new(0.0, 0.0);

/// Width of the raised-cosine taper centred on each −6 dB band edge, octaves.
const TAPER_OCT: f64 = 1.0;

/// Band used for periodicity detection. Periodicity belongs to the excitation, not to the
/// analysis band, so it is looked for where the repeat has the most independent samples.
const PERIOD_BAND: (f64, f64) = (40.0, 16_000.0);

/// Smallest repeat the period detector reports, samples.
const PERIOD_MIN_LAG: usize = 256;

/// Upper −6 dB edge clipped so the taper ends at or below Nyquist.
pub(super) fn band_hi(hi: f64, fs: f64) -> f64 {
    hi.min(0.5 * fs * 2f64.powf(-TAPER_OCT / 2.0))
}

fn ramp(x: f64) -> f64 {
    let u = (x / TAPER_OCT + 0.5).clamp(0.0, 1.0);
    0.5 - 0.5 * (PI * u).cos()
}

/// Zero-phase band weighting B(f): 1 inside, raised cosine in log₂ f across each −6 dB edge.
/// `hi` must already be clipped by [`band_hi`].
pub(super) fn band_weight(f: f64, lo: f64, hi: f64) -> f64 {
    if f <= 0.0 {
        return 0.0;
    }
    let lf = f.log2();
    ramp(lf - lo.log2()) * ramp(hi.log2() - lf)
}

/// Segment starts of a grid centred in a block of `n_avail` samples.
pub(super) fn segment_starts(n_avail: usize, nseg: usize, hop: usize, out: &mut Vec<usize>) {
    out.clear();
    if n_avail < nseg {
        return;
    }
    let k = (n_avail - nseg) / hop + 1;
    let used = (k - 1) * hop + nseg;
    let off = (n_avail - used) / 2;
    out.extend((0..k).map(|i| off + i * hop));
}

/// FFT plans and their working buffers. The planners cache every size they have built, so
/// a scratch reused across calls with one configuration plans once.
pub(super) struct Ffts {
    real: RealFftPlanner<f64>,
    cplx: FftPlanner<f64>,
    rbuf: Vec<f64>,
    scratch: Vec<Complex64>,
}

impl Ffts {
    pub(super) fn new() -> Self {
        Self {
            real: RealFftPlanner::new(),
            cplx: FftPlanner::new(),
            rbuf: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Build (and cache) the plans an `nfft`-point analysis uses.
    pub(super) fn plan(&mut self, nfft: usize) {
        self.real.plan_fft_forward(nfft);
        self.cplx.plan_fft_inverse(nfft);
    }

    /// One-sided spectrum (`nfft/2 + 1` bins) of an `nfft`-point real frame. `fill` writes
    /// the frame into a zeroed buffer, so a shorter segment is zero-padded.
    pub(super) fn rfft(
        &mut self,
        nfft: usize,
        out: &mut [Complex64],
        fill: impl FnOnce(&mut [f64]),
    ) {
        let plan = self.real.plan_fft_forward(nfft);
        self.rbuf.clear();
        self.rbuf.resize(nfft, 0.0);
        fill(&mut self.rbuf);
        let need = plan.get_scratch_len();
        if self.scratch.len() < need {
            self.scratch.resize(need, ZERO);
        }
        // realfft only fails on wrong buffer lengths, which are sized right here.
        if plan
            .process_with_scratch(&mut self.rbuf, out, &mut self.scratch[..need])
            .is_err()
        {
            unreachable!("FFT buffers are sized by the planner");
        }
    }

    /// Unnormalised inverse complex FFT in place.
    pub(super) fn ifft(&mut self, buf: &mut [Complex64]) {
        let plan = self.cplx.plan_fft_inverse(buf.len());
        let need = plan.get_inplace_scratch_len();
        if self.scratch.len() < need {
            self.scratch.resize(need, ZERO);
        }
        plan.process_with_scratch(buf, &mut self.scratch[..need]);
    }

    /// Complex analytic IR (unnormalised inverse FFT) of the one-sided spectrum `hk`:
    /// positive bins doubled, negative bins zero.
    fn analytic(&mut self, hk: &[Complex64], out: &mut Vec<Complex64>) {
        let nfft = 2 * (hk.len() - 1);
        out.clear();
        out.resize(nfft, ZERO);
        out[0] = hk[0];
        for k in 1..nfft / 2 {
            out[k] = 2.0 * hk[k];
        }
        out[nfft / 2] = hk[nfft / 2];
        self.ifft(out);
    }
}

/// The parts of a Welch grid that depend only on (fs, band, segment length).
#[derive(Debug)]
pub(super) struct GridShape {
    pub nseg: usize,
    pub hop: usize,
    pub nfft: usize,
    pub fs: f64,
    pub lo: f64,
    pub hi: f64,
    pub w: Vec<f64>,
    /// Band weights on the `nfft/2 + 1` bins.
    pub b: Vec<f64>,
    b_sum: f64,
    /// 1 / (B₀ + 2 Σ B_k + B_N): makes a unit pure delay read 1 after an unnormalised
    /// inverse FFT.
    ir_scale: f64,
    /// Hann overlap ρ(δ), indexed by δ mod nfft.
    rho: Vec<f64>,
}

impl GridShape {
    pub(super) fn new(ffts: &mut Ffts, nseg: usize, hop: usize, fs: f64, lo: f64, hi: f64) -> Self {
        let nfft = 2 * nseg;
        ffts.plan(nfft);
        let w = Window::Hann.coefficients(nseg);
        let bins = nfft / 2 + 1;
        let df = fs / nfft as f64;
        let b: Vec<f64> = (0..bins)
            .map(|k| band_weight(k as f64 * df, lo, hi))
            .collect();
        let b_sum = b.iter().sum();
        let inner: f64 = b[1..bins - 1].iter().sum();
        let ir_scale = 1.0 / (b[0] + 2.0 * inner + b[bins - 1]);
        // ρ(δ) = Σ w[n] w[n+δ] / Σ w²: the autocorrelation of the window, linear thanks to
        // the 2× zero padding.
        let mut spec = vec![ZERO; bins];
        ffts.rfft(nfft, &mut spec, |buf| buf[..nseg].copy_from_slice(&w));
        let mut ac: Vec<Complex64> = Vec::with_capacity(nfft);
        ac.extend(spec.iter().map(|x| Complex64::new(x.norm_sqr(), 0.0)));
        ac.extend(
            (1..nfft / 2)
                .rev()
                .map(|k| Complex64::new(spec[k].norm_sqr(), 0.0)),
        );
        ffts.ifft(&mut ac);
        let rho0 = ac[0].re;
        let rho = ac.iter().map(|v| v.re / rho0).collect();
        Self {
            nseg,
            hop,
            nfft,
            fs,
            lo,
            hi,
            w,
            b,
            b_sum,
            ir_scale,
            rho,
        }
    }

    pub(super) fn bins(&self) -> usize {
        self.nfft / 2 + 1
    }

    pub(super) fn df(&self) -> f64 {
        self.fs / self.nfft as f64
    }

    pub(super) fn matches(&self, nseg: usize, hop: usize, fs: f64, lo: f64, hi: f64) -> bool {
        self.nseg == nseg && self.hop == hop && self.fs == fs && self.lo == lo && self.hi == hi
    }

    /// B-weighted in-band mean of a power spectrum.
    fn band_mean(&self, gxx: &[f64]) -> f64 {
        self.b.iter().zip(gxx).map(|(b, g)| b * g).sum::<f64>() / self.b_sum
    }

    /// ρ(δ).
    pub(super) fn rho(&self, delta: i64) -> f64 {
        self.rho[delta.rem_euclid(self.nfft as i64) as usize]
    }
}

/// Meas segment spectra of one grid.
#[derive(Debug, Default)]
pub(super) struct MeasSpectra {
    pub starts: Vec<usize>,
    pub y: Vec<Complex64>,
}

impl MeasSpectra {
    pub(super) fn compute(&mut self, ffts: &mut Ffts, g: &GridShape, meas: &[f64]) {
        segment_starts(meas.len(), g.nseg, g.hop, &mut self.starts);
        let bins = g.bins();
        self.y.clear();
        self.y.resize(self.starts.len() * bins, ZERO);
        for (i, &s) in self.starts.iter().enumerate() {
            let seg = &meas[s..s + g.nseg];
            ffts.rfft(g.nfft, &mut self.y[i * bins..(i + 1) * bins], |buf| {
                for ((o, x), w) in buf.iter_mut().zip(seg).zip(&g.w) {
                    *o = x * w;
                }
            });
        }
    }

    pub(super) fn segments(&self) -> usize {
        self.starts.len()
    }
}

/// Estimator choice with its regularisation ε.
#[derive(Debug, Clone, Copy)]
pub(super) struct Reg {
    pub estimator: Estimator,
    pub eps: f64,
}

/// Absolute positions of the pair: `a` = index of ref[0], `b` = index of meas[0].
#[derive(Debug, Clone, Copy)]
pub(super) struct Pair<'a> {
    pub r: &'a [f64],
    pub a: i64,
    pub m: &'a [f64],
    pub b: i64,
}

/// Output of one tile.
#[derive(Debug, Default)]
pub(super) struct Tile {
    /// Analytic IR, scaled so a unit pure delay reads 1 at its lag (before ρ).
    pub ir: Vec<Complex64>,
    pub gxx: Vec<f64>,
    gxy: Vec<Complex64>,
    xspec: Vec<Complex64>,
    hspec: Vec<Complex64>,
    /// Segments that had ref coverage.
    pub k: usize,
}

impl Tile {
    /// Regularised H1 (or PHAT) at integer pre-shift `d0` (§4.2).
    pub(super) fn compute(
        &mut self,
        ffts: &mut Ffts,
        g: &GridShape,
        y: &MeasSpectra,
        pair: Pair<'_>,
        d0: i64,
        reg: Reg,
    ) {
        let bins = g.bins();
        self.gxy.clear();
        self.gxy.resize(bins, ZERO);
        self.gxx.clear();
        self.gxx.resize(bins, 0.0);
        self.xspec.resize(bins, ZERO);
        self.k = 0;
        for (i, &s) in y.starts.iter().enumerate() {
            let r0 = pair.b + s as i64 - d0 - pair.a;
            if r0 < 0 || r0 as usize + g.nseg > pair.r.len() {
                continue; // the ref block does not cover this segment at this lag
            }
            let seg = &pair.r[r0 as usize..r0 as usize + g.nseg];
            ffts.rfft(g.nfft, &mut self.xspec, |buf| {
                for ((o, x), w) in buf.iter_mut().zip(seg).zip(&g.w) {
                    *o = x * w;
                }
            });
            let yi = &y.y[i * bins..(i + 1) * bins];
            for (((xy, xx), x), y) in self
                .gxy
                .iter_mut()
                .zip(&mut self.gxx)
                .zip(&self.xspec)
                .zip(yi)
            {
                *xy += x.conj() * y;
                *xx += x.norm_sqr();
            }
            self.k += 1;
        }
        if self.k == 0 {
            return;
        }
        self.hspec.clear();
        match reg.estimator {
            Estimator::RegularisedH1 => {
                let floor = reg.eps * g.band_mean(&self.gxx);
                self.hspec.extend(
                    self.gxy
                        .iter()
                        .zip(&self.gxx)
                        .zip(&g.b)
                        .map(|((xy, xx), b)| {
                            let den = xx + floor;
                            if den > 0.0 { *b * *xy / den } else { ZERO }
                        }),
                );
            }
            Estimator::Phat => {
                self.hspec.extend(self.gxy.iter().zip(&g.b).map(|(xy, b)| {
                    let mag = xy.norm();
                    if mag > 0.0 { *b * *xy / mag } else { ZERO }
                }));
            }
        }
        ffts.analytic(&self.hspec, &mut self.ir);
        for v in &mut self.ir {
            *v *= g.ir_scale;
        }
    }

    /// h(d0 + δ) = a[δ mod 2N] / ρ(δ).
    pub(super) fn at(&self, g: &GridShape, delta: i64) -> Complex64 {
        self.ir[delta.rem_euclid(g.nfft as i64) as usize] / g.rho(delta)
    }
}

/// Expected shape of one pure-delay arrival under this excitation (§4.5).
#[derive(Debug, Clone)]
pub(super) struct Pulse {
    /// P_k = B_k·W_k on the grid's bins.
    pub pw: Vec<f64>,
    /// Regularisation shrink W_k.
    pub shrink: Vec<f64>,
    /// |p| on the integer grid, peak 1, indexed by lag mod nfft.
    env: Vec<f64>,
    pub nfft: usize,
    /// −6 dB full width, samples.
    pub width: f64,
}

impl Pulse {
    /// `gxx = None` gives the nominal model (W ≡ 1).
    pub(super) fn new(ffts: &mut Ffts, g: &GridShape, gxx: Option<&[f64]>, reg: Reg) -> Self {
        let shrink: Vec<f64> = match (gxx, reg.estimator) {
            (Some(gxx), Estimator::RegularisedH1) => {
                let floor = reg.eps * g.band_mean(gxx);
                gxx.iter()
                    .map(|&x| {
                        if x + floor > 0.0 {
                            x / (x + floor)
                        } else {
                            0.0
                        }
                    })
                    .collect()
            }
            _ => vec![1.0; g.bins()],
        };
        let pw: Vec<f64> = g.b.iter().zip(&shrink).map(|(b, w)| b * w).collect();
        let spec: Vec<Complex64> = pw.iter().map(|&v| Complex64::new(v, 0.0)).collect();
        let mut a = Vec::new();
        ffts.analytic(&spec, &mut a);
        let mut env: Vec<f64> = a.iter().map(|v| v.norm()).collect();
        let peak = env.iter().copied().fold(0.0, f64::max);
        if peak > 0.0 {
            for v in &mut env {
                *v /= peak;
            }
        }
        let nfft = g.nfft;
        let mut p = Self {
            pw,
            shrink,
            env,
            nfft,
            width: 0.0,
        };
        p.width = p.halfwidth(-1, 0.5) + p.halfwidth(1, 0.5);
        p
    }

    /// |p(δ)| for integer δ, zero beyond ±nfft/2.
    pub(super) fn env_at(&self, delta: i64) -> f64 {
        let h = (self.nfft / 2) as i64;
        if delta < -h || delta >= h {
            0.0
        } else {
            self.env[delta.rem_euclid(self.nfft as i64) as usize]
        }
    }

    /// Distance from the peak to the `level` crossing in `dir`, linearly interpolated.
    fn halfwidth(&self, dir: i64, level: f64) -> f64 {
        let h = (self.nfft / 2) as i64;
        let inside = |d: i64| d >= -h && d < h;
        let mut j = 0i64;
        while inside(j + dir) && self.env_at(j + dir) >= level {
            j += dir;
        }
        let k = j + dir;
        if !inside(k) {
            return j.abs() as f64;
        }
        let (ej, ek) = (self.env_at(j), self.env_at(k));
        let frac = if ej != ek {
            (ej - level) / (ej - ek)
        } else {
            0.0
        };
        j.abs() as f64 + frac
    }

    /// Equivalent noise bandwidth of the pulse spectrum, Hz.
    pub(super) fn b_eff(&self, df: f64) -> f64 {
        let s: f64 = self.pw.iter().sum();
        let s2: f64 = self.pw.iter().map(|v| v * v).sum();
        if s2 > 0.0 { s * s / s2 * df } else { 0.0 }
    }

    /// Fraction of the band's octave span where W ≥ 0.5 (§7 `InsufficientExcitation`).
    pub(super) fn excited_fraction(&self, g: &GridShape) -> f64 {
        let df = g.df();
        let span = (g.hi / g.lo).log2();
        if span <= 0.0 {
            return 0.0;
        }
        let mut octs = 0.0;
        for (k, &w) in self.shrink.iter().enumerate() {
            let f = k as f64 * df;
            if f > 0.0 && f >= g.lo && f <= g.hi && w >= 0.5 {
                octs += df / (f * LN_2);
            }
        }
        octs / span
    }
}

/// Complex pulse model on a 1/OS-sample grid for the lobe fit and deblending.
#[derive(Debug)]
pub(super) struct Model {
    half: i64,
    re: Vec<f64>,
    im: Vec<f64>,
    abs: Vec<f64>,
}

/// Oversampling of the pulse model.
const OS: i64 = 16;

impl Model {
    /// p(t) for |t| ≤ `half_span`, peak 1 on the grid. Equivalent to zero-padding the
    /// spectrum OS-fold and inverting once; done as OS inverse FFTs of the original size,
    /// each with a fractional phase ramp, so the large buffer is never formed.
    pub(super) fn new(ffts: &mut Ffts, pw: &[f64], half_span: f64) -> Self {
        let nfft = 2 * (pw.len() - 1);
        let half = (half_span * OS as f64).ceil() as i64;
        let len = (2 * half + 1) as usize;
        let mut vals = vec![ZERO; len];
        let mut buf = vec![ZERO; nfft];
        for q in 0..OS {
            buf.fill(ZERO);
            for k in 0..nfft / 2 {
                let c = if k == 0 { 1.0 } else { 2.0 };
                let ph = 2.0 * PI * (k as f64) * (q as f64) / ((OS as usize * nfft) as f64);
                buf[k] = Complex64::from_polar(c * pw[k], ph);
            }
            ffts.ifft(&mut buf);
            // grid points n = OS·i + q
            let mut n = -half + (q - (-half).rem_euclid(OS)).rem_euclid(OS);
            while n <= half {
                let i = n.div_euclid(OS).rem_euclid(nfft as i64) as usize;
                vals[(n + half) as usize] = buf[i];
                n += OS;
            }
        }
        let peak = vals.iter().map(|v| v.norm()).fold(0.0, f64::max);
        let scale = if peak > 0.0 { 1.0 / peak } else { 0.0 };
        Self {
            half,
            re: vals.iter().map(|v| v.re * scale).collect(),
            im: vals.iter().map(|v| v.im * scale).collect(),
            abs: vals.iter().map(|v| v.norm() * scale).collect(),
        }
    }

    /// Linear interpolation on the grid, zero outside it.
    fn interp(&self, v: &[f64], x: f64) -> f64 {
        let pos = x * OS as f64 + self.half as f64;
        let last = (v.len() - 1) as f64;
        if !(0.0..=last).contains(&pos) {
            return 0.0;
        }
        let i = pos.floor() as usize;
        if i + 1 >= v.len() {
            return v[v.len() - 1];
        }
        let f = pos - i as f64;
        v[i] + f * (v[i + 1] - v[i])
    }

    /// Complex p(x).
    pub(super) fn at(&self, x: f64) -> Complex64 {
        Complex64::new(self.interp(&self.re, x), self.interp(&self.im, x))
    }

    /// |p|(x), interpolated from the grid magnitudes.
    pub(super) fn abs_at(&self, x: f64) -> f64 {
        self.interp(&self.abs, x)
    }
}

/// Repeat period of the ref block from its whitened, band-limited autocorrelation (§10.1).
/// Returns the period when its peak reaches `level_db` re lag 0.
pub(super) fn detect_period(
    ffts: &mut Ffts,
    r: &[f64],
    fs: f64,
    eps: f64,
    level_db: f64,
) -> Option<u64> {
    let n = r.len();
    let nfft = (2 * n).next_power_of_two();
    let bins = nfft / 2 + 1;
    let mut spec = vec![ZERO; bins];
    ffts.rfft(nfft, &mut spec, |buf| buf[..n].copy_from_slice(r));
    let df = fs / nfft as f64;
    let (lo, hi) = (PERIOD_BAND.0, band_hi(PERIOD_BAND.1, fs));
    let b: Vec<f64> = (0..bins)
        .map(|k| band_weight(k as f64 * df, lo, hi))
        .collect();
    let g: Vec<f64> = spec.iter().map(|x| x.norm_sqr()).collect();
    // |R|² smoothed to the excitation shape (zero beyond the ends, like a "same" convolution)
    let k = (nfft / 4096).max(1);
    let mut prefix = vec![0.0; bins + 1];
    for i in 0..bins {
        prefix[i + 1] = prefix[i] + g[i];
    }
    let gs: Vec<f64> = (0..bins)
        .map(|i| {
            let lo_i = i.saturating_sub(k);
            let hi_i = (i + k + 1).min(bins);
            (prefix[hi_i] - prefix[lo_i]) / (2 * k + 1) as f64
        })
        .collect();
    let b_sum: f64 = b.iter().sum();
    let mean_b = b.iter().zip(&gs).map(|(b, g)| b * g).sum::<f64>() / b_sum;
    let reg = eps * mean_b;
    let white: Vec<Complex64> = (0..bins)
        .map(|i| {
            let den = gs[i] + reg;
            Complex64::new(if den > 0.0 { b[i] * g[i] / den } else { 0.0 }, 0.0)
        })
        .collect();
    let mut ac = Vec::new();
    ffts.analytic(&white, &mut ac);
    let ac0 = ac[0].norm();
    if ac0 <= 0.0 {
        return None;
    }
    let max_lag = n.checked_sub(2048.max(n / 8))?;
    if max_lag < PERIOD_MIN_LAG {
        return None;
    }
    let mut best = (0usize, f64::NEG_INFINITY);
    for (i, v) in ac[PERIOD_MIN_LAG..=max_lag].iter().enumerate() {
        let tau = PERIOD_MIN_LAG + i;
        let v = v.norm() / ac0 * n as f64 / (n - tau) as f64;
        if v > best.1 {
            best = (tau, v);
        }
    }
    let lvl = 20.0 * best.1.max(1e-300).log10();
    (lvl >= level_db).then_some(best.0 as u64)
}

/// Coherent-to-incoherent band power with the pair aligned at `delay` (§7 band SNR).
pub(super) fn band_snr(
    ffts: &mut Ffts,
    pair: Pair<'_>,
    delay: i64,
    nseg: usize,
    lo: f64,
    hi: f64,
    fs: f64,
) -> f64 {
    let w = Window::Hann.coefficients(nseg);
    let bins = nseg / 2 + 1;
    let df = fs / nseg as f64;
    let b: Vec<f64> = (0..bins)
        .map(|k| band_weight(k as f64 * df, lo, hi))
        .collect();
    let mut sxx = vec![0.0; bins];
    let mut syy = vec![0.0; bins];
    let mut sxy = vec![ZERO; bins];
    let mut x = vec![ZERO; bins];
    let mut y = vec![ZERO; bins];
    let mut starts = Vec::new();
    segment_starts(pair.m.len(), nseg, nseg / 2, &mut starts);
    let mut k = 0;
    for &s in &starts {
        let r0 = pair.b + s as i64 - delay - pair.a;
        if r0 < 0 || r0 as usize + nseg > pair.r.len() {
            continue;
        }
        let rs = &pair.r[r0 as usize..r0 as usize + nseg];
        let ms = &pair.m[s..s + nseg];
        ffts.rfft(nseg, &mut x, |buf| {
            for ((o, v), w) in buf.iter_mut().zip(rs).zip(&w) {
                *o = v * w;
            }
        });
        ffts.rfft(nseg, &mut y, |buf| {
            for ((o, v), w) in buf.iter_mut().zip(ms).zip(&w) {
                *o = v * w;
            }
        });
        for i in 0..bins {
            sxx[i] += x[i].norm_sqr();
            syy[i] += y[i].norm_sqr();
            sxy[i] += x[i].conj() * y[i];
        }
        k += 1;
    }
    if k < 2 {
        return f64::NAN;
    }
    let (mut num, mut den) = (0.0, 0.0);
    for i in 0..bins {
        let coh = sxy[i].norm_sqr() / (sxx[i] * syy[i]).max(1e-300);
        num += b[i] * syy[i] * coh;
        den += b[i] * syy[i] * (1.0 - coh);
    }
    if num <= 0.0 {
        return f64::NEG_INFINITY;
    }
    10.0 * (num / den.max(1e-300 * num)).log10()
}

/// Shared handle to a cached grid shape.
pub(super) type Shape = Arc<GridShape>;
