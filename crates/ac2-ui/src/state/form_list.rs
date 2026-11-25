//! The list of a dialog's channel field ([`crate::forms::list`]): opened with the channels
//! the session's measurements used last, so the few inputs a rig keeps measuring come first.

use crate::forms::{FieldId, Value};
use crate::state::*;

impl AppState {
    /// The inputs (or `outputs`) the session's measurements use, those of the measurement
    /// made last first, each once.
    fn recent_channels(&self, outputs: bool) -> Vec<u16> {
        let mut all: Vec<u16> = Vec::new();
        let mut meas = self.measurements();
        meas.sort_by_key(|m| std::cmp::Reverse(m.id.0));
        for m in meas {
            let used: Vec<u16> = match (&m.config.kind, outputs) {
                (MeasKind::Sweep { config }, true) => config.outputs.clone(),
                (MeasKind::Dynamics { config }, true) => config.outputs.clone(),
                (_, true) => Vec::new(),
                (MeasKind::Transfer { config }, false) => {
                    vec![config.measurement_input, config.reference_input]
                }
                (MeasKind::Spectrum { config }, false) => vec![config.input],
                (MeasKind::Rta { config }, false) => vec![config.input],
                (MeasKind::Spl { config }, false) => vec![config.input],
                (MeasKind::Sweep { config }, false) => config
                    .measurement_inputs
                    .iter()
                    .copied()
                    .chain([config.reference_input])
                    .collect(),
                (MeasKind::Dynamics { config }, false) => config
                    .measurement_inputs
                    .iter()
                    .copied()
                    .chain([config.reference_input])
                    .collect(),
                (MeasKind::Math { .. }, false) => Vec::new(),
            };
            for c in used {
                if !all.contains(&c) {
                    all.push(c);
                }
            }
        }
        all
    }

    /// Opens the list of the dialog's channel field `i` (`None`: the focused field). False
    /// when that field is not a channel field: the key or click is someone else's.
    pub(in crate::state) fn open_form_list(&mut self, i: Option<usize>) -> bool {
        let Overlay::Form(f) = &self.overlay else {
            return false;
        };
        let i = i.unwrap_or(f.focus);
        let Some(field) = f.fields.get(i) else {
            return false;
        };
        if !matches!(field.value, Value::Channel { .. }) {
            return false;
        }
        let outputs = matches!(field.id, FieldId::Output | FieldId::MoreOutput(_));
        let recent = self.recent_channels(outputs);
        if let Overlay::Form(f) = &mut self.overlay {
            f.open_list(i, &recent);
        }
        true
    }
}
