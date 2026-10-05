//! Calibration wording (`docs/design/q7-calibration.md` §10): which mic curve and which
//! sensitivity calibration an input uses, said the same way in the input setup, the
//! calibrations view, the meter labels, the pane captions and the CLI.
//!
//! A correction that silently changes (or silently does not apply) is a measurement error,
//! so every state has words, including the ones where nothing applies.

use ac2_proto::cal::{CurveUse, Sensitivity};
use ac2_proto::model::{
    CalBasis, CalEntry, CalMethod, CalStatus, ElectricalConnection, Mic, MicCurveRef,
    SensitivitySource,
};
use ac2_proto::units::{Db, MvPerPa, WallNs};

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

/// `in-line`, `injected`.
pub fn connection(c: ElectricalConnection) -> &'static str {
    match c {
        ElectricalConnection::InLine => "in-line",
        ElectricalConnection::Injected => "injected",
    }
}

/// A stated uncertainty: `±1 dB`, `±0.5 dB`.
pub fn uncertainty(db: Db) -> String {
    let t = format::fixed(db.0, 1);
    format!("±{} dB", t.strip_suffix(".0").unwrap_or(&t))
}

/// A mic sensitivity: `15.0 mV/Pa`.
pub fn mv_per_pa(s: MvPerPa) -> String {
    format!("{} mV/Pa", format::fixed(s.0, 1))
}

/// What a calibration rests on, as readouts name it: `cal 94 dB` (the calibrator level),
/// `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB`.
pub fn basis(b: &CalBasis) -> String {
    match b {
        CalBasis::Acoustic { calibrator_level } => {
            let t = format::level(calibrator_level.0);
            format!("cal {} dB", t.strip_suffix(".0").unwrap_or(&t))
        }
        CalBasis::Electrical {
            connection: c,
            mic_sensitivity,
            data_sheet,
            uncertainty: u,
        } => format!(
            "electrical cal ({}, {}{}) {}",
            connection(*c),
            if *data_sheet { "data sheet " } else { "" },
            mv_per_pa(*mic_sensitivity),
            uncertainty(*u)
        ),
    }
}

/// How a calibration was taken, short: `94.0 dB SPL at 1.00 kHz`, `electrical (in-line,
/// data sheet 15.0 mV/Pa) ±1 dB`.
pub fn calibrator(e: &CalEntry) -> String {
    match &e.spl.method {
        CalMethod::Acoustic { calibrator_level } => format!(
            "{} dB SPL at {}",
            format::level(calibrator_level.0),
            format::freq_readout(e.spl.freq.0)
        ),
        m => basis(&m.basis()).replacen("electrical cal", "electrical", 1),
    }
}

/// How a calibration was taken, in full (calibrations view, `ac2 cal list`): `94.0 dB SPL
/// at 1.00 kHz · read −26.0 dBFS`; `electrical, in-line · 15.00 mV at 1.00 kHz read
/// −40.0 dBFS · 0 dBFS = 1.500 V · 15.0 mV/Pa from the data sheet (0°,
/// 449350_34804_0Grad.txt) · ±1 dB`.
pub fn method_detail(e: &CalEntry) -> String {
    let read = format!("read {} dBFS", format::level(e.spl.measured.0));
    match &e.spl.method {
        CalMethod::Acoustic { .. } => format!("{} · {read}", calibrator(e)),
        CalMethod::Electrical {
            connection: c,
            volts,
            full_scale,
            mic_sensitivity,
            mic_sensitivity_from,
            uncertainty: u,
        } => format!(
            "electrical, {} · {} at {} {read} · 0 dBFS = {} · {} {} · {}",
            connection(*c),
            format::volts(volts.0),
            format::freq_readout(e.spl.freq.0),
            format::volts(full_scale.0),
            mv_per_pa(*mic_sensitivity),
            match mic_sensitivity_from {
                SensitivitySource::Typed => "typed".to_owned(),
                SensitivitySource::DataSheet { label, file_name } => {
                    format!("from the data sheet ({label}, {file_name})")
                }
            },
            uncertainty(*u)
        ),
    }
}

/// Notes on an electrical calibration just taken: the tone's frequency when it is not
/// the 1 kHz mic sensitivities are stated at.
pub fn electrical_notes(e: &CalEntry) -> Vec<String> {
    let mut v = Vec::new();
    if matches!(e.spl.method, CalMethod::Electrical { .. })
        && (e.spl.freq.0 - ac2_proto::cal::MIC_SENSITIVITY_FREQ.0).abs() > 0.5
    {
        v.push(format!(
            "the tone was {}: the mic's sensitivity is stated at 1 kHz, so this assumes the \
             preamp is flat between them (the mic curve is still 0 dB at 1 kHz)",
            format::freq_readout(e.spl.freq.0)
        ));
    }
    v
}

/// The method column of a calibrations table: `calibrator 94.0 dB SPL at 1.00 kHz`,
/// `electrical in-line, 15.03 mV at 1.00 kHz, 0 dBFS = 1.500 V, data sheet 15.0 mV/Pa`.
pub fn method_cell(e: &CalEntry) -> String {
    match &e.spl.method {
        CalMethod::Acoustic { .. } => format!("calibrator {}", calibrator(e)),
        CalMethod::Electrical {
            connection: c,
            volts,
            full_scale,
            mic_sensitivity,
            mic_sensitivity_from,
            ..
        } => format!(
            "electrical {}, {} at {}, 0 dBFS = {}, {}{}",
            connection(*c),
            format::volts(volts.0),
            format::freq_readout(e.spl.freq.0),
            format::volts(full_scale.0),
            if matches!(mic_sensitivity_from, SensitivitySource::DataSheet { .. }) {
                "data sheet "
            } else {
                ""
            },
            mv_per_pa(*mic_sensitivity)
        ),
    }
}

/// The uncertainty column: `±1 dB` for an electrical calibration; `—` for a calibrator's,
/// whose class states it.
pub fn uncertainty_cell(e: &CalEntry) -> String {
    match &e.spl.method {
        CalMethod::Acoustic { .. } => format::NO_VALUE.into(),
        CalMethod::Electrical { uncertainty: u, .. } => uncertainty(*u),
    }
}

/// What to do before reading a calibrator (the acoustic calibration dialog).
pub fn acoustic_steps() -> &'static str {
    "Fit the calibrator snugly on the mic (the right adapter for its diameter), switch it on \
     and wait for the level to settle; set the gain you will measure with first: a gain \
     change needs a new calibration."
}

/// What to do once an acoustic calibration is stored.
pub fn acoustic_after() -> &'static str {
    "Done: take the calibrator off; keep the gain as it is. Every SPL meter, spectrum and RTA \
     on this input now reads dB SPL."
}

/// What to do once an electrical calibration is stored.
pub fn electrical_after(c: ElectricalConnection) -> &'static str {
    match c {
        ElectricalConnection::InLine => {
            "Done: unplug the meter from the breakout; keep the gain as it is."
        }
        ElectricalConnection::Injected => {
            "Done: disconnect the generator, plug the mic back in and switch phantom power \
             back ON for it; keep the gain as it is."
        }
    }
}

/// What to mind while measuring the voltage, by where it is measured.
pub fn electrical_safety(c: ElectricalConnection) -> &'static str {
    match c {
        ElectricalConnection::InLine => {
            "Phantom power is on: +48 V on pins 2 and 3 against pin 1. Measure AC volts \
             between pins 2 and 3 only (an XLR breakout), never to pin 1, never short pins. \
             Use the meter's lowest AC range that fits and mind its accuracy there at 1 kHz."
        }
        ElectricalConnection::Injected => {
            "Switch phantom power OFF on this input first: 48 V on the XLR can damage the \
             generator (ac2 cannot switch it). Keep the gain you will measure with; switch \
             phantom back on for the mic afterwards."
        }
    }
}

/// The sensitivity field of an input setup row: `verified · 94.0 dB SPL at 1.00 kHz ·
/// 3 h ago`, `from ECM on in 2 · 94.0 dB SPL at 1.00 kHz · 3 h ago`, `uncalibrated`;
/// electrical: `verified · electrical (in-line, data sheet 15.0 mV/Pa) ±1 dB · 3 h ago`.
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

/// A readout's calibration (decisions 7a/7b) with the calibration's age `age_s`: `cal 94 dB
/// · 3 h ago`, `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 3 h ago`; a
/// mismatch says so instead of an age (the age would be another mic's): `cal from other mic
/// / input`, `electrical cal from other mic / input ±1 dB`; `None` uncalibrated.
pub fn status_text(cal: CalStatus, age_s: f64) -> Option<String> {
    match cal {
        CalStatus::Uncalibrated => None,
        CalStatus::Verified { basis: b, .. } => {
            Some(format!("{} · {}", basis(&b), format::ago(age_s)))
        }
        CalStatus::OtherMicOrInput {
            basis: CalBasis::Acoustic { .. },
            ..
        } => Some("cal from other mic / input".into()),
        CalStatus::OtherMicOrInput {
            basis: CalBasis::Electrical { uncertainty: u, .. },
            ..
        } => Some(format!(
            "electrical cal from other mic / input {}",
            uncertainty(u)
        )),
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
    use ac2_proto::units::{DbSpl, Dbfs, Hz};

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
                method: ac2_proto::model::CalMethod::Acoustic {
                    calibrator_level: DbSpl(94.0),
                },
                freq: Hz(1000.0),
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

    #[test]
    fn electrical_wording() {
        use ac2_proto::units::{Dbfs, Hz, Volts};
        let mut e = CalEntry {
            key: CalKey {
                device: DeviceId("hw:A".into()),
                channel: 0,
                mic: "MM1 34804".into(),
            },
            spl: SplCal {
                sensitivity: Db(133.98),
                method: CalMethod::Electrical {
                    connection: ElectricalConnection::InLine,
                    volts: Volts(0.015),
                    full_scale: Volts(1.5),
                    mic_sensitivity: MvPerPa(15.0),
                    mic_sensitivity_from: SensitivitySource::DataSheet {
                        label: "0°".into(),
                        file_name: "449350_34804_0Grad.txt".into(),
                    },
                    uncertainty: Db(1.0),
                },
                freq: Hz(1000.0),
                measured: Dbfs(-40.0),
                calibrated_at: WallNs(0),
            },
        };
        let now = WallNs(2 * 3_600_000_000_000 + 5);
        let off = ClockOffset(0);
        assert_eq!(
            sensitivity_state(&Sensitivity::Verified(&e), now, off),
            "verified · electrical (in-line, data sheet 15.0 mV/Pa) ±1 dB · 2 h ago"
        );
        assert_eq!(
            method_detail(&e),
            "electrical, in-line · 15.00 mV at 1.00 kHz read −40.0 dBFS · 0 dBFS = 1.500 V \
             · 15.0 mV/Pa from the data sheet (0°, 449350_34804_0Grad.txt) · ±1 dB"
        );
        let b = e.spl.method.basis();
        assert_eq!(
            basis(&b),
            "electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB"
        );
        let status = Sensitivity::Verified(&e).status();
        assert_eq!(
            status_text(status, 7200.0).as_deref(),
            Some("electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 2 h ago")
        );
        assert_eq!(
            status_text(Sensitivity::OtherMicOrInput(&e).status(), 7200.0).as_deref(),
            Some("electrical cal from other mic / input ±1 dB")
        );
        assert!(electrical_notes(&e).is_empty());

        e.spl.freq = Hz(400.0);
        e.spl.method = CalMethod::Electrical {
            connection: ElectricalConnection::Injected,
            volts: Volts(0.1),
            full_scale: Volts(3.162),
            mic_sensitivity: MvPerPa(10.0),
            mic_sensitivity_from: SensitivitySource::Typed,
            uncertainty: Db(0.5),
        };
        assert_eq!(
            basis(&e.spl.method.basis()),
            "electrical cal (injected, 10.0 mV/Pa) ±0.5 dB"
        );
        assert_eq!(
            method_detail(&e),
            "electrical, injected · 100.0 mV at 400 Hz read −40.0 dBFS · 0 dBFS = 3.162 V · \
             10.0 mV/Pa typed · ±0.5 dB"
        );
        assert_eq!(electrical_notes(&e).len(), 1);
        assert!(electrical_notes(&e)[0].starts_with("the tone was 400 Hz"));
        assert!(electrical_safety(ElectricalConnection::Injected).contains("phantom power OFF"));
        assert!(electrical_safety(ElectricalConnection::InLine).contains("pins 2 and 3"));

        // Acoustic, for contrast: the calibrator level names it.
        let acoustic = CalBasis::Acoustic {
            calibrator_level: ac2_proto::units::DbSpl(114.0),
        };
        assert_eq!(basis(&acoustic), "cal 114 dB");
        assert_eq!(uncertainty(Db(1.0)), "±1 dB");
        assert_eq!(format::volts(0.000_25), "250.0 µV");
        assert_eq!(format::volts(1.228), "1.228 V");
    }
}
