//! The calibration matching rules (`docs/design/q7-calibration.md` §3, §10): which
//! sensitivity calibration and which mic curve an input uses, from the mirrored state alone.
//!
//! The daemon applies exactly what these functions say, and every client words its
//! readouts from the same functions, so what is shown as in use is what is in use.

use crate::model::{
    CalEntry, CalStatus, CurveChoice, DeviceId, InputSetup, Mic, MicCurveRef, State,
};

/// Longest mic name, characters.
pub const MAX_MIC_NAME: usize = 64;
/// Longest curve label, characters.
pub const MAX_LABEL: usize = 32;
/// Labels the CLI reads as "no curve"; a curve cannot be called that.
const RESERVED_LABELS: [&str; 2] = ["off", "none"];

/// A mic name is 1 … 64 characters without surrounding spaces or control characters.
pub fn check_mic_name(m: &str) -> Result<(), String> {
    if m.is_empty() {
        return Err("mic name is required".into());
    }
    if m.chars().count() > MAX_MIC_NAME {
        return Err(format!("mic name longer than {MAX_MIC_NAME} characters"));
    }
    if m.trim() != m || m.chars().any(char::is_control) {
        return Err(format!(
            "mic name {m:?} has surrounding spaces or control characters"
        ));
    }
    Ok(())
}

/// A curve label is 1 … 32 characters without surrounding spaces or control characters,
/// and not `off` / `none`.
pub fn check_label(l: &str) -> Result<(), String> {
    if l.is_empty() {
        return Err("curve label is required".into());
    }
    if l.chars().count() > MAX_LABEL {
        return Err(format!("curve label longer than {MAX_LABEL} characters"));
    }
    if l.trim() != l || l.chars().any(char::is_control) {
        return Err(format!(
            "curve label {l:?} has surrounding spaces or control characters"
        ));
    }
    if RESERVED_LABELS.iter().any(|r| l.eq_ignore_ascii_case(r)) {
        return Err(format!("{l:?} means no curve; label the curve otherwise"));
    }
    Ok(())
}

/// The label a curve gets when the operator gives none: the incidence angle the file
/// names (`90°`), else the file stem (cut to [`MAX_LABEL`] characters).
pub fn default_label(angle_deg: Option<u32>, file_name: &str) -> String {
    if let Some(a) = angle_deg {
        return format!("{a}°");
    }
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s).trim();
    let l: String = stem
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL)
        .collect::<String>()
        .trim()
        .to_owned();
    if check_label(&l).is_ok() {
        l
    } else {
        "curve".into()
    }
}

/// The library entry of mic `name`.
pub fn mic<'a>(mics: &'a [Mic], name: &str) -> Option<&'a Mic> {
    mics.iter().find(|m| m.name == name)
}

/// The curve `label` of mic `name`.
pub fn curve<'a>(mics: &'a [Mic], name: &str, label: &str) -> Option<&'a MicCurveRef> {
    mic(mics, name)?.curves.iter().find(|c| c.label == label)
}

/// The input setup row of `channel` (default: no mic, no curve chosen).
pub fn input_setup(inputs: &[InputSetup], channel: u16) -> InputSetup {
    inputs
        .iter()
        .find(|i| i.channel == channel)
        .cloned()
        .unwrap_or(InputSetup {
            channel,
            mic: None,
            curve: CurveChoice::NotChosen,
        })
}

/// Chooses the only curve of a mic that has exactly one, on a row where none is chosen:
/// with one curve there is nothing to confuse it with. With several, the operator chooses.
pub fn settle(row: &mut InputSetup, mics: &[Mic]) {
    if row.curve != CurveChoice::NotChosen {
        return;
    }
    if let Some(m) = row.mic.as_deref().and_then(|n| mic(mics, n))
        && let [only] = m.curves.as_slice()
    {
        row.curve = CurveChoice::Curve {
            label: only.label.clone(),
        };
    }
}

/// The choices ←/→ step through for a row with mic `m`: off, then the mic's curves in
/// import order. From a choice not in the list (none chosen, a curve no longer stored),
/// → goes to the first curve and ← to the last.
pub fn step(current: &CurveChoice, m: Option<&Mic>, forward: bool) -> CurveChoice {
    let mut options = vec![CurveChoice::Off];
    options.extend(m.into_iter().flat_map(|m| {
        m.curves.iter().map(|c| CurveChoice::Curve {
            label: c.label.clone(),
        })
    }));
    let n = options.len();
    let next = match options.iter().position(|o| o == current) {
        Some(i) if forward => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        None if forward => 1 % n,
        None => n - 1,
    };
    options.swap_remove(next)
}

/// Which sensitivity calibration an input uses (decisions 7a/7b).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sensitivity<'a> {
    /// None applies: dBFS.
    Uncalibrated,
    /// The calibration of this device + input + mic.
    Verified(&'a CalEntry),
    /// A calibration of another mic on this input, or of this mic on another input or
    /// device: applied, and said so.
    OtherMicOrInput(&'a CalEntry),
}

impl<'a> Sensitivity<'a> {
    /// The entry applied.
    pub fn entry(&self) -> Option<&'a CalEntry> {
        match self {
            Self::Uncalibrated => None,
            Self::Verified(e) | Self::OtherMicOrInput(e) => Some(e),
        }
    }

    /// The readout state frames carry.
    pub fn status(&self) -> CalStatus {
        match self {
            Self::Uncalibrated => CalStatus::Uncalibrated,
            Self::Verified(e) => CalStatus::Verified {
                calibrated_at: e.spl.calibrated_at,
            },
            Self::OtherMicOrInput(e) => CalStatus::OtherMicOrInput {
                calibrated_at: e.spl.calibrated_at,
            },
        }
    }
}

/// Which mic curve an input applies, and when none, why.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CurveUse<'a> {
    /// The input has no mic name: no curve can apply.
    NoMic,
    /// The mic has no curve in the library.
    NoneStored {
        /// Mic name.
        mic: &'a str,
    },
    /// The mic has curves and none is chosen: none applies until the operator chooses.
    NotChosen {
        /// Mic name.
        mic: &'a str,
        /// Its curves.
        stored: &'a [MicCurveRef],
    },
    /// Switched off.
    Off {
        /// Mic name.
        mic: &'a str,
    },
    /// The chosen curve is not stored for this mic (deleted, or the mic renamed).
    Missing {
        /// Mic name.
        mic: &'a str,
        /// Chosen label.
        label: &'a str,
    },
    /// Applied.
    Applied {
        /// Mic name.
        mic: &'a str,
        /// The curve.
        curve: &'a MicCurveRef,
    },
}

impl<'a> CurveUse<'a> {
    /// The curve applied.
    pub fn applied(&self) -> Option<&'a MicCurveRef> {
        match self {
            Self::Applied { curve, .. } => Some(curve),
            _ => None,
        }
    }
}

/// What an input uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputUse<'a> {
    /// Input channel.
    pub channel: u16,
    /// Mic name on the input setup.
    pub mic: Option<&'a str>,
    /// Sensitivity calibration.
    pub sensitivity: Sensitivity<'a>,
    /// Mic curve.
    pub curve: CurveUse<'a>,
}

/// What input `channel` of capture device `device` uses (`None`: no open session, so only
/// calibrations of the input's mic on any device can match).
///
/// Sensitivity, first match wins: the entry (device, input, mic) → verified; the newest on
/// (device, input) → other mic / input; the newest for the mic anywhere → other mic / input;
/// else uncalibrated. Curve: the input's chosen curve of its mic, only that one.
pub fn input_use<'a>(
    calibrations: &'a [CalEntry],
    mics: &'a [Mic],
    inputs: &'a [InputSetup],
    device: Option<&DeviceId>,
    channel: u16,
) -> InputUse<'a> {
    let row = inputs.iter().find(|i| i.channel == channel);
    let mic_name = row.and_then(|r| r.mic.as_deref());
    let here =
        |e: &&CalEntry| device.is_some_and(|d| e.key.device == *d) && e.key.channel == channel;
    let newest =
        |it: &mut dyn Iterator<Item = &'a CalEntry>| it.max_by_key(|e| e.spl.calibrated_at);
    let sensitivity = if let Some(e) =
        mic_name.and_then(|m| calibrations.iter().find(|e| here(e) && e.key.mic == m))
    {
        Sensitivity::Verified(e)
    } else if let Some(e) = newest(&mut calibrations.iter().filter(here)).or_else(|| {
        mic_name.and_then(|m| newest(&mut calibrations.iter().filter(|e| e.key.mic == m)))
    }) {
        Sensitivity::OtherMicOrInput(e)
    } else {
        Sensitivity::Uncalibrated
    };
    let curve = match (mic_name, row.map(|r| &r.curve)) {
        (None, _) | (_, None) => CurveUse::NoMic,
        (Some(m), Some(choice)) => {
            let stored = mic(mics, m).map_or(&[][..], |x| x.curves.as_slice());
            match choice {
                CurveChoice::Off => CurveUse::Off { mic: m },
                CurveChoice::Curve { label } => match stored.iter().find(|c| c.label == *label) {
                    Some(c) => CurveUse::Applied { mic: m, curve: c },
                    None => CurveUse::Missing { mic: m, label },
                },
                CurveChoice::NotChosen if stored.is_empty() => CurveUse::NoneStored { mic: m },
                CurveChoice::NotChosen => CurveUse::NotChosen { mic: m, stored },
            }
        }
    };
    InputUse {
        channel,
        mic: mic_name,
        sensitivity,
        curve,
    }
}

/// [`input_use`] on the open session's capture device.
pub fn state_input_use(s: &State, channel: u16) -> InputUse<'_> {
    input_use(
        &s.calibrations,
        &s.mics,
        &s.inputs,
        s.session.open.as_ref().map(|o| &o.input_device),
        channel,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CalKey, SplCal};
    use crate::units::{Db, DbSpl, Dbfs, Hz, WallNs};

    fn curve_ref(label: &str) -> MicCurveRef {
        MicCurveRef {
            label: label.into(),
            file_name: format!("{label}.txt"),
            content_hash: "0".into(),
            points: 2,
            f_lo: Hz(20.0),
            f_hi: Hz(20_000.0),
            imported_at: WallNs(1),
            stated_sensitivity: None,
        }
    }

    fn entry(dev: &str, ch: u16, mic: &str, at: u64) -> CalEntry {
        CalEntry {
            key: CalKey {
                device: DeviceId(dev.into()),
                channel: ch,
                mic: mic.into(),
            },
            spl: SplCal {
                sensitivity: Db(130.0),
                calibrator_level: DbSpl(94.0),
                calibrator_freq: Hz(1000.0),
                measured: Dbfs(-36.0),
                calibrated_at: WallNs(at),
            },
        }
    }

    fn row(ch: u16, mic: Option<&str>, curve: CurveChoice) -> InputSetup {
        InputSetup {
            channel: ch,
            mic: mic.map(Into::into),
            curve,
        }
    }

    #[test]
    fn labels() {
        assert_eq!(default_label(Some(90), "x.txt"), "90°");
        assert_eq!(default_label(None, "/a/b/MM1 cal.frd"), "MM1 cal");
        assert_eq!(
            default_label(None, "a-very-long-file-name-of-a-mic-curve-file.txt").len(),
            MAX_LABEL
        );
        assert_eq!(default_label(None, "off.txt"), "curve");
        assert!(check_label("0°").is_ok());
        assert!(check_label("Off").is_err());
        assert!(check_label(" 90°").is_err());
        assert!(check_label("").is_err());
    }

    #[test]
    fn curve_states() {
        let mics = vec![Mic {
            name: "MM1".into(),
            curves: vec![curve_ref("0°"), curve_ref("90°")],
        }];
        let at = |r: InputSetup| {
            let inputs = vec![r];
            format!("{:?}", input_use(&[], &mics, &inputs, None, 0).curve)
        };
        assert!(at(row(0, None, CurveChoice::Off)).starts_with("NoMic"));
        assert!(at(row(0, Some("ECM"), CurveChoice::NotChosen)).starts_with("NoneStored"));
        assert!(at(row(0, Some("MM1"), CurveChoice::NotChosen)).starts_with("NotChosen"));
        assert!(at(row(0, Some("MM1"), CurveChoice::Off)).starts_with("Off"));
        let l = |s: &str| CurveChoice::Curve { label: s.into() };
        assert!(at(row(0, Some("MM1"), l("45°"))).starts_with("Missing"));
        let inputs = vec![row(0, Some("MM1"), l("90°"))];
        let u = input_use(&[], &mics, &inputs, None, 0);
        assert_eq!(u.curve.applied().map(|c| c.label.as_str()), Some("90°"));
        // No row at all: no mic.
        assert_eq!(input_use(&[], &mics, &[], None, 3).curve, CurveUse::NoMic);
    }

    #[test]
    fn settle_and_step() {
        let mut mics = vec![Mic {
            name: "MM1".into(),
            curves: vec![curve_ref("0°")],
        }];
        let mut r = row(0, Some("MM1"), CurveChoice::NotChosen);
        settle(&mut r, &mics);
        assert_eq!(
            r.curve,
            CurveChoice::Curve {
                label: "0°".into()
            }
        );
        mics[0].curves.push(curve_ref("90°"));
        let mut r = row(0, Some("MM1"), CurveChoice::NotChosen);
        settle(&mut r, &mics);
        assert_eq!(r.curve, CurveChoice::NotChosen);
        let mut off = row(0, Some("MM1"), CurveChoice::Off);
        settle(&mut off, &mics[..1]);
        assert_eq!(off.curve, CurveChoice::Off);

        let l = |s: &str| CurveChoice::Curve { label: s.into() };
        let m = Some(&mics[0]);
        assert_eq!(step(&CurveChoice::NotChosen, m, true), l("0°"));
        assert_eq!(step(&l("0°"), m, true), l("90°"));
        assert_eq!(step(&l("90°"), m, true), CurveChoice::Off);
        assert_eq!(step(&CurveChoice::Off, m, false), l("90°"));
        assert_eq!(step(&l("45°"), m, false), l("90°"));
        assert_eq!(step(&CurveChoice::NotChosen, None, true), CurveChoice::Off);
    }

    #[test]
    fn sensitivity_matching() {
        let d = DeviceId("hw:A".into());
        let cals = vec![
            entry("hw:A", 0, "MM1", 10),
            entry("hw:A", 1, "ECM", 20),
            entry("hw:B", 3, "M30", 30),
        ];
        let inputs = vec![
            row(0, Some("MM1"), CurveChoice::Off),
            row(1, Some("MM1"), CurveChoice::Off),
            row(2, Some("M30"), CurveChoice::Off),
        ];
        let s =
            |ch: u16, dev: Option<&DeviceId>| input_use(&cals, &[], &inputs, dev, ch).sensitivity;
        assert!(matches!(s(0, Some(&d)), Sensitivity::Verified(e) if e.key.channel == 0));
        // Another mic on this input wins over this mic on another input.
        assert!(matches!(s(1, Some(&d)), Sensitivity::OtherMicOrInput(e) if e.key.mic == "ECM"));
        assert!(
            matches!(s(2, Some(&d)), Sensitivity::OtherMicOrInput(e) if e.key.device.0 == "hw:B")
        );
        assert_eq!(s(3, Some(&d)), Sensitivity::Uncalibrated);
        // Without a session only the mic matches.
        assert!(matches!(s(0, None), Sensitivity::OtherMicOrInput(_)));
    }
}
