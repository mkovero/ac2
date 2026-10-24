//! EBU Tech 3342 cases 1–4 and case 1 repeated: LRA at the end of the signal (no trailing
//! silence fed), within ±1 LU of the published value and within the histogram's bound of
//! the unbinned reference, at every rate.

use super::{RATES, assert_near, param, run, set};

/// Half a bin on each percentile, and a gate-bin block at most.
const RANGE_PARITY_LU: f64 = 0.02;

fn case(name: &str) {
    let g = set("loudness_ebu3342");
    let c = param(&g, &format!("cases/{name}"));
    let published = c["published"].as_f64().expect("published");
    for fs in RATES {
        let r = run(&c["programme"], fs);
        let range = r.range.expect("range");
        let reference = g.scalar(&format!("c{name}_{fs}_range")).expect("scalar");
        assert_near(
            &format!("3342 case {name} at {fs} Hz vs reference"),
            range,
            reference,
            RANGE_PARITY_LU,
        );
        assert_near(
            &format!("3342 case {name} at {fs} Hz"),
            range,
            published,
            1.0,
        );
    }
}

#[test]
fn case_1_ten_lu() {
    case("1");
}

#[test]
fn case_2_five_lu() {
    case("2");
}

#[test]
fn case_3_twenty_lu() {
    case("3");
}

#[test]
fn case_4_relative_gate_drops_the_quietest() {
    case("4");
}

#[test]
fn case_1_repeated_reads_the_same() {
    case("1x2");
}
