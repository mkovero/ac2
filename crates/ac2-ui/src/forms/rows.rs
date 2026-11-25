//! The rows of a sweep or dynamics dialog that pick several channels: its measurement inputs
//! (the first on [`FieldId::Measurement`], each further one on a [`FieldId::MoreMeasurement`]
//! row) and the outputs it plays on ([`FieldId::Output`], [`FieldId::MoreOutput`]). After the
//! last pick of each comes one empty row, to add the next in. One play drives every output
//! and records every input, each input into its own trace.

use ac2_proto::model::{OpenStream, SweepConfig};

use super::{Field, FieldId, Form, FormKind, Value};

/// An extra row that names no channel.
pub(crate) const NONE: &str = "none";

/// One set of rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rows {
    Measurement,
    Output,
}

impl Rows {
    /// The set field `id` is a row of.
    pub(super) fn of(id: FieldId) -> Option<Self> {
        match id {
            FieldId::Measurement | FieldId::MoreMeasurement(_) => Some(Self::Measurement),
            FieldId::Output | FieldId::MoreOutput(_) => Some(Self::Output),
            _ => None,
        }
    }

    fn first(self) -> FieldId {
        match self {
            Self::Measurement => FieldId::Measurement,
            Self::Output => FieldId::Output,
        }
    }

    fn more(self, n: u8) -> FieldId {
        match self {
            Self::Measurement => FieldId::MoreMeasurement(n),
            Self::Output => FieldId::MoreOutput(n),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Measurement => "Measurement",
            Self::Output => "Output",
        }
    }

    fn more_hint(self) -> &'static str {
        match self {
            Self::Measurement => {
                "another mic the same play records (its own trace); none: not used"
            }
            Self::Output => "another output the same play drives; none: not used",
        }
    }

    /// Most rows: a sweep takes no more measurement inputs than this; the outputs are
    /// bounded by the session's own.
    fn max(self) -> usize {
        match self {
            Self::Measurement => SweepConfig::MAX_INPUTS,
            Self::Output => usize::MAX,
        }
    }
}

impl Form {
    /// The dialogs with rows; a transfer measurement's Measurement field is a single pick.
    fn has_rows(&self) -> bool {
        matches!(
            self.kind,
            FormKind::Sweep | FormKind::SweepEdit | FormKind::Dynamics | FormKind::DynamicsEdit
        )
    }

    /// What each row of `set` names, in row order.
    fn rows(&self, set: Rows) -> Vec<Option<u16>> {
        self.fields
            .iter()
            .filter(|f| Rows::of(f.id) == Some(set))
            .map(Field::channel_value)
            .collect()
    }

    /// Every measurement input the dialog has picked, in row order.
    pub fn sweep_inputs(&self) -> Vec<u16> {
        self.rows(Rows::Measurement).into_iter().flatten().collect()
    }

    /// Every output the dialog plays on, in row order.
    pub fn sweep_outputs(&self) -> Vec<u16> {
        self.rows(Rows::Output).into_iter().flatten().collect()
    }

    /// Lays the measurement rows out for `picks` (the first on the Measurement row).
    pub(crate) fn set_sweep_inputs(&mut self, picks: &[u16]) {
        self.set_rows(Rows::Measurement, picks);
    }

    /// Lays the output rows out for `picks` (the first on the Output row).
    pub(crate) fn set_sweep_outputs(&mut self, picks: &[u16]) {
        self.set_rows(Rows::Output, picks);
    }

    /// The input no measurement row may name: a sweep divides each by its reference.
    fn barred(&self, set: Rows) -> Option<u16> {
        match set {
            Rows::Measurement => self.channel(FieldId::Reference),
            Rows::Output => None,
        }
    }

    /// Lays the rows of `set` out for `picks`: the first on the set's own field, one row per
    /// further pick, then one empty row while another channel could still be added. The
    /// focus stays on the field it was on.
    fn set_rows(&mut self, set: Rows, picks: &[u16]) {
        let Some(at) = self.fields.iter().position(|f| f.id == set.first()) else {
            return;
        };
        let focused = self.fields.get(self.focus).map(|f| f.id);
        let (channels, options) = match &mut self.fields[at].value {
            Value::Channel {
                channels,
                options,
                index,
            } => {
                *index = picks
                    .first()
                    .and_then(|p| channels.iter().position(|c| c == p))
                    .or(*index);
                (channels.clone(), options.clone())
            }
            _ => return,
        };
        let all: Vec<(u16, String)> = channels.into_iter().zip(options).collect();
        self.fields
            .retain(|f| f.id == set.first() || Rows::of(f.id) != Some(set));
        let mut rows: Vec<Option<u16>> = picks.iter().skip(1).copied().map(Some).collect();
        // An empty row only while a channel is left to add.
        let barred = self.barred(set);
        let left = all
            .iter()
            .any(|(c, _)| !picks.contains(c) && Some(*c) != barred);
        if !picks.is_empty() && picks.len() < set.max() && left {
            rows.push(None);
        }
        for (i, r) in rows.into_iter().enumerate() {
            let n = u8::try_from(i + 2).unwrap_or(u8::MAX);
            let mut f = Field::channel(set.more(n), "", &all, r, set.more_hint());
            f.label = format!("{} {n}", set.label());
            self.fields.insert(at + 1 + i, f);
        }
        if let Some(id) = focused {
            self.focus = self
                .fields
                .iter()
                .position(|f| f.id == id)
                .unwrap_or(at.min(self.fields.len().saturating_sub(1)));
        }
    }

    /// The output rows become the app's stimulus outputs (`stimulus`: the S roles of
    /// Inputs & outputs), so a new measurement plays where the I/O settings say. The
    /// loopback's output stays among them while the reference is its input: without it the
    /// reference records nothing. Stimulus outputs the session lacks are left out; with none
    /// left the rows stay as they are.
    pub fn play_on(mut self, open: Option<&OpenStream>, stimulus: &[u16]) -> Self {
        let offered: Vec<u16> = match self.field(FieldId::Output).map(|f| &f.value) {
            Some(Value::Channel { channels, .. }) => channels.clone(),
            _ => return self,
        };
        let mut plays: Vec<u16> = Vec::new();
        for o in stimulus {
            if offered.contains(o) && !plays.contains(o) {
                plays.push(*o);
            }
        }
        if plays.is_empty() {
            return self;
        }
        if let Some(l) = open.and_then(|o| o.config.loopback)
            && self.channel(FieldId::Reference) == Some(l.input)
            && offered.contains(&l.output)
            && !plays.contains(&l.output)
        {
            plays.insert(0, l.output);
        }
        self.set_sweep_outputs(&plays);
        self
    }

    /// The outputs the dialog's measurement plays on, or why it cannot play: none picked,
    /// or the reference is the loopback's input and the loopback's output is not one of
    /// them (the division by a silent reference gives nothing).
    pub(super) fn play_outputs(&self, open: Option<&OpenStream>) -> Result<Vec<u16>, String> {
        let outputs = self.sweep_outputs();
        if outputs.is_empty() {
            return Err("no output to play on".into());
        }
        if let Some(l) = open.and_then(|o| o.config.loopback)
            && self.channel(FieldId::Reference) == Some(l.input)
            && !outputs.contains(&l.output)
        {
            let name = |id: FieldId, c: u16| {
                self.channel_options(self.fields.iter().position(|f| f.id == id)?)?
                    .into_iter()
                    .find(|(x, _)| *x == Some(c))
                    .map(|(_, n)| n)
            };
            let reference = self
                .field(FieldId::Reference)
                .map(Field::display)
                .unwrap_or_default();
            let output = name(FieldId::Output, l.output)
                .unwrap_or_else(|| format!("output {}", l.output + 1));
            return Err(format!(
                "the reference, {reference}, records {output}: add {output} to the outputs \
                 (the empty Output row), or pick another reference"
            ));
        }
        Ok(outputs)
    }

    /// The reference changed: the measurement rows offer one more or one fewer input.
    pub(super) fn reference_moved(&mut self) {
        if self.has_rows() && self.fields.get(self.focus).map(|f| f.id) == Some(FieldId::Reference)
        {
            let picks = self.sweep_inputs();
            self.set_sweep_inputs(&picks);
        }
    }

    /// What row field `i` can name, in order: the first row of a set every channel, a
    /// further row `None` (not used) and every channel, each without the channels another
    /// row of the set names and without the reference. `None` when field `i` is not a row.
    pub(super) fn row_options(&self, i: usize) -> Option<Vec<Option<u16>>> {
        if !self.has_rows() {
            return None;
        }
        let f = self.fields.get(i)?;
        let set = Rows::of(f.id)?;
        let Value::Channel { channels, .. } = &f.value else {
            return None;
        };
        let row = self.row_of(i, set)?;
        let taken: Vec<u16> = self
            .rows(set)
            .iter()
            .enumerate()
            .filter(|(r, _)| *r != row)
            .filter_map(|(_, c)| *c)
            .chain(self.barred(set))
            .collect();
        Some(
            (row > 0)
                .then_some(None)
                .into_iter()
                .chain(
                    channels
                        .iter()
                        .filter(|c| !taken.contains(c))
                        .map(|c| Some(*c)),
                )
                .collect(),
        )
    }

    /// Which row of `set` field `i` is.
    fn row_of(&self, i: usize, set: Rows) -> Option<usize> {
        let id = self.fields.get(i)?.id;
        self.fields
            .iter()
            .filter(|f| Rows::of(f.id) == Some(set))
            .position(|f| f.id == id)
    }

    /// Row field `i` names `to`; the set's rows are then laid out again. False when field
    /// `i` is not a row.
    pub(super) fn set_row(&mut self, i: usize, to: Option<u16>) -> bool {
        let Some(set) = self
            .fields
            .get(i)
            .and_then(|f| Rows::of(f.id))
            .filter(|_| self.has_rows())
        else {
            return false;
        };
        let Some(row) = self.row_of(i, set) else {
            return false;
        };
        let mut picks = self.rows(set);
        picks[row] = to;
        let Some(first) = picks[0] else {
            // Nothing on the first row yet: further rows wait for it.
            return true;
        };
        let picks: Vec<u16> = std::iter::once(first)
            .chain(picks.into_iter().skip(1).flatten())
            .collect();
        self.set_rows(set, &picks);
        self.error = None;
        true
    }

    /// ←/→ on a row: the next or previous of what it can name, stopping at the ends. False
    /// for any other field.
    pub(super) fn cycle_row(&mut self, d: i32) -> bool {
        let i = self.focus;
        let Some(opts) = self.row_options(i) else {
            return false;
        };
        if opts.is_empty() {
            return true;
        }
        let cur = self.fields[i].channel_value();
        let to = match opts.iter().position(|o| *o == cur) {
            Some(at) => (at as i64 + i64::from(d.signum())).clamp(0, opts.len() as i64 - 1),
            None if d < 0 => opts.len() as i64 - 1,
            None => 0,
        } as usize;
        self.set_row(i, opts[to])
    }
}
