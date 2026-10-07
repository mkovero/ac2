//! What the stimulus keys start next, as the top bar says it.
//!
//! The focused view decides what Space arms: the sweep view a run of the selected sweep
//! measurement with its settings (the new-sweep-measurement dialog when there is none),
//! every other view the noise generator for live measuring. Enter fires what is armed. The hint names it with its
//! level, so the operator reads what will play before it plays.

use ac2_proto::model::{OutputSetup, Signal};
use ac2_proto::units::Dbfs;

use crate::format;

/// What the stimulus keys would play.
#[derive(Clone, Debug, PartialEq)]
pub enum Stimulus {
    /// The generator: a noise (or tone) at a level on outputs (zero-based), named by the
    /// rig's `labels` where it has them.
    Generator {
        signal: Signal,
        level: Option<Dbfs>,
        outputs: Vec<u16>,
        labels: Vec<OutputSetup>,
    },
    /// A run of the sweep measurement named `meas`, with its settings.
    Sweep {
        meas: String,
        duration_s: f64,
        level: Option<Dbfs>,
    },
    /// The sweep view with no sweep measurement yet: Space opens the dialog that makes one.
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

/// The top bar's hint, long and short: `Enter fires: sweep Genelec 1 m · 3 s −50 dBFS`,
/// `Space arms: pink −50 dBFS → out 1`. `level_key` names the key that types a level, for
/// the generator without one.
pub fn hint(next: Next, what: &Stimulus, level_key: &str) -> (String, String) {
    match (next, what) {
        (Next::Space, Stimulus::SweepDialog) => (
            "Space sets up a sweep measurement".into(),
            "Space: sweep…".into(),
        ),
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

/// `pink −50 dBFS → out 1`, `sweep Genelec 1 m · 3 s −50 dBFS`.
pub fn describe(what: &Stimulus) -> String {
    match what {
        Stimulus::Generator {
            signal,
            level,
            outputs,
            labels,
        } => format!(
            "{} {} → {}",
            signal_name(signal),
            level_text(*level),
            crate::rig::stimulus_outputs(outputs, labels)
        ),
        Stimulus::Sweep {
            meas,
            duration_s,
            level,
        } => format!(
            "sweep {meas} · {} s {}",
            short_number(*duration_s),
            level_text(*level)
        ),
        Stimulus::SweepDialog => "new sweep measurement".into(),
    }
}

/// What this app's own stimulus is doing, for a transfer measurement whose reference
/// carries nothing (NO REFERENCE): with nothing armed or playing here the likely cause is
/// that nobody started it, and the texts name the keys that start it; with it playing, the
/// patch between output and reference input is what is left to check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Drive {
    /// Playing, a request in flight, or another client's stimulus: the loopback patch is
    /// the suspect.
    #[default]
    Playing,
    /// Nothing armed or playing: `arm` arms the generator on this view, `fire` plays it.
    Idle { arm: String, fire: String },
    /// The generator armed and silent: `fire` plays it.
    Armed { fire: String },
    /// Nothing playing and the sweep view focused: there the arm key arms a sweep, so the
    /// generator is armed from a transfer pane.
    SweepView,
}

/// NO REFERENCE's detail: `stimulus off: Space arms, Enter starts it`. Every variant is
/// short enough to sit beside NO REFERENCE in a banner row in every theme (a longer one is
/// left out of the row, and then the operator reads only the fault).
pub fn no_reference_detail(d: &Drive) -> String {
    match d {
        Drive::Playing => "reference silent: check the loopback cable".into(),
        Drive::Idle { arm, fire } => format!("stimulus off: {arm} arms, {fire} starts it"),
        Drive::Armed { fire } => format!("stimulus armed: {fire} starts it"),
        Drive::SweepView => "stimulus off: arm it from a transfer pane".into(),
    }
}

/// The empty IR pane's reason without a reference, which has the room to say it whole:
/// `no reference: nothing is playing — Space arms, Enter starts the stimulus` (it breaks
/// at the dash in a narrow pane).
pub fn no_reference_note(d: &Drive) -> String {
    match d {
        Drive::Playing => "no reference: nothing is driving the loopback".into(),
        Drive::Idle { arm, fire } => {
            format!("no reference: nothing is playing — {arm} arms, {fire} starts the stimulus")
        }
        Drive::Armed { fire } => format!("no reference: armed — {fire} starts the stimulus"),
        Drive::SweepView => {
            "no reference: nothing is playing — arm the stimulus from a transfer pane".into()
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::units::Hz;

    fn pink(level: Option<f64>) -> Stimulus {
        Stimulus::Generator {
            signal: Signal::Pink,
            level: level.map(Dbfs),
            outputs: vec![0],
            labels: Vec::new(),
        }
    }

    fn resweep() -> Stimulus {
        Stimulus::Sweep {
            meas: "Genelec 1 m".into(),
            duration_s: 3.0,
            level: Some(Dbfs(-50.0)),
        }
    }

    #[test]
    fn enter_names_what_fires() {
        assert_eq!(
            hint(Next::Enter, &resweep(), "L"),
            (
                "Enter fires: sweep Genelec 1 m · 3 s −50 dBFS".into(),
                "Enter fires".into()
            )
        );
        assert_eq!(
            hint(Next::Enter, &pink(Some(-50.0)), "L").0,
            "Enter fires: pink −50 dBFS → out 1"
        );
        let first = Stimulus::Sweep {
            meas: "Sweep 1".into(),
            duration_s: 1.0,
            level: Some(Dbfs(-20.5)),
        };
        assert_eq!(
            hint(Next::Enter, &first, "L").0,
            "Enter fires: sweep Sweep 1 · 1 s −20.5 dBFS"
        );
    }

    fn idle() -> Drive {
        Drive::Idle {
            arm: "Space".into(),
            fire: "Enter".into(),
        }
    }

    /// NO REFERENCE says what to press while nothing here plays, and points at the patch
    /// once something does.
    #[test]
    fn no_reference_names_the_keys_by_phase() {
        let armed = Drive::Armed {
            fire: "Enter".into(),
        };
        assert_eq!(
            no_reference_detail(&idle()),
            "stimulus off: Space arms, Enter starts it"
        );
        assert_eq!(
            no_reference_detail(&armed),
            "stimulus armed: Enter starts it"
        );
        assert_eq!(
            no_reference_detail(&Drive::SweepView),
            "stimulus off: arm it from a transfer pane"
        );
        assert_eq!(
            no_reference_detail(&Drive::Playing),
            "reference silent: check the loopback cable"
        );
        assert_eq!(
            no_reference_note(&idle()),
            "no reference: nothing is playing — Space arms, Enter starts the stimulus"
        );
        assert_eq!(
            no_reference_note(&armed),
            "no reference: armed — Enter starts the stimulus"
        );
        assert_eq!(
            no_reference_note(&Drive::SweepView),
            "no reference: nothing is playing — arm the stimulus from a transfer pane"
        );
        assert_eq!(
            no_reference_note(&Drive::Playing),
            "no reference: nothing is driving the loopback"
        );
    }

    #[test]
    fn space_names_what_arms() {
        assert_eq!(
            hint(Next::Space, &resweep(), "L"),
            (
                "Space arms: sweep Genelec 1 m · 3 s −50 dBFS".into(),
                "Space arms".into()
            )
        );
        let two = Stimulus::Generator {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Some(Dbfs(-12.0)),
            outputs: vec![0, 1],
            labels: Vec::new(),
        };
        assert_eq!(
            hint(Next::Space, &two, "L").0,
            "Space arms: sine 1.00 kHz −12 dBFS → out 1, 2"
        );
        // Outputs the rig has named are named.
        let named = Stimulus::Generator {
            signal: Signal::Pink,
            level: Some(Dbfs(-50.0)),
            outputs: vec![0, 1],
            labels: vec![OutputSetup {
                channel: 0,
                label: Some("Main L".into()),
            }],
        };
        assert_eq!(
            hint(Next::Enter, &named, "L").0,
            "Enter fires: pink −50 dBFS → Main L, out 2"
        );
        assert_eq!(
            hint(Next::Space, &pink(None), "L"),
            ("L types a level".into(), "L types a level".into())
        );
        assert_eq!(
            hint(Next::Space, &Stimulus::SweepDialog, "L"),
            (
                "Space sets up a sweep measurement".into(),
                "Space: sweep…".into()
            )
        );
    }
}
