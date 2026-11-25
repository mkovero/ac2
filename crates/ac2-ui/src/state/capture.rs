//! The slot keys: Ctrl+1…9 store every live curve the focused pane draws, each into its own
//! slot of that number (slots are per measurement), and 1…9 show or hide the pane's traces
//! in a slot.

use ac2_proto::model::{MeasKind, Measurement, TraceMeta, TraceOwner};
use ac2_scene::capture::{Outcome, slot_empty, slot_toast};

use super::{AppState, Scope, text};
use crate::conn::{CaptureItem, CaptureRefusal, Request};

/// The owner a capture of `m` is filed under, whose slot it takes: a math trace's is the
/// one its configuration names, every other measurement's is itself.
fn capture_owner(m: &Measurement) -> TraceOwner {
    match &m.config.kind {
        MeasKind::Math { config } => config.owner,
        _ => TraceOwner::Meas { meas: m.id },
    }
}

impl AppState {
    /// Ctrl+N: every live curve the focused pane draws into slot `slot`, as `{name} S{slot}`.
    /// A trace of the same owner in the slot is replaced unless it is locked; a locked one
    /// only gives up the slot.
    pub(super) fn capture_slot(&mut self, slot: u8, out: &mut Vec<Request>) {
        let items: Vec<CaptureItem> = self
            .capture_sources()
            .iter()
            .map(|m| {
                let owner = capture_owner(m);
                let replace = self
                    .stored_traces()
                    .into_iter()
                    .find(|t| t.edit.owner == owner && t.edit.slot == Some(slot))
                    .filter(|t| !t.edit.locked)
                    .map(|t| t.id);
                CaptureItem {
                    meas: m.id,
                    label: m.config.name.clone(),
                    name: format!("{} S{slot}", m.config.name),
                    replace,
                }
            })
            .collect();
        if !items.is_empty() {
            out.push(Request::Capture { slot, items });
        }
    }

    /// The answer to a [`Request::Capture`]: one toast naming what was stored and what was
    /// not.
    pub(super) fn captured(
        &mut self,
        slot: u8,
        results: Vec<(String, Result<Box<TraceMeta>, CaptureRefusal>)>,
    ) {
        let rows: Vec<(&str, Outcome<'_>)> = results
            .iter()
            .map(|(name, r)| {
                let o = match r {
                    Ok(_) => Outcome::Captured,
                    Err(CaptureRefusal::NoResult) => Outcome::NoResult,
                    Err(CaptureRefusal::Failed(e)) => Outcome::Failed(e),
                };
                (name.as_str(), o)
            })
            .collect();
        let msg = slot_toast(slot, &rows);
        if results.iter().any(|(_, r)| r.is_ok()) {
            self.toast(msg);
        } else {
            self.warn(msg);
        }
    }

    /// The stored traces in slot `slot` that the focused pane draws when shown: its group's
    /// and those compared behind its lead.
    fn pane_slot(&self, slot: u8) -> Vec<&TraceMeta> {
        let f = self.layout.focus;
        let kind = self.layout.kind(f);
        let shown = self.pane_meas(f);
        self.stored_traces()
            .into_iter()
            .filter(|t| t.edit.slot == Some(slot))
            .filter(|t| {
                let mut s = (*t).clone();
                s.edit.visible = true;
                self.on_pane(kind, &s, shown) || self.trace_compared_on(kind, &s, f)
            })
            .collect()
    }

    /// 1…9: shows the focused pane's traces in slot `slot`, or hides them when any is shown.
    /// With none of the pane's in the slot, every trace in it (one filed under Imported,
    /// which no pane draws, is reached the same way).
    pub(super) fn show_slot(&mut self, slot: u8, out: &mut Vec<Request>) {
        let mut in_slot: Vec<&TraceMeta> = self.pane_slot(slot);
        if in_slot.is_empty() {
            in_slot = self
                .stored_traces()
                .into_iter()
                .filter(|t| t.edit.slot == Some(slot))
                .collect();
        }
        let in_slot: Vec<(ac2_proto::units::TraceId, bool)> =
            in_slot.iter().map(|t| (t.id, t.edit.visible)).collect();
        if in_slot.is_empty() {
            let key = text::CAPTURE_SLOT[usize::from(slot - 1)];
            let palette = self.keys.palette_entry(key.title());
            let k = self.keys.label(key, Scope::Global);
            self.warn(slot_empty(slot, k.as_deref(), &palette));
            return;
        }
        let show = in_slot.iter().all(|(_, visible)| !visible);
        for (id, visible) in in_slot {
            if visible != show {
                self.toggle_shown(id, out);
            }
        }
    }
}
