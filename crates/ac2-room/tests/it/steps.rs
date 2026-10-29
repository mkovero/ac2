use ac2_room::steps::{Step, assign, group};

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Every permutation of `0..n`, for a brute-force least pairing.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for p in permutations(n - 1) {
        for i in 0..=p.len() {
            let mut q = p.clone();
            q.insert(i, n - 1);
            out.push(q);
        }
    }
    out
}

/// The least summed distance pairing every `from` with a distinct `to`, by trying them all.
fn brute(from: &[[f64; 2]], to: &[[f64; 2]]) -> f64 {
    permutations(to.len())
        .into_iter()
        .map(|p| {
            (0..from.len())
                .map(|i| dist(from[i], to[p[i]]))
                .sum::<f64>()
        })
        .fold(f64::INFINITY, f64::min)
}

fn total(from: &[[f64; 2]], to: &[[f64; 2]], a: &[usize]) -> f64 {
    a.iter()
        .enumerate()
        .map(|(i, &j)| dist(from[i], to[j]))
        .sum()
}

#[test]
fn assign_crossing_mics_uncross() {
    // Two mics, two seats a row further on: each takes the one in front of it.
    let from = [[0.0, 0.0], [10.0, 0.0]];
    let to = [[10.0, 2.0], [0.0, 2.0]];
    assert_eq!(assign(&from, &to), vec![1, 0]);
}

#[test]
fn assign_is_least_against_brute_force() {
    // A fixed pseudo-random scatter: an exact pairing equals the best of every permutation.
    let mut s = 12345u64;
    let mut next = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((s >> 33) as f64) / f64::from(1u32 << 31) * 20.0
    };
    for (n, m) in [(2, 2), (3, 5), (4, 4), (3, 6), (1, 4)] {
        let from: Vec<[f64; 2]> = (0..n).map(|_| [next(), next()]).collect();
        let to: Vec<[f64; 2]> = (0..m).map(|_| [next(), next()]).collect();
        let a = assign(&from, &to);
        let mut seen = a.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), n, "distinct targets");
        assert!((total(&from, &to, &a) - brute(&from, &to)).abs() < 1e-9);
    }
}

#[test]
fn greedy_would_carry_further() {
    // Mics at 0, 2 and 10 m; seats at 1.9, -2 and 9 m. Taking the nearest seat mic by mic
    // sends the first to 1.9 and the second back to -2 (6.9 m in all); the least pairing
    // sends the first to -2 and the second to 1.9 (3.1 m).
    let from = [[0.0, 0.0], [2.0, 0.0], [10.0, 0.0]];
    let to = [[1.9, 0.0], [-2.0, 0.0], [9.0, 0.0]];
    let a = assign(&from, &to);
    assert!((total(&from, &to, &a) - brute(&from, &to)).abs() < 1e-9);
    assert_eq!(a, vec![1, 0, 2]);
}

#[test]
fn one_mic_keeps_the_walking_order() {
    let pts = [[0.0, 0.0], [9.0, 9.0], [1.0, 0.0]];
    let want: Vec<Step> = vec![vec![Some(0)], vec![Some(1)], vec![Some(2)]];
    assert_eq!(group(&pts, 1), want);
}

#[test]
fn two_mics_walk_two_rows_without_crossing() {
    // A row of seats at y = 0 and one at y = 4: mics start at P1 (left) and P2 (right), and
    // walk their own sides.
    let pts = [
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 4.0],
        [0.0, 4.0],
        [1.0, 8.0],
        [9.0, 8.0],
    ];
    let steps = group(&pts, 2);
    assert_eq!(
        steps,
        vec![
            vec![Some(0), Some(1)],
            vec![Some(3), Some(2)],
            vec![Some(4), Some(5)]
        ]
    );
}

#[test]
fn mics_not_dividing_points_sit_out_at_the_end() {
    // Five points, two mics: the last step places the mic nearest the last point.
    let pts = [[0.0, 0.0], [10.0, 0.0], [0.0, 2.0], [10.0, 2.0], [9.0, 4.0]];
    let steps = group(&pts, 2);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[2], vec![None, Some(4)]);
    // Every point once, every step at most one point per mic.
    let mut all: Vec<usize> = steps.iter().flatten().flatten().copied().collect();
    all.sort_unstable();
    assert_eq!(all, vec![0, 1, 2, 3, 4]);
}

#[test]
fn more_mics_than_points() {
    let pts = [[0.0, 0.0], [1.0, 0.0]];
    assert_eq!(group(&pts, 3), vec![vec![Some(0), Some(1), None]]);
    assert!(group(&[], 2).is_empty());
    assert!(group(&pts, 0).is_empty());
}

#[test]
fn grouping_is_deterministic_and_covers_every_point() {
    let mut s = 99u64;
    let mut next = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((s >> 33) as f64) / f64::from(1u32 << 31) * 30.0
    };
    let pts: Vec<[f64; 2]> = (0..23).map(|_| [next(), next()]).collect();
    for mics in 2..=4 {
        let a = group(&pts, mics);
        assert_eq!(a, group(&pts, mics));
        assert_eq!(a.len(), pts.len().div_ceil(mics));
        let mut all: Vec<usize> = a.iter().flatten().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, (0..pts.len()).collect::<Vec<_>>());
        assert!(a.iter().all(|s| s.len() == mics));
    }
}
