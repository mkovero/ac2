//! The New measurement chooser (Ctrl+M): every kind of measurement in one list, each
//! opening the same settings form its own command does.

use super::*;
use ac2_scene::new_meas::{NewKind, new_choices};

impl AppState {
    /// Opens the chooser, the kind pane `p` shows highlighted. A kind that cannot be made
    /// now is listed with the reason its own command would give.
    pub(super) fn open_new_measurement(&mut self, p: PaneId, keymap: &Keymap) {
        let preselect = match self.layout.kind(p) {
            PaneKind::Transfer => NewKind::Transfer,
            PaneKind::Spectrum => match self.pane_meas(p).map(|m| &m.config.kind) {
                Some(MeasKind::Rta { .. }) => NewKind::Rta,
                _ => NewKind::Spectrum,
            },
            PaneKind::Spl => NewKind::Spl,
            PaneKind::Distortion => NewKind::Sweep,
        };
        // Each kind's own key, where bound: the direct route next time.
        let key = |k: NewKind| keymap.label(kind_command(k), Scope::Global);
        let blocked = |k: NewKind| self.new_refusal(k, keymap);
        let q = new_choices(preselect, key, blocked);
        self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
            purpose: ChoicePurpose::NewMeasurement,
            label: q.title.clone(),
            title: q.title,
            lines: q.lines,
            choices: q.choices,
            owners: Vec::new(),
            index: q.default,
            hint: q.hint,
        }));
    }

    /// After the session dialog opened a session, once the daemon's state shows it (the
    /// chooser says which kinds can be made from that state): a fresh session's first step
    /// is choosing what to measure; one with measurements, or a window open, asks nothing.
    pub(super) fn choose_first(&mut self) {
        if !self.choose_when_open || self.open_session().is_none() {
            return;
        }
        self.choose_when_open = false;
        let none_yet = self.daemon().is_some_and(|s| s.measurements.is_empty());
        if none_yet && self.overlay == Overlay::None {
            let keymap = self.keys.clone();
            self.open_new_measurement(self.layout.focus, &keymap);
        }
    }

    /// The measurement list with nothing in it: how to make the first one.
    pub fn empty_list_text(&self) -> String {
        ac2_scene::new_meas::empty_list(&self.new_meas_reach())
    }

    /// How message text says to open the chooser: its key, else its palette entry.
    pub(super) fn new_meas_reach(&self) -> String {
        self.keys.reach(CommandId::NewMeasurement)
    }

    /// A row of the chooser taken: that kind's own command opens its form.
    pub(super) fn new_measurement_pick(&mut self, i: usize, out: &mut Vec<Request>) {
        let Some(kind) = NewKind::ALL.get(i).copied() else {
            return;
        };
        let keymap = self.keys.clone();
        self.command(kind_command(kind), &keymap, out);
    }

    /// Why kind `k` cannot be made now, in the words its own command warns with; `None`
    /// when its form would open.
    pub(super) fn new_refusal(&self, k: NewKind, keymap: &Keymap) -> Option<String> {
        match k {
            NewKind::Math => self.math_form().err(),
            _ if self.open_session().is_some() => None,
            NewKind::Sweep => Some(format!(
                "no audio session to sweep: {}",
                open_session_hint(keymap)
            )),
            _ => Some(format!(
                "no audio session to measure: {}",
                open_session_hint(keymap)
            )),
        }
    }

    /// The math channel form as Shift+M opens it, or why it cannot open.
    pub(super) fn math_form(&self) -> Result<Form, String> {
        // The channel lives under the measurement selected now (a selected trace's or math
        // channel's owner): its live curve and traces are offered first.
        let owner = self
            .selected_group()
            .unwrap_or(ac2_proto::model::TraceOwner::Imported);
        // A starts as what the focused pane shows: its measurement, or the selected trace.
        let first = self
            .selected_trace_meta()
            .map(|t| Operand::Trace { trace: t.id })
            .or_else(|| {
                let p = match self.layout.focus_kind() {
                    PaneKind::Spectrum => PaneKind::Spectrum,
                    _ => PaneKind::Transfer,
                };
                // A sweep has no live curve to be an operand: its runs are traces.
                self.kind_meas(p)
                    .filter(|m| m.config.kind.is_job())
                    .map(|m| Operand::Meas { meas: m.id })
            });
        Form::math(self.math_candidates(Some(owner)), first, owner).ok_or_else(|| {
            format!(
                "a math channel needs two transfer functions, spectra or RTAs of one kind, \
                 live or stored: make a measurement or capture one ({}) first",
                self.capture_keys()
            )
        })
    }
}

/// The command that opens kind `k`'s form.
fn kind_command(k: NewKind) -> CommandId {
    match k {
        NewKind::Transfer => CommandId::NewTransfer,
        NewKind::Spectrum => CommandId::NewSpectrum,
        NewKind::Rta => CommandId::NewRta,
        NewKind::Spl => CommandId::NewSpl,
        NewKind::Sweep => CommandId::SweepNew,
        NewKind::Math => CommandId::NewMath,
    }
}
