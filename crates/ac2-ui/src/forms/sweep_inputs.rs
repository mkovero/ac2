//! The sweep dialog's measurement inputs: the first on [`FieldId::Measurement`], every
//! further one on its own row ([`FieldId::MoreMeasurement`]), with one empty row after the
//! last pick to add the next mic in. One play records them all, each into its own trace.

use ac2_proto::model::SweepConfig;

use super::{Field, FieldId, Form, FormKind, Value};

/// An extra measurement row that names no input.
pub(crate) const NONE: &str = "none";
/// The hint of an extra measurement row.
const MORE_HINT: &str = "another mic the same play records (its own trace); none: not used";

impl Form {
    fn is_sweep(&self) -> bool {
        matches!(self.kind, FormKind::Sweep | FormKind::SweepEdit)
    }

    /// Every measurement input the sweep dialog has picked, in row order.
    pub fn sweep_inputs(&self) -> Vec<u16> {
        self.fields
            .iter()
            .filter(|f| matches!(f.id, FieldId::Measurement | FieldId::MoreMeasurement(_)))
            .filter_map(Field::channel_value)
            .collect()
    }

    /// Lays the measurement rows out for `picks` (the first on the Measurement row): one row
    /// per further pick, then one empty row while another input could still be added. The
    /// focus stays on the field it was on.
    pub(crate) fn set_sweep_inputs(&mut self, picks: &[u16]) {
        let Some(at) = self
            .fields
            .iter()
            .position(|f| f.id == FieldId::Measurement)
        else {
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
        let inputs: Vec<(u16, String)> = channels.into_iter().zip(options).collect();
        self.fields
            .retain(|f| !matches!(f.id, FieldId::MoreMeasurement(_)));
        let mut rows: Vec<Option<u16>> = picks.iter().skip(1).copied().map(Some).collect();
        // An empty row only while an input is left to add: one neither picked nor the
        // reference.
        let reference = self.channel(FieldId::Reference);
        let left = inputs
            .iter()
            .any(|(c, _)| !picks.contains(c) && Some(*c) != reference);
        if !picks.is_empty() && picks.len() < SweepConfig::MAX_INPUTS && left {
            rows.push(None);
        }
        for (i, r) in rows.into_iter().enumerate() {
            let n = u8::try_from(i + 2).unwrap_or(u8::MAX);
            let mut f = Field::channel(FieldId::MoreMeasurement(n), "", &inputs, r, MORE_HINT);
            f.label = format!("Measurement {n}");
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

    /// A sweep's reference stepped: the measurement rows offer one more or one fewer input.
    pub(super) fn reference_moved(&mut self) {
        if self.is_sweep() && self.fields.get(self.focus).map(|f| f.id) == Some(FieldId::Reference)
        {
            let picks = self.sweep_inputs();
            self.set_sweep_inputs(&picks);
        }
    }

    /// ←/→ on a sweep's measurement row: the first row through the inputs, a further row
    /// through `none` and the inputs, each skipping the inputs another row or the reference
    /// names; the rows are then laid out again. False for any other field.
    pub(super) fn cycle_sweep_input(&mut self, d: i32) -> bool {
        if !self.is_sweep() {
            return false;
        }
        let Some(f) = self.fields.get(self.focus) else {
            return false;
        };
        let id = f.id;
        let Value::Channel { channels, .. } = &f.value else {
            return false;
        };
        let channels = channels.clone();
        let rows: Vec<Option<u16>> = self
            .fields
            .iter()
            .filter(|f| matches!(f.id, FieldId::Measurement | FieldId::MoreMeasurement(_)))
            .map(Field::channel_value)
            .collect();
        let Some(row) = self
            .fields
            .iter()
            .filter(|f| matches!(f.id, FieldId::Measurement | FieldId::MoreMeasurement(_)))
            .position(|f| f.id == id)
        else {
            return false;
        };
        let taken: Vec<u16> = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != row)
            .filter_map(|(_, c)| *c)
            .chain(self.channel(FieldId::Reference))
            .collect();
        // The first row always names an input; a further one may name none. No two rows
        // name one input, and none the reference (a sweep divides each by it).
        let opts: Vec<Option<u16>> = (row > 0)
            .then_some(None)
            .into_iter()
            .chain(
                channels
                    .iter()
                    .filter(|c| !taken.contains(c))
                    .map(|c| Some(*c)),
            )
            .collect();
        if opts.is_empty() {
            return true;
        }
        let cur = rows[row];
        let to = match opts.iter().position(|o| *o == cur) {
            Some(i) => (i as i64 + i64::from(d.signum())).clamp(0, opts.len() as i64 - 1),
            None if d < 0 => opts.len() as i64 - 1,
            None => 0,
        } as usize;
        let mut picks = rows;
        picks[row] = opts[to];
        let first = picks[0];
        let picks: Vec<u16> = first
            .into_iter()
            .chain(picks.into_iter().skip(1).flatten())
            .collect();
        if first.is_none() {
            // Nothing on the first row yet: further rows wait for it.
            return true;
        }
        self.set_sweep_inputs(&picks);
        self.error = None;
        true
    }
}
