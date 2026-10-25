//! The New measurement chooser (Ctrl+M): every kind of measurement in one list, each
//! opening the same settings form its own command does; the Programme meter's form opens a
//! new Programme pane.

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
            PaneKind::Programme => NewKind::Programme,
            PaneKind::Distortion => NewKind::Sweep,
        };
        // Each kind's own key, where bound: the direct route next time.
        let key = |k: NewKind| kind_command(k).and_then(|c| keymap.label(c, Scope::Global));
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

    /// A row of the chooser taken: that kind's own command opens its form; the meter's form
    /// has no command of its own.
    pub(super) fn new_measurement_pick(&mut self, i: usize, out: &mut Vec<Request>) {
        let Some(kind) = NewKind::ALL.get(i).copied() else {
            return;
        };
        match kind_command(kind) {
            Some(c) => {
                let keymap = self.keys.clone();
                self.command(c, &keymap, out);
            }
            None => self.open_meter_form(None, out),
        }
    }

    /// Why kind `k` cannot be made now, in the words its own command warns with; `None`
    /// when its form would open.
    pub(super) fn new_refusal(&self, k: NewKind, keymap: &Keymap) -> Option<String> {
        match k {
            NewKind::Math => self.math_form().err(),
            NewKind::Alignment => self.alignment_form().err(),
            NewKind::Programme if self.open_session().is_none() => Some(format!(
                "no audio I/O to meter: {}",
                open_session_hint(keymap)
            )),
            NewKind::Programme => self.meter_form(None).err(),
            _ if self.open_session().is_some() => None,
            NewKind::Sweep => Some(format!(
                "no audio I/O to sweep: {}",
                open_session_hint(keymap)
            )),
            _ => Some(format!(
                "no audio I/O to measure: {}",
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

    /// The math channel form with its operator on align (Alt+M), A a transfer curve, the
    /// band prefilled from the curves drawn now; or why it cannot open.
    pub(super) fn alignment_form(&self) -> Result<Form, String> {
        let owner = self
            .selected_group()
            .unwrap_or(ac2_proto::model::TraceOwner::Imported);
        let candidates = self.math_candidates(Some(owner));
        let transfer = |o: &Operand| {
            candidates
                .iter()
                .any(|c| c.operand == *o && c.domain == ac2_proto::model::MathDomain::Transfer)
        };
        let first = self
            .selected_trace_meta()
            .map(|t| Operand::Trace { trace: t.id })
            .or_else(|| {
                self.kind_meas(PaneKind::Transfer)
                    .filter(|m| m.config.kind.is_job())
                    .map(|m| Operand::Meas { meas: m.id })
            })
            .filter(transfer)
            .or_else(|| {
                candidates
                    .iter()
                    .find(|c| c.domain == ac2_proto::model::MathDomain::Transfer)
                    .map(|c| c.operand)
            });
        let mut f = first
            .and_then(|_| Form::math(candidates.clone(), first, owner))
            .filter(|_| candidates.iter().filter(|c| transfer(&c.operand)).count() >= 2)
            .ok_or_else(|| {
                format!(
                    "an alignment needs two transfer curves, live or stored: make a transfer \
                     measurement or capture one ({}) first",
                    self.capture_keys()
                )
            })?;
        f.set_levels(self.transfer_levels());
        f.choose_align();
        Ok(f)
    }

    /// Every transfer curve's magnitude as drawn now, live and stored: what an alignment's
    /// band is prefilled from.
    pub(super) fn transfer_levels(&self) -> Vec<crate::math_dialog::Level> {
        use ac2_scene::grid::columns;
        let mut v = Vec::new();
        if let Some(d) = self.data.as_ref() {
            for m in self.measurements() {
                let Some(tf) = d.latest.get(&Topic::Data {
                    meas: m.id,
                    stream: Stream::Tf,
                }) else {
                    continue;
                };
                let (ac2_proto::FrameData::Tf(f), Some(g)) = (
                    &tf.frame.data,
                    tf.frame.stamp.grid_id.and_then(|g| d.grids.get(&g)),
                ) else {
                    continue;
                };
                v.push(crate::math_dialog::Level {
                    operand: Operand::Meas { meas: m.id },
                    cols: columns(g),
                    db: f.mag.clone(),
                });
            }
        }
        for (t, g) in self.traces.values() {
            v.push(crate::math_dialog::Level {
                operand: Operand::Trace { trace: t.meta.id },
                cols: columns(g),
                db: t.mag_db.clone(),
            });
        }
        v
    }
}

/// The command that opens kind `k`'s form; `None` for the meter, whose own key (P) needs
/// a picked Inputs row the chooser has not got: its form opens from the chooser alone.
fn kind_command(k: NewKind) -> Option<CommandId> {
    Some(match k {
        NewKind::Transfer => CommandId::NewTransfer,
        NewKind::Spectrum => CommandId::NewSpectrum,
        NewKind::Rta => CommandId::NewRta,
        NewKind::Spl => CommandId::NewSpl,
        NewKind::Sweep => CommandId::SweepNew,
        NewKind::Math => CommandId::NewMath,
        NewKind::Alignment => CommandId::NewAlignment,
        NewKind::Programme => return None,
    })
}
