//! The new-measurement dialogs: a few fields that build a `meas.create` (a sweep
//! measurement's too: its settings, run later with `sweep.run`), and the dialogs that edit
//! a math channel, a transfer or sweep measurement, a spectrum or an RTA. Inputs and outputs
//! are picked by name from the session's channels (inputs with their meters in the view), never typed as numbers. Pure
//! data; the reducer routes keys here and the view draws them. Every default is the CLI's
//! (`ac2 meas new`, `ac2 ir capture`), taken from the shared constructors in `ac2-proto`.
//!
//! The audio I/O dialog is [`crate::session_dialog`]; the rate and buffer parsers it
//! shares live here.

use ac2_proto::model::{
    BandFraction, DepthPolicy, EssSpec, LfHarmonics, MeasConfig, MeasKind, Measurement,
    OpenSession, Operand, Resolution, RtaConfig, Smoothing, SmoothingFraction, SpecAveraging,
    SpecLevel, SpecResolution, SpectrumConfig, SplConfig, SweepConfig, TimeWeighting,
    TransferConfig, Weighting, Window,
};
use ac2_proto::units::{Dbfs, Hz, MeasId, Seconds};

mod build;
mod options;

use options::*;

pub(crate) use options::resolution_field;
pub use options::{LF_HARMONICS_HINT, RESOLUTION_HINT, parse_buffer, parse_rate};

/// Which dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormKind {
    Transfer,
    Spectrum,
    Rta,
    Spl,
    /// Sweep measurement (harmonic distortion): created, run later.
    Sweep,
    /// A sweep measurement's settings edited (for its next run).
    SweepEdit,
    /// A new math channel ([`crate::math_dialog`]).
    Math,
    /// A math channel edited.
    MathEdit,
    /// A spectrum's settings edited.
    SpectrumEdit,
    /// An RTA's settings edited.
    RtaEdit,
    /// A transfer measurement's settings edited (its averaging restarts).
    TransferEdit,
    /// A WAV / RF64 file imported as an impulse response ([`crate::ir_import_dialog`]).
    IrImport,
    /// A house tilt target curve ([`crate::target_dialog`]).
    TargetTilt,
    /// A loudness shift target curve ([`crate::target_dialog`]).
    TargetLoudness,
    /// A new Programme pane ([`crate::meter_dialog`]).
    Programme,
    /// What a Programme pane meters, edited in place.
    ProgrammeEdit,
}

impl FormKind {
    pub fn title(self) -> &'static str {
        match self {
            FormKind::Transfer => "New transfer measurement",
            FormKind::Spectrum => "New spectrum",
            FormKind::Rta => "New RTA",
            FormKind::Spl => "New SPL meter",
            FormKind::Math => "New math channel (A ÷ × + − ↔ B, or the average of several)",
            FormKind::MathEdit => "Edit math channel",
            FormKind::Sweep => "New sweep measurement (response and harmonic distortion)",
            FormKind::SweepEdit => "Edit sweep measurement (its next run)",
            FormKind::SpectrumEdit => "Edit spectrum",
            FormKind::RtaEdit => "Edit RTA",
            FormKind::TransferEdit => "Edit transfer measurement (averaging restarts)",
            FormKind::IrImport => "Import impulse response",
            FormKind::TargetTilt => ac2_scene::target_curve::tilt::TITLE,
            FormKind::TargetLoudness => ac2_scene::target_curve::loudness::TITLE,
            FormKind::Programme => ac2_scene::programme::form::NEW_TITLE,
            FormKind::ProgrammeEdit => ac2_scene::programme::form::EDIT_TITLE,
        }
    }

    /// What Enter does.
    pub fn submit(self) -> &'static str {
        match self {
            FormKind::Sweep => {
                "Enter creates it; nothing plays · Space on the sweep pane arms a run, Enter plays it"
            }
            FormKind::IrImport => "Enter imports it as a transfer trace",
            FormKind::TargetTilt | FormKind::TargetLoudness => ac2_scene::target_curve::SUBMIT,
            FormKind::Programme => ac2_scene::programme::form::NEW_SUBMIT,
            FormKind::ProgrammeEdit => ac2_scene::programme::form::EDIT_SUBMIT,
            FormKind::MathEdit
            | FormKind::SweepEdit
            | FormKind::SpectrumEdit
            | FormKind::RtaEdit
            | FormKind::TransferEdit => "Enter applies",
            _ => "Enter creates and starts",
        }
    }

    /// What closing the dialog does to the stimulus, for a dialog that arms one (none
    /// does: a sweep measurement is created without arming).
    pub fn close_note(self) -> Option<String> {
        None
    }

    /// The button that does it.
    pub fn verb(self) -> &'static str {
        match self {
            FormKind::Sweep => "Create",
            FormKind::IrImport => "Import",
            FormKind::TargetTilt | FormKind::TargetLoudness => "Add",
            FormKind::Programme => "Open",
            FormKind::MathEdit
            | FormKind::SweepEdit
            | FormKind::SpectrumEdit
            | FormKind::RtaEdit
            | FormKind::TransferEdit
            | FormKind::ProgrammeEdit => "Apply",
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
    /// Spectrum / RTA averaging (on power).
    Averaging,
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
    /// Harmonic windows at a sweep's lowest columns.
    LfHarmonics,
    /// Frequency points per octave of a transfer or sweep curve.
    Resolution,
    /// A spectrum's resolution: one FFT's length, or the multi-resolution bands.
    FftSize,
    /// A spectrum's FFT window.
    Window,
    /// A spectrum's level: tone or density.
    SpecLevel,
    /// Whether this operand is in a math channel's average.
    Member(Operand),
    /// How a math channel's average combines its operands.
    Method,
    /// A math channel's first operand.
    OperandA,
    /// A math channel's operator.
    Operator,
    /// A math channel's second operand.
    OperandB,
    /// The operand whose delay a math channel's sum or average is referred to.
    PhaseRef,
    /// An alignment's overlap band, lower and upper edge.
    AlignFrom,
    AlignTo,
    /// The longest delay an alignment looks at.
    MaxDelay,
    /// The channel of a multi-channel impulse-response file.
    IrChannel,
    /// An imported impulse response's time zero: its peak or the file's start.
    TimeZero,
    /// An imported impulse response's gate after the peak (`off`: the whole file).
    Gate,
    /// An imported impulse response's gate before the peak.
    GatePre,
    /// A target tilt's slope in dB per octave.
    Slope,
    /// Where a target tilt is 0 dB.
    Pivot,
    /// A target tilt's LF shelf gain (`off`: none).
    ShelfGain,
    /// Where a target tilt's LF shelf has half its gain.
    ShelfCorner,
    /// A loudness shift's levels: judged at, played at.
    PhonFrom,
    PhonTo,
    /// What a Programme pane meters: one of the session's pairs, or inputs.
    Metered,
    /// A Programme pane's input row `n` (zero-based) when it meters inputs.
    Bar(u8),
    /// The rig's programme-meter settings ([`ac2_proto::model::LevelsSettings`]).
    Ballistic,
    LoudnessTarget,
    CorrelationWindow,
    OverThreshold,
    CorrelationGate,
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
    pub label: String,
    pub value: Value,
    /// Shown dimmed after the value.
    pub hint: String,
}

impl Field {
    pub(crate) fn text(
        id: FieldId,
        label: &'static str,
        text: impl Into<String>,
        hint: &str,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            value: Value::Text(text.into()),
            hint: hint.into(),
        }
    }

    pub(crate) fn choice(
        id: FieldId,
        label: impl Into<String>,
        options: &[&str],
        index: usize,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            value: Value::Choice {
                options: options.iter().map(|s| (*s).to_owned()).collect(),
                index,
            },
            hint: String::new(),
        }
    }

    pub(crate) fn channel(
        id: FieldId,
        label: &'static str,
        inputs: &[(u16, String)],
        pick: Option<u16>,
        hint: &str,
    ) -> Self {
        let index = pick.map(|p| inputs.iter().position(|(c, _)| *c == p).unwrap_or(0));
        Self {
            id,
            label: label.into(),
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
            Value::Channel { index: None, .. } => match self.id {
                FieldId::Bar(n) if n > 0 => ac2_scene::programme::form::NO_BAR.to_owned(),
                _ => format!("choose the {}", self.label.to_lowercase()),
            },
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
    /// The math channel dialog's own state ([`crate::math_dialog`]).
    pub math: Option<Box<crate::math_dialog::MathForm>>,
    /// The sweep measurement this dialog edits.
    pub sweep_edit: Option<MeasId>,
    /// The spectrum or RTA this dialog edits: what the fields don't show stays its own.
    pub meas_edit: Option<Box<Measurement>>,
    /// The file the impulse-response import dialog reads.
    pub ir_import: Option<Box<crate::ir_import_dialog::IrTarget>>,
    /// The programme meter dialog's own state ([`crate::meter_dialog`]).
    pub meter: Option<Box<crate::meter_dialog::MeterForm>>,
    /// A heading drawn above the field it names.
    pub headings: Vec<(FieldId, String)>,
    /// The averaging field's values, one per option.
    averaging: Vec<SpecAveraging>,
    /// The open session's sample rate: what the FFT size field's hint is stated at.
    sample_rate: Option<u32>,
}

impl Form {
    pub(crate) fn field(&self, id: FieldId) -> Option<&Field> {
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

    pub(crate) fn choice_index(&self, id: FieldId) -> Option<usize> {
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

    /// The footer's key cue: only the keys that do something in this form, so a dialog of
    /// plain values never promises ←→ or an input list it does not have.
    pub fn keys_hint(&self) -> String {
        let mut parts = vec![self.kind.submit().to_string()];
        if self.fields.len() > 1 {
            parts.push("↑↓ field".into());
        }
        let channel = self
            .fields
            .iter()
            .any(|f| matches!(f.value, Value::Channel { .. }));
        let choice = self
            .fields
            .iter()
            .any(|f| matches!(f.value, Value::Choice { .. }));
        if channel {
            parts.push("←→ choose (inputs by name)".into());
        } else if choice {
            parts.push("←→ choose".into());
        }
        parts.push("Esc closes".into());
        parts.join(" · ")
    }

    /// The inputs whose meters are drawn beside field `i`: a channel field's input (an
    /// output has none), both sides of a pair the programme meter picks.
    pub fn field_meters(&self, i: usize) -> Vec<u16> {
        let Some(f) = self.fields.get(i) else {
            return Vec::new();
        };
        if let Some(m) = self.meter.as_ref().filter(|_| f.id == FieldId::Metered) {
            return m.pair_of(f).map_or_else(Vec::new, |(l, r)| vec![l, r]);
        }
        f.channel_value()
            .filter(|_| f.id != FieldId::Output)
            .into_iter()
            .collect()
    }

    /// The heading drawn above field `id`, if any.
    pub fn heading(&self, id: FieldId) -> Option<&str> {
        self.headings
            .iter()
            .find(|(f, _)| *f == id)
            .map(|(_, h)| h.as_str())
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
        if crate::meter_dialog::cycle(self, d) {
            return;
        }
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
        if self.math.is_some() {
            self.math_changed();
        }
        crate::ir_import_dialog::rehint(self, self.focus);
        if let Some((i, r)) = self
            .choice_index(FieldId::FftSize)
            .and_then(|i| spec_resolutions().get(i).map(|r| (i, *r)))
            && self.fields[self.focus].id == FieldId::FftSize
        {
            // The option before this step (← / → move one option).
            let before = usize::try_from(i as i64 - i64::from(d))
                .ok()
                .and_then(|j| spec_resolutions().get(j).copied());
            self.fields[self.focus].hint = self.fft_hint(r);
            // Between one FFT and the multi-resolution bands the level follows the new
            // resolution's default (`SpecLevel::default_for`), as `ac2 meas set` does;
            // the operator can still pick the other one after.
            let level = SpecLevel::default_for(r);
            if let Some(f) = self.fields.iter_mut().find(|f| f.id == FieldId::SpecLevel)
                && let Value::Choice { index, .. } = &mut f.value
            {
                let was = SPEC_LEVELS.get(*index).map(|l| l.1);
                let kind_changed = matches!(
                    (before, r),
                    (
                        Some(SpecResolution::Fixed { .. }),
                        SpecResolution::Multi { .. }
                    ) | (
                        Some(SpecResolution::Multi { .. }),
                        SpecResolution::Fixed { .. }
                    )
                );
                if kind_changed && was != Some(level) {
                    *index = SPEC_LEVELS.iter().position(|l| l.1 == level).unwrap_or(0);
                    f.hint = spec_level_hint(level).to_owned();
                }
            }
        }
        if self.fields[self.focus].id == FieldId::SpecLevel {
            let level = self.spec_level();
            self.fields[self.focus].hint = spec_level_hint(level).to_owned();
        }
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
}

#[cfg(test)]
mod tests;
