//! Tolerance comparison of numeric slices with readable failure reports.

use std::fmt;

use num_complex::Complex64;

/// Linear tolerance: element passes if `|actual − expected| ≤ abs + rel·|expected|`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    pub abs: f64,
    pub rel: f64,
}

impl Tolerance {
    pub fn abs(abs: f64) -> Self {
        Self { abs, rel: 0.0 }
    }

    pub fn rel(rel: f64) -> Self {
        Self { abs: 0.0, rel }
    }

    fn allowed(&self, expected_magnitude: f64) -> f64 {
        self.abs + self.rel * expected_magnitude
    }
}

/// dB-domain tolerance: both values are clamped to at least `floor_db`, then must agree
/// within `db`. The floor keeps deep notches and noise-floor bins, whose dB value is
/// dominated by rounding, from failing a comparison meant for the useful range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DbTolerance {
    pub db: f64,
    pub floor_db: f64,
}

/// `10·log10(p)` for a power-like quantity.
pub fn power_to_db(p: f64) -> f64 {
    10.0 * p.log10()
}

/// `20·log10(a)` for an amplitude-like quantity.
pub fn amplitude_to_db(a: f64) -> f64 {
    20.0 * a.log10()
}

/// One failing element.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub index: usize,
    /// Axis value (e.g. frequency) at `index`, if the comparison has an axis.
    pub axis: Option<f64>,
    pub expected: String,
    pub actual: String,
    pub error: f64,
    pub allowed: f64,
}

/// Report of a failed comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    pub label: String,
    pub criterion: String,
    pub axis_unit: String,
    /// `(expected_len, actual_len)` when the lengths differ; no element comparison then.
    pub length: Option<(usize, usize)>,
    pub total: usize,
    pub failed: usize,
    /// Element with the largest `error / allowed` (NaN errors rank highest).
    pub worst: Option<Failure>,
    /// The first few failures in index order.
    pub first: Vec<Failure>,
}

impl Mismatch {
    fn fmt_failure(&self, f: &mut fmt::Formatter<'_>, x: &Failure) -> fmt::Result {
        write!(f, "[{}]", x.index)?;
        if let Some(axis) = x.axis {
            write!(f, " at {axis} {}", self.axis_unit)?;
        }
        write!(
            f,
            ": expected {}, actual {}, |err| {:.3e} > allowed {:.3e}",
            x.expected, x.actual, x.error, x.allowed
        )
    }
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((e, a)) = self.length {
            return write!(
                f,
                "{}: length mismatch: expected {e} values, actual {a}",
                self.label
            );
        }
        write!(
            f,
            "{}: {} of {} values out of tolerance ({})",
            self.label, self.failed, self.total, self.criterion
        )?;
        if let Some(w) = &self.worst {
            write!(f, "\n  worst ")?;
            self.fmt_failure(f, w)?;
        }
        for x in &self.first {
            write!(f, "\n  ")?;
            self.fmt_failure(f, x)?;
        }
        if self.failed > self.first.len() {
            write!(f, "\n  ... and {} more", self.failed - self.first.len())?;
        }
        Ok(())
    }
}

impl std::error::Error for Mismatch {}

/// Panic with the mismatch report.
#[track_caller]
pub fn assert_ok(result: Result<(), Box<Mismatch>>) {
    if let Err(m) = result {
        panic!("{m}");
    }
}

/// Settings for one comparison: label for messages and an optional axis (frequency).
#[derive(Debug, Clone)]
pub struct Comparison<'a> {
    label: &'a str,
    axis: Option<&'a [f64]>,
    axis_unit: &'a str,
    max_listed: usize,
}

impl<'a> Comparison<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            axis: None,
            axis_unit: "",
            max_listed: 8,
        }
    }

    /// Report `axis[index]` (e.g. frequency in Hz) with each failure.
    pub fn with_axis(mut self, axis: &'a [f64], unit: &'a str) -> Self {
        self.axis = Some(axis);
        self.axis_unit = unit;
        self
    }

    /// How many failures are listed individually (default 8).
    pub fn max_listed(mut self, n: usize) -> Self {
        self.max_listed = n;
        self
    }

    /// Real values within a linear tolerance.
    pub fn close_f64(
        &self,
        expected: &[f64],
        actual: &[f64],
        tol: Tolerance,
    ) -> Result<(), Box<Mismatch>> {
        self.run(
            expected,
            actual,
            format!("abs {:e}, rel {:e}", tol.abs, tol.rel),
            |e, a| ((a - e).abs(), tol.allowed(e.abs())),
            |v| format!("{v:.17e}"),
        )
    }

    /// dB values within a dB tolerance (see [`DbTolerance`]).
    pub fn close_db(
        &self,
        expected_db: &[f64],
        actual_db: &[f64],
        tol: DbTolerance,
    ) -> Result<(), Box<Mismatch>> {
        self.run(
            expected_db,
            actual_db,
            format!("{} dB, floor {} dB", tol.db, tol.floor_db),
            |e, a| {
                // NaN must fail, so it is not clamped by max().
                let clamp = |v: f64| if v.is_nan() { v } else { v.max(tol.floor_db) };
                ((clamp(a) - clamp(e)).abs(), tol.db)
            },
            |v| format!("{v:.9} dB"),
        )
    }

    /// Complex values: `|actual − expected| ≤ abs + rel·|expected|`.
    pub fn close_c64(
        &self,
        expected: &[Complex64],
        actual: &[Complex64],
        tol: Tolerance,
    ) -> Result<(), Box<Mismatch>> {
        self.run(
            expected,
            actual,
            format!("complex, abs {:e}, rel {:e}", tol.abs, tol.rel),
            |e, a| ((a - e).norm(), tol.allowed(e.norm())),
            |v| format!("{:.12e}{:+.12e}i", v.re, v.im),
        )
    }

    fn run<T: Copy>(
        &self,
        expected: &[T],
        actual: &[T],
        criterion: String,
        check: impl Fn(T, T) -> (f64, f64),
        show: impl Fn(T) -> String,
    ) -> Result<(), Box<Mismatch>> {
        let mut report = Mismatch {
            label: self.label.to_owned(),
            criterion,
            axis_unit: self.axis_unit.to_owned(),
            length: None,
            total: expected.len(),
            failed: 0,
            worst: None,
            first: Vec::new(),
        };
        if expected.len() != actual.len() {
            report.length = Some((expected.len(), actual.len()));
            return Err(Box::new(report));
        }
        let mut worst_ratio = f64::NEG_INFINITY;
        for (i, (&e, &a)) in expected.iter().zip(actual).enumerate() {
            let (error, allowed) = check(e, a);
            // Written so that a NaN error fails.
            if error <= allowed {
                continue;
            }
            report.failed += 1;
            let failure = || Failure {
                index: i,
                axis: self.axis.and_then(|ax| ax.get(i).copied()),
                expected: show(e),
                actual: show(a),
                error,
                allowed,
            };
            let ratio = if error.is_nan() {
                f64::INFINITY
            } else {
                error / allowed
            };
            if report.worst.is_none() || ratio > worst_ratio {
                worst_ratio = ratio;
                report.worst = Some(failure());
            }
            if report.first.len() < self.max_listed {
                report.first.push(failure());
            }
        }
        if report.failed == 0 {
            Ok(())
        } else {
            Err(Box::new(report))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_passes_within_abs_plus_rel() {
        let c = Comparison::new("x");
        let tol = Tolerance {
            abs: 1e-3,
            rel: 1e-2,
        };
        // allowed at 10.0 is 1e-3 + 0.1
        assert!(c.close_f64(&[10.0, 0.0], &[10.1, 0.0009], tol).is_ok());
        assert!(c.close_f64(&[10.0], &[10.102], tol).is_err());
        assert!(c.close_f64(&[0.0], &[0.0011], tol).is_err());
    }

    #[test]
    fn linear_reports_index_axis_and_values() {
        let freqs = [0.0, 100.0, 200.0, 300.0];
        let c = Comparison::new("spectrum").with_axis(&freqs, "Hz");
        let err = c
            .close_f64(
                &[1.0, 2.0, 3.0, 4.0],
                &[1.0, 2.5, 3.0, 4.1],
                Tolerance::abs(0.01),
            )
            .expect_err("must fail");
        assert_eq!(err.failed, 2);
        assert_eq!(err.total, 4);
        let worst = err.worst.as_ref().expect("worst");
        assert_eq!(worst.index, 1);
        assert_eq!(worst.axis, Some(100.0));
        assert_eq!(
            err.first.iter().map(|f| f.index).collect::<Vec<_>>(),
            [1, 3]
        );
        let msg = err.to_string();
        assert!(msg.contains("spectrum: 2 of 4 values"), "{msg}");
        assert!(msg.contains("[1] at 100 Hz"), "{msg}");
        assert!(msg.contains("[3] at 300 Hz"), "{msg}");
        assert!(msg.contains("expected 2.0"), "{msg}");
        assert!(msg.contains("actual 2.5"), "{msg}");
    }

    #[test]
    fn nan_and_infinity_fail() {
        let c = Comparison::new("x");
        assert!(
            c.close_f64(&[1.0], &[f64::NAN], Tolerance::abs(1.0))
                .is_err()
        );
        assert!(
            c.close_f64(&[1.0], &[f64::INFINITY], Tolerance::rel(1.0))
                .is_err()
        );
        let db = DbTolerance {
            db: 0.1,
            floor_db: -100.0,
        };
        assert!(c.close_db(&[-10.0], &[f64::NAN], db).is_err());
        let err = c
            .close_f64(&[1.0, 1.0], &[1.5, f64::NAN], Tolerance::abs(0.1))
            .expect_err("must fail");
        assert_eq!(err.worst.expect("worst").index, 1, "NaN ranks as worst");
    }

    #[test]
    fn length_mismatch() {
        let err = Comparison::new("x")
            .close_f64(&[1.0, 2.0], &[1.0], Tolerance::abs(1.0))
            .expect_err("must fail");
        assert_eq!(err.length, Some((2, 1)));
        assert!(err.to_string().contains("length mismatch"));
    }

    #[test]
    fn db_tolerance_and_floor() {
        let c = Comparison::new("psd");
        let tol = DbTolerance {
            db: 0.05,
            floor_db: -120.0,
        };
        assert!(c.close_db(&[-3.0, -6.0], &[-3.04, -5.96], tol).is_ok());
        assert!(c.close_db(&[-3.0], &[-3.06], tol).is_err());
        // Both below the floor: equal after clamping.
        assert!(c.close_db(&[-200.0], &[-150.0], tol).is_ok());
        // Above vs below the floor: compared against the floor.
        assert!(c.close_db(&[-119.0], &[-300.0], tol).is_err());
        assert!(c.close_db(&[f64::NEG_INFINITY], &[-130.0], tol).is_ok());
        assert_eq!(power_to_db(100.0), 20.0);
        assert_eq!(amplitude_to_db(10.0), 20.0);
    }

    #[test]
    fn complex_tolerance_uses_modulus() {
        let c = Comparison::new("h1");
        let e = [Complex64::new(3.0, 4.0)];
        // |diff| = 0.05, allowed = 0 + 0.01 * 5
        assert!(
            c.close_c64(&e, &[Complex64::new(3.03, 4.04)], Tolerance::rel(0.01))
                .is_ok()
        );
        let err = c
            .close_c64(&e, &[Complex64::new(3.0, 4.1)], Tolerance::rel(0.01))
            .expect_err("must fail");
        let msg = err.to_string();
        assert!(msg.contains("complex"), "{msg}");
        assert!(msg.contains('i'), "{msg}");
    }

    #[test]
    fn listing_is_capped() {
        let e = vec![0.0; 20];
        let a = vec![1.0; 20];
        let err = Comparison::new("x")
            .max_listed(3)
            .close_f64(&e, &a, Tolerance::abs(0.5))
            .expect_err("must fail");
        assert_eq!(err.first.len(), 3);
        assert!(err.to_string().contains("... and 17 more"));
    }

    #[test]
    #[should_panic(expected = "x: 1 of 1 values")]
    fn assert_ok_panics_with_report() {
        assert_ok(Comparison::new("x").close_f64(&[0.0], &[1.0], Tolerance::abs(0.5)));
    }
}
