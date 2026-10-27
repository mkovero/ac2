//! The programme meter dialog ([`crate::meter_dialog`]): opened by the New measurement
//! chooser's Programme meter row for a new pane, and by *Edit measurement…* on a focused
//! Programme pane for that pane; and its Enter.

use super::*;
use crate::meter_dialog::{ember_decay, ember_glow, levels, metered, wanted};

impl AppState {
    /// The session's pairs as the dialog offers them: each pair row of the Inputs list whose
    /// sides the session both captures, by the row's name.
    fn meter_pairs(&self) -> Vec<((u16, u16), String)> {
        self.session_inputs()
            .into_iter()
            .filter(|r| r.right.is_some())
            .filter_map(|r| Some((self.row_meters(r.channel)?.pair()?, r.label)))
            .collect()
    }

    /// The dialog for a new pane (`edit`: `None`; it starts from the picked Inputs row) or
    /// for pane `edit`'s meters; or why it cannot open.
    pub(super) fn meter_form(&self, edit: Option<PaneId>) -> Result<Form, String> {
        let inputs = self.session_input_names();
        if inputs.is_empty() {
            return Err(ac2_scene::programme::form::NO_INPUTS.to_owned());
        }
        let now = match edit {
            Some(p) => self.layout.view(p).and_then(|v| v.meters),
            None => self.rig.input_pick.and_then(|c| self.row_meters(c)),
        };
        let rig = self.daemon().map(|s| s.rig.levels).unwrap_or_default();
        Ok(crate::meter_dialog::form(
            self.meter_pairs(),
            inputs,
            now,
            rig,
            self.ember_cooling(),
            edit,
        ))
    }

    pub(super) fn open_meter_form(&mut self, edit: Option<PaneId>, out: &mut Vec<Request>) {
        match self.meter_form(edit) {
            Ok(f) => {
                self.overlay = Overlay::Form(Box::new(f));
                if self.rig.devices.is_none() {
                    // Channel names arrive with the device list.
                    out.push(Request::Devices);
                }
            }
            Err(why) => self.warn(why),
        }
    }

    /// Enter on the dialog: the ember decay and glow are this app's and set here; the rig's settings
    /// changed here go out as one `levels.set`, then
    /// the meters open in a new pane beside the focused one, or change in the edited pane.
    pub(super) fn submit_meter(&mut self, out: &mut Vec<Request>) {
        let Overlay::Form(f) = &mut self.overlay else {
            return;
        };
        let (m, shown) = match metered(f) {
            Ok(m) => (m, levels(f)),
            Err(e) => {
                f.error = Some(e);
                return;
            }
        };
        let Some((opened, edit)) = f.meter.as_ref().map(|x| (x.opened, x.edit)) else {
            return;
        };
        let (ember, glow) = (ember_decay(f), ember_glow(f));
        self.overlay = Overlay::None;
        // This app's, as on Settings › Display: one value, whichever page sets it.
        if let Some(e) = ember.filter(|e| *e != self.prefs.ember_decay_s) {
            self.prefs.ember_decay_s = e;
            self.prefs_dirty = true;
        }
        if let Some(g) = glow.filter(|g| *g != self.prefs.ember_glow) {
            self.prefs.ember_glow = g;
            self.prefs_dirty = true;
        }
        let rig = self.daemon().map(|s| s.rig.clone()).unwrap_or_default();
        if let Some((cmd, what)) = shown.and_then(|shown| {
            crate::settings::levels_set(rig.rev, rig.levels, wanted(opened, shown, rig.levels))
        }) {
            self.call(out, cmd, what);
        }
        match edit {
            Some(p) => self.pane_pick(p, PaneMenuRow::Meter(m)),
            None => self.open_programme(m),
        }
    }
}
