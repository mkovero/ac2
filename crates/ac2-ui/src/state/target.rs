//! The Target chooser (Z): a curve from a file, or one of the built-in curves the daemon
//! generates.

use super::*;
use ac2_proto::model::TargetCurve;
use ac2_scene::target_curve::{TargetChoice, choices, name};

impl AppState {
    /// Opens the chooser, the file row highlighted: loading a curve stays one Enter away.
    pub(super) fn open_target_chooser(&mut self) {
        let q = choices();
        self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
            purpose: ChoicePurpose::Target,
            label: q.title.clone(),
            title: q.title,
            lines: q.lines,
            choices: q.choices,
            owners: Vec::new(),
            index: 0,
            hint: q.hint,
        }));
    }

    /// A row of the chooser taken: that choice's own command runs.
    pub(super) fn target_pick(&mut self, i: usize, out: &mut Vec<Request>) {
        let Some(c) = TargetChoice::ALL.get(i).copied() else {
            return;
        };
        let id = match c {
            TargetChoice::File => CommandId::TargetFile,
            TargetChoice::Flat => CommandId::TargetFlat,
            TargetChoice::Tilt => CommandId::TargetTilt,
            TargetChoice::Loudness => CommandId::TargetLoudness,
        };
        let keymap = self.keys.clone();
        self.command(id, &keymap, out);
    }

    /// Asks the daemon for `curve` as a target trace, named as the trace list names it.
    pub(super) fn add_target(&mut self, curve: TargetCurve, out: &mut Vec<Request>) {
        let name = name(&curve);
        let what = format!("target {name} added");
        self.call(out, Command::TraceTarget { curve, name }, what);
    }
}
