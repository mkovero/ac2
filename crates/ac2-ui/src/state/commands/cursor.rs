//! The comparison cursor's jumps to peaks and notches, and its mark.

use ac2_scene::axis::Range;
use ac2_scene::cursor_jump::{self, Landmark, Step};

use crate::state::*;

impl AppState {
    /// The jump and mark [`CommandId`]s [`AppState::command`] routes here.
    pub(in crate::state) fn command_cursor(&mut self, c: CommandId) {
        use CommandId as C;
        match c {
            C::NextPeak => self.cursor_jump(Landmark::Peak, Step::Up),
            C::PrevPeak => self.cursor_jump(Landmark::Peak, Step::Down),
            C::NextNotch => self.cursor_jump(Landmark::Notch, Step::Up),
            C::PrevNotch => self.cursor_jump(Landmark::Notch, Step::Down),
            // On the cursor; a cursor that is off comes on in the middle of the view first,
            // as Shift+T puts it.
            C::CursorMark => {
                let t = self.nav.target;
                let hz = *self.view.cursor_hz.get_or_insert((t.lo * t.hi).sqrt());
                self.view.cursor_mark_hz = Some(hz);
            }
            C::ClearMark => self.view.cursor_mark_hz = None,
            _ => {}
        }
    }

    /// The pane a jump reads: the focused frequency pane, else the transfer pane worked in
    /// last, else the spectrum pane. A transfer pane counts while it shows the magnitude:
    /// its peaks and notches are what a jump goes to.
    fn jump_pane(&self) -> Option<(PaneId, PaneKind)> {
        let ok = |id: PaneId| match self.layout.kind(id) {
            PaneKind::Transfer => (self.pane_modes(id).transfer == TransferView::Magnitude)
                .then_some((id, PaneKind::Transfer)),
            PaneKind::Spectrum => Some((id, PaneKind::Spectrum)),
            _ => None,
        };
        ok(self.layout.focus)
            .or_else(|| self.layout.lead(PaneKind::Transfer).and_then(ok))
            .or_else(|| self.layout.lead(PaneKind::Spectrum).and_then(ok))
    }

    /// Puts the cursor on the next `kind` that way of the curve the pane draws inside the
    /// frequency range it is going to, standing out by the prominence setting.
    fn cursor_jump(&mut self, kind: Landmark, step: Step) {
        let Some((pane, pk)) = self.jump_pane() else {
            self.warn(cursor_jump::NO_PANE);
            return;
        };
        let t = self.nav.target;
        let shown = Range::new(t.lo, t.hi);
        let to = match pk {
            PaneKind::Spectrum => crate::scenes::spectrum_jump(self, pane, shown, kind, step),
            _ => crate::scenes::transfer_jump(self, pane, shown, kind, step),
        };
        match to {
            Some(hz) => {
                self.view.cursor_hz = Some(hz);
                // The jump is to a frequency: the spectrograph's time goes with the old spot.
                self.view.spectrum.spectrograph.cursor_s = None;
            }
            None => self.toast(cursor_jump::none_text(
                kind,
                step,
                self.view.jump_prominence_db,
            )),
        }
    }
}
