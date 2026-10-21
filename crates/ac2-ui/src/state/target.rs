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

    /// Asks the daemon for `curve` as a target trace, named as the trace list names it,
    /// under the measurement the operator is on ([`Self::curve_owner`]).
    pub(super) fn add_target(&mut self, curve: TargetCurve, out: &mut Vec<Request>) {
        let name = name(&curve);
        let owner = self.curve_owner();
        let what = ac2_scene::target_curve::added(&name, self.owner_name(owner).as_deref());
        self.call(out, Command::TraceTarget { curve, name, owner }, what);
    }

    /// Where a new target curve is listed: the transfer or sweep measurement of the selected
    /// row (a trace's, a math channel's owner), else the one the transfer pane shows, else
    /// Imported. A target is read against that system's curves, so it belongs with them and
    /// is drawn in its panes; a curve under Imported is drawn in none.
    pub(super) fn curve_owner(&self) -> TraceOwner {
        let fits = |id: &MeasId| {
            self.meas(*id).is_some_and(|m| {
                matches!(
                    m.config.kind,
                    MeasKind::Transfer { .. } | MeasKind::Sweep { .. }
                )
            })
        };
        self.selected_group()
            .and_then(TraceOwner::meas)
            .filter(fits)
            .or_else(|| {
                let shown = self.kind_meas(PaneKind::Transfer);
                self.group_of_shown(shown).filter(fits)
            })
            .map_or(TraceOwner::Imported, |meas| TraceOwner::Meas { meas })
    }

    /// The name of the measurement `owner` is, `None` for Imported.
    pub(super) fn owner_name(&self, owner: TraceOwner) -> Option<String> {
        owner
            .meas()
            .and_then(|id| self.meas(id))
            .map(|m| m.config.name.clone())
    }
}
