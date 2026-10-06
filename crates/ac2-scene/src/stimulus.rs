//! What the stimulus keys start next, as the top bar says it.
//!
//! The focused view decides what Space arms: the sweep view a re-sweep with the last
//! sweep's parameters (the dialog when there is none), every other view the noise
//! generator for live measuring. Enter fires what is armed. The hint names it with its
//! level, so the operator reads what will play before it plays.

use ac2_proto::model::Signal;
use ac2_proto::units::Dbfs;

use crate::format;

/// What the stimulus keys would play.
#[derive(Clone, Debug, PartialEq)]
pub enum Stimulus {
    /// The generator: a noise (or tone) at a level on outputs (zero-based).
    Generator {
        signal: Signal,
        level: Option<Dbfs>,
        outputs: Vec<u16>,
    },
    /// A sweep: the one the dialog set up, or a re-sweep with the last sweep's parameters.
    Sweep {
        again: bool,
        duration_s: f64,
        level: Option<Dbfs>,
    },
    /// The sweep view with no sweep yet: Space opens the sweep dialog.
    SweepDialog,
}

/// Which key the hint is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Idle: Space arms.
    Space,
    /// Armed: Enter fires.
    Enter,
}

/// The top bar's hint, long and short: `Enter fires: re-sweep 3 s −50 dBFS`,
/// `Space arms: pink −50 dBFS → out 1`. `level_key` names the key that types a level, for
/// the generator without one.
pub fn hint(next: Next, what: &Stimulus, level_key: &str) -> (String, String) {
    match (next, what) {
        (Next::Space, Stimulus::SweepDialog) => {
            ("Space sets up a sweep".into(), "Space: sweep…".into())
        }
        (Next::Space, Stimulus::Generator { level: None, .. }) => {
            let s = format!("{level_key} types a level");
            (s.clone(), s)
        }
        (Next::Space, w) => (format!("Space arms: {}", describe(w)), "Space arms".into()),
        (Next::Enter, w) => (
            format!("Enter fires: {}", describe(w)),
            "Enter fires".into(),
        ),
    }
}

/// `pink −50 dBFS → out 1`, `re-sweep 3 s −50 dBFS`, `sweep 1 s −20 dBFS`.
pub fn describe(what: &Stimulus) -> String {
    match what {
        Stimulus::Generator {
            signal,
            level,
            outputs,
        } => format!(
            "{} {} → out {}",
            signal_name(signal),
            level_text(*level),
            outputs_text(outputs)
        ),
        Stimulus::Sweep {
            again,
            duration_s,
            level,
        } => format!(
            "{} {} s {}",
            if *again { "re-sweep" } else { "sweep" },
            short_number(*duration_s),
            level_text(*level)
        ),
        Stimulus::SweepDialog => "new sweep".into(),
    }
}

fn signal_name(s: &Signal) -> String {
    match s {
        Signal::White => "white".into(),
        Signal::Pink => "pink".into(),
        Signal::PeriodicPink { .. } => "periodic pink".into(),
        Signal::Sine { freq } => format!("sine {}", format::freq_readout(freq.0)),
        Signal::Ess { sweep } => format!("sweep {} s", short_number(sweep.duration.0)),
    }
}

fn level_text(l: Option<Dbfs>) -> String {
    l.map_or_else(
        || "no level".into(),
        |l| format!("{} dBFS", short_number(l.0)),
    )
}

/// Whole numbers without decimals (`−50`, `3`), else one (`−50.5`).
fn short_number(v: f64) -> String {
    let one = format::fixed(v, 1);
    match one.strip_suffix(".0") {
        Some(whole) => whole.to_string(),
        None => one,
    }
}

/// One-based: `1, 2`.
fn outputs_text(o: &[u16]) -> String {
    o.iter()
        .map(|c| (u32::from(*c) + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::units::Hz;

    fn pink(level: Option<f64>) -> Stimulus {
        Stimulus::Generator {
            signal: Signal::Pink,
            level: level.map(Dbfs),
            outputs: vec![0],
        }
    }

    fn resweep() -> Stimulus {
        Stimulus::Sweep {
            again: true,
            duration_s: 3.0,
            level: Some(Dbfs(-50.0)),
        }
    }

    #[test]
    fn enter_names_what_fires() {
        assert_eq!(
            hint(Next::Enter, &resweep(), "L"),
            (
                "Enter fires: re-sweep 3 s −50 dBFS".into(),
                "Enter fires".into()
            )
        );
        assert_eq!(
            hint(Next::Enter, &pink(Some(-50.0)), "L").0,
            "Enter fires: pink −50 dBFS → out 1"
        );
        let first = Stimulus::Sweep {
            again: false,
            duration_s: 1.0,
            level: Some(Dbfs(-20.5)),
        };
        assert_eq!(
            hint(Next::Enter, &first, "L").0,
            "Enter fires: sweep 1 s −20.5 dBFS"
        );
    }

    #[test]
    fn space_names_what_arms() {
        assert_eq!(
            hint(Next::Space, &resweep(), "L"),
            (
                "Space arms: re-sweep 3 s −50 dBFS".into(),
                "Space arms".into()
            )
        );
        let two = Stimulus::Generator {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Some(Dbfs(-12.0)),
            outputs: vec![0, 1],
        };
        assert_eq!(
            hint(Next::Space, &two, "L").0,
            "Space arms: sine 1.00 kHz −12 dBFS → out 1, 2"
        );
        assert_eq!(
            hint(Next::Space, &pink(None), "L"),
            ("L types a level".into(), "L types a level".into())
        );
        assert_eq!(
            hint(Next::Space, &Stimulus::SweepDialog, "L"),
            ("Space sets up a sweep".into(), "Space: sweep…".into())
        );
    }
}
