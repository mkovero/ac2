//! The new-measurement dialogs: a few fields that build a `meas.create`. Inputs are picked
//! by name from the session's captured inputs (with their meters in the view), never typed
//! as numbers. Pure data; the reducer routes keys here and the view draws them. Every
//! default is the CLI's (`ac2 meas new`), taken from the shared constructors in `ac2-proto`.
//!
//! The audio session dialog is [`crate::session_dialog`]; the rate and buffer parsers it
//! shares live here.

use ac2_proto::model::{
    BandFraction, DepthPolicy, MeasConfig, MeasKind, Measurement, OpenSession, RtaConfig,
    Smoothing, SmoothingFraction, SpectrumConfig, SplConfig, TimeWeighting, TransferConfig,
    Weighting,
};
use ac2_proto::units::Seconds;

/// Which dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormKind {
    Transfer,
    Spectrum,
    Rta,
    Spl,
}

impl FormKind {
    pub fn title(self) -> &'static str {
        match self {
            FormKind::Transfer => "New transfer measurement",
            FormKind::Spectrum => "New spectrum",
            FormKind::Rta => "New RTA",
            FormKind::Spl => "New SPL meter",
        }
    }

    /// What Enter does.
    pub fn submit(self) -> &'static str {
        "Enter creates and starts"
    }
}

/// A field of a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldId {
    Name,
    Reference,
    Measurement,
    Input,
    Smoothing,
    Depth,
    Fraction,
    Weighting,
    TimeWeighting,
}

/// A field's value: typed text, one of a few options (←/→ pick), or an input of the session
/// by name (←/→ pick; the view shows its meter).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Choice {
        options: Vec<String>,
        index: usize,
    },
    /// `channels[index]` is the zero-based device input; `options` their names.
    Channel {
        channels: Vec<u16>,
        options: Vec<String>,
        index: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: FieldId,
    pub label: &'static str,
    pub value: Value,
    /// Shown dimmed after the value.
    pub hint: String,
}

impl Field {
    fn text(id: FieldId, label: &'static str, text: impl Into<String>, hint: &str) -> Self {
        Self {
            id,
            label,
            value: Value::Text(text.into()),
            hint: hint.into(),
        }
    }

    fn choice(id: FieldId, label: &'static str, options: &[&str], index: usize) -> Self {
        Self {
            id,
            label,
            value: Value::Choice {
                options: options.iter().map(|s| (*s).to_owned()).collect(),
                index,
            },
            hint: String::new(),
        }
    }

    fn channel(
        id: FieldId,
        label: &'static str,
        inputs: &[(u16, String)],
        pick: u16,
        hint: &str,
    ) -> Self {
        Self {
            id,
            label,
            value: Value::Channel {
                channels: inputs.iter().map(|(c, _)| *c).collect(),
                options: inputs.iter().map(|(_, n)| n.clone()).collect(),
                index: inputs.iter().position(|(c, _)| *c == pick).unwrap_or(0),
            },
            hint: hint.into(),
        }
    }

    /// The value as shown.
    pub fn display(&self) -> String {
        match &self.value {
            Value::Text(t) => t.clone(),
            Value::Choice { options, index } | Value::Channel { options, index, .. } => {
                options.get(*index).cloned().unwrap_or_default()
            }
        }
    }

    /// The input a channel field has picked.
    pub fn channel_value(&self) -> Option<u16> {
        match &self.value {
            Value::Channel {
                channels, index, ..
            } => channels.get(*index).copied(),
            _ => None,
        }
    }
}

/// An open dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub kind: FormKind,
    pub fields: Vec<Field>,
    pub focus: usize,
    /// Why the last Enter was refused.
    pub error: Option<String>,
}

/// Smoothing choices of the transfer and spectrum dialogs (index 0: none, as `ac2 meas new`
/// without `--smooth`).
const SMOOTHING: [(&str, Option<SmoothingFraction>); 6] = [
    ("off", None),
    ("1/3 octave", Some(SmoothingFraction::Third)),
    ("1/6 octave", Some(SmoothingFraction::Sixth)),
    ("1/12 octave", Some(SmoothingFraction::Twelfth)),
    ("1/24 octave", Some(SmoothingFraction::TwentyFourth)),
    ("1/48 octave", Some(SmoothingFraction::FortyEighth)),
];
const DEPTH: [&str; 2] = [
    "equal confidence (every frequency alike)",
    "fast LF (low stages settle within 1 s)",
];
const FRACTIONS: [(&str, BandFraction); 5] = [
    ("1/1 octave", BandFraction::Octave),
    ("1/3 octave", BandFraction::Third),
    ("1/6 octave", BandFraction::Sixth),
    ("1/12 octave", BandFraction::Twelfth),
    ("1/24 octave", BandFraction::TwentyFourth),
];
const WEIGHTINGS: [(&str, Weighting); 3] = [
    ("Z (flat)", Weighting::Z),
    ("A", Weighting::A),
    ("C", Weighting::C),
];
const TIME_WEIGHTINGS: [(&str, TimeWeighting); 3] = [
    ("fast (125 ms)", TimeWeighting::Fast),
    ("slow (1 s)", TimeWeighting::Slow),
    ("impulse", TimeWeighting::Impulse),
];

/// `48000`, `48k`, `48 kHz`, `44.1khz`; empty = the device default.
pub fn parse_rate(text: &str) -> Result<Option<u32>, String> {
    let t = text.trim().to_ascii_lowercase().replace(' ', "");
    if t.is_empty() {
        return Ok(None);
    }
    let (num, scale) = if let Some(n) = t.strip_suffix("khz").or_else(|| t.strip_suffix('k')) {
        (n, 1000.0)
    } else {
        (t.strip_suffix("hz").unwrap_or(&t), 1.0)
    };
    let v = num
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| format!("not a sample rate: {:?}", text.trim()))?
        * scale;
    let r = v.round();
    if !(1000.0..=1_000_000.0).contains(&r) || (v - r).abs() > 1e-6 {
        return Err(format!("{} Hz is not a usable sample rate", text.trim()));
    }
    Ok(Some(r as u32))
}

/// Buffer size in frames; empty = the device default.
pub fn parse_buffer(text: &str) -> Result<Option<u32>, String> {
    let t = text.trim().to_ascii_lowercase();
    let t = t
        .strip_suffix("samples")
        .or_else(|| t.strip_suffix("frames"))
        .unwrap_or(&t)
        .trim();
    if t.is_empty() {
        return Ok(None);
    }
    match t.parse::<u32>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(format!("not a buffer size: {:?}", text.trim())),
    }
}

impl Form {
    fn new(kind: FormKind, fields: Vec<Field>) -> Self {
        Self {
            kind,
            fields,
            focus: 0,
            error: None,
        }
    }

    /// A measurement dialog over the session's captured `inputs` (channel, name). The
    /// reference defaults to the session's loopback input, the measurement input to the
    /// first mic (`mics`: inputs with a mic name), else the first other captured input.
    pub fn measurement(
        kind: FormKind,
        open: Option<&OpenSession>,
        existing: &[&Measurement],
        inputs: &[(u16, String)],
        mics: &[u16],
    ) -> Self {
        let captured: Vec<u16> = inputs.iter().map(|(c, _)| *c).collect();
        let reference = open
            .and_then(|o| o.config.loopback.map(|l| l.input))
            .filter(|r| captured.contains(r))
            .or_else(|| captured.first().copied())
            .unwrap_or(0);
        let measurement = mics
            .iter()
            .copied()
            .find(|m| *m != reference && captured.contains(m))
            .or_else(|| captured.iter().copied().find(|c| *c != reference))
            .unwrap_or(reference);
        let n = existing
            .iter()
            .filter(|m| form_kind(&m.config.kind) == kind)
            .count()
            + 1;
        let name = |base: &str| Field::text(FieldId::Name, "Name", format!("{base} {n}"), "");
        let input_field = Field::channel(FieldId::Input, "Input", inputs, measurement, "");
        let fields = match kind {
            FormKind::Transfer => vec![
                Field::channel(
                    FieldId::Reference,
                    "Reference",
                    inputs,
                    reference,
                    "the loopback (stimulus copy)",
                ),
                Field::channel(
                    FieldId::Measurement,
                    "Measurement",
                    inputs,
                    measurement,
                    "the mic",
                ),
                name("TF"),
                Field::choice(FieldId::Smoothing, "Smoothing", &SMOOTHING.map(|s| s.0), 0),
                Field::choice(FieldId::Depth, "Depth", &DEPTH, 0),
            ],
            FormKind::Spectrum => vec![
                input_field,
                name("Spectrum"),
                Field::choice(FieldId::Smoothing, "Smoothing", &SMOOTHING.map(|s| s.0), 0),
            ],
            FormKind::Rta => vec![
                input_field,
                name("RTA"),
                Field::choice(FieldId::Fraction, "Bands", &FRACTIONS.map(|f| f.0), 1),
            ],
            FormKind::Spl => vec![
                input_field,
                name("SPL"),
                Field::choice(FieldId::Weighting, "Weighting", &WEIGHTINGS.map(|w| w.0), 0),
                Field::choice(
                    FieldId::TimeWeighting,
                    "Time weighting",
                    &TIME_WEIGHTINGS.map(|w| w.0),
                    0,
                ),
            ],
        };
        Self::new(kind, fields)
    }

    fn field(&self, id: FieldId) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    fn field_mut(&mut self, id: FieldId) -> Option<&mut Field> {
        self.fields.iter_mut().find(|f| f.id == id)
    }

    /// The text of field `id` ("" when it is not a text field).
    pub fn text(&self, id: FieldId) -> &str {
        match self.field(id).map(|f| &f.value) {
            Some(Value::Text(t)) => t,
            _ => "",
        }
    }

    /// The input a channel field has picked.
    pub fn channel(&self, id: FieldId) -> Option<u16> {
        self.field(id).and_then(Field::channel_value)
    }

    fn choice_index(&self, id: FieldId) -> Option<usize> {
        match self.field(id).map(|f| &f.value) {
            Some(Value::Choice { options, index }) if *index < options.len() => Some(*index),
            _ => None,
        }
    }

    /// Replaces the text of field `id` (tests, prefills).
    pub fn set_text(&mut self, id: FieldId, text: &str) {
        if let Some(Value::Text(t)) = self.field_mut(id).map(|f| &mut f.value) {
            *t = text.to_owned();
        }
    }

    /// Picks input `channel` in channel field `id` (tests).
    pub fn set_channel(&mut self, id: FieldId, channel: u16) -> bool {
        if let Some(Value::Channel {
            channels, index, ..
        }) = self.field_mut(id).map(|f| &mut f.value)
            && let Some(i) = channels.iter().position(|c| *c == channel)
        {
            *index = i;
            return true;
        }
        false
    }

    /// Moves the focus by `d` fields, wrapping.
    pub fn move_focus(&mut self, d: i32) {
        let n = self.fields.len() as i32;
        if n > 0 {
            self.focus = (self.focus as i32 + d).rem_euclid(n) as usize;
        }
    }

    /// Focuses field `i`.
    pub fn focus_field(&mut self, i: usize) {
        if i < self.fields.len() {
            self.focus = i;
        }
    }

    /// ←/→ on the focused choice or channel.
    pub fn cycle(&mut self, d: i32) {
        let Some(f) = self.fields.get_mut(self.focus) else {
            return;
        };
        let (Value::Choice { options, index } | Value::Channel { options, index, .. }) =
            &mut f.value
        else {
            return;
        };
        if options.is_empty() {
            return;
        }
        let n = options.len() as i32;
        *index = (*index as i32 + d).rem_euclid(n) as usize;
        self.error = None;
    }

    /// Typed text into the focused text field.
    pub fn type_text(&mut self, s: &str) {
        if let Some(Field {
            value: Value::Text(t),
            ..
        }) = self.fields.get_mut(self.focus)
        {
            t.push_str(s);
            self.error = None;
        }
    }

    pub fn backspace(&mut self) {
        if let Some(Field {
            value: Value::Text(t),
            ..
        }) = self.fields.get_mut(self.focus)
        {
            t.pop();
            self.error = None;
        }
    }

    /// `meas.create` of the dialog. Inputs must be captured by the open session.
    pub fn meas_config(&self, open: Option<&OpenSession>) -> Result<MeasConfig, String> {
        let captured = open.map(|o| o.config.input_channels.as_slice());
        let input = |id: FieldId, what: &str| -> Result<u16, String> {
            let c = self
                .channel(id)
                .ok_or_else(|| format!("{what}: the session captures no input"))?;
            match captured {
                Some(cap) if !cap.contains(&c) => Err(format!(
                    "{what} {} is not captured by the session: reopen it with that input (Space on its row)",
                    c + 1
                )),
                _ => Ok(c),
            }
        };
        let pick = |id: FieldId| self.choice_index(id).unwrap_or(0);
        let smoothing = SMOOTHING[pick(FieldId::Smoothing).min(SMOOTHING.len() - 1)].1;
        let kind = match self.kind {
            FormKind::Transfer => {
                let r = input(FieldId::Reference, "reference input")?;
                let m = input(FieldId::Measurement, "measurement input")?;
                if r == m {
                    return Err(
                        "the reference and the measurement are the same input: pick the mic as \
                         the measurement"
                            .into(),
                    );
                }
                let mut config = TransferConfig::with_inputs(r, m);
                config.smoothing = smoothing.map(Smoothing::of);
                if pick(FieldId::Depth) == 1 {
                    config.depth = DepthPolicy::FastLf {
                        max_settle_s: Seconds(DepthPolicy::DEFAULT_FAST_LF_S),
                    };
                }
                MeasKind::Transfer { config }
            }
            FormKind::Spectrum => MeasKind::Spectrum {
                config: SpectrumConfig {
                    smoothing,
                    ..SpectrumConfig::on_input(input(FieldId::Input, "input")?)
                },
            },
            FormKind::Rta => MeasKind::Rta {
                config: RtaConfig::on_input(
                    input(FieldId::Input, "input")?,
                    FRACTIONS[pick(FieldId::Fraction).min(FRACTIONS.len() - 1)].1,
                ),
            },
            FormKind::Spl => MeasKind::Spl {
                config: SplConfig::on_input(
                    input(FieldId::Input, "input")?,
                    WEIGHTINGS[pick(FieldId::Weighting).min(WEIGHTINGS.len() - 1)].1,
                    TIME_WEIGHTINGS[pick(FieldId::TimeWeighting).min(TIME_WEIGHTINGS.len() - 1)].1,
                ),
            },
        };
        let name = self.text(FieldId::Name).trim();
        if name.is_empty() {
            return Err("type a name".into());
        }
        Ok(MeasConfig {
            name: name.to_owned(),
            kind,
        })
    }
}

/// The dialog that makes a measurement of kind `k`.
fn form_kind(k: &MeasKind) -> FormKind {
    match k {
        MeasKind::Transfer { .. } => FormKind::Transfer,
        MeasKind::Spectrum { .. } => FormKind::Spectrum,
        MeasKind::Rta { .. } => FormKind::Rta,
        MeasKind::Spl { .. } => FormKind::Spl,
    }
}

#[cfg(test)]
mod tests {
    use ac2_proto::model::{
        BackendKind, ClockRelation, DeviceId, DeviceSelector, LogGridSpec, LoopbackRoute,
        PeakWeighting, SessionConfig, SpecAveraging, TfAveraging, Window,
    };
    use ac2_proto::units::{Hz, WallNs};

    use super::*;
    use ac2_proto::model::SmoothingMode;

    fn open(inputs: Vec<u16>, loopback: Option<LoopbackRoute>) -> OpenSession {
        OpenSession {
            config: SessionConfig {
                backend: None,
                input_device: DeviceSelector::Default,
                output_device: DeviceSelector::Default,
                input_channels: inputs,
                output_channels: 2,
                sample_rate_hz: None,
                buffer_frames: None,
                loopback,
            },
            backend: BackendKind::Fake,
            input_device: DeviceId("card".into()),
            output_device: DeviceId("card".into()),
            sample_rate_hz: 48_000,
            buffer_frames: 256,
            clock: ClockRelation::SingleCallback,
            opened_at: WallNs(0),
        }
    }

    fn names(o: &OpenSession) -> Vec<(u16, String)> {
        o.config
            .input_channels
            .iter()
            .map(|c| (*c, format!("In {}", c + 1)))
            .collect()
    }

    #[test]
    fn rates_and_buffers() {
        assert_eq!(parse_rate("48k"), Ok(Some(48_000)));
        assert_eq!(parse_rate("44.1 kHz"), Ok(Some(44_100)));
        assert_eq!(parse_rate("96000"), Ok(Some(96_000)));
        assert_eq!(parse_rate(""), Ok(None));
        assert!(parse_rate("fast").is_err());
        assert_eq!(parse_buffer("256"), Ok(Some(256)));
        assert_eq!(parse_buffer(""), Ok(None));
        assert!(parse_buffer("0").is_err());
    }

    #[test]
    fn measurement_defaults_match_the_cli() {
        let o = open(
            vec![0, 1],
            Some(LoopbackRoute {
                output: 0,
                input: 0,
            }),
        );
        let f = Form::measurement(FormKind::Transfer, Some(&o), &[], &names(&o), &[]);
        let c = f.meas_config(Some(&o)).expect("tf");
        assert_eq!(c.name, "TF 1");
        // `ac2 meas new tf --ref 1 --meas 2`.
        assert_eq!(
            c.kind,
            MeasKind::Transfer {
                config: TransferConfig {
                    reference_input: 0,
                    measurement_input: 1,
                    averaging: TfAveraging::Fifo { blocks: 8 },
                    grid: LogGridSpec {
                        ppo: 48,
                        k_min: -240,
                        k_max: 239
                    },
                    smoothing: None,
                    depth: DepthPolicy::EqualConfidence,
                }
            }
        );
        let f = Form::measurement(FormKind::Spectrum, Some(&o), &[], &names(&o), &[]);
        let c = f.meas_config(Some(&o)).expect("spectrum");
        assert_eq!(
            c.kind,
            MeasKind::Spectrum {
                config: SpectrumConfig {
                    input: 1,
                    fft_len: 65_536,
                    window: Window::Hann,
                    averaging: SpecAveraging::Off,
                    smoothing: None,
                }
            }
        );
        let mut f = Form::measurement(FormKind::Spectrum, Some(&o), &[], &names(&o), &[]);
        f.focus = f
            .fields
            .iter()
            .position(|x| x.id == FieldId::Smoothing)
            .expect("smoothing field");
        f.cycle(2);
        let MeasKind::Spectrum { config } = f.meas_config(Some(&o)).expect("spectrum").kind else {
            panic!("kind");
        };
        assert_eq!(config.smoothing, Some(SmoothingFraction::Sixth));
        let c = Form::measurement(FormKind::Rta, Some(&o), &[], &names(&o), &[])
            .meas_config(Some(&o))
            .expect("rta");
        assert_eq!(
            c.kind,
            MeasKind::Rta {
                config: RtaConfig {
                    input: 1,
                    fraction: BandFraction::Third,
                    f_lo: Hz(20.0),
                    f_hi: Hz(20_000.0),
                    weighting: Weighting::Z,
                    averaging: SpecAveraging::Off,
                }
            }
        );
        let c = Form::measurement(FormKind::Spl, Some(&o), &[], &names(&o), &[])
            .meas_config(Some(&o))
            .expect("spl");
        assert_eq!(
            c.kind,
            MeasKind::Spl {
                config: SplConfig {
                    input: 1,
                    weighting: Weighting::Z,
                    time_weighting: TimeWeighting::Fast,
                    peak_weighting: PeakWeighting::C,
                }
            }
        );
    }

    #[test]
    fn inputs_are_picked_by_name_and_mics_come_first() {
        let o = open(
            vec![0, 1, 2, 3],
            Some(LoopbackRoute {
                output: 0,
                input: 2,
            }),
        );
        // The loopback is the reference, the first named mic the measurement.
        let f = Form::measurement(FormKind::Transfer, Some(&o), &[], &names(&o), &[3]);
        assert_eq!(f.channel(FieldId::Reference), Some(2));
        assert_eq!(f.channel(FieldId::Measurement), Some(3));
        let at = |f: &Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
        assert_eq!(f.fields[at(&f, FieldId::Measurement)].display(), "In 4");
        // ←/→ walks the captured inputs by name.
        let mut f = f;
        f.focus = at(&f, FieldId::Measurement);
        f.cycle(1);
        assert_eq!(f.channel(FieldId::Measurement), Some(0));
        assert_eq!(f.fields[f.focus].display(), "In 1");
        // The same input twice is refused in plain words.
        f.cycle(1);
        f.cycle(1);
        assert_eq!(f.channel(FieldId::Measurement), Some(2));
        let e = f.meas_config(Some(&o)).expect_err("same input");
        assert!(e.contains("same input"), "{e}");
    }

    #[test]
    fn measurement_choices() {
        let o = open(vec![0, 1], None);
        let mut f = Form::measurement(FormKind::Transfer, Some(&o), &[], &names(&o), &[]);
        let at = |f: &Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
        f.focus = at(&f, FieldId::Smoothing);
        f.cycle(2);
        f.focus = at(&f, FieldId::Depth);
        f.cycle(1);
        f.focus = at(&f, FieldId::Name);
        f.backspace();
        f.type_text("main");
        let c = f.meas_config(Some(&o)).expect("tf");
        assert_eq!(c.name, "TF main");
        let MeasKind::Transfer { config } = c.kind else {
            panic!("kind");
        };
        // Phase is smoothed with the magnitude.
        assert_eq!(
            config.smoothing,
            Some(Smoothing {
                fraction: SmoothingFraction::Sixth,
                mode: SmoothingMode::MagnitudePhase
            })
        );
        assert_eq!(
            config.depth,
            DepthPolicy::FastLf {
                max_settle_s: Seconds(1.0)
            }
        );
        assert!(f.set_channel(FieldId::Measurement, 0));
        assert!(f.meas_config(Some(&o)).is_err());

        let mut f = Form::measurement(FormKind::Spl, Some(&o), &[], &names(&o), &[]);
        f.focus = at(&f, FieldId::Weighting);
        f.cycle(1);
        f.focus = at(&f, FieldId::TimeWeighting);
        f.cycle(-1);
        let MeasKind::Spl { config } = f.meas_config(Some(&o)).expect("spl").kind else {
            panic!("kind");
        };
        assert_eq!(config.weighting, Weighting::A);
        assert_eq!(config.time_weighting, TimeWeighting::Impulse);
        // Typing into a choice does nothing; focus wraps.
        f.type_text("x");
        f.move_focus(1);
        assert_eq!(f.focus, 0);
        f.move_focus(-1);
        assert_eq!(f.focus, f.fields.len() - 1);
    }
}
