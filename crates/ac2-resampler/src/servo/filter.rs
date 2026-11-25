//! The servo's four-state Kalman filter: phase, rate, stamp-path wander and audio bias, in
//! device samples (rate in device samples per second of media time).

// Matrix products read as their formulas with indices; iterator chains would hide them.
#![allow(clippy::needless_range_loop)]

/// Device phase against the nominal line, samples.
pub(super) const THETA: usize = 0;
/// Rate of `THETA`, samples per second.
pub(super) const RATE: usize = 1;
/// Stamp-path wander, samples.
pub(super) const WANDER: usize = 2;
/// Audio bias (path delay and interface constants), samples.
pub(super) const BIAS: usize = 3;

const N: usize = 4;

/// Process noise, in the filter's units.
#[derive(Clone, Copy, Debug)]
pub(super) struct Model {
    /// Spectral density of the rate's random walk, samples²/s³.
    pub q_rate: f64,
    /// Stationary variance of the wander, samples².
    pub wander_var: f64,
    /// Correlation time of the wander, seconds.
    pub wander_tau: f64,
    /// Spectral density of the bias's random walk, samples²/s.
    pub q_bias: f64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Filter {
    pub x: [f64; N],
    pub p: [[f64; N]; N],
}

impl Filter {
    /// The state `dt` seconds on (`dt ≥ 0`).
    pub fn predict(&mut self, dt: f64, m: &Model) {
        let a = (-dt / m.wander_tau).exp();
        self.x[THETA] += dt * self.x[RATE];
        self.x[WANDER] *= a;
        // P ← F P Fᵀ with F = I except F[θ][r] = dt and F[w][w] = a: rows first, then columns.
        let p = &mut self.p;
        for j in 0..N {
            p[THETA][j] += dt * p[RATE][j];
            p[WANDER][j] *= a;
        }
        for row in p.iter_mut() {
            row[THETA] += dt * row[RATE];
            row[WANDER] *= a;
        }
        // An integrated random walk: the rate's walk reaches the phase as dt³/3 and dt²/2.
        p[THETA][THETA] += m.q_rate * dt * dt * dt / 3.0;
        p[THETA][RATE] += m.q_rate * dt * dt / 2.0;
        p[RATE][THETA] += m.q_rate * dt * dt / 2.0;
        p[RATE][RATE] += m.q_rate * dt;
        p[WANDER][WANDER] += m.wander_var * (1.0 - a * a);
        p[BIAS][BIAS] += m.q_bias * dt;
    }

    /// `h·x`.
    pub fn project(&self, h: &[f64; N]) -> f64 {
        (0..N).map(|i| h[i] * self.x[i]).sum()
    }

    /// `h P hᵀ`.
    pub fn variance(&self, h: &[f64; N]) -> f64 {
        let ph = self.p_times(h);
        (0..N).map(|i| h[i] * ph[i]).sum()
    }

    fn p_times(&self, h: &[f64; N]) -> [f64; N] {
        let mut out = [0.0; N];
        for (o, row) in out.iter_mut().zip(&self.p) {
            *o = (0..N).map(|k| row[k] * h[k]).sum();
        }
        out
    }

    /// The innovation `z − h·x` and its variance `h P hᵀ + r`.
    pub fn innovation(&self, h: &[f64; N], z: f64, r: f64) -> (f64, f64) {
        (z - self.project(h), self.variance(h) + r)
    }

    /// A scalar measurement `z = h·x + v`, `var v = r`, in Joseph form, which keeps P
    /// symmetric and positive however small `r` is against P.
    pub fn update(&mut self, h: &[f64; N], z: f64, r: f64) {
        let (nu, s) = self.innovation(h, z, r);
        let ph = self.p_times(h);
        let k: [f64; N] = std::array::from_fn(|i| ph[i] / s);
        for i in 0..N {
            self.x[i] += k[i] * nu;
        }
        // A = I − k hᵀ; P ← A P Aᵀ + k r kᵀ.
        let a: [[f64; N]; N] =
            std::array::from_fn(|i| std::array::from_fn(|j| f64::from(i == j) - k[i] * h[j]));
        let mut ap = [[0.0; N]; N];
        for i in 0..N {
            for j in 0..N {
                ap[i][j] = (0..N).map(|l| a[i][l] * self.p[l][j]).sum();
            }
        }
        for i in 0..N {
            for j in 0..N {
                self.p[i][j] = (0..N).map(|l| ap[i][l] * a[j][l]).sum::<f64>() + k[i] * r * k[j];
            }
        }
        self.symmetrise();
    }

    /// Sets the bias from one audio measurement `z = g·x + bias + v` (`g[BIAS] = 0`), with the
    /// covariance that follows: the new bias errs by minus the error of `g·x` minus `v`, so it
    /// is correlated with the states `g` reads. A later audio measurement then predicts
    /// itself with variance `r` alone: audio tells about changes from here on, nothing about
    /// where `θ` was.
    pub fn reset_bias(&mut self, g: &[f64; N], z: f64, r: f64) {
        debug_assert!(g[BIAS] == 0.0);
        self.x[BIAS] = z - self.project(g);
        let pg = self.p_times(g);
        let var = self.variance(g) + r;
        for j in 0..N {
            self.p[BIAS][j] = -pg[j];
            self.p[j][BIAS] = -pg[j];
        }
        self.p[BIAS][BIAS] = var;
    }

    fn symmetrise(&mut self) {
        for i in 0..N {
            for j in i + 1..N {
                let m = 0.5 * (self.p[i][j] + self.p[j][i]);
                self.p[i][j] = m;
                self.p[j][i] = m;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STILL: Model = Model {
        q_rate: 0.0,
        wander_var: 0.0,
        wander_tau: 1.0,
        q_bias: 0.0,
    };

    fn diag(v: [f64; N]) -> Filter {
        Filter {
            x: [0.0; N],
            p: std::array::from_fn(|i| std::array::from_fn(|j| if i == j { v[i] } else { 0.0 })),
        }
    }

    #[test]
    fn two_measurements_of_a_line_give_its_slope() {
        let mut f = diag([1e6, 1e6, 0.0, 0.0]);
        let h = [1.0, 0.0, 0.0, 0.0];
        f.update(&h, 2.0, 1e-12);
        f.predict(10.0, &STILL);
        f.update(&h, 7.0, 1e-12);
        assert!((f.x[THETA] - 7.0).abs() < 1e-6);
        assert!((f.x[RATE] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn reset_bias_leaves_only_the_measurement_noise_for_the_next_one() {
        let mut f = diag([4.0, 1.0, 0.0, 0.0]);
        f.p[THETA][RATE] = 0.5;
        f.p[RATE][THETA] = 0.5;
        let g = [1.0, -2.0, 0.0, 0.0];
        f.reset_bias(&g, 10.0, 0.25);
        let h = [1.0, -2.0, 0.0, 1.0];
        let (nu, s) = f.innovation(&h, 10.0, 0.25);
        assert!(nu.abs() < 1e-12);
        assert!((s - 0.5).abs() < 1e-12, "{s}");
    }
}
