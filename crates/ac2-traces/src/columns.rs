//! Column data of a stored trace and resampling between grids.

use ac2_proto::GridDef;
use ac2_proto::model::{TraceData, TraceMeta};

/// A trace's columns, in grid order. NaN = no value in that column (a gap, never bridged).
#[derive(Debug, Clone, PartialEq)]
pub struct Columns {
    /// Magnitude or level, dB.
    pub mag_db: Vec<f32>,
    /// Phase, degrees in (−180, 180]; `None` for magnitude-only traces.
    pub phase_deg: Option<Vec<f32>>,
    /// Coherence γ² (0…1); `None` when not known.
    pub coherence: Option<Vec<f32>>,
}

impl Columns {
    /// Number of columns.
    pub fn len(&self) -> usize {
        self.mag_db.len()
    }

    /// No columns.
    pub fn is_empty(&self) -> bool {
        self.mag_db.is_empty()
    }

    /// Every optional array has as many columns as the magnitude.
    pub fn consistent(&self) -> bool {
        let n = self.mag_db.len();
        self.phase_deg.as_ref().is_none_or(|p| p.len() == n)
            && self.coherence.as_ref().is_none_or(|c| c.len() == n)
    }
}

/// A trace as the store keeps it: metadata, its grid and its columns.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredTrace {
    /// Metadata (the mirrored entity).
    pub meta: TraceMeta,
    /// Grid of the columns; `meta.grid_id` is its id.
    pub grid: GridDef,
    /// Data.
    pub columns: Columns,
}

impl StoredTrace {
    /// The columns as displayed: the stored ones with the trace's display smoothing.
    pub fn display_columns(&self) -> Columns {
        match self.meta.edit.smoothing {
            Some(s) if crate::smooth::smoothable(self.meta.kind) => {
                crate::smooth::smooth(&self.grid, &self.columns, s)
            }
            _ => self.columns.clone(),
        }
    }

    /// The `trace.get` reply: display columns (smoothing applied).
    pub fn data(&self) -> TraceData {
        let c = self.display_columns();
        TraceData {
            meta: self.meta.clone(),
            mag_db: c.mag_db,
            phase_deg: c.phase_deg,
            coherence: c.coherence,
        }
    }
}

/// Centre frequency of every column of `g`, in column order.
pub fn frequencies(g: &GridDef) -> Vec<f64> {
    match g {
        GridDef::Log { ppo, k_min, k_max } => (*k_min..=*k_max)
            .map(|k| 1000.0 * 2f64.powf(f64::from(k) / f64::from(*ppo)))
            .collect(),
        GridDef::IecBands { centres, .. } => centres.iter().map(|c| c.0).collect(),
        GridDef::Linear { fs, n } => (0..=*n / 2)
            .map(|k| f64::from(k) * fs.0 / f64::from(*n))
            .collect(),
    }
}

/// Unwraps degrees along the columns (NaN gaps restart nothing: the next finite value is
/// unwrapped against the last finite one).
fn unwrap_deg(p: &[f32]) -> Vec<f64> {
    let mut out = Vec::with_capacity(p.len());
    let mut last: Option<f64> = None;
    for &v in p {
        let v = f64::from(v);
        if !v.is_finite() {
            out.push(f64::NAN);
            continue;
        }
        let u = match last {
            None => v,
            Some(l) => v + 360.0 * ((l - v) / 360.0).round(),
        };
        last = Some(u);
        out.push(u);
    }
    out
}

/// Wraps degrees into (−180, 180].
pub fn wrap_deg(d: f64) -> f64 {
    let w = (d + 180.0).rem_euclid(360.0) - 180.0;
    if w == -180.0 { 180.0 } else { w }
}

/// Linear interpolation of `y` (sampled at ascending `x`) at `at`; NaN outside the sampled
/// range or where a neighbour is NaN. `x` is log frequency, so magnitude in dB is
/// interpolated linearly over log frequency — how response curves are drawn and how target
/// curves are specified.
fn interp(x: &[f64], y: &[f64], at: f64) -> f64 {
    let n = x.len();
    if n == 0 || !at.is_finite() || at < x[0] || at > x[n - 1] {
        return f64::NAN;
    }
    let i = x.partition_point(|v| *v < at);
    if i < n && x[i] == at {
        return y[i];
    }
    if i == 0 || i >= n {
        return f64::NAN;
    }
    let (x0, x1, y0, y1) = (x[i - 1], x[i], y[i - 1], y[i]);
    y0 + (y1 - y0) * (at - x0) / (x1 - x0)
}

/// Resamples columns given at frequencies `from` (ascending, positive) onto `to`.
/// Magnitude and coherence interpolate linearly over log frequency; phase is unwrapped,
/// interpolated and wrapped again so a wrap between two columns does not average to a
/// false 0°. Columns outside the source range (and 0 Hz) are NaN.
pub fn resample(from: &[f64], c: &Columns, to: &[f64]) -> Columns {
    let lx: Vec<f64> = from
        .iter()
        .map(|f| if *f > 0.0 { f.ln() } else { f64::NEG_INFINITY })
        .collect();
    // Columns at or below 0 Hz cannot sit on a log axis; drop them from the source.
    let first = lx.iter().position(|v| v.is_finite()).unwrap_or(lx.len());
    let lx = &lx[first..];
    let at: Vec<f64> = to
        .iter()
        .map(|f| if *f > 0.0 { f.ln() } else { f64::NAN })
        .collect();
    let run =
        |y: Vec<f64>| -> Vec<f64> { at.iter().map(|a| interp(lx, &y[first..], *a)).collect() };
    let to32 = |v: Vec<f64>| v.into_iter().map(|x| x as f32).collect::<Vec<f32>>();
    Columns {
        mag_db: to32(run(c.mag_db.iter().map(|v| f64::from(*v)).collect())),
        phase_deg: c.phase_deg.as_ref().map(|p| {
            run(unwrap_deg(p))
                .into_iter()
                .map(|v| {
                    if v.is_finite() {
                        wrap_deg(v) as f32
                    } else {
                        f32::NAN
                    }
                })
                .collect()
        }),
        coherence: c
            .coherence
            .as_ref()
            .map(|k| to32(run(k.iter().map(|v| f64::from(*v)).collect()))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn log_grid_frequencies() {
        let f = frequencies(&GridDef::Log {
            ppo: 3,
            k_min: -3,
            k_max: 3,
        });
        assert_eq!(f.len(), 7);
        assert!((f[0] - 500.0).abs() < 1e-9);
        assert!((f[3] - 1000.0).abs() < 1e-9);
        assert!((f[6] - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn resample_is_exact_on_shared_points_and_linear_in_log_f() {
        let from = [100.0, 1000.0, 10_000.0];
        let c = Columns {
            mag_db: vec![0.0, -10.0, -20.0],
            phase_deg: Some(vec![170.0, -170.0, -150.0]),
            coherence: Some(vec![1.0, 0.5, 0.0]),
        };
        let to = [50.0, 100.0, 316.227_766, 1000.0, 20_000.0];
        let r = resample(&from, &c, &to);
        assert!(r.mag_db[0].is_nan() && r.mag_db[4].is_nan());
        assert_eq!(r.mag_db[1], 0.0);
        assert!((r.mag_db[2] + 5.0).abs() < 1e-4, "{}", r.mag_db[2]);
        assert_eq!(r.mag_db[3], -10.0);
        // 170° → −170° is a 20° step across the wrap, not a 340° swing through 0°.
        let p = r.phase_deg.unwrap();
        assert!((p[2] - 180.0).abs() < 1e-3, "{}", p[2]);
        let k = r.coherence.unwrap();
        assert!((k[2] - 0.75).abs() < 1e-4);
    }

    #[test]
    fn wrap() {
        assert_eq!(wrap_deg(180.0), 180.0);
        assert_eq!(wrap_deg(-180.0), 180.0);
        assert_eq!(wrap_deg(190.0), -170.0);
        assert_eq!(wrap_deg(-540.0), 180.0);
    }
}
