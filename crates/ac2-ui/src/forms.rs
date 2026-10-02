//! Dialogs that build a daemon command from a few fields: the audio session dialog and the
//! new-measurement dialogs. Pure data and parsing; the reducer routes keys here and the view
//! draws them. Every default is the CLI's (`ac2 session open`, `ac2 meas new`), taken from
//! the shared constructors in `ac2-proto`.

use ac2_proto::model::{
    BackendKind, BandFraction, DepthPolicy, DeviceInfo, DeviceSelector, LoopbackRoute, MeasConfig,
    MeasKind, Measurement, OpenSession, RtaConfig, SessionConfig, Smoothing, SmoothingFraction,
    SmoothingMode, SpectrumConfig, SplConfig, TimeWeighting, TransferConfig, Weighting,
};
use ac2_proto::units::Seconds;

/// Which dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormKind {
    Session,
    Transfer,
    Spectrum,
    Rta,
    Spl,
}

impl FormKind {
    pub fn title(self) -> &'static str {
        match self {
            FormKind::Session => "Open audio session",
            FormKind::Transfer => "New transfer measurement",
            FormKind::Spectrum => "New spectrum",
            FormKind::Rta => "New RTA",
            FormKind::Spl => "New SPL meter",
        }
    }

    /// What Enter does.
    pub fn submit(self) -> &'static str {
        match self {
            FormKind::Session => "Enter opens",
            _ => "Enter creates and starts",
        }
    }
}

/// A field of a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldId {
    Backend,
    Device,
    Inputs,
    Outputs,
    Rate,
    Buffer,
    Loopback,
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

/// A field's value: typed text, or one of a few options (←/→ pick).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Choice { options: Vec<String>, index: usize },
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

    /// The value as shown.
    pub fn display(&self) -> String {
        match &self.value {
            Value::Text(t) => t.clone(),
            Value::Choice { options, index } => options.get(*index).cloned().unwrap_or_default(),
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
    /// Session dialog: the daemon's devices; `None` while `session.devices` is on its way.
    pub devices: Option<Vec<DeviceInfo>>,
    /// Session dialog: the device of the open session, preselected when listed.
    prefer_device: Option<String>,
}

/// Smoothing choices of the transfer dialog (index 0: none, as `ac2 meas new` without
/// `--smooth`).
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

fn backend_name(b: BackendKind) -> &'static str {
    match b {
        BackendKind::Cpal => "cpal (system audio)",
        BackendKind::Jack => "jack",
        BackendKind::Fake => "fake (simulated rig, no audio)",
    }
}

/// The loopback a backend's dialog starts with: the fake rig is wired out 1 → in 1; a real
/// interface's wiring is the operator's to type.
fn default_loopback(b: Option<BackendKind>) -> &'static str {
    match b {
        Some(BackendKind::Fake) => "1>1",
        _ => "",
    }
}

/// `1-2`, `1,3`, `1-4, 7` (one-based, as `ac2 session open --in`) → zero-based channels.
pub fn parse_channels(text: &str) -> Result<Vec<u16>, String> {
    let mut out: Vec<u16> = Vec::new();
    for part in text.split(',').map(str::trim) {
        if part.is_empty() {
            continue;
        }
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (parse_channel(a)?, parse_channel(b)?),
            None => {
                let c = parse_channel(part)?;
                (c, c)
            }
        };
        if b < a {
            return Err(format!("{part}: range runs backwards"));
        }
        for c in a..=b {
            if out.contains(&c) {
                return Err(format!("channel {} listed twice", c + 1));
            }
            out.push(c);
        }
    }
    if out.is_empty() {
        return Err("at least one channel, e.g. 1-2".into());
    }
    Ok(out)
}

/// One one-based channel → zero-based.
pub fn parse_channel(text: &str) -> Result<u16, String> {
    let t = text.trim();
    match t.parse::<u16>() {
        Ok(0) => Err("channels count from 1".into()),
        Ok(n) => Ok(n - 1),
        Err(_) => Err(format!("not a channel number: {t:?}")),
    }
}

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

/// `1>1`, `1->1`, `1 → 1`, `1:1` (output → input, one-based); empty = no loopback.
pub fn parse_loopback(text: &str) -> Result<Option<LoopbackRoute>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let norm = t.replace("->", ">").replace(['→', ':'], ">");
    let (o, i) = norm
        .split_once('>')
        .ok_or_else(|| format!("{t:?}: expected out>in, e.g. 1>1"))?;
    Ok(Some(LoopbackRoute {
        output: parse_channel(o)?,
        input: parse_channel(i)?,
    }))
}

impl Form {
    fn new(kind: FormKind, fields: Vec<Field>) -> Self {
        Self {
            kind,
            fields,
            focus: 0,
            error: None,
            devices: None,
            prefer_device: None,
        }
    }

    /// The session dialog, prefilled from the open session (if any) until the device list
    /// arrives ([`Form::set_devices`]).
    pub fn session(open: Option<&OpenSession>) -> Self {
        let c = open.map(|o| &o.config);
        let inputs = c.map_or_else(|| "1-2".to_owned(), |c| channels_text(&c.input_channels));
        let outputs = c.map_or_else(|| "2".to_owned(), |c| c.output_channels.to_string());
        let rate = c
            .and_then(|c| c.sample_rate_hz)
            .map(|r| r.to_string())
            .unwrap_or_default();
        let buffer = c
            .and_then(|c| c.buffer_frames)
            .map(|b| b.to_string())
            .unwrap_or_default();
        let loopback = c
            .and_then(|c| c.loopback)
            .map(|l| format!("{}>{}", l.output + 1, l.input + 1))
            .unwrap_or_default();
        let mut f = Self::new(
            FormKind::Session,
            vec![
                Field::choice(FieldId::Backend, "Backend", &[], 0),
                Field::choice(FieldId::Device, "Device", &[], 0),
                Field::text(FieldId::Inputs, "Input channels", inputs, "e.g. 1-2 or 1,3"),
                Field::text(FieldId::Outputs, "Output channels", outputs, "how many"),
                Field::text(FieldId::Rate, "Sample rate", rate, "empty = device default"),
                Field::text(
                    FieldId::Buffer,
                    "Buffer (frames)",
                    buffer,
                    "empty = device default",
                ),
                Field::text(
                    FieldId::Loopback,
                    "Loopback out>in",
                    loopback,
                    "reference copy, e.g. 1>1; empty = none",
                ),
            ],
        );
        f.prefer_device = open.map(|o| o.input_device.0.clone());
        f.set_hint(FieldId::Device, "listing devices…");
        f
    }

    /// Fills the backend and device choices from `session.devices`. The preselected backend
    /// is a real one when the daemon lists one; the simulated rig is only preselected on a
    /// daemon that offers nothing else (it was started on it explicitly).
    pub fn set_devices(&mut self, devices: Vec<DeviceInfo>) {
        let mut backends: Vec<BackendKind> = Vec::new();
        for d in &devices {
            if !backends.contains(&d.backend) {
                backends.push(d.backend);
            }
        }
        let prefer = self
            .prefer_device
            .as_ref()
            .and_then(|id| devices.iter().find(|d| &d.id.0 == id))
            .map(|d| d.backend);
        let index = prefer
            .and_then(|b| backends.iter().position(|x| *x == b))
            .or_else(|| backends.iter().position(|b| *b != BackendKind::Fake))
            .unwrap_or(0);
        if let Some(f) = self.field_mut(FieldId::Backend) {
            f.value = Value::Choice {
                options: backends
                    .iter()
                    .map(|b| backend_name(*b).to_owned())
                    .collect(),
                index,
            };
        }
        let fresh = self.prefer_device.is_none();
        self.devices = Some(devices);
        self.refresh_devices();
        if fresh {
            let lb = default_loopback(self.backend());
            if let Some(Value::Text(t)) = self.field_mut(FieldId::Loopback).map(|f| &mut f.value) {
                *t = lb.to_owned();
            }
        }
        if self.device().is_none() {
            self.set_hint(FieldId::Device, "the daemon lists no device");
        }
    }

    fn backends(&self) -> Vec<BackendKind> {
        let mut v: Vec<BackendKind> = Vec::new();
        for d in self.devices.iter().flatten() {
            if !v.contains(&d.backend) {
                v.push(d.backend);
            }
        }
        v
    }

    /// The chosen backend.
    pub fn backend(&self) -> Option<BackendKind> {
        let i = self.choice_index(FieldId::Backend)?;
        self.backends().get(i).copied()
    }

    fn backend_devices(&self) -> Vec<&DeviceInfo> {
        let b = self.backend();
        self.devices
            .iter()
            .flatten()
            .filter(|d| Some(d.backend) == b)
            .collect()
    }

    /// The chosen device.
    pub fn device(&self) -> Option<&DeviceInfo> {
        let i = self.choice_index(FieldId::Device)?;
        self.backend_devices().get(i).copied()
    }

    fn refresh_devices(&mut self) {
        let devs: Vec<DeviceInfo> = self.backend_devices().into_iter().cloned().collect();
        let index = self
            .prefer_device
            .as_ref()
            .and_then(|id| devs.iter().position(|d| &d.id.0 == id))
            .unwrap_or(0);
        let options = devs
            .iter()
            .map(|d| {
                if devs.iter().filter(|x| x.name == d.name).count() > 1 {
                    format!("{} ({})", d.name, d.id.0)
                } else {
                    d.name.clone()
                }
            })
            .collect();
        if let Some(f) = self.field_mut(FieldId::Device) {
            f.value = Value::Choice { options, index };
        }
        self.describe_device();
    }

    fn describe_device(&mut self) {
        let hint = match self.device() {
            Some(d) => {
                let ins = d.input.as_ref().map_or(0, |i| i.max_channels);
                let outs = d.output.as_ref().map_or(0, |o| o.max_channels);
                let rate = d
                    .input
                    .as_ref()
                    .and_then(|i| i.default_rate_hz)
                    .map(|r| format!(" · {} Hz default", r))
                    .unwrap_or_default();
                format!("{ins} in / {outs} out{rate}")
            }
            None => String::new(),
        };
        self.set_hint(FieldId::Device, &hint);
    }

    /// A measurement dialog, its inputs defaulting from the open session: the loopback input
    /// is the reference, the first other captured input the measurement.
    pub fn measurement(
        kind: FormKind,
        open: Option<&OpenSession>,
        existing: &[&Measurement],
    ) -> Self {
        let captured = open
            .map(|o| o.config.input_channels.clone())
            .unwrap_or_default();
        let reference = open
            .and_then(|o| o.config.loopback.map(|l| l.input))
            .or_else(|| captured.first().copied())
            .unwrap_or(0);
        let measurement = captured
            .iter()
            .copied()
            .find(|c| *c != reference)
            .unwrap_or(reference + 1);
        let input = captured
            .iter()
            .copied()
            .find(|c| *c != reference)
            .or_else(|| captured.first().copied())
            .unwrap_or(0);
        let n = existing
            .iter()
            .filter(|m| form_kind(&m.config.kind) == kind)
            .count()
            + 1;
        let one = |c: u16| (u32::from(c) + 1).to_string();
        let name = |base: &str| Field::text(FieldId::Name, "Name", format!("{base} {n}"), "");
        let input_field = Field::text(FieldId::Input, "Input", one(input), "one-based");
        let fields = match kind {
            FormKind::Transfer => vec![
                Field::text(
                    FieldId::Reference,
                    "Reference input",
                    one(reference),
                    "the loopback (stimulus copy)",
                ),
                Field::text(
                    FieldId::Measurement,
                    "Measurement input",
                    one(measurement),
                    "the mic",
                ),
                name("TF"),
                Field::choice(FieldId::Smoothing, "Smoothing", &SMOOTHING.map(|s| s.0), 0),
                Field::choice(FieldId::Depth, "Depth", &DEPTH, 0),
            ],
            FormKind::Spectrum => vec![input_field, name("Spectrum")],
            FormKind::Rta => vec![
                input_field,
                name("RTA"),
                Field::choice(FieldId::Fraction, "Bands", &FRACTIONS.map(|f| f.0), 1),
            ],
            FormKind::Spl | FormKind::Session => vec![
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

    fn set_hint(&mut self, id: FieldId, hint: &str) {
        if let Some(f) = self.field_mut(id) {
            f.hint = hint.to_owned();
        }
    }

    /// The text of field `id` ("" when it is not a text field).
    pub fn text(&self, id: FieldId) -> &str {
        match self.field(id).map(|f| &f.value) {
            Some(Value::Text(t)) => t,
            _ => "",
        }
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

    /// ←/→ on the focused choice.
    pub fn cycle(&mut self, d: i32) {
        let Some(f) = self.fields.get_mut(self.focus) else {
            return;
        };
        let id = f.id;
        let Value::Choice { options, index } = &mut f.value else {
            return;
        };
        if options.is_empty() {
            return;
        }
        let n = options.len() as i32;
        *index = (*index as i32 + d).rem_euclid(n) as usize;
        self.error = None;
        match id {
            FieldId::Backend => {
                let before = self.text(FieldId::Loopback).to_owned();
                let was_default = ["", "1>1"].contains(&before.as_str());
                self.prefer_device = None;
                self.refresh_devices();
                if was_default {
                    let lb = default_loopback(self.backend());
                    self.set_text(FieldId::Loopback, lb);
                }
            }
            FieldId::Device => self.describe_device(),
            _ => {}
        }
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

    /// `session.open` of the session dialog and the device's name.
    pub fn session_config(&self) -> Result<(SessionConfig, String), String> {
        if self.devices.is_none() {
            return Err("the device list has not arrived yet".into());
        }
        let dev = self.device().ok_or_else(|| {
            "no device to open: the daemon lists none for this backend".to_string()
        })?;
        let max_in = dev.input.as_ref().map_or(0, |i| i.max_channels);
        let max_out = dev.output.as_ref().map_or(0, |o| o.max_channels);
        let inputs = parse_channels(self.text(FieldId::Inputs))
            .map_err(|e| format!("input channels: {e}"))?;
        if let Some(c) = inputs.iter().find(|c| **c >= max_in) {
            return Err(format!(
                "input {} does not exist: {} has {max_in} inputs",
                c + 1,
                dev.name
            ));
        }
        let outputs: u16 = self
            .text(FieldId::Outputs)
            .trim()
            .parse()
            .map_err(|_| "output channels: type how many, e.g. 2".to_string())?;
        if outputs == 0 || outputs > max_out {
            return Err(format!("output channels: 1 … {max_out} on {}", dev.name));
        }
        let rate = parse_rate(self.text(FieldId::Rate))?;
        let buffer = parse_buffer(self.text(FieldId::Buffer))?;
        let loopback = parse_loopback(self.text(FieldId::Loopback))?;
        if let Some(l) = loopback {
            if l.output >= outputs {
                return Err(format!(
                    "loopback output {} is not among the {outputs} output channels",
                    l.output + 1
                ));
            }
            if !inputs.contains(&l.input) {
                return Err(format!(
                    "loopback input {} is not captured (input channels {})",
                    l.input + 1,
                    channels_text(&inputs)
                ));
            }
        }
        let sel = DeviceSelector::Id { id: dev.id.clone() };
        Ok((
            SessionConfig {
                input_device: sel.clone(),
                output_device: sel,
                input_channels: inputs,
                output_channels: outputs,
                sample_rate_hz: rate,
                buffer_frames: buffer,
                loopback,
            },
            dev.name.clone(),
        ))
    }

    /// `meas.create` of a measurement dialog. Inputs must be captured by the open session.
    pub fn meas_config(&self, open: Option<&OpenSession>) -> Result<MeasConfig, String> {
        let captured = open.map(|o| o.config.input_channels.as_slice());
        let input = |id: FieldId, what: &str| -> Result<u16, String> {
            let c = parse_channel(self.text(id)).map_err(|e| format!("{what}: {e}"))?;
            match captured {
                Some(cap) if !cap.contains(&c) => Err(format!(
                    "{what} {} is not captured by the session (inputs {})",
                    c + 1,
                    channels_text(cap)
                )),
                _ => Ok(c),
            }
        };
        let pick = |id: FieldId| self.choice_index(id).unwrap_or(0);
        let kind = match self.kind {
            FormKind::Transfer => {
                let r = input(FieldId::Reference, "reference input")?;
                let m = input(FieldId::Measurement, "measurement input")?;
                if r == m {
                    return Err("reference and measurement are the same input".into());
                }
                let mut config = TransferConfig::with_inputs(r, m);
                config.smoothing = SMOOTHING[pick(FieldId::Smoothing).min(SMOOTHING.len() - 1)]
                    .1
                    .map(|fraction| Smoothing {
                        fraction,
                        mode: SmoothingMode::Power,
                    });
                if pick(FieldId::Depth) == 1 {
                    config.depth = DepthPolicy::FastLf {
                        max_settle_s: Seconds(DepthPolicy::DEFAULT_FAST_LF_S),
                    };
                }
                MeasKind::Transfer { config }
            }
            FormKind::Spectrum => MeasKind::Spectrum {
                config: SpectrumConfig::on_input(input(FieldId::Input, "input")?),
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
            FormKind::Session => return Err("not a measurement dialog".into()),
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

/// `1-2`, `1, 3` (one-based) of zero-based channels; consecutive runs as ranges.
pub fn channels_text(ch: &[u16]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < ch.len() {
        let mut j = i;
        while j + 1 < ch.len() && ch[j + 1] == ch[j] + 1 {
            j += 1;
        }
        if j > i {
            parts.push(format!("{}-{}", ch[i] + 1, ch[j] + 1));
        } else {
            parts.push((ch[i] + 1).to_string());
        }
        i = j + 1;
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use ac2_proto::model::{
        ClockRelation, DeviceId, DirectionInfo, IndexExactness, LogGridSpec, PeakWeighting,
        RangeU32, SpecAveraging, TfAveraging, Window,
    };
    use ac2_proto::units::{Hz, WallNs};

    use super::*;

    fn dev(backend: BackendKind, id: &str, ins: u16, outs: u16) -> DeviceInfo {
        let dir = |ch| DirectionInfo {
            max_channels: ch,
            rates_hz: vec![RangeU32 {
                min: 48_000,
                max: 48_000,
            }],
            buffer_frames: None,
            default_rate_hz: Some(48_000),
        };
        DeviceInfo {
            backend,
            host: "test".into(),
            id: DeviceId(id.into()),
            name: id.into(),
            input: Some(dir(ins)),
            output: Some(dir(outs)),
            duplex_clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            notes: vec![],
        }
    }

    fn open(inputs: Vec<u16>, loopback: Option<LoopbackRoute>) -> OpenSession {
        OpenSession {
            config: SessionConfig {
                input_device: DeviceSelector::Default,
                output_device: DeviceSelector::Default,
                input_channels: inputs,
                output_channels: 2,
                sample_rate_hz: None,
                buffer_frames: None,
                loopback,
            },
            input_device: DeviceId("card".into()),
            output_device: DeviceId("card".into()),
            sample_rate_hz: 48_000,
            buffer_frames: 256,
            clock: ClockRelation::SingleCallback,
            opened_at: WallNs(0),
        }
    }

    #[test]
    fn channel_parsing() {
        assert_eq!(parse_channels("1-2"), Ok(vec![0, 1]));
        assert_eq!(parse_channels("1, 3-4"), Ok(vec![0, 2, 3]));
        assert!(parse_channels("0").is_err());
        assert!(parse_channels("2-1").is_err());
        assert!(parse_channels("1,1").is_err());
        assert!(parse_channels("").is_err());
        assert_eq!(channels_text(&[0, 1, 3]), "1-2, 4");
        assert_eq!(parse_rate("48k"), Ok(Some(48_000)));
        assert_eq!(parse_rate("44.1 kHz"), Ok(Some(44_100)));
        assert_eq!(parse_rate("96000"), Ok(Some(96_000)));
        assert_eq!(parse_rate(""), Ok(None));
        assert!(parse_rate("fast").is_err());
        assert_eq!(parse_buffer("256"), Ok(Some(256)));
        assert_eq!(parse_buffer(""), Ok(None));
        assert!(parse_buffer("0").is_err());
        assert_eq!(
            parse_loopback("2 → 1"),
            Ok(Some(LoopbackRoute {
                output: 1,
                input: 0
            }))
        );
        assert_eq!(
            parse_loopback("1->1").ok().flatten().map(|l| l.input),
            Some(0)
        );
        assert_eq!(parse_loopback(""), Ok(None));
        assert!(parse_loopback("1").is_err());
    }

    #[test]
    fn session_prefers_a_real_backend_and_never_defaults_a_loopback_on_it() {
        let mut f = Form::session(None);
        assert!(f.session_config().is_err(), "no devices yet");
        f.set_devices(vec![
            dev(BackendKind::Fake, "fake:loop", 4, 2),
            dev(BackendKind::Cpal, "card", 8, 8),
        ]);
        assert_eq!(f.backend(), Some(BackendKind::Cpal));
        assert_eq!(f.text(FieldId::Loopback), "");
        let (c, name) = f.session_config().expect("config");
        assert_eq!(name, "card");
        assert_eq!(c.input_channels, vec![0, 1]);
        assert_eq!(c.output_channels, 2);
        assert_eq!(c.loopback, None);
        assert_eq!(
            c.input_device,
            DeviceSelector::Id {
                id: DeviceId("card".into())
            }
        );
        // Choosing the simulated rig explicitly brings its wiring.
        f.focus = 0;
        f.cycle(1);
        assert_eq!(f.backend(), Some(BackendKind::Fake));
        assert_eq!(f.text(FieldId::Loopback), "1>1");
        let (c, _) = f.session_config().expect("config");
        assert_eq!(
            c.loopback,
            Some(LoopbackRoute {
                output: 0,
                input: 0
            })
        );
    }

    #[test]
    fn session_validates_against_the_device() {
        let mut f = Form::session(None);
        f.set_devices(vec![dev(BackendKind::Fake, "fake:loop", 4, 2)]);
        assert_eq!(f.backend(), Some(BackendKind::Fake));
        for (field, text, want) in [
            (FieldId::Inputs, "1-5", "does not exist"),
            (FieldId::Outputs, "3", "output channels"),
            (FieldId::Rate, "fast", "sample rate"),
            (FieldId::Loopback, "1>3", "not captured"),
            (FieldId::Loopback, "3>1", "not among"),
        ] {
            let mut g = f.clone();
            g.set_text(field, text);
            let e = g.session_config().expect_err(text);
            assert!(e.contains(want), "{text}: {e}");
        }
        f.set_text(FieldId::Rate, "48k");
        f.set_text(FieldId::Buffer, "256");
        let (c, _) = f.session_config().expect("config");
        assert_eq!(
            (c.sample_rate_hz, c.buffer_frames),
            (Some(48_000), Some(256))
        );
    }

    #[test]
    fn session_prefills_from_the_open_session() {
        let o = open(
            vec![0, 1, 2],
            Some(LoopbackRoute {
                output: 1,
                input: 2,
            }),
        );
        let mut f = Form::session(Some(&o));
        f.set_devices(vec![
            dev(BackendKind::Cpal, "other", 2, 2),
            dev(BackendKind::Cpal, "card", 8, 8),
        ]);
        assert_eq!(f.device().map(|d| d.id.0.as_str()), Some("card"));
        assert_eq!(f.text(FieldId::Inputs), "1-3");
        assert_eq!(f.text(FieldId::Loopback), "2>3");
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
        let f = Form::measurement(FormKind::Transfer, Some(&o), &[]);
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
        let mut f = Form::measurement(FormKind::Spectrum, Some(&o), &[]);
        let c = f.meas_config(Some(&o)).expect("spectrum");
        assert_eq!(
            c.kind,
            MeasKind::Spectrum {
                config: SpectrumConfig {
                    input: 1,
                    fft_len: 65_536,
                    window: Window::Hann,
                    averaging: SpecAveraging::Off,
                }
            }
        );
        f.set_text(FieldId::Input, "3");
        assert!(
            f.meas_config(Some(&o))
                .expect_err("uncaptured")
                .contains("not captured")
        );
        let c = Form::measurement(FormKind::Rta, Some(&o), &[])
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
        let c = Form::measurement(FormKind::Spl, Some(&o), &[])
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
    fn measurement_choices() {
        let o = open(vec![0, 1], None);
        let mut f = Form::measurement(FormKind::Transfer, Some(&o), &[]);
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
        assert_eq!(
            config.smoothing.map(|s| s.fraction),
            Some(SmoothingFraction::Sixth)
        );
        assert_eq!(
            config.depth,
            DepthPolicy::FastLf {
                max_settle_s: Seconds(1.0)
            }
        );
        f.set_text(FieldId::Measurement, "1");
        assert!(f.meas_config(Some(&o)).is_err());

        let mut f = Form::measurement(FormKind::Spl, Some(&o), &[]);
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
