//! The decision run of one finder call: acquisition, refinement and the first-arrival pick.

use std::sync::Arc;

use arrayvec::ArrayVec;
use num_complex::Complex64;

use super::candidates::{Lobe, RAYLEIGH_MEDIAN, deblend, detection_floor, local_maxima, parabolic};
use super::estimator::{Model, Pulse, Reg, Shape, band_snr, detect_period};
use super::{AmbiguityReason, Arrival, Confidence, NoEstimateReason, Outcome, Run, SearchRange};

/// A candidate while the decision is made.
#[derive(Debug, Clone, Copy)]
struct Cand {
    delay: f64,
    delay_int: i64,
    level_db: f64,
    phase_deg: f64,
    misfit: f64,
    refined: bool,
    uncertainty: f64,
    /// σ_n / |A|.
    noise_ratio: f64,
}

impl Cand {
    fn arrival(&self) -> Arrival {
        Arrival {
            delay: self.delay_int,
            delay_frac: self.delay,
            level_db: self.level_db,
            phase_deg: self.phase_deg,
            uncertainty: self.uncertainty,
            misfit: self.misfit,
            refined: self.refined,
        }
    }
}

/// Pulse model in use (from the first refinement tile, else from acquisition).
struct ModelSrc {
    pulse: Pulse,
    grid: Shape,
}

/// Python-style round half to even, so integer delays match the reference prototype.
fn round_lag(x: f64) -> i64 {
    x.round_ties_even() as i64
}

impl Run<'_, '_> {
    /// Lags searched.
    fn search(&self) -> SearchRange {
        self.local.map_or(self.cfg.search, |l| l.search)
    }

    pub(super) fn find(&mut self) -> (Outcome, Vec<Arrival>, Confidence) {
        let cfg = self.cfg;
        let t = cfg.tuning;
        let reg = Reg {
            estimator: cfg.estimator,
            eps: t.eps,
        };
        let fs = cfg.fs;
        let class = cfg.band.class();
        let (lo, hi) = cfg.band.edges(fs);
        let search = self.search();
        let (d_min, d_max) = (search.min, search.max);
        let mut conf = Confidence::unknown();
        let mut reasons = Vec::new();
        let refuse = |reasons, conf| (Outcome::NoEstimate { reasons }, Vec::new(), conf);

        if !self.pair.r.iter().any(|v| *v != 0.0) {
            reasons.push(NoEstimateReason::NoReference);
        }
        if !self.pair.m.iter().any(|v| *v != 0.0) {
            reasons.push(NoEstimateReason::NoSignal);
        }
        if !reasons.is_empty() {
            return refuse(reasons, conf);
        }
        let n1 = class.segment(fs);
        let n2 = 2 * n1;
        if self.pair.m.len() < n2 {
            return refuse(vec![NoEstimateReason::ObservationTooShort], conf);
        }

        // ---- pass 1: acquisition over the whole search range (§4.3) ----------------------
        let g1 = self.s.shape(n1, n1 / 2, fs, lo, hi);
        let w = &mut *self.w;
        let ffts = &mut self.s.ffts;
        w.y1.compute(ffts, &g1, self.pair.m);
        let n_lags = (d_max - d_min + 1) as usize;
        w.hcx.clear();
        w.hcx.resize(n_lags, Complex64::new(f64::NAN, f64::NAN));
        w.kseg.clear();
        w.kseg.resize(n_lags, 0);
        let mut gxx1 = vec![0.0; g1.bins()];
        let step = (n1 / 4) as i64;
        let half = step / 2;
        let mut d0 = d_min + half;
        w.refs.reset(&g1, w.y1.segments());
        while d0 < d_max + step {
            w.tile
                .compute(ffts, &g1, &w.y1, self.pair, d0, reg, Some(&mut w.refs));
            if w.tile.k > 0 {
                for d in (d0 - half).max(d_min)..=(d0 - half + step - 1).min(d_max) {
                    let i = (d - d_min) as usize;
                    w.hcx[i] = w.tile.at(&g1, d - d0);
                    w.kseg[i] = w.tile.k;
                }
                for (acc, v) in gxx1.iter_mut().zip(&w.tile.gxx) {
                    *acc += v;
                }
            }
            d0 += step;
        }
        // a lag whose ref is missing for more than half the meas segments is invalid (§4.1)
        let k_need = w.y1.segments().div_ceil(2).max(1);
        w.valid.clear();
        w.valid.extend(w.kseg.iter().map(|&k| k >= k_need));
        let n_valid = w.valid.iter().filter(|v| **v).count();
        if n_valid == 0 {
            return refuse(vec![NoEstimateReason::InsufficientOverlap], conf);
        }
        w.env.clear();
        w.env.extend(
            w.hcx
                .iter()
                .zip(&w.valid)
                .map(|(h, &v)| if v { h.norm() } else { 0.0 }),
        );

        let pulse1 = Pulse::new(ffts, &g1, Some(&gxx1), reg);
        let env_valid: Vec<f64> = w
            .env
            .iter()
            .zip(&w.valid)
            .filter_map(|(e, &v)| v.then_some(*e))
            .collect();
        // a local search samples the floor near the arrivals but keeps the full search's
        // false-peak level, so its gates match the full search's
        let region = match self.local {
            Some(_) => cfg.search.len() as usize,
            None => env_valid.len(),
        };
        let (med1, det1) = detection_floor(
            &env_valid,
            region,
            pulse1.b_eff(g1.df()),
            fs,
            t.p_fa,
            t.floor_excl_k * pulse1.width,
            &mut w.tmp,
        );
        let i_s1 = argmax(&w.env);
        let e_s1 = w.env[i_s1];
        conf.psr_acq_db = if det1 > 0.0 {
            20.0 * (e_s1 / det1).log10()
        } else {
            f64::INFINITY
        };
        if e_s1 <= 0.0 || e_s1.is_nan() {
            return refuse(vec![NoEstimateReason::NoSignal], conf);
        }

        // ---- pass 2: refinement tiles (§4.4) ------------------------------------------------
        w.det.clear();
        w.det.resize(n_lags, det1);
        w.med.clear();
        w.med.resize(n_lags, med1);
        w.in_win.clear();
        w.in_win.resize(n_lags, false);
        let g2 = self.s.shape(n2, n2 / 4, fs, lo, hi);
        self.w.y2.compute(&mut self.s.ffts, &g2, self.pair.m);
        let mut model = ModelSrc {
            pulse: pulse1,
            grid: Arc::clone(&g1),
        };
        let mut windows: Vec<(i64, i64)> = Vec::new();
        self.refine_at(d_min + i_s1 as i64, &g2, &mut model, &mut windows);

        let width = model.pulse.width;
        conf.refinement_window = windows.first().copied();
        conf.pulse_width = width;
        let flat = Pulse::new(&mut self.s.ffts, &model.grid, None, reg);
        conf.nominal_width = flat.width;
        conf.excited_fraction = model.pulse.excited_fraction(&model.grid);

        // ---- refusals that do not depend on the IR (§7) -------------------------------------
        if conf.excited_fraction < t.min_excited_fraction {
            reasons.push(NoEstimateReason::InsufficientExcitation);
        }
        let period = match self.local {
            Some(l) => l.period,
            None => cfg.excitation_period.or_else(|| {
                detect_period(
                    &mut self.s.ffts,
                    &mut self.w.period,
                    self.pair.r,
                    fs,
                    t.eps,
                    t.periodic_db,
                )
            }),
        };
        conf.period = period;
        if let Some(p) = period
            && (p as f64) <= cfg.search.span() as f64 + cfg.tail_s * fs
        {
            reasons.push(NoEstimateReason::PeriodicExcitation { period: p });
        }

        // ---- candidates (§5, §6) -------------------------------------------------------------
        let reach = t.deblend_k * width;
        let om = Model::new(&mut self.s.ffts, &model.pulse.pw, reach + 1.5 * width + 3.0);
        let mut cands = self.candidates(&model.pulse, &om);
        if let Some(first) = cands.iter().find(|c| c.level_db >= cfg.threshold_db)
            && !first.refined
        {
            let d = first.delay_int;
            self.refine_at(d, &g2, &mut model, &mut windows);
            cands = self.candidates(&model.pulse, &om);
        }

        let w = &*self.w;
        let i_max = argmax(&w.env);
        let e_max = w.env[i_max];
        let det_s = w.det[i_max];
        conf.psr_db = if det_s > 0.0 {
            20.0 * (e_max / det_s).log10()
        } else {
            f64::INFINITY
        };
        if conf.psr_acq_db < t.psr_acq_min_db || conf.psr_db < t.psr_min_db {
            reasons.push(NoEstimateReason::LowPsr);
        }
        let arrivals: Vec<Arrival> = cands.iter().map(Cand::arrival).collect();
        let Some(si) = argmax_by(&cands, |c| c.level_db) else {
            if !reasons.contains(&NoEstimateReason::LowPsr) {
                reasons.push(NoEstimateReason::LowPsr);
            }
            return (Outcome::NoEstimate { reasons }, arrivals, conf);
        };
        let strongest = cands[si];
        let pi = cands
            .iter()
            .position(|c| c.level_db >= cfg.threshold_db)
            .unwrap_or(si);
        let pick = cands[pi];

        if t.precision_k * pick.uncertainty > class.tolerance_samples(fs) {
            reasons.push(NoEstimateReason::LowPrecision);
        }
        if pick.delay_int - d_min < t.edge_guard || d_max - strongest.delay_int < t.edge_guard {
            reasons.push(NoEstimateReason::PeakAtSearchEdge);
        }
        conf.band_snr_db = band_snr(
            &mut self.s.ffts,
            self.pair,
            strongest.delay_int,
            n1,
            lo,
            hi,
            fs,
        );
        if conf.band_snr_db.is_nan() || conf.band_snr_db < t.band_snr_min_db {
            reasons.push(NoEstimateReason::LowBandSnr);
        }
        if !reasons.is_empty() {
            return (Outcome::NoEstimate { reasons }, arrivals, conf);
        }

        // ---- ambiguity (§8) ------------------------------------------------------------------
        let mut amb = Vec::new();
        let clear_lvl = cfg.threshold_db + t.borderline_db;
        let ci = cands
            .iter()
            .position(|c| c.level_db >= clear_lvl)
            .unwrap_or(si);
        let pick_clear = cands[ci];
        let unsure: Vec<usize> = (0..cands.len())
            .filter(|&i| {
                let c = &cands[i];
                c.level_db >= cfg.threshold_db - t.borderline_db
                    && c.level_db < clear_lvl
                    && c.delay < pick_clear.delay
            })
            .collect();
        if !unsure.is_empty() {
            amb.push(AmbiguityReason::BorderlineLevel);
        }
        let close: Vec<usize> = (0..cands.len())
            .filter(|&i| {
                let c = &cands[i];
                i != pi
                    && (c.delay - pick.delay).abs() < t.close_k * width
                    && c.level_db >= t.close_depth_db
            })
            .collect();
        if !close.is_empty() {
            amb.push(AmbiguityReason::CloseArrivals);
        }
        if pick.misfit
            > class
                .merged_misfit()
                .max(t.merged_noise_k * pick.noise_ratio)
        {
            amb.push(AmbiguityReason::MergedLobe);
        }
        if !pick.refined {
            amb.push(AmbiguityReason::OutsideRefinement);
        }
        if amb.is_empty() {
            return (
                Outcome::Accepted {
                    first: pick.arrival(),
                    strongest: strongest.arrival(),
                },
                arrivals,
                conf,
            );
        }
        // ranked: the rule pick, pick_clear, the strongest, then unsure and close by level
        let mut rest: Vec<usize> = unsure.into_iter().chain(close).collect();
        rest.sort_by(|&x, &y| cands[y].level_db.total_cmp(&cands[x].level_db));
        let mut order = vec![pi];
        for i in [ci, si].into_iter().chain(rest) {
            if !order.contains(&i) {
                order.push(i);
            }
        }
        let ranked: ArrayVec<Arrival, 3> =
            order.iter().take(3).map(|&i| cands[i].arrival()).collect();
        (
            Outcome::Ambiguous {
                reasons: amb,
                ranked,
                strongest: strongest.arrival(),
            },
            arrivals,
            conf,
        )
    }

    /// Refinement tile at lag `d0` (§4.4): replaces the acquisition values in its window,
    /// sets its floor and, the first time, the pulse model.
    fn refine_at(
        &mut self,
        d0: i64,
        g2: &Shape,
        model: &mut ModelSrc,
        windows: &mut Vec<(i64, i64)>,
    ) {
        let cfg = self.cfg;
        let t = cfg.tuning;
        let reg = Reg {
            estimator: cfg.estimator,
            eps: t.eps,
        };
        let search = self.search();
        let (d_min, d_max) = (search.min, search.max);
        let w = &mut *self.w;
        let ffts = &mut self.s.ffts;
        w.tile.compute(ffts, g2, &w.y2, self.pair, d0, reg, None);
        if w.tile.k == 0 {
            return;
        }
        let r2 = (g2.nseg / 8) as i64;
        let lo = (d0 - r2).max(d_min);
        let hi = (d0 + r2).min(d_max);
        if lo > hi {
            return;
        }
        for d in lo..=hi {
            let i = (d - d_min) as usize;
            let h = w.tile.at(g2, d - d0);
            w.hcx[i] = h;
            w.env[i] = h.norm();
        }
        if windows.is_empty() {
            model.pulse = Pulse::new(ffts, g2, Some(&w.tile.gxx), reg);
            model.grid = Arc::clone(g2);
        }
        let (i0, i1) = ((lo - d_min) as usize, (hi - d_min) as usize);
        let (med, det) = detection_floor(
            &w.env[i0..=i1],
            i1 - i0 + 1,
            model.pulse.b_eff(g2.df()),
            cfg.fs,
            t.p_fa,
            t.floor_excl_k * model.pulse.width,
            &mut w.tmp,
        );
        w.det[i0..=i1].fill(det);
        w.med[i0..=i1].fill(med);
        w.in_win[i0..=i1].fill(true);
        windows.push((lo, hi));
    }

    /// Candidate list, by delay (§5.2, §5.3).
    fn candidates(&mut self, pulse: &Pulse, om: &Model) -> Vec<Cand> {
        let cfg = self.cfg;
        let t = cfg.tuning;
        let d_min = self.search().min;
        let w = &mut *self.w;
        let width = pulse.width;
        let e_max = w.env.iter().copied().fold(0.0, f64::max);
        let depth = e_max * 10f64.powf(t.list_depth_db / 20.0);
        let margin = 10f64.powf(t.sidelobe_margin_db / 20.0);
        local_maxima(&w.env, &mut w.maxima);
        let mut idx: Vec<usize> = w
            .maxima
            .iter()
            .copied()
            .filter(|&j| w.valid[j] && w.env[j] >= w.det[j].max(depth))
            .collect();
        idx.sort_by(|&x, &y| w.env[y].total_cmp(&w.env[x]));
        let mut kept: Vec<usize> = Vec::new();
        for &j in &idx {
            let explained = kept.iter().any(|&s| {
                let dl = j as i64 - s as i64;
                (dl.abs() as f64) < width || w.env[j] <= w.env[s] * pulse.env_at(dl) * margin
            });
            if !explained {
                kept.push(j);
            }
        }
        let mut taus: Vec<f64> = kept
            .iter()
            .map(|&j| {
                let e = &w.env;
                let frac = if j > 0 && j + 1 < e.len() && e[j - 1] > 0.0 && e[j + 1] > 0.0 {
                    parabolic(e[j - 1].ln(), e[j].ln(), e[j + 1].ln())
                } else {
                    0.0
                };
                j as f64 + frac
            })
            .collect();
        let mut amps: Vec<Complex64> = kept.iter().map(|&j| w.hcx[j]).collect();
        let mut misfit = vec![0.0; kept.len()];
        let win: Vec<usize> = (0..kept.len()).filter(|&a| w.in_win[kept[a]]).collect();
        if !win.is_empty() {
            let mut lobes: Vec<Lobe> = win
                .iter()
                .map(|&a| Lobe {
                    j: kept[a],
                    tau: taus[a],
                    amp: amps[a],
                    misfit: 0.0,
                })
                .collect();
            deblend(&w.hcx, &mut lobes, om, width, t.deblend_k * width);
            for (&a, l) in win.iter().zip(&lobes) {
                taus[a] = l.tau;
                amps[a] = l.amp;
                misfit[a] = l.misfit;
            }
        }
        let lv: Vec<f64> = kept
            .iter()
            .enumerate()
            .map(|(a, &j)| {
                if w.in_win[j] {
                    amps[a].norm()
                } else {
                    w.env[j]
                }
            })
            .collect();
        let l_max = if lv.is_empty() {
            e_max
        } else {
            lv.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        };
        let mut out: Vec<Cand> = kept
            .iter()
            .enumerate()
            .map(|(a, &j)| {
                let delay = d_min as f64 + taus[a];
                let l = lv[a].max(1e-300);
                let noise_ratio = (w.med[j] / RAYLEIGH_MEDIAN) / l;
                Cand {
                    delay,
                    delay_int: round_lag(delay),
                    level_db: 20.0 * (l / l_max).log10(),
                    phase_deg: amps[a].arg().to_degrees(),
                    misfit: misfit[a],
                    refined: w.in_win[j],
                    uncertainty: (t.uncertainty_coef * width * noise_ratio)
                        .max(t.uncertainty_floor),
                    noise_ratio,
                }
            })
            .collect();
        out.sort_by(|x, y| x.delay.total_cmp(&y.delay));
        out
    }
}

/// Index of the first maximum.
fn argmax(v: &[f64]) -> usize {
    let mut best = 0;
    for (i, x) in v.iter().enumerate() {
        if *x > v[best] {
            best = i;
        }
    }
    best
}

fn argmax_by<T>(v: &[T], key: impl Fn(&T) -> f64) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, x) in v.iter().enumerate() {
        if best.is_none_or(|b| key(x) > key(&v[b])) {
            best = Some(i);
        }
    }
    best
}
