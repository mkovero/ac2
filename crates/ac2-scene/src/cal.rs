//! Calibration wording (`docs/design/q7-calibration.md` §10): which mic curve and which
//! sensitivity calibration an input uses, said the same way in the input setup, the
//! calibrations view, the meter labels, the pane captions and the CLI.
//!
//! A correction that silently changes (or silently does not apply) is a measurement error,
//! so every state has words, including the ones where nothing applies.

use ac2_proto::cal::{CurveUse, Sensitivity};
use ac2_proto::model::{CalEntry, Mic, MicCurveRef};
use ac2_proto::units::WallNs;

use crate::format;
use crate::time::{self, ClockOffset};

/// A curve by mic and label: `MM1 34804 90°`.
pub fn curve_name(mic: &str, label: &str) -> String {
    format!("{mic} {label}")
}

/// `0°, 90°`.
pub fn labels(curves: &[MicCurveRef]) -> String {
    curves
        .iter()
        .map(|c| c.label.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The curve field of an input setup row: `90°`, `off`, `choose: 0°, 90°`,
/// `90° — not stored for MM1 34804`, `no curve stored for MM1 34804`, `—` without a mic.
pub fn curve_state(u: &CurveUse<'_>) -> String {
    match u {
        CurveUse::NoMic => format::NO_VALUE.into(),
        CurveUse::NoneStored { mic } => format!("no curve stored for {mic}"),
        CurveUse::NotChosen { stored, .. } => format!("choose: {}", labels(stored)),
        CurveUse::Off { .. } => "off".into(),
        CurveUse::Missing { mic, label } => format!("{label} — not stored for {mic}"),
        CurveUse::Applied { curve, .. } => curve.label.clone(),
    }
}

/// The curve part of an input's row in the session dialog and the input setup view:
/// `curve 90°`, `curve off`, `choose a curve: 0°, 90°`, `curve 90° not stored`,
/// `no curve stored` (the row names the mic already).
pub fn curve_row(u: &CurveUse<'_>) -> String {
    match u {
        CurveUse::NoMic => format::NO_VALUE.into(),
        CurveUse::NoneStored { .. } => "no curve stored".into(),
        CurveUse::NotChosen { stored, .. } => format!("choose a curve: {}", labels(stored)),
        CurveUse::Off { .. } => "curve off".into(),
        CurveUse::Missing { label, .. } => format!("curve {label} not stored"),
        CurveUse::Applied { curve, .. } => format!("curve {}", curve.label),
    }
}

/// The short curve part of an input's meter label (`MM1 34804 · 90° · mic (in 1)`):
/// the label when applied, else why none applies; nothing without a mic.
pub fn curve_short(u: &CurveUse<'_>) -> Option<String> {
    match u {
        CurveUse::NoMic => None,
        CurveUse::NoneStored { .. } => Some("no curve stored".into()),
        CurveUse::NotChosen { .. } => Some("curve not chosen".into()),
        CurveUse::Off { .. } => Some("curve off".into()),
        CurveUse::Missing { label, .. } => Some(format!("{label} not stored")),
        CurveUse::Applied { curve, .. } => Some(curve.label.clone()),
    }
}

/// The caption part of a corrected readout. `applied` is what the frame says the daemon
/// did; `u` is the input's setup. `mic curve: MM1 34804 90°` when applied; when not, why
/// (`mic curve off`, `no mic curve stored for MM1 34804`, `mic curve not chosen`,
/// `mic curve 90° not stored for MM1 34804`); nothing for an input without a mic.
pub fn curve_note(applied: bool, u: &CurveUse<'_>) -> Option<String> {
    match (applied, u) {
        (true, CurveUse::Applied { mic, curve }) => {
            Some(format!("mic curve: {}", curve_name(mic, &curve.label)))
        }
        // The setup changed after the frame was made: the next frame says which.
        (true, _) => Some("mic curve".into()),
        (false, CurveUse::NoMic | CurveUse::Applied { .. }) => None,
        (false, CurveUse::Off { .. }) => Some("mic curve off".into()),
        (false, CurveUse::NoneStored { mic }) => Some(format!("no mic curve stored for {mic}")),
        (false, CurveUse::NotChosen { .. }) => Some("mic curve not chosen".into()),
        (false, CurveUse::Missing { mic, label }) => {
            Some(format!("mic curve {label} not stored for {mic}"))
        }
    }
}

/// `94.0 dB SPL at 1.00 kHz`.
pub fn calibrator(e: &CalEntry) -> String {
    format!(
        "{} dB SPL at {}",
        format::level(e.spl.calibrator_level.0),
        format::freq_readout(e.spl.calibrator_freq.0)
    )
}

/// The sensitivity field of an input setup row: `verified · 94.0 dB SPL at 1.00 kHz ·
/// 3 h ago`, `from ECM on in 2 · 94.0 dB SPL at 1.00 kHz · 3 h ago`, `uncalibrated`.
pub fn sensitivity_state(s: &Sensitivity<'_>, client_now: WallNs, offset: ClockOffset) -> String {
    let tail = |e: &CalEntry| {
        format!(
            "{} · {}",
            calibrator(e),
            format::ago(time::age_s(e.spl.calibrated_at, client_now, offset))
        )
    };
    match s {
        Sensitivity::Uncalibrated => "uncalibrated".into(),
        Sensitivity::Verified(e) => format!("verified · {}", tail(e)),
        Sensitivity::OtherMicOrInput(e) => format!(
            "from {} on in {} · {}",
            e.key.mic,
            u32::from(e.key.channel) + 1,
            tail(e)
        ),
    }
}

/// A manufacturer's stated sensitivity: `15.0 mV/Pa (−36.5 dBV/Pa)`.
pub fn stated_sensitivity(mv_per_pa: f64) -> String {
    format!(
        "{} mV/Pa ({} dBV/Pa)",
        format::fixed(mv_per_pa, 1),
        format::fixed(20.0 * (mv_per_pa / 1000.0).log10(), 1)
    )
}

/// One curve of the library (its label is named beside it): `449350_34804_90Grad.txt · 100
/// points, 50.0 Hz – 20.0 kHz · data sheet 15.0 mV/Pa (−36.5 dBV/Pa)`.
pub fn curve_line(c: &MicCurveRef) -> String {
    let mut s = format!(
        "{} · {} points, {} – {}",
        c.file_name,
        c.points,
        format::freq_readout(c.f_lo.0),
        format::freq_readout(c.f_hi.0)
    );
    if let Some(mv) = c.stated_sensitivity {
        s.push_str(&format!(" · data sheet {}", stated_sensitivity(mv)));
    }
    s
}

/// The stated sensitivity of a mic's curves, when its files state one (they come from the
/// same capsule, so the first stated value stands for the mic).
pub fn mic_stated_sensitivity(m: &Mic) -> Option<String> {
    m.curves
        .iter()
        .find_map(|c| c.stated_sensitivity)
        .map(stated_sensitivity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{CalKey, DeviceId, SplCal};
    use ac2_proto::units::{Db, DbSpl, Dbfs, Hz};

    fn curve(label: &str) -> MicCurveRef {
        MicCurveRef {
            label: label.into(),
            file_name: "449350_34804_90Grad.txt".into(),
            content_hash: "0".into(),
            points: 100,
            f_lo: Hz(50.0),
            f_hi: Hz(20_000.0),
            imported_at: WallNs(1),
            stated_sensitivity: Some(15.0),
        }
    }

    #[test]
    fn curve_wording() {
        let stored = [curve("0°"), curve("90°")];
        let mic = "MM1 34804";
        let applied = CurveUse::Applied {
            mic,
            curve: &stored[1],
        };
        assert_eq!(curve_state(&applied), "90°");
        assert_eq!(curve_row(&applied), "curve 90°");
        assert_eq!(curve_row(&CurveUse::Off { mic }), "curve off");
        assert_eq!(curve_row(&CurveUse::NoneStored { mic }), "no curve stored");
        assert_eq!(
            curve_row(&CurveUse::NotChosen {
                mic,
                stored: &stored
            }),
            "choose a curve: 0°, 90°"
        );
        assert_eq!(
            curve_row(&CurveUse::Missing { mic, label: "90°" }),
            "curve 90° not stored"
        );
        assert_eq!(curve_short(&applied).as_deref(), Some("90°"));
        assert_eq!(
            curve_note(true, &applied).as_deref(),
            Some("mic curve: MM1 34804 90°")
        );
        assert_eq!(curve_note(false, &applied), None);
        let none = CurveUse::NoneStored { mic };
        assert_eq!(curve_state(&none), "no curve stored for MM1 34804");
        assert_eq!(
            curve_note(false, &none).as_deref(),
            Some("no mic curve stored for MM1 34804")
        );
        let choose = CurveUse::NotChosen {
            mic,
            stored: &stored,
        };
        assert_eq!(curve_state(&choose), "choose: 0°, 90°");
        assert_eq!(curve_short(&choose).as_deref(), Some("curve not chosen"));
        let missing = CurveUse::Missing { mic, label: "90°" };
        assert_eq!(curve_state(&missing), "90° — not stored for MM1 34804");
        assert_eq!(
            curve_note(false, &missing).as_deref(),
            Some("mic curve 90° not stored for MM1 34804")
        );
        assert_eq!(curve_state(&CurveUse::Off { mic }), "off");
        assert_eq!(
            curve_note(false, &CurveUse::Off { mic }).as_deref(),
            Some("mic curve off")
        );
        assert_eq!(curve_state(&CurveUse::NoMic), "—");
        assert_eq!(curve_note(false, &CurveUse::NoMic), None);
        assert_eq!(curve_short(&CurveUse::NoMic), None);
    }

    #[test]
    fn sensitivity_wording() {
        let e = CalEntry {
            key: CalKey {
                device: DeviceId("hw:A".into()),
                channel: 1,
                mic: "ECM".into(),
            },
            spl: SplCal {
                sensitivity: Db(120.0),
                calibrator_level: DbSpl(94.0),
                calibrator_freq: Hz(1000.0),
                measured: Dbfs(-26.0),
                calibrated_at: WallNs(0),
            },
        };
        let now = WallNs(3 * 3_600_000_000_000 + 5);
        let off = ClockOffset(0);
        assert_eq!(
            sensitivity_state(&Sensitivity::Verified(&e), now, off),
            "verified · 94.0 dB SPL at 1.00 kHz · 3 h ago"
        );
        assert_eq!(
            sensitivity_state(&Sensitivity::OtherMicOrInput(&e), now, off),
            "from ECM on in 2 · 94.0 dB SPL at 1.00 kHz · 3 h ago"
        );
        assert_eq!(
            sensitivity_state(&Sensitivity::Uncalibrated, now, off),
            "uncalibrated"
        );
        assert_eq!(stated_sensitivity(15.0), "15.0 mV/Pa (−36.5 dBV/Pa)");
        assert_eq!(
            curve_line(&curve("90°")),
            "449350_34804_90Grad.txt · 100 points, 50.0 Hz – 20.0 kHz · data sheet \
             15.0 mV/Pa (−36.5 dBV/Pa)"
        );
    }
}
