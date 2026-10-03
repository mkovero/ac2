//! Delay-finder results as text: outcome, reasons, band, confidence and the candidate rows
//! the operator picks from (decision 1c). Shared by the UI readout and the CLI so both
//! say the same thing.

use ac2_proto::model::{
    AmbiguityReason, DelayArrival, DelayBand, DelayConfidence, DelayFinding, DelayOutcome,
    NoEstimateReason,
};

use crate::format;

/// What a refusal reason means, phrased as what to check.
pub fn no_estimate_text(r: &NoEstimateReason) -> String {
    match r {
        NoEstimateReason::NoReference => "no reference signal".into(),
        NoEstimateReason::NoSignal => "no measurement signal".into(),
        NoEstimateReason::ObservationTooShort => "not enough audio yet".into(),
        NoEstimateReason::InsufficientOverlap => "too little reference overlap".into(),
        NoEstimateReason::InsufficientExcitation => "band not excited".into(),
        NoEstimateReason::PeriodicExcitation { period } => {
            format!("periodic stimulus ({} samples)", period.0)
        }
        NoEstimateReason::LowPsr => "no clear peak".into(),
        NoEstimateReason::LowPrecision => "timing too uncertain".into(),
        NoEstimateReason::PeakAtSearchEdge => "peak at the search edge".into(),
        NoEstimateReason::LowBandSnr => "too noisy in band".into(),
    }
}

/// Why a finding is ambiguous.
pub fn ambiguity_text(r: AmbiguityReason) -> &'static str {
    match r {
        AmbiguityReason::BorderlineLevel => "near the threshold",
        AmbiguityReason::CloseArrivals => "close arrivals",
        AmbiguityReason::MergedLobe => "arrivals merged into one peak",
        AmbiguityReason::OutsideRefinement => "coarse evidence only",
    }
}

/// All reasons of a refusal, joined.
pub fn no_estimate_reasons(rs: &[NoEstimateReason]) -> String {
    if rs.is_empty() {
        return "no reason given".into();
    }
    rs.iter()
        .map(no_estimate_text)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The analysed band: `sub 20–120 Hz`.
pub fn band_text(b: DelayBand) -> String {
    let edges =
        |lo: f64, hi: f64| format!("{}–{} Hz", format::freq_tick(lo), format::freq_tick(hi));
    match b {
        DelayBand::Full => format!("full {}", edges(2000.0, 16_000.0)),
        DelayBand::Mid => format!("mid {}", edges(300.0, 3000.0)),
        DelayBand::Sub => format!("sub {}", edges(20.0, 120.0)),
        DelayBand::Custom { lo_hz, hi_hz } => format!("custom {}", edges(lo_hz.0, hi_hz.0)),
    }
}

/// What an ambiguous finding's list holds when it offers a single candidate, and what the
/// operator can do about it; `None` when the list itself shows the choice (two or more rows)
/// or the finding is not ambiguous.
///
/// A merged lobe is a peak whose shape does not fit one arrival in the analysed band: two
/// arrivals closer than the band's pulse width, or a dispersive path such as a crossover's
/// group delay inside the band. No estimator in that band can split it, so the finder lists
/// the peak and never invents a second arrival (design Q1 §2, §8).
pub fn ambiguity_note(o: &DelayOutcome) -> Option<String> {
    let DelayOutcome::Ambiguous {
        reasons, ranked, ..
    } = o
    else {
        return None;
    };
    if ranked.len() > 1 {
        return None;
    }
    let mut s = if reasons.contains(&AmbiguityReason::MergedLobe) {
        "One peak only: two arrivals closer than this band resolves (or a crossover's group \
         delay) are merged into it and cannot be listed apart. 1 inserts the peak's delay; to \
         find the first arrival, run the finder in another band (one without the crossover) \
         and compare."
            .to_owned()
    } else {
        "One candidate only: 1 inserts it.".to_owned()
    };
    if reasons.contains(&AmbiguityReason::OutsideRefinement) {
        s.push_str(" Its delay is coarse: a longer reference block would refine it.");
    }
    Some(s)
}

/// The keys that pick from `n` listed candidates: `1`, `1–2`, `1–3`.
pub fn pick_keys(n: usize) -> String {
    match n.min(3) {
        0 => String::new(),
        1 => "1".into(),
        k => format!("1–{k}"),
    }
}

/// One-line outcome: `ACCEPTED`, `AMBIGUOUS · near the threshold`, `NO ESTIMATE · …`.
pub fn outcome_text(o: &DelayOutcome) -> String {
    match o {
        DelayOutcome::Accepted { .. } => "ACCEPTED".into(),
        DelayOutcome::Ambiguous { reasons, .. } => {
            let r: Vec<&str> = reasons.iter().map(|r| ambiguity_text(*r)).collect();
            if r.is_empty() {
                "AMBIGUOUS".into()
            } else {
                format!("AMBIGUOUS · {}", r.join(", "))
            }
        }
        DelayOutcome::NoEstimate { reasons } => {
            format!("NO ESTIMATE · {}", no_estimate_reasons(reasons))
        }
    }
}

/// Confidence figures that were reached: `PSR 24.0 dB · band SNR 30.0 dB · excited 100 % ·
/// σ 0.20 smp`.
pub fn confidence_text(c: &DelayConfidence) -> String {
    let mut parts = Vec::new();
    if let Some(p) = c.psr_db {
        parts.push(format!("PSR {} dB", format::fixed(p.0, 1)));
    }
    if let Some(s) = c.band_snr_db {
        parts.push(format!("band SNR {} dB", format::fixed(s.0, 1)));
    }
    if let Some(e) = c.excited_fraction {
        parts.push(format!("excited {} %", format::fixed(e * 100.0, 0)));
    }
    if let Some(u) = c.uncertainty_samples {
        parts.push(format!("σ {} smp", format::fixed(u, 2)));
    }
    if parts.is_empty() {
        format::NO_VALUE.into()
    } else {
        parts.join(" · ")
    }
}

/// One arrival: `12.50 ms  −11.5 dB`.
pub fn arrival_text(a: &DelayArrival) -> String {
    format!(
        "{} ms  {}",
        format::fixed(a.delay.0 * 1000.0, 2),
        format::db_readout(a.level.0)
    )
}

/// A row of the candidate list an ambiguous finding offers.
#[derive(Clone, Debug, PartialEq)]
pub struct PickRow {
    /// The key that picks it: `1`, `2`, `3`.
    pub key: String,
    /// `12.50 ms  −11.5 dB`, plus `· strongest` and `· rule pick` marks.
    pub text: String,
}

/// The candidate list of an ambiguous finding (≤ 3 rows, the rule pick first); empty for
/// any other outcome.
pub fn pick_rows(f: &DelayFinding) -> Vec<PickRow> {
    let DelayOutcome::Ambiguous {
        ranked, strongest, ..
    } = &f.outcome
    else {
        return Vec::new();
    };
    ranked
        .iter()
        .take(3)
        .enumerate()
        .map(|(i, a)| {
            let mut text = arrival_text(a);
            if i == 0 {
                text.push_str(" · rule pick");
            }
            if a.delay_samples == strongest.delay_samples {
                text.push_str(" · strongest");
            }
            PickRow {
                key: (i + 1).to_string(),
                text,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::units::{Db, Degrees, Hz, Samples, Seconds, WallNs};

    fn a(ms: f64, level: f64) -> DelayArrival {
        DelayArrival {
            delay: Seconds(ms / 1000.0),
            delay_samples: ms * 48.0,
            level: Db(level),
            phase: Degrees(0.0),
            uncertainty_samples: 0.2,
            misfit: 0.0,
            refined: true,
        }
    }

    fn finding(outcome: DelayOutcome) -> DelayFinding {
        DelayFinding {
            outcome,
            confidence: DelayConfidence {
                psr_db: Some(Db(24.0)),
                psr_acq_db: None,
                band_snr_db: Some(Db(30.04)),
                excited_fraction: Some(1.0),
                uncertainty_samples: Some(0.2),
                pulse_width_samples: None,
                period: None,
            },
            band: DelayBand::Sub,
            observation: Seconds(8.0),
            candidates: vec![],
            found_at: WallNs(0),
        }
    }

    #[test]
    fn ambiguous_rows_mark_rule_pick_and_strongest() {
        let f = finding(DelayOutcome::Ambiguous {
            reasons: vec![
                AmbiguityReason::BorderlineLevel,
                AmbiguityReason::MergedLobe,
            ],
            ranked: vec![a(12.5, -11.5), a(12.7, 0.0), a(13.4, -6.0)],
            strongest: a(12.7, 0.0),
        });
        let rows = pick_rows(&f);
        assert_eq!(
            rows,
            [
                PickRow {
                    key: "1".into(),
                    text: "12.50 ms  −11.5 dB · rule pick".into()
                },
                PickRow {
                    key: "2".into(),
                    text: "12.70 ms  0.0 dB · strongest".into()
                },
                PickRow {
                    key: "3".into(),
                    text: "13.40 ms  −6.0 dB".into()
                },
            ]
        );
        assert_eq!(
            outcome_text(&f.outcome),
            "AMBIGUOUS · near the threshold, arrivals merged into one peak"
        );
        assert_eq!(
            confidence_text(&f.confidence),
            "PSR 24.0 dB · band SNR 30.0 dB · excited 100 % · σ 0.20 smp"
        );
    }

    /// The rig case: a three-way box's crossover group delay inside the full band widens the
    /// one peak there is, so the finder lists that peak alone. The text must explain the single
    /// row instead of promising arrivals it does not show.
    #[test]
    fn merged_lobe_with_one_candidate_says_why() {
        let peak = DelayArrival {
            phase: Degrees(160.0),
            misfit: 0.17,
            ..a(3.346, 0.0)
        };
        let f = finding(DelayOutcome::Ambiguous {
            reasons: vec![AmbiguityReason::MergedLobe],
            ranked: vec![peak],
            strongest: peak,
        });
        assert_eq!(
            outcome_text(&f.outcome),
            "AMBIGUOUS · arrivals merged into one peak"
        );
        let rows = pick_rows(&f);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "3.35 ms  0.0 dB · rule pick · strongest");
        assert_eq!(pick_keys(rows.len()), "1");
        let note = ambiguity_note(&f.outcome).expect("a single row is explained");
        assert!(note.starts_with("One peak only: two arrivals"), "{note}");
        assert!(note.contains("1 inserts the peak's delay"), "{note}");
        assert!(note.contains("another band"), "{note}");

        let coarse = DelayOutcome::Ambiguous {
            reasons: vec![AmbiguityReason::OutsideRefinement],
            ranked: vec![a(3.0, 0.0)],
            strongest: a(3.0, 0.0),
        };
        assert_eq!(
            ambiguity_note(&coarse).as_deref(),
            Some(
                "One candidate only: 1 inserts it. Its delay is coarse: a longer reference block would refine it."
            )
        );
    }

    #[test]
    fn several_rows_need_no_note() {
        let o = DelayOutcome::Ambiguous {
            reasons: vec![AmbiguityReason::MergedLobe, AmbiguityReason::CloseArrivals],
            ranked: vec![a(3.0, 0.0), a(3.1, -3.0)],
            strongest: a(3.0, 0.0),
        };
        assert_eq!(ambiguity_note(&o), None);
        assert_eq!(pick_keys(2), "1–2");
        assert_eq!(pick_keys(3), "1–3");
        assert_eq!(pick_keys(5), "1–3");
        assert_eq!(
            ambiguity_note(&DelayOutcome::NoEstimate { reasons: vec![] }),
            None
        );
    }

    #[test]
    fn refusals_and_bands_read_as_text() {
        let o = DelayOutcome::NoEstimate {
            reasons: vec![
                NoEstimateReason::LowPsr,
                NoEstimateReason::PeriodicExcitation {
                    period: Samples(131_072),
                },
            ],
        };
        assert_eq!(
            outcome_text(&o),
            "NO ESTIMATE · no clear peak, periodic stimulus (131072 samples)"
        );
        assert!(pick_rows(&finding(o)).is_empty());
        assert_eq!(band_text(DelayBand::Sub), "sub 20–120 Hz");
        assert_eq!(band_text(DelayBand::Full), "full 2k–16k Hz");
        assert_eq!(
            band_text(DelayBand::Custom {
                lo_hz: Hz(80.0),
                hi_hz: Hz(800.0)
            }),
            "custom 80–800 Hz"
        );
        assert_eq!(no_estimate_reasons(&[]), "no reason given");
    }
}
