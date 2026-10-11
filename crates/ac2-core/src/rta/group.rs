//! Bands of one rate and order filtered side by side.

use super::{BASE_ORDER, MAX_ORDER};
use crate::weighting::Biquad;

/// Bands run side by side in one inner loop.
pub(super) const LANES: usize = 4;

/// Up to [`LANES`] bands of the same rate and order. Each section is the band-pass biquad
/// with its gain factored out, b = (1, 0, −1), so the inner loop needs two multiplies per
/// section; the product of the section gains squared scales the accumulated energy.
#[derive(Debug, Clone)]
pub(super) struct Group {
    /// Band index per lane; unused lanes run a harmless all-zero-feedback section on the
    /// input and are never read.
    bands: [Option<usize>; LANES],
    a1: Vec<[f64; LANES]>,
    a2: Vec<[f64; LANES]>,
    s1: Vec<[f64; LANES]>,
    s2: Vec<[f64; LANES]>,
    pub(super) energy: [f64; LANES],
}

impl Group {
    pub(super) fn new(lanes: &[(usize, &[Biquad])]) -> Self {
        let nsec = lanes[0].1.len();
        let mut g = Self {
            bands: [None; LANES],
            a1: vec![[0.0; LANES]; nsec],
            a2: vec![[0.0; LANES]; nsec],
            s1: vec![[0.0; LANES]; nsec],
            s2: vec![[0.0; LANES]; nsec],
            energy: [0.0; LANES],
        };
        for (l, &(band, sections)) in lanes.iter().enumerate() {
            debug_assert_eq!(sections.len(), nsec);
            debug_assert!(sections.iter().all(|s| s.b[1] == 0.0 && s.b[2] == -s.b[0]));
            g.bands[l] = Some(band);
            for (j, s) in sections.iter().enumerate() {
                g.a1[j][l] = s.a[0];
                g.a2[j][l] = s.a[1];
            }
        }
        g
    }

    pub(super) fn process(&mut self, x: &[f64]) {
        match self.a1.len() {
            3 => self.run::<3>(x),
            4 => self.run::<4>(x),
            5 => self.run::<5>(x),
            6 => self.run::<6>(x),
            n => unreachable!("band order {n} outside {BASE_ORDER}..={MAX_ORDER}"),
        }
    }

    /// The cascade for a compile-time section count, with coefficients and state in local
    /// arrays so the per-sample loop works in registers.
    fn run<const S: usize>(&mut self, x: &[f64]) {
        let load = |v: &[[f64; LANES]]| -> [[f64; LANES]; S] {
            let mut a = [[0.0; LANES]; S];
            a.copy_from_slice(v);
            a
        };
        let (a1, a2) = (load(&self.a1), load(&self.a2));
        let (mut s1, mut s2) = (load(&self.s1), load(&self.s2));
        let mut e = self.energy;
        for &xi in x {
            let mut v = [xi; LANES];
            for j in 0..S {
                for l in 0..LANES {
                    // Transposed direct form II with b = (1, 0, −1).
                    let y = v[l] + s1[j][l];
                    s1[j][l] = s2[j][l] - a1[j][l] * y;
                    s2[j][l] = -v[l] - a2[j][l] * y;
                    v[l] = y;
                }
            }
            for l in 0..LANES {
                e[l] += v[l] * v[l];
            }
        }
        self.s1.copy_from_slice(&s1);
        self.s2.copy_from_slice(&s2);
        self.energy = e;
    }

    pub(super) fn reset(&mut self) {
        for s in self.s1.iter_mut().chain(self.s2.iter_mut()) {
            *s = [0.0; LANES];
        }
        self.energy = [0.0; LANES];
    }
}
