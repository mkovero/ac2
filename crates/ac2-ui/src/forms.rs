//! The new-measurement dialogs: a few fields that build a `meas.create`, and the sweep
//! dialog that sets up an `ir.capture`. Inputs and outputs are picked by name from the
//! session's channels (inputs with their meters in the view), never typed as numbers. Pure
//! data; the reducer routes keys here and the view draws them. Every default is the CLI's
//! (`ac2 meas new`, `ac2 ir capture`), taken from the shared constructors in `ac2-proto`.
//!
//! The audio session dialog is [`crate::session_dialog`]; the rate and buffer parsers it
//! shares live here.

use ac2_proto::model::{
    BandFraction, DepthPolicy, EssSpec, MeasConfig, MeasKind, Measurement, OpenSession, RtaConfig,
    Smoothing, SmoothingFraction, SpectrumConfig, SplConfig, SweepInputs, SweepRequest,
    TimeWeighting, TransferConfig, Weighting,
};
use ac2_proto::units::{Dbfs, Hz, Seconds};

/// Which dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormKind {
    Transfer,
    Spectrum,
    Rta,
    Spl,
    /// Sweep measurement (harmonic distortion).
    Sweep,
}

impl FormKind {
    pub fn title(self) -> &'static str {
        match self {
            FormKind::Transfer => "New transfer measurement",
            FormKind::Spectrum => "New spectrum",
            FormKind::Rta => "New RTA",
            FormKind::Spl => "New SPL meter",
            FormKind::Sweep => "Sweep measurement (response and harmonic distortion)",
        }
    }

    /// What Enter does.
    pub fn submit(self) -> &'static str {
        match self {
            FormKind::Sweep => "Enter arms the sweep (then Enter plays it, Esc stops)",
            _ => "Enter creates and starts",
        }
    }

    /// What closing the dialog does to the stimulus, for a dialog that arms one.
    pub fn close_note(self) -> Option<String> {
        match self {
            FormKind::Sweep => Some(format!(
                "Esc closes and disarms a stimulus armed but not playing · one that plays keeps \
                 playing: {} or the strip's Stop stops it",
                crate::keys::STOP_ANYWHERE.label()
            )),
            _ => None,
        }
    }

    /// The button that does it.
    pub fn verb(self) -> &'static str {
        match self {
            FormKind::Sweep => "Arm",
            _ => "Create and start",
        }
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
    /// The output the sweep plays on (the speaker's).
    Output,
    Level,
    From,
    To,
    Duration,
    Repeats,
    /// Silence recorded after each sweep: the room's decay and its noise.
    Tail,
}

/// A field's value: typed text, one of a few options (←/→ pick), or an input of the session
/// by name (←/→ pick; the view shows its meter). ←/→ step through the options in order and
/// stop at the ends.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Choice {
        options: Vec<String>,
        index: usize,
    },
    /// `channels[index]` is the zero-based device input; `options` their names. `None`:
    /// nothing chosen yet, where guessing could pick the wrong input.
    Channel {
        channels: Vec<u16>,
        options: Vec<String>,
        index: Option<usize>,
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
        pick: Option<u16>,
        hint: &str,
    ) -> Self {
        let index = pick.map(|p| inputs.iter().position(|(c, _)| *c == p).unwrap_or(0));
        Self {
            id,
            label,
            value: Value::Channel {
                channels: inputs.iter().map(|(c, _)| *c).collect(),
                options: inputs.iter().map(|(_, n)| n.clone()).collect(),
                index,
            },
            hint: hint.into(),
        }
    }

    /// The value as shown.
    pub fn display(&self) -> String {
        match &self.value {
            Value::Text(t) => t.clone(),
            Value::Choice { options, index } => options.get(*index).cloned().unwrap_or_default(),
            Value::Channel {
                options,
                index: Some(i),
                ..
            } => options.get(*i).cloned().unwrap_or_default(),
            Value::Channel { index: None, .. } => {
                format!("choose the {}", self.label.to_lowercase())
            }
        }
    }

    /// The input a channel field has picked.
    pub fn channel_value(&self) -> Option<u16> {
        match &self.value {
            Value::Channel {
                channels,
                index: Some(i),
                ..
            } => channels.get(*i).copied(),
            _ => None,
        }
    }
}

/// A sweep the dialog set up: what `ir.capture` gets once armed and fired.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepPlan {
    pub request: SweepRequest,
    pub name: String,
}

/// Sweep durations offered, shortest first: → is longer, ← shorter.
const DURATIONS: [(&str, f64); 4] = [
    ("1 s (quick look)", 1.0),
    ("3 s", 3.0),
    ("6 s (lower floor, longer windows)", 6.0),
    ("12 s", 12.0),
];
/// The CLI's default duration, 3 s.
const DEFAULT_DURATION: usize = 1;
const REPEATS: [(&str, u8); 4] = [("1", 1), ("2", 2), ("4", 4), ("8", 8)];
/// Silence after each sweep, shortest first: the room parameters need the decay and some
/// noise after it inside it (a hall's 2 s decay needs about 4 s).
const TAILS: [(&str, f64); 4] = [
    ("1 s (small rooms)", 1.0),
    ("2 s", 2.0),
    ("4 s (halls)", 4.0),
    ("8 s (large halls, churches)", 8.0),
];

/// `20`, `20 Hz`, `20k`, `1.5 kHz`.
pub fn parse_freq(text: &str) -> Result<f64, String> {
    let t = text.trim().to_ascii_lowercase().replace(' ', "");
    let (num, scale) = if let Some(n) = t.strip_suffix("khz").or_else(|| t.strip_suffix('k')) {
        (n, 1000.0)
    } else {
        (t.strip_suffix("hz").unwrap_or(&t), 1.0)
    };
    let v = num
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| format!("not a frequency: {:?}", text.trim()))?
        * scale;
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("not a frequency: {:?}", text.trim()))
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
    /// The focused text field's whole text is selected: typing replaces it, Backspace
    /// clears it. A text field is selected when it gets the focus, and by Ctrl+A.
    pub selected: bool,
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
            selected: false,
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
        let input_field = Field::channel(FieldId::Input, "Input", inputs, Some(measurement), "");
        let fields = match kind {
            // Built by [`Form::sweep`].
            FormKind::Sweep => Vec::new(),
            FormKind::Transfer => vec![
                Field::channel(
                    FieldId::Reference,
                    "Reference",
                    inputs,
                    Some(reference),
                    "the loopback (stimulus copy)",
                ),
                Field::channel(
                    FieldId::Measurement,
                    "Measurement",
                    inputs,
                    Some(measurement),
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
                // An SPL meter reads A-weighted Fast by default: the level hearing limits and
                // venue rules are written in.
                Field::choice(FieldId::Weighting, "Weighting", &WEIGHTINGS.map(|w| w.0), 1),
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

    /// The sweep dialog over the session's captured `inputs` and its `outputs` (channel,
    /// name). The reference is the session's loopback input; without one it is left for the
    /// operator to choose, because the analysis divides by it and an input picked by channel
    /// order would give a plausible but meaningless result. Mic and speaker output default
    /// as for a transfer measurement; the loopback output (when the session has one) always
    /// plays the sweep too. `level` is the operator's typed stimulus level, if any: there is
    /// no default.
    pub fn sweep(
        open: Option<&OpenSession>,
        sweeps: usize,
        inputs: &[(u16, String)],
        outputs: &[(u16, String)],
        mics: &[u16],
        level: Option<Dbfs>,
    ) -> Self {
        let base = Self::measurement(FormKind::Transfer, open, &[], inputs, mics);
        let reference = open
            .and_then(|o| o.config.loopback.map(|l| l.input))
            .filter(|r| inputs.iter().any(|(c, _)| c == r));
        let loopback_out = open.and_then(|o| o.config.loopback.map(|l| l.output));
        let speaker = outputs
            .iter()
            .map(|(c, _)| *c)
            .find(|c| Some(*c) != loopback_out)
            .or_else(|| outputs.first().map(|(c, _)| *c))
            .unwrap_or(0);
        let level_text = level.map_or_else(String::new, |l| {
            ac2_scene::format::fixed(l.0, 1).replace(ac2_scene::format::MINUS, "-")
        });
        let hint_out = match loopback_out {
            Some(l) => {
                let name = outputs
                    .iter()
                    .find(|(c, _)| *c == l)
                    .map_or_else(|| format!("output {}", l + 1), |(_, n)| n.clone());
                format!("the speaker; the loopback output, {name}, plays too")
            }
            None => "the speaker".to_owned(),
        };
        let fields = vec![
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
                base.channel(FieldId::Measurement),
                "the mic",
            ),
            Field::channel(FieldId::Output, "Output", outputs, Some(speaker), &hint_out),
            Field::text(
                FieldId::Level,
                "Level",
                level_text,
                "dBFS RMS, required (e.g. -50)",
            ),
            Field::text(FieldId::From, "From", "20 Hz", ""),
            Field::text(FieldId::To, "To", "20 kHz", ""),
            Field::choice(
                FieldId::Duration,
                "Duration",
                &DURATIONS.map(|d| d.0),
                DEFAULT_DURATION,
            ),
            Field::choice(FieldId::Repeats, "Repeats", &REPEATS.map(|r| r.0), 0),
            Field::choice(FieldId::Tail, "Silence after", &TAILS.map(|t| t.0), 0),
            Field::text(FieldId::Name, "Name", format!("Sweep {}", sweeps + 1), ""),
        ];
        Self::new(FormKind::Sweep, fields)
    }

    /// The sweep the dialog sets up, checked against the session and the daemon's ceiling.
    pub fn sweep_plan(
        &self,
        open: Option<&OpenSession>,
        ceiling: Option<Dbfs>,
    ) -> Result<SweepPlan, String> {
        let open = open.ok_or("no open audio session")?;
        let captured = &open.config.input_channels;
        let input = |id: FieldId, what: &str| -> Result<u16, String> {
            let c = self
                .channel(id)
                .ok_or_else(|| format!("{what}: the session captures no input"))?;
            if captured.contains(&c) {
                Ok(c)
            } else {
                Err(format!("{what} {} is not captured by the session", c + 1))
            }
        };
        if self.channel(FieldId::Reference).is_none() {
            return Err(
                "choose the reference: the input that records the loopback (a copy of the \
                 stimulus)"
                    .into(),
            );
        }
        let reference = input(FieldId::Reference, "reference input")?;
        let measurement = input(FieldId::Measurement, "measurement input")?;
        if reference == measurement {
            return Err(
                "the reference and the measurement are the same input: pick the mic as the \
                 measurement"
                    .into(),
            );
        }
        let level = self.text(FieldId::Level).trim();
        if level.is_empty() {
            return Err("type the level (dBFS): a sweep has no default level".into());
        }
        let level = crate::state::parse_number(level, &["dbfs", "db"])?;
        if level > 0.0 {
            return Err("level must be ≤ 0 dBFS".into());
        }
        if let Some(c) = ceiling
            && level > c.0
        {
            return Err(format!(
                "above the daemon's ceiling {} dBFS",
                ac2_scene::format::fixed(c.0, 1)
            ));
        }
        let from = parse_freq(self.text(FieldId::From))?;
        let to = parse_freq(self.text(FieldId::To))?;
        let nyquist = f64::from(open.sample_rate_hz) / 2.0;
        if from >= to || to > nyquist {
            return Err(format!(
                "the sweep must run up, within the rate's limit of {}",
                ac2_scene::format::freq_readout(nyquist)
            ));
        }
        let speaker = self
            .channel(FieldId::Output)
            .ok_or("no output to play on")?;
        let mut outputs = vec![speaker];
        if let Some(l) = open.config.loopback
            && l.output != speaker
        {
            outputs.push(l.output);
        }
        let duration = DURATIONS[self
            .choice_index(FieldId::Duration)
            .unwrap_or(DEFAULT_DURATION)]
        .1;
        let repeats = REPEATS[self.choice_index(FieldId::Repeats).unwrap_or(0)].1;
        let tail = TAILS[self.choice_index(FieldId::Tail).unwrap_or(0)].1;
        let name = self.text(FieldId::Name).trim();
        if name.is_empty() {
            return Err("type a name".into());
        }
        Ok(SweepPlan {
            request: SweepRequest {
                inputs: SweepInputs::Channels {
                    reference,
                    measurement,
                },
                outputs,
                level: Some(Dbfs(level)),
                sweep: EssSpec::with_fades(Hz(from), Hz(to), Seconds(duration)),
                repeats,
                gate: None,
                tail: Some(Seconds(tail)),
            },
            name: name.to_owned(),
        })
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
            *index = Some(i);
            return true;
        }
        false
    }

    /// Moves the focus by `d` fields, wrapping.
    pub fn move_focus(&mut self, d: i32) {
        let n = self.fields.len() as i32;
        if n > 0 {
            self.focus_field((self.focus as i32 + d).rem_euclid(n) as usize);
        }
    }

    /// Focuses field `i`; a text field arrives with its text selected, so typing replaces a
    /// default instead of appending to it.
    pub fn focus_field(&mut self, i: usize) {
        if i < self.fields.len() && i != self.focus {
            self.focus = i;
            self.select_all();
        }
    }

    /// Ctrl+A: selects the focused text field's text.
    pub fn select_all(&mut self) {
        self.selected = matches!(
            self.fields.get(self.focus).map(|f| &f.value),
            Some(Value::Text(_))
        );
    }

    /// ←/→ on the focused choice or channel: the next or previous option, stopping at the
    /// ends. An unchosen channel takes the first option on →, the last on ←.
    pub fn cycle(&mut self, d: i32) {
        let Some(f) = self.fields.get_mut(self.focus) else {
            return;
        };
        let step = |i: usize, n: usize| (i as i64 + i64::from(d)).clamp(0, n as i64 - 1) as usize;
        match &mut f.value {
            Value::Text(_) => return,
            Value::Choice { options, index } => {
                if options.is_empty() {
                    return;
                }
                *index = step(*index, options.len());
            }
            Value::Channel { options, index, .. } => {
                let n = options.len();
                if n == 0 {
                    return;
                }
                *index = Some(match *index {
                    None if d < 0 => n - 1,
                    None => 0,
                    Some(i) => step(i, n),
                });
            }
        }
        self.error = None;
    }

    /// Typed text into the focused text field; replaces a selected text.
    pub fn type_text(&mut self, s: &str) {
        let selected = std::mem::take(&mut self.selected);
        if let Some(Field {
            value: Value::Text(t),
            ..
        }) = self.fields.get_mut(self.focus)
        {
            if selected {
                t.clear();
            }
            t.push_str(s);
            self.error = None;
        }
    }

    /// Deletes the last character, or the selected text.
    pub fn backspace(&mut self) {
        let selected = std::mem::take(&mut self.selected);
        if let Some(Field {
            value: Value::Text(t),
            ..
        }) = self.fields.get_mut(self.focus)
        {
            if selected {
                t.clear();
            } else {
                t.pop();
            }
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
            FormKind::Sweep => return Err("a sweep makes no measurement".into()),
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
            replay: None,
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
                    weighting: Weighting::A,
                    time_weighting: TimeWeighting::Fast,
                    peak_weighting: PeakWeighting::C,
                    leq: ac2_proto::model::LeqConfig::default_windows(),
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
        // ←/→ walks the captured inputs by name and stops at the ends.
        let mut f = f;
        f.focus = at(&f, FieldId::Measurement);
        f.cycle(1);
        assert_eq!(f.channel(FieldId::Measurement), Some(3));
        f.cycle(-3);
        assert_eq!(f.channel(FieldId::Measurement), Some(0));
        assert_eq!(f.fields[f.focus].display(), "In 1");
        f.cycle(-1);
        assert_eq!(f.channel(FieldId::Measurement), Some(0));
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
        // A by default; → steps on to C.
        f.focus = at(&f, FieldId::Weighting);
        f.cycle(1);
        f.focus = at(&f, FieldId::TimeWeighting);
        // ← at the first choice stays there; → steps on and stops at the last.
        f.cycle(-1);
        assert_eq!(f.fields[f.focus].display(), "fast (125 ms)");
        f.cycle(5);
        let MeasKind::Spl { config } = f.meas_config(Some(&o)).expect("spl").kind else {
            panic!("kind");
        };
        assert_eq!(config.weighting, Weighting::C);
        assert_eq!(config.time_weighting, TimeWeighting::Impulse);
        // Typing into a choice does nothing; focus wraps.
        f.type_text("x");
        f.move_focus(1);
        assert_eq!(f.focus, 0);
        f.move_focus(-1);
        assert_eq!(f.focus, f.fields.len() - 1);
    }

    fn outs() -> Vec<(u16, String)> {
        vec![(0, "Out 1".into()), (1, "Out 2".into())]
    }

    fn focus(f: &mut Form, id: FieldId) {
        let i = f.fields.iter().position(|x| x.id == id).expect("field");
        f.focus_field(i);
    }

    /// Duration and repeats step in order from the 3 s / 1× defaults and stop at the ends:
    /// 6 s is one → away, and holding a key never wraps to the other end.
    #[test]
    fn sweep_steppers_are_sorted_and_stop_at_the_ends() {
        let o = open(
            vec![0, 1],
            Some(LoopbackRoute {
                output: 1,
                input: 0,
            }),
        );
        let mut f = Form::sweep(Some(&o), 0, &names(&o), &outs(), &[], Some(Dbfs(-50.0)));
        let duration = |f: &Form| f.sweep_plan(Some(&o), None).expect("plan").request.sweep;
        let repeats = |f: &Form| f.sweep_plan(Some(&o), None).expect("plan").request.repeats;
        assert_eq!(duration(&f).duration, Seconds(3.0));
        assert_eq!(repeats(&f), 1);
        focus(&mut f, FieldId::Duration);
        f.cycle(1);
        assert_eq!(duration(&f).duration, Seconds(6.0));
        f.cycle(1);
        f.cycle(1);
        assert_eq!(duration(&f).duration, Seconds(12.0));
        for _ in 0..3 {
            f.cycle(-1);
        }
        assert_eq!(duration(&f).duration, Seconds(1.0));
        f.cycle(-1);
        assert_eq!(duration(&f).duration, Seconds(1.0));
        let shown: Vec<f64> = DURATIONS.iter().map(|d| d.1).collect();
        assert!(shown.is_sorted(), "{shown:?}");

        focus(&mut f, FieldId::Repeats);
        f.cycle(-1);
        assert_eq!(repeats(&f), 1);
        for want in [2, 4, 8, 8] {
            f.cycle(1);
            assert_eq!(repeats(&f), want);
        }
    }

    /// Without a session loopback nothing guesses the reference: the dialog asks for it and
    /// refuses to arm until it is chosen. With one, the loopback input is the default.
    #[test]
    fn sweep_reference_is_the_loopback_or_chosen() {
        let o = open(vec![0, 1, 2], None);
        let mut f = Form::sweep(Some(&o), 0, &names(&o), &outs(), &[2], Some(Dbfs(-50.0)));
        assert_eq!(f.channel(FieldId::Reference), None);
        assert_eq!(f.fields[0].display(), "choose the reference");
        let e = f.sweep_plan(Some(&o), None).expect_err("no reference");
        assert!(e.contains("choose the reference"), "{e}");
        // → takes the first input, ← from nothing the last.
        f.focus_field(0);
        f.cycle(-1);
        assert_eq!(f.channel(FieldId::Reference), Some(2));
        f.cycle(-1);
        assert_eq!(f.channel(FieldId::Reference), Some(1));
        let p = f.sweep_plan(Some(&o), None).expect("plan");
        assert_eq!(
            p.request.inputs,
            SweepInputs::Channels {
                reference: 1,
                measurement: 2
            }
        );

        let o = open(
            vec![0, 1, 2],
            Some(LoopbackRoute {
                output: 1,
                input: 2,
            }),
        );
        let f = Form::sweep(Some(&o), 0, &names(&o), &outs(), &[1], Some(Dbfs(-50.0)));
        assert_eq!(f.channel(FieldId::Reference), Some(2));
        assert_eq!(f.channel(FieldId::Measurement), Some(1));
        assert!(f.sweep_plan(Some(&o), None).is_ok());
    }

    /// A text field gets its text selected with the focus (and by Ctrl+A): typing replaces
    /// the default name or level instead of appending to it.
    #[test]
    fn typing_replaces_a_selected_default() {
        let o = open(
            vec![0, 1],
            Some(LoopbackRoute {
                output: 1,
                input: 0,
            }),
        );
        let mut f = Form::sweep(Some(&o), 2, &names(&o), &outs(), &[], Some(Dbfs(-50.0)));
        assert_eq!(f.text(FieldId::Name), "Sweep 3");
        focus(&mut f, FieldId::Name);
        assert!(f.selected);
        f.type_text("1083 ");
        f.type_text("on axis");
        assert_eq!(f.text(FieldId::Name), "1083 on axis");
        // Ctrl+A then Backspace clears it; plain Backspace takes one character.
        f.select_all();
        f.backspace();
        assert_eq!(f.text(FieldId::Name), "");
        f.type_text("ab");
        f.backspace();
        assert_eq!(f.text(FieldId::Name), "a");

        focus(&mut f, FieldId::Level);
        assert_eq!(f.text(FieldId::Level), "-50.0");
        f.type_text("-40");
        assert_eq!(f.text(FieldId::Level), "-40");
        // A choice is never "selected": Ctrl+A there changes nothing.
        focus(&mut f, FieldId::Duration);
        assert!(!f.selected);
        f.select_all();
        assert!(!f.selected);
    }
}
