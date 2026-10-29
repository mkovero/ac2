//! A room parameter over several measurement positions (ISO 3382-1 §7: the spatial average
//! is the arithmetic mean of the positions' values, reported with their spread).
//!
//! A position whose decay does not give the parameter in a band (too little range, too
//! short for the filter, no decay) has no value there; averaging it in as anything would
//! invent one, so it is left out and the count says how many positions the mean is of.

/// A parameter over the positions that give it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionSpread {
    /// Arithmetic mean.
    pub mean: f64,
    /// Least.
    pub min: f64,
    /// Greatest.
    pub max: f64,
    /// How many positions give it.
    pub n: usize,
}

/// The mean, least and greatest of the positions' `values` that are given (`Some` and
/// finite); `None` when no position gives the parameter.
pub fn spread(values: &[Option<f64>]) -> Option<PositionSpread> {
    let given: Vec<f64> = values
        .iter()
        .filter_map(|v| v.filter(|x| x.is_finite()))
        .collect();
    let n = given.len();
    if n == 0 {
        return None;
    }
    let (min, max) = given
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| {
            (lo.min(x), hi.max(x))
        });
    Some(PositionSpread {
        mean: given.iter().sum::<f64>() / n as f64,
        min,
        max,
        n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_and_range_of_the_positions_that_give_it() {
        let s = spread(&[Some(1.0), None, Some(1.4), Some(1.2)]).expect("three give it");
        assert!((s.mean - 1.2).abs() < 1e-12);
        assert_eq!((s.min, s.max, s.n), (1.0, 1.4, 3));
    }

    #[test]
    fn no_position_gives_it() {
        assert_eq!(spread(&[None, None]), None);
        assert_eq!(spread(&[]), None);
        assert_eq!(spread(&[Some(f64::NAN)]), None);
    }

    #[test]
    fn one_position_is_its_own_mean_and_range() {
        let s = spread(&[None, Some(-2.5)]).expect("one gives it");
        assert_eq!((s.mean, s.min, s.max, s.n), (-2.5, -2.5, -2.5, 1));
    }
}
