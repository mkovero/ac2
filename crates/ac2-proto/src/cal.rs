//! The calibration matching rules (`docs/design/q7-calibration.md` §3, §10): which
//! sensitivity calibration and which mic curve an input uses, from the mirrored state alone.
//!
//! The daemon applies exactly what these functions say, and every client words its
//! readouts from the same functions, so what is shown as in use is what is in use.

use crate::model::{
    CalEntry, CalStatus, CurveChoice, DeviceId, InputKind, InputSetup, LineCalEntry, Mic,
    MicCurveRef, Pair, PairSide, State,
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

/// A pair name follows the mic name's rules: 1 … 64 characters without surrounding spaces or
/// control characters.
pub fn check_pair_name(n: &str) -> Result<(), String> {
    check_mic_name(n).map_err(|e| e.replacen("mic name", "pair name", 1))
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

/// The kind of an input nobody has declared: a microphone with no name and no curve chosen.
/// A measurement on an undeclared input is acoustic by default so that it reads dBFS, as an
/// uncalibrated mic does, rather than being refused for a declaration no one made.
pub fn default_kind() -> InputKind {
    InputKind::Mic {
        mic: None,
        curve: CurveChoice::NotChosen,
    }
}

/// The input setup row of `channel` (default: an undeclared mic, unpaired).
pub fn input_setup(inputs: &[InputSetup], channel: u16) -> InputSetup {
    inputs
        .iter()
        .find(|i| i.channel == channel)
        .cloned()
        .unwrap_or(InputSetup {
            channel,
            kind: default_kind(),
            pair: None,
        })
}

/// Whether the VU ballistic is offered on `channel` of `device`: a `Line` input with a line
/// calibration on this device (0 VU is +4 dBu, which needs the volt). The daemon reads this
/// to decide what `ppm` carries where VU is chosen, and every client words the row from
/// it, so the bar and its label agree.
pub fn offers_vu(
    inputs: &[InputSetup],
    line_calibrations: &[LineCalEntry],
    device: &DeviceId,
    channel: u16,
) -> bool {
    input_kind(inputs, channel) == InputKind::Line
        && line_calibrations
            .iter()
            .any(|e| e.key.device == *device && e.key.channel == channel)
}

/// The kind of `channel` ([`input_setup`]'s default when undeclared).
pub fn input_kind(inputs: &[InputSetup], channel: u16) -> InputKind {
    input_setup(inputs, channel).kind
}

/// Chooses the only curve of a mic that has exactly one, on a mic row where none is chosen:
/// with one curve there is nothing to confuse it with. With several, the operator chooses.
pub fn settle(row: &mut InputSetup, mics: &[Mic]) {
    let InputKind::Mic {
        mic: Some(name),
        curve,
    } = &mut row.kind
    else {
        return;
    };
    if *curve != CurveChoice::NotChosen {
        return;
    }
    if let Some(m) = mic(mics, name)
        && let [only] = m.curves.as_slice()
    {
        *curve = CurveChoice::Curve {
            label: only.label.clone(),
        };
    }
}

/// `inputs` with every row's `pair` side set from `pairs` (unpaired rows get `None`); a
/// paired channel without a row gets one of the default kind.
pub fn apply_pair_sides(inputs: &[InputSetup], pairs: &[Pair]) -> Vec<InputSetup> {
    let mut rows: Vec<InputSetup> = inputs.to_vec();
    for p in pairs {
        for ch in [p.left, p.right] {
            if !rows.iter().any(|r| r.channel == ch) {
                rows.push(input_setup(inputs, ch));
            }
        }
    }
    for r in &mut rows {
        r.pair = pairs.iter().find_map(|p| {
            if p.left == r.channel {
                Some(PairSide::Left)
            } else if p.right == r.channel {
                Some(PairSide::Right)
            } else {
                None
            }
        });
    }
    rows.sort_by_key(|r| r.channel);
    rows
}

/// The rules of a pair list over `inputs` (decision V2), one message per violation naming
/// the inputs by channel (1-based, as printed on the interface): both sides are of one kind
/// and neither is a mic, an input is in at most one pair, and a pair is two inputs.
/// `captured` says whether the open session captures a channel (`None`: no open session, so
/// any channel may be declared).
pub fn check_pairs(
    inputs: &[InputSetup],
    pairs: &[Pair],
    captured: Option<&dyn Fn(u16) -> bool>,
) -> Result<(), String> {
    let n = |ch: u16| u32::from(ch) + 1;
    for (i, p) in pairs.iter().enumerate() {
        if let Some(name) = &p.name {
            check_pair_name(name)?;
        }
        if p.left == p.right {
            return Err(format!("input {} cannot be paired with itself", n(p.left)));
        }
        for ch in [p.left, p.right] {
            if let Some(c) = captured
                && !c(ch)
            {
                return Err(format!(
                    "input {} is not captured by the session, so it cannot be paired",
                    n(ch)
                ));
            }
            if pairs[..i].iter().any(|o| o.left == ch || o.right == ch) {
                return Err(format!("input {} is in two pairs", n(ch)));
            }
        }
        let (l, r) = (input_kind(inputs, p.left), input_kind(inputs, p.right));
        for (ch, k) in [(p.left, &l), (p.right, &r)] {
            if matches!(k, InputKind::Mic { .. }) {
                return Err(format!(
                    "input {} is a microphone; a pair is two line or two digital inputs",
                    n(ch)
                ));
            }
        }
        if l != r {
            return Err(format!(
                "inputs {} and {} are of different kinds ({} and {}); a pair is two of one kind",
                n(p.left),
                n(p.right),
                l.word(),
                r.word()
            ));
        }
    }
    Ok(())
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
    /// A line input with a line calibration: dBu of 0 dBFS, no capsule, no dB SPL.
    Line(&'a LineCalEntry),
}

impl<'a> Sensitivity<'a> {
    /// The sensitivity calibration applied (none on a line input: it has no dB SPL).
    pub fn entry(&self) -> Option<&'a CalEntry> {
        match self {
            Self::Uncalibrated | Self::Line(_) => None,
            Self::Verified(e) | Self::OtherMicOrInput(e) => Some(e),
        }
    }

    /// The readout state frames carry.
    pub fn status(&self) -> CalStatus {
        match self {
            Self::Uncalibrated => CalStatus::Uncalibrated,
            Self::Verified(e) => CalStatus::Verified {
                calibrated_at: e.spl.calibrated_at,
                basis: e.spl.method.basis(),
            },
            Self::OtherMicOrInput(e) => CalStatus::OtherMicOrInput {
                calibrated_at: e.spl.calibrated_at,
                basis: e.spl.method.basis(),
            },
            Self::Line(e) => CalStatus::Line {
                calibrated_at: e.line.calibrated_at,
                dbu_fs: dbu_fs(e.line.v_fs.0),
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
    /// What is on it ([`default_kind`] when undeclared).
    pub kind: &'a InputKind,
    /// Mic name on the input setup.
    pub mic: Option<&'a str>,
    /// Sensitivity calibration.
    pub sensitivity: Sensitivity<'a>,
    /// Mic curve.
    pub curve: CurveUse<'a>,
}

/// The kind of an undeclared input, with the lifetime of the state it is read from.
static DEFAULT_KIND: InputKind = InputKind::Mic {
    mic: None,
    curve: CurveChoice::NotChosen,
};

/// What input `channel` of capture device `device` uses (`None`: no open session, so only
/// calibrations of the input's mic on any device can match).
///
/// A mic's sensitivity, first match wins: the entry (device, input, mic) → verified; the
/// newest on (device, input) → other mic / input; the newest for the mic anywhere → other
/// mic / input; else uncalibrated. Curve: the input's chosen curve of its mic, only that
/// one. A line input: its line calibration on (device, input), else uncalibrated (dBFS). A
/// digital input is never calibrated.
pub fn input_use<'a>(
    calibrations: &'a [CalEntry],
    line_calibrations: &'a [LineCalEntry],
    mics: &'a [Mic],
    inputs: &'a [InputSetup],
    device: Option<&DeviceId>,
    channel: u16,
) -> InputUse<'a> {
    let row = inputs.iter().find(|i| i.channel == channel);
    let kind = row.map_or(&DEFAULT_KIND, |r| &r.kind);
    let (mic_name, choice) = match kind {
        InputKind::Mic { mic, curve } => (mic.as_deref(), Some(curve)),
        InputKind::Line => {
            let line = device.and_then(|d| {
                line_calibrations
                    .iter()
                    .find(|e| e.key.device == *d && e.key.channel == channel)
            });
            return InputUse {
                channel,
                kind,
                mic: None,
                sensitivity: line.map_or(Sensitivity::Uncalibrated, Sensitivity::Line),
                curve: CurveUse::NoMic,
            };
        }
        InputKind::Digital => {
            return InputUse {
                channel,
                kind,
                mic: None,
                sensitivity: Sensitivity::Uncalibrated,
                curve: CurveUse::NoMic,
            };
        }
    };
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
    let curve = match (mic_name, choice) {
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
        kind,
        mic: mic_name,
        sensitivity,
        curve,
    }
}

/// [`input_use`] on the open session's capture device.
pub fn state_input_use(s: &State, channel: u16) -> InputUse<'_> {
    input_use(
        &s.calibrations,
        &s.line_calibrations,
        &s.mics,
        &s.inputs,
        s.session.open.as_ref().map(|o| &o.input_device),
        channel,
    )
}

mod electrical;
mod loudness;
pub use electrical::*;
pub use loudness::*;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::{
        CalKey, CalMethod, ElectricalConnection, LineCal, LineCalKey, MicLoading,
        SensitivitySource, SplCal,
    };
    use crate::units::{Db, DbSpl, Dbfs, Hz, MvPerPa, Volts, WallNs};

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
                method: CalMethod::Acoustic {
                    calibrator_level: DbSpl(94.0),
                },
                freq: Hz(1000.0),
                measured: Dbfs(-36.0),
                calibrated_at: WallNs(at),
            },
        }
    }

    fn row(ch: u16, mic: Option<&str>, curve: CurveChoice) -> InputSetup {
        InputSetup {
            channel: ch,
            kind: InputKind::Mic {
                mic: mic.map(Into::into),
                curve,
            },
            pair: None,
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
            format!("{:?}", input_use(&[], &[], &mics, &inputs, None, 0).curve)
        };
        assert!(at(row(0, None, CurveChoice::Off)).starts_with("NoMic"));
        assert!(at(row(0, Some("ECM"), CurveChoice::NotChosen)).starts_with("NoneStored"));
        assert!(at(row(0, Some("MM1"), CurveChoice::NotChosen)).starts_with("NotChosen"));
        assert!(at(row(0, Some("MM1"), CurveChoice::Off)).starts_with("Off"));
        let l = |s: &str| CurveChoice::Curve { label: s.into() };
        assert!(at(row(0, Some("MM1"), l("45°"))).starts_with("Missing"));
        let inputs = vec![row(0, Some("MM1"), l("90°"))];
        let u = input_use(&[], &[], &mics, &inputs, None, 0);
        assert_eq!(u.curve.applied().map(|c| c.label.as_str()), Some("90°"));
        // No row at all: no mic.
        assert_eq!(
            input_use(&[], &[], &mics, &[], None, 3).curve,
            CurveUse::NoMic
        );
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
            r.kind.curve(),
            Some(&CurveChoice::Curve {
                label: "0°".into()
            })
        );
        mics[0].curves.push(curve_ref("90°"));
        let mut r = row(0, Some("MM1"), CurveChoice::NotChosen);
        settle(&mut r, &mics);
        assert_eq!(r.kind.curve(), Some(&CurveChoice::NotChosen));
        let mut off = row(0, Some("MM1"), CurveChoice::Off);
        settle(&mut off, &mics[..1]);
        assert_eq!(off.kind.curve(), Some(&CurveChoice::Off));

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
        let s = |ch: u16, dev: Option<&DeviceId>| {
            input_use(&cals, &[], &[], &inputs, dev, ch).sensitivity
        };
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

    #[test]
    fn electrical_maths_by_hand() {
        // 15 mV read at −40.0 dBFS: 0 dBFS is 100 × 15 mV = 1.5 V.
        let vfs = full_scale_volts(0.015, -40.0);
        assert!((vfs - 1.5).abs() < 1e-12, "{vfs}");
        // With 15 mV/Pa, 1.5 V is 100 Pa: 20·lg(100 / 20 µPa) = 133.979 dB SPL.
        let s = electrical_sensitivity_db(vfs, 15.0);
        assert!((s - 133.979_400_086_720_4).abs() < 1e-9, "{s}");
        // That mic at 1 Pa (93.98 dB SPL) gives 15 mV, which reads −40 dBFS: 94.0 shown.
        let spl = -40.0 + s;
        assert!((spl - 20.0 * (1.0 / P_REF_PA).log10()).abs() < 1e-9);
        assert_eq!(format!("{spl:.1}"), "94.0");
        // 0 dBFS = 1.228 V (+4 dBu), 50 mV/Pa: 24.56 Pa, 20·lg(1.228e6) = 121.784 dB.
        assert!((electrical_sensitivity_db(1.228, 50.0) - 121.784).abs() < 1e-3);
    }

    #[test]
    fn loading_maths_by_hand() {
        use crate::units::Ohms;
        // MM1 (15 mV/Pa open circuit, 330 Ω) into a 2 kΩ mic input: 2000 / 2330 of the
        // voltage, 20·lg(0.858369) = −1.3265 dB; 15 mV/Pa reads as 12.876 mV/Pa.
        let l = MicLoading {
            mic_impedance: Ohms(330.0),
            input_impedance: Ohms(2000.0),
        };
        let db = loading_db(&l).expect("valid").0;
        assert!((db - 20.0 * (2000.0f64 / 2330.0).log10()).abs() < 1e-12);
        assert!((db - -1.3265).abs() < 1e-4, "{db}");
        let s = loaded_sensitivity(MvPerPa(15.0), Some(&l))
            .expect("valid")
            .0;
        assert!((s - 15.0 * 2000.0 / 2330.0).abs() < 1e-12, "{s}");
        // 0 dBFS = 1.5 V: unloaded 133.979 dB SPL, loaded 1.327 dB more (each pascal gives
        // less voltage, so full scale is more pascal).
        let unloaded = electrical_sensitivity_db(1.5, 15.0);
        let loaded = electrical_sensitivity_db(1.5, s);
        assert!(
            (loaded - unloaded - 1.3265).abs() < 1e-4,
            "{}",
            loaded - unloaded
        );
        assert_eq!(loaded_sensitivity(MvPerPa(15.0), None), Ok(MvPerPa(15.0)));
        // Equal impedances halve the voltage: −6.02 dB.
        let half = MicLoading {
            mic_impedance: Ohms(600.0),
            input_impedance: Ohms(600.0),
        };
        assert!((loading_db(&half).expect("valid").0 - -6.0206).abs() < 1e-4);
        // Impedances that cannot divide are refused, never taken as unloaded.
        for (zm, zi, e) in [
            (0.0, 2000.0, LoadingError::MicImpedance(0.0)),
            (-330.0, 2000.0, LoadingError::MicImpedance(-330.0)),
            (330.0, 0.0, LoadingError::InputImpedance(0.0)),
            (
                330.0,
                f64::INFINITY,
                LoadingError::InputImpedance(f64::INFINITY),
            ),
        ] {
            let l = MicLoading {
                mic_impedance: Ohms(zm),
                input_impedance: Ohms(zi),
            };
            assert_eq!(loading_db(&l), Err(e));
            assert_eq!(loaded_sensitivity(MvPerPa(15.0), Some(&l)), Err(e));
        }
        let nan = MicLoading {
            mic_impedance: Ohms(f64::NAN),
            input_impedance: Ohms(2000.0),
        };
        assert!(matches!(
            loading_db(&nan),
            Err(LoadingError::MicImpedance(_))
        ));
    }

    #[test]
    fn electrical_f_norm_and_data_sheet() {
        let mut e = entry("hw:A", 0, "MM1", 1).spl;
        e.freq = Hz(250.0);
        assert_eq!(f_norm(&e), Hz(250.0));
        e.method = CalMethod::Electrical {
            connection: ElectricalConnection::InLine,
            volts: Volts(0.015),
            full_scale: Volts(1.5),
            mic_sensitivity: MvPerPa(15.0),
            mic_sensitivity_from: SensitivitySource::Typed,
            loading: None,
            uncertainty: Db(1.0),
        };
        // The data-sheet sensitivity is the capsule's at 1 kHz, whatever tone was read.
        assert_eq!(f_norm(&e), Hz(1000.0));

        let stated = |label: &str, v: Option<f64>| MicCurveRef {
            stated_sensitivity: v,
            ..curve_ref(label)
        };
        let lib = |curves: Vec<MicCurveRef>| {
            vec![Mic {
                name: "MM1".into(),
                curves,
            }]
        };
        assert_eq!(data_sheet(&[], "MM1"), DataSheet::NoMic);
        assert_eq!(
            data_sheet(&lib(vec![stated("0°", None)]), "MM1"),
            DataSheet::NoneStated
        );
        assert_eq!(
            data_sheet(
                &lib(vec![
                    stated("0°", None),
                    stated("90°", Some(15.0)),
                    stated("x", Some(15.0))
                ]),
                "MM1"
            ),
            DataSheet::One(
                MvPerPa(15.0),
                SensitivitySource::DataSheet {
                    label: "90°".into(),
                    file_name: "90°.txt".into()
                }
            )
        );
        assert_eq!(
            data_sheet(
                &lib(vec![stated("0°", Some(15.0)), stated("90°", Some(14.2))]),
                "MM1"
            ),
            DataSheet::Differ(vec![15.0, 14.2])
        );
    }

    fn line_row(ch: u16) -> InputSetup {
        InputSetup {
            channel: ch,
            kind: InputKind::Line,
            pair: None,
        }
    }

    #[test]
    fn line_calibration_maths_by_hand() {
        // An interface whose full scale is +13 dBu reads a +4 dBu tone at −9 dBFS.
        let tone = DBU_REF_VOLTS * 10f64.powf(4.0 / 20.0);
        let v_fs = full_scale_volts(tone, -9.0);
        assert!((dbu_fs(v_fs).0 - 13.0).abs() < 1e-9, "{}", dbu_fs(v_fs).0);
        // The MSD's scale test point: 1.55 V is +6 dBu.
        assert_eq!(format!("{:.1}", volts_to_dbu(1.55).0), "6.0");
        assert!((dbu_fs(DBU_REF_VOLTS).0).abs() < 1e-12);
    }

    #[test]
    fn pair_rules_name_the_inputs() {
        let inputs = vec![
            row(0, Some("M30"), CurveChoice::Off),
            line_row(1),
            line_row(2),
            line_row(3),
            InputSetup {
                channel: 4,
                kind: InputKind::Digital,
                pair: None,
            },
        ];
        let pair = |l, r| Pair {
            left: l,
            right: r,
            name: None,
        };
        let check = |pairs: &[Pair]| check_pairs(&inputs, pairs, None);
        assert_eq!(check(&[pair(1, 2)]), Ok(()));
        assert_eq!(
            check(&[pair(0, 1)]).unwrap_err(),
            "input 1 is a microphone; a pair is two line or two digital inputs"
        );
        // An undeclared input is a mic.
        assert!(
            check(&[pair(1, 9)])
                .unwrap_err()
                .starts_with("input 10 is a microphone")
        );
        assert_eq!(
            check(&[pair(3, 4)]).unwrap_err(),
            "inputs 4 and 5 are of different kinds (line input and digital input); a pair is \
             two of one kind"
        );
        assert_eq!(
            check(&[pair(1, 2), pair(2, 3)]).unwrap_err(),
            "input 3 is in two pairs"
        );
        assert_eq!(
            check(&[pair(2, 2)]).unwrap_err(),
            "input 3 cannot be paired with itself"
        );
        let captured = |ch: u16| ch < 3;
        assert_eq!(
            check_pairs(&inputs, &[pair(2, 3)], Some(&captured)).unwrap_err(),
            "input 4 is not captured by the session, so it cannot be paired"
        );
        assert!(
            check(&[Pair {
                name: Some(" x".into()),
                ..pair(1, 2)
            }])
            .unwrap_err()
            .starts_with("pair name")
        );
        let rows = apply_pair_sides(&inputs, &[pair(2, 1)]);
        assert_eq!(rows[1].pair, Some(PairSide::Right));
        assert_eq!(rows[2].pair, Some(PairSide::Left));
        assert_eq!(rows[3].pair, None);
    }

    #[test]
    fn line_input_uses_its_line_calibration_only() {
        let dev = DeviceId("d".into());
        let cals = [entry("d", 1, "M30", 5)];
        let line = [LineCalEntry {
            key: LineCalKey {
                device: dev.clone(),
                channel: 1,
            },
            line: LineCal {
                v_fs: Volts(DBU_REF_VOLTS * 10f64.powf(13.0 / 20.0)),
                volts: Volts(1.0),
                measured: Dbfs(-9.0),
                calibrated_at: WallNs(7),
            },
        }];
        let inputs = [
            line_row(1),
            InputSetup {
                channel: 2,
                kind: InputKind::Digital,
                pair: None,
            },
        ];
        let u = input_use(&cals, &line, &[], &inputs, Some(&dev), 1);
        assert_eq!(u.sensitivity, Sensitivity::Line(&line[0]));
        assert_eq!(u.sensitivity.entry(), None);
        match u.sensitivity.status() {
            CalStatus::Line {
                calibrated_at,
                dbu_fs,
            } => {
                assert_eq!(calibrated_at, WallNs(7));
                assert!((dbu_fs.0 - 13.0).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(u.curve, CurveUse::NoMic);
        // Another device: no line calibration, dBFS.
        let other = DeviceId("e".into());
        let u = input_use(&cals, &line, &[], &inputs, Some(&other), 1);
        assert_eq!(u.sensitivity, Sensitivity::Uncalibrated);
        // Digital: never calibrated, whatever is stored.
        let u = input_use(&cals, &line, &[], &inputs, Some(&dev), 2);
        assert_eq!(u.sensitivity, Sensitivity::Uncalibrated);
        // Undeclared: a mic, the sensitivity calibration on (device, input) applies.
        let u = input_use(&cals, &line, &[], &[], Some(&dev), 1);
        assert_eq!(u.sensitivity, Sensitivity::OtherMicOrInput(&cals[0]));
    }
}
