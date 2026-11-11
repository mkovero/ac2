//! The 3D spectrograph's camera: keys and the mouse orbit, tilt and dolly it around the
//! skyline. The camera stored with the pane (and sent with the measurement's layout) is
//! whole degrees and zoom steps; what is drawn eases towards it, or follows a drag, which
//! is stored only on release so a drag sends one layout, not one per frame.

use ac2_proto::model::layout::SpectrumCamera;
use ac2_scene::spectrograph_3d::{self, Pose, ZOOM_KEY_STEPS};

use super::{AppState, PaneId};
use crate::anim::Spring;
use crate::keys::CommandId;

/// What the mouse does on a 3D spectrograph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cam3dMsg {
    /// A drag moved by `dx`, `dy` logical pixels (right, down): orbit and tilt.
    Drag { dx: f64, dy: f64 },
    /// The drag ended: the camera snaps to whole degrees and is stored.
    Release,
    /// Wheel notches or a pinch: `steps` zoom steps (positive in).
    Dolly { steps: i32 },
    /// The pointer came over the view (`true`) or left it: the axes show while it is there.
    Hover(bool),
    /// A touch or click on the view: the axes show for a few seconds (a kiosk's touch
    /// screen has no hover).
    Touch,
}

/// The camera drawn while it moves: springs towards the stored camera, or the pointer's
/// pose during a drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamNav {
    pane: PaneId,
    yaw: Spring,
    pitch: Spring,
    zoom: Spring,
    drag: Option<Pose>,
}

/// Settled within this many degrees (and zoom steps).
const SETTLE: f64 = 0.02;

impl CamNav {
    fn at(pane: PaneId, p: Pose) -> Self {
        Self {
            pane,
            yaw: Spring::at(p.yaw_deg),
            pitch: Spring::at(p.pitch_deg),
            zoom: Spring::at(p.zoom_step),
            drag: None,
        }
    }

    fn pose(&self) -> Pose {
        self.drag.unwrap_or(Pose {
            yaw_deg: self.yaw.pos,
            pitch_deg: self.pitch.pos,
            zoom_step: self.zoom.pos,
        })
    }

    fn settled(&self, to: Pose) -> bool {
        self.drag.is_none()
            && self.yaw.settled(to.yaw_deg, SETTLE)
            && self.pitch.settled(to.pitch_deg, SETTLE)
            && self.zoom.settled(to.zoom_step, SETTLE)
    }
}

/// The commands the 3D view takes over: the frequency axis's navigation keys move the
/// camera, and the cursor and compare keys do nothing (there is no cursor and no compared
/// curve in 3D).
pub(super) fn is_nav(c: CommandId) -> bool {
    use CommandId as C;
    matches!(
        c,
        C::ZoomIn
            | C::ZoomOut
            | C::PanLeft
            | C::PanRight
            | C::ResetView
            | C::TiltUp
            | C::TiltDown
            | C::ToggleCursor
            | C::CursorLeft
            | C::CursorRight
            | C::Compare
    )
}

impl AppState {
    /// The focused pane, when it shows the 3D spectrograph.
    pub fn cam3d_target(&self) -> Option<PaneId> {
        self.layout
            .focused()
            .shows_3d(self.three_d())
            .then_some(self.layout.focus)
    }

    fn stored_camera(&self, pane: PaneId) -> SpectrumCamera {
        self.layout
            .view(pane)
            .map_or(SpectrumCamera::DEFAULT, |v| v.modes.spectrum_camera)
    }

    /// The camera pane `pane` draws now.
    pub fn drawn_camera(&self, pane: PaneId) -> Pose {
        match &self.pane_local.cam3d {
            Some(n) if n.pane == pane => n.pose(),
            _ => Pose::from(self.stored_camera(pane)),
        }
    }

    /// Stores `c` as pane `pane`'s camera, the drawn one easing to it from where it is.
    fn set_camera(&mut self, pane: PaneId, c: SpectrumCamera) {
        let from = self.drawn_camera(pane);
        if !matches!(&self.pane_local.cam3d, Some(n) if n.pane == pane) {
            self.pane_local.cam3d = Some(CamNav::at(pane, from));
        }
        if let Some(n) = &mut self.pane_local.cam3d {
            n.drag = None;
            n.yaw.pos = from.yaw_deg;
            n.pitch.pos = from.pitch_deg;
            n.zoom.pos = from.zoom_step;
        }
        if let Some(v) = self.layout.view_mut(pane) {
            v.modes.spectrum_camera = c;
        }
    }

    /// How much pane `pane`'s 3D axes show now, 0 … 1.
    pub fn axes3d_alpha(&self, pane: PaneId) -> f32 {
        self.pane_local
            .axes3d
            .get(&pane)
            .map_or(0.0, |r| r.alpha(self.now_s))
    }

    /// Whether the pointer is over pane `pane`'s 3D view, as last reported.
    pub fn axes3d_hovered(&self, pane: PaneId) -> bool {
        self.pane_local
            .axes3d
            .get(&pane)
            .is_some_and(|r| r.hovered())
    }

    /// When some 3D view's axes next change: `Some(0.0)` while one fades, `Some(wait)`
    /// while one holds before fading out, `None` when all rest.
    pub fn axes3d_wait(&self) -> Option<f64> {
        self.pane_local
            .axes3d
            .values()
            .filter_map(|r| r.next_change(self.now_s))
            .min_by(f64::total_cmp)
    }

    /// Something the operator did on pane `pane` (its camera, its level range): its 3D
    /// axes show for a while, so the change reads against them.
    pub(super) fn poke_axes3d(&mut self, pane: PaneId) {
        if self
            .layout
            .view(pane)
            .is_some_and(|v| v.shows_3d(self.three_d()))
        {
            let now = self.now_s;
            self.pane_local.axes3d.entry(pane).or_default().poke(now);
        }
    }

    pub(super) fn cam3d_key(&mut self, pane: PaneId, c: CommandId) {
        use CommandId as C;
        if is_nav(c) {
            self.poke_axes3d(pane);
        }
        let now = self.stored_camera(pane);
        let next = match c {
            C::PanLeft => spectrograph_3d::step(now, -1, 0, 0),
            C::PanRight => spectrograph_3d::step(now, 1, 0, 0),
            C::TiltUp => spectrograph_3d::step(now, 0, 1, 0),
            C::TiltDown => spectrograph_3d::step(now, 0, -1, 0),
            C::ZoomIn => spectrograph_3d::step(now, 0, 0, ZOOM_KEY_STEPS),
            C::ZoomOut => spectrograph_3d::step(now, 0, 0, -ZOOM_KEY_STEPS),
            C::ResetView => SpectrumCamera::DEFAULT,
            // No cursor and no compared curves in 3D.
            _ => return,
        };
        if next != now {
            self.set_camera(pane, next);
        }
    }

    pub(super) fn cam3d_msg(&mut self, pane: PaneId, m: Cam3dMsg) {
        if !self
            .layout
            .view(pane)
            .is_some_and(|v| v.shows_3d(self.three_d()))
        {
            return;
        }
        // Anything but the pointer's coming and going is an interaction: the axes hold.
        let now = self.now_s;
        let reveal = self.pane_local.axes3d.entry(pane).or_default();
        match m {
            Cam3dMsg::Hover(on) => reveal.hover(on, now),
            _ => reveal.poke(now),
        }
        match m {
            Cam3dMsg::Hover(_) | Cam3dMsg::Touch => {}
            Cam3dMsg::Drag { dx, dy } => {
                let from = self.drawn_camera(pane);
                let to = spectrograph_3d::drag(from, dx, dy);
                let n = self.pane_local.cam3d.get_or_insert(CamNav::at(pane, from));
                if n.pane != pane {
                    *n = CamNav::at(pane, from);
                }
                n.drag = Some(to);
            }
            Cam3dMsg::Release => {
                let Some(p) = self
                    .pane_local
                    .cam3d
                    .as_ref()
                    .filter(|n| n.pane == pane)
                    .and_then(|n| n.drag)
                else {
                    return;
                };
                if let Some(n) = &mut self.pane_local.cam3d {
                    *n = CamNav::at(pane, p);
                }
                let c = p.snapped();
                if let Some(v) = self.layout.view_mut(pane) {
                    v.modes.spectrum_camera = c;
                }
            }
            Cam3dMsg::Dolly { steps } => {
                let now = self.stored_camera(pane);
                let next = spectrograph_3d::step(now, 0, 0, steps);
                if next != now {
                    self.set_camera(pane, next);
                }
            }
        }
    }

    /// Moves the drawn camera `dt` seconds on; drops it once it rests on the stored one.
    pub(super) fn step_cam3d(&mut self, dt: f64) {
        let Some(mut n) = self.pane_local.cam3d else {
            return;
        };
        if !self
            .layout
            .view(n.pane)
            .is_some_and(|v| v.shows_3d(self.three_d()))
        {
            self.pane_local.cam3d = None;
            return;
        }
        if n.drag.is_some() {
            return;
        }
        let to = Pose::from(self.stored_camera(n.pane));
        n.yaw.step(to.yaw_deg, dt);
        n.pitch.step(to.pitch_deg, dt);
        n.zoom.step(to.zoom_step, dt);
        self.pane_local.cam3d = (!n.settled(to)).then_some(n);
    }
}
