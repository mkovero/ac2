//! Progress of a multi-step operation the daemon runs for the operator (a set of sweeps):
//! what runs, which step it is on, how far it is and roughly how long is left.
//!
//! The daemon reports only the step in progress; the time within a step is the client's
//! own count since it saw that step begin, so the bar and the time left are estimates that
//! snap back to the truth at every step.

use ac2_proto::model::{SweepRun, SweepStatus};

use crate::format;

/// One running operation as the progress strip shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// What runs: `sweep "Sweep 3"`.
    pub title: String,
    /// Where it is: `sweep 1 of 2`, `analysing…`.
    pub step: String,
    /// How far, 0 … 1.
    pub fraction: f32,
    /// What it plays: `−20.0 dBFS`.
    pub detail: String,
    /// Time left while it plays: `about 4 s left`; `None` once nothing is left to play.
    pub remaining: Option<String>,
}

/// Time left, rounded up to whole seconds: `about 4 s left`, `about 1 min 05 s left`,
/// `finishing` under half a second.
pub fn remaining(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.5 {
        return "finishing".into();
    }
    let s = seconds.ceil() as u64;
    if s < 60 {
        format!("about {s} s left")
    } else {
        format!("about {} min {:02} s left", s / 60, s % 60)
    }
}

/// A sweep run's progress, `in_step_s` seconds after the client saw its current step
/// begin; `None` once it is stored or failed.
pub fn sweep(run: &SweepRun, in_step_s: f64) -> Option<Progress> {
    let title = format!("sweep {:?}", run.name);
    let detail = format!("{} dBFS", format::signed(run.level.0, 1));
    let n = run.repeats.max(1);
    match run.status {
        SweepStatus::Playing { repeat } => {
            let k = repeat.clamp(1, n);
            // Each repeat is the sweep and the silence its tail and room decay need.
            let period = (run.sweep_duration.0 + run.post_roll.0).max(0.0);
            let within = if period > 0.0 {
                (in_step_s.max(0.0) / period).min(1.0)
            } else {
                1.0
            };
            let fraction = ((f64::from(k - 1) + within) / f64::from(n)) as f32;
            let left = f64::from(n - k) * period + period * (1.0 - within);
            Some(Progress {
                title,
                detail,
                step: format!("sweep {k} of {n}"),
                fraction,
                remaining: Some(remaining(left)),
            })
        }
        SweepStatus::Analysing => Some(Progress {
            title,
            detail,
            step: "analysing…".into(),
            fraction: 1.0,
            remaining: None,
        }),
        SweepStatus::Done { .. } | SweepStatus::Failed { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{EssSpec, SweepFailure};
    use ac2_proto::units::{ClientId, Dbfs, Hz, Seconds, SweepId, TraceId, WallNs};

    fn run(status: SweepStatus) -> SweepRun {
        SweepRun {
            id: SweepId(1),
            owner: ClientId("ui".into()),
            name: "Sweep 3".into(),
            reference_input: 0,
            measurement_input: 1,
            outputs: vec![0],
            level: Dbfs(-20.0),
            sweep: EssSpec {
                start: Hz(20.0),
                end: Hz(20_000.0),
                duration: Seconds(2.0),
                fade_in: Seconds(0.0),
                fade_out: Seconds(0.0),
            },
            sweep_duration: Seconds(2.0),
            post_roll: Seconds(1.0),
            repeats: 2,
            gate: None,
            status,
            started_at: WallNs(0),
        }
    }

    #[test]
    fn steps_bar_and_time_left() {
        let p = sweep(&run(SweepStatus::Playing { repeat: 1 }), 0.0).expect("running");
        assert_eq!(p.title, "sweep \"Sweep 3\"");
        assert_eq!(p.step, "sweep 1 of 2");
        assert_eq!(p.fraction, 0.0);
        assert_eq!(p.remaining.as_deref(), Some("about 6 s left"));

        let p = sweep(&run(SweepStatus::Playing { repeat: 1 }), 1.5).expect("running");
        assert!((p.fraction - 0.25).abs() < 1e-6);
        assert_eq!(p.remaining.as_deref(), Some("about 5 s left"));

        let p = sweep(&run(SweepStatus::Playing { repeat: 2 }), 0.2).expect("running");
        assert_eq!(p.step, "sweep 2 of 2");
        assert_eq!(p.remaining.as_deref(), Some("about 3 s left"));

        // A step longer than planned (the mirror lags) holds at its end instead of running on.
        let p = sweep(&run(SweepStatus::Playing { repeat: 2 }), 9.0).expect("running");
        assert_eq!(p.fraction, 1.0);
        assert_eq!(p.remaining.as_deref(), Some("finishing"));

        let p = sweep(&run(SweepStatus::Analysing), 0.0).expect("running");
        assert_eq!(p.step, "analysing…");
        assert_eq!(p.remaining, None);

        assert_eq!(
            sweep(&run(SweepStatus::Done { trace: TraceId(4) }), 0.0),
            None
        );
        assert_eq!(
            sweep(
                &run(SweepStatus::Failed {
                    reason: SweepFailure::Stopped,
                    msg: String::new()
                }),
                0.0
            ),
            None
        );
        assert_eq!(p.detail, "\u{2212}20.0 dBFS");
    }

    #[test]
    fn time_left_reads_in_whole_seconds() {
        assert_eq!(remaining(0.2), "finishing");
        assert_eq!(remaining(0.6), "about 1 s left");
        assert_eq!(remaining(58.2), "about 59 s left");
        assert_eq!(remaining(59.2), "about 1 min 00 s left");
        assert_eq!(remaining(65.0), "about 1 min 05 s left");
    }
}
