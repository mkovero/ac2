//! The recorder joins the stream at whatever block the fan-out hands it first, so the sweep
//! lands at a different sample of the recording from run to run. The analysis locates the
//! onset in the reference and cuts around it, so the response must not move with it.

use super::*;

#[test]
fn the_response_does_not_depend_on_where_the_recording_starts() {
    let s = spec(ess(10.0, 20000.0, 2.0));
    let (reference, mic) = record(&s, 1, |x| poly(x, 0.01, 0.003), 1e-6, 7);
    let base = analyse_recording(&s, &reference, &mic, 1).expect("analysis");
    // Whole periods of a 4096 and an 8192 sample hand-off, odd offsets, and starts that leave
    // less than the pre-roll before the onset (the cut starts at the recording's first sample).
    for k in [1usize, 511, 4096, 8192, 13000, 17000] {
        let r = analyse_recording(&s, &reference[k..], &mic[k..], 1).expect("analysis");
        assert!(
            (base.first_onset_s - r.first_onset_s - k as f64 / FS).abs() < 1e-12,
            "trim {k}: onset {} vs {}",
            r.first_onset_s,
            base.first_onset_s
        );
        // A start inside the pre-roll shortens the cut, so the arrival is the same sum taken
        // over fewer samples: equal to rounding, whose last bit depends on the platform's
        // math library.
        assert!(
            (base.arrival_s - r.arrival_s).abs() < 1e-12,
            "trim {k}: arrival {} vs {}",
            r.arrival_s,
            base.arrival_s
        );
        let curves = [
            (&base.magnitude_db, &r.magnitude_db, "magnitude"),
            (&base.phase_deg, &r.phase_deg, "phase"),
            (&base.harmonics[0].level_db, &r.harmonics[0].level_db, "H2"),
        ];
        for (a, b, what) in curves {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                assert_eq!(x.is_finite(), y.is_finite(), "trim {k}: {what} column {i}");
                // A start inside the pre-roll shortens the cut by its noise-only samples:
                // equal to rounding then, bit-equal otherwise.
                if x.is_finite() {
                    assert!(
                        (x - y).abs() < 1e-6,
                        "trim {k}: {what} column {i}: {x} vs {y}"
                    );
                }
            }
        }
    }
}
