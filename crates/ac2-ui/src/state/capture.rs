//! The slot keys: Ctrl+1…9 store every live curve the focused pane draws, each into its own
//! slot of that number (a capture's slots are its source curve's), and 1…9 show or hide the
//! pane's traces in a slot.

use ac2_proto::model::{SlotHolder, TraceMeta};
use ac2_scene::capture::{Outcome, nothing_of_pane, slot_empty, slot_toast, still_capturing};

use super::{AppState, Scope, text};
use crate::conn::{CaptureItem, Request};

impl AppState {
    /// Ctrl+N: every live curve the focused pane draws into slot `slot`, as `{name} S{slot}`.
    /// The curve's earlier capture in the slot is replaced unless it is locked (a locked
    /// one only gives up the slot), once the new one is stored.
    pub(super) fn capture_slot(&mut self, slot: u8, out: &mut Vec<Request>) {
        let bit = 1u16 << slot;
        if self.rig.capturing & bit != 0 {
            self.toast(still_capturing(slot));
            return;
        }
        let items: Vec<CaptureItem> = self
            .capture_sources()
            .iter()
            .map(|m| {
                let replace = self
                    .stored_traces()
                    .into_iter()
                    .find(|t| {
                        t.slot_holder() == SlotHolder::Curve(m.id) && t.edit.slot == Some(slot)
                    })
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
            self.rig.capturing |= bit;
            out.push(Request::Capture { slot, items });
        }
    }

    /// The answer to a [`Request::Capture`]: one toast naming what was stored and what was
    /// not.
    pub(super) fn captured(
        &mut self,
        slot: u8,
        results: Vec<(String, Result<Box<TraceMeta>, String>)>,
    ) {
        self.rig.capturing &= !(1u16 << slot);
        let rows: Vec<(&str, Outcome<'_>)> = results
            .iter()
            .map(|(name, r)| {
                let o = match r {
                    Ok(_) => Outcome::Captured,
                    Err(why) => Outcome::NotStored(why),
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
    /// Other panes' traces in the slot are left as they are, and so are the imported
    /// group's, which the tree shows and hides.
    pub(super) fn show_slot(&mut self, slot: u8, out: &mut Vec<Request>) {
        let mine: Vec<(ac2_proto::units::TraceId, bool)> = self
            .pane_slot(slot)
            .iter()
            .map(|t| (t.id, t.edit.visible))
            .collect();
        if mine.is_empty() {
            if self
                .stored_traces()
                .iter()
                .any(|t| t.edit.slot == Some(slot))
            {
                self.warn(nothing_of_pane(slot));
                return;
            }
            let key = text::CAPTURE_SLOT[usize::from(slot - 1)];
            let palette = self.keys.palette_entry(key.title());
            let k = self.keys.label(key, Scope::Global);
            self.warn(slot_empty(slot, k.as_deref(), &palette));
            return;
        }
        let show = mine.iter().all(|(_, visible)| !visible);
        for (id, visible) in mine {
            if visible != show {
                self.toggle_shown(id, out);
            }
        }
    }
}
