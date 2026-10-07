//! Navigation of the impulse-response pictures (the IR pane, the sweep pane's IR view): the
//! same keys and mouse gestures as the frequency panes, on a time axis. I / O and the wheel
//! zoom time, ←/→ and a drag pan it, Ctrl+I / Ctrl+O, Ctrl+↑/↓ and Ctrl / Shift+wheel move
//! the amplitude (linear) or dB (log, ETC) axis, Home shows the whole IR, Shift+Home frames
//! the curve, Ctrl+Home goes back to the defaults; C, Shift+←/→ and a click place the
//! cursor. Navigation only: the IR is drawn as received.

use std::borrow::Cow;

use ac2_proto::FrameData;
use ac2_proto::frame::IrFrame;
use ac2_proto::topic::Stream;
use ac2_scene::axis::Range;
use ac2_scene::format;
use ac2_scene::view::{IR_AMPLITUDE_BOUNDS, IrAxes, IrExtent, IrMode, IrPane, SweepMode, level};

use super::{AppState, LEVEL_ZOOM_FACTOR, PaneKind, ZOOM_FACTOR};
use crate::keys::CommandId;

/// What the mouse does on an IR picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IrNavMsg {
    /// The wheel: time zooms by `factor` (> 1 in) about `about_ms`.
    Zoom { about_ms: f64, factor: f64 },
    /// A drag: time pans by `ms` (positive shows later times).
    Pan { ms: f64 },
    /// Ctrl+wheel: the value axis zooms by `factor` about `about` (FS or dB).
    ValueZoom { about: Option<f64>, factor: f64 },
    /// Shift+wheel: the value axis pans by `by` (FS or dB; positive shows higher values).
    ValuePan { by: f64 },
    /// A click: the cursor at `t_ms`.
    Cursor { t_ms: f64 },
}

/// The navigation commands an IR picture takes over from the frequency panes.
pub(super) fn is_nav(c: CommandId) -> bool {
    use CommandId as C;
    matches!(
        c,
        C::ZoomIn
            | C::ZoomOut
            | C::PanLeft
            | C::PanRight
            | C::ResetView
            | C::LevelZoomIn
            | C::LevelZoomOut
            | C::LevelPanUp
            | C::LevelPanDown
            | C::LevelFit
            | C::LevelReset
            | C::ToggleCursor
            | C::CursorLeft
            | C::CursorRight
    )
}

impl AppState {
    /// The IR picture the focused pane shows, if it shows one: the IR pane always (with or
    /// without an IR), the sweep pane in its IR view while a sweep with an IR is shown
    /// (without one it draws the distortion view, whose keys stay).
    pub fn ir_target(&self) -> Option<IrPane> {
        match self.layout.focus {
            PaneKind::Ir => Some(IrPane::Live),
            PaneKind::Distortion
                if self.view.distortion.mode == SweepMode::Ir
                    && self.shown_sweep().is_some_and(|(d, _)| d.sweep.is_some()) =>
            {
                Some(IrPane::Sweep)
            }
            _ => None,
        }
    }

    /// The IR picture `p` draws, when it has one.
    pub fn ir_frame_of(&self, p: IrPane) -> Option<Cow<'_, IrFrame>> {
        match p {
            IrPane::Live => {
                let m = crate::scenes::focus_tf(self)?;
                if self.meas_hidden(m) {
                    return None;
                }
                match &crate::scenes::frame(self, m.id, Stream::Ir)?.frame.data {
                    FrameData::Ir(f) => Some(Cow::Borrowed(f)),
                    _ => None,
                }
            }
            IrPane::Sweep => {
                let (d, _) = self.shown_sweep()?;
                ac2_scene::distortion::ir_frame(d).map(Cow::Owned)
            }
        }
    }

    /// Where the IR of picture `p` lies in time, when it has one.
    pub fn ir_extent(&self, p: IrPane) -> Option<IrExtent> {
        match p {
            IrPane::Live => self
                .ir_frame_of(p)
                .map(|f| ac2_scene::ir::extent(f.as_ref())),
            IrPane::Sweep => {
                let s = self.shown_sweep()?.0.sweep.as_ref()?;
                Some(IrExtent::of(s.ir.t0.0, s.ir.dt.0, s.ir.linear.len()))
            }
        }
    }

    /// The time and value ranges picture `p` shows now, with its IR's extent.
    fn ir_shown(&self, p: IrPane) -> Option<(IrExtent, Range, Range)> {
        let f = self.ir_frame_of(p)?;
        let axes = self.view.ir_axes(p);
        Some((
            ac2_scene::ir::extent(&f),
            ac2_scene::ir::time_range(&f, axes),
            ac2_scene::ir::y_range(&f, self.view.ir.mode, axes),
        ))
    }

    fn ir_title(p: IrPane) -> &'static str {
        match p {
            IrPane::Live => "Impulse response",
            IrPane::Sweep => "Sweep impulse response",
        }
    }

    /// Time zoom of picture `p` by `factor` (> 1 in) about `about_ms`.
    fn ir_time_zoom(&mut self, p: IrPane, about_ms: Option<f64>, factor: f64) {
        let Some((e, t, _)) = self.ir_shown(p) else {
            return;
        };
        let about = about_ms.unwrap_or((t.lo + t.hi) / 2.0);
        self.view.ir_axes_mut(p).time_ms = Some(e.bounds().zoom(t, about, factor));
    }

    fn ir_time_pan(&mut self, p: IrPane, ms: f64) {
        let Some((e, t, _)) = self.ir_shown(p) else {
            return;
        };
        self.view.ir_axes_mut(p).time_ms = Some(e.bounds().pan(t, ms));
    }

    /// The value axis of picture `p` in the shown mode: amplitude (linear) or dB re peak.
    fn ir_value_set(&mut self, p: IrPane, f: impl Fn(Range) -> Range) {
        let Some((_, _, y)) = self.ir_shown(p) else {
            return;
        };
        let mode = self.view.ir.mode;
        let axes = self.view.ir_axes_mut(p);
        match mode {
            IrMode::Linear => axes.amplitude = Some(f(y)),
            IrMode::Log | IrMode::Etc => axes.level_db = f(y),
        }
    }

    fn ir_value_zoom(&mut self, p: IrPane, about: Option<f64>, factor: f64) {
        let linear = self.view.ir.mode == IrMode::Linear;
        self.ir_value_set(p, |y| {
            let a = about.unwrap_or((y.lo + y.hi) / 2.0);
            if linear {
                IR_AMPLITUDE_BOUNDS.zoom(y, a, factor)
            } else {
                level::zoom(y, a, factor)
            }
        });
    }

    fn ir_value_pan(&mut self, p: IrPane, by: f64) {
        let linear = self.view.ir.mode == IrMode::Linear;
        self.ir_value_set(p, |y| {
            if linear {
                IR_AMPLITUDE_BOUNDS.pan(y, by)
            } else {
                level::pan(y, by)
            }
        });
    }

    /// The cursor of picture `p` at `t_ms`, on the IR's nearest sample.
    fn ir_cursor_at(&mut self, p: IrPane, t_ms: f64) {
        if let Some(t) = self.ir_extent(p).and_then(|e| e.snap(t_ms)) {
            self.view.ir_axes_mut(p).cursor_ms = Some(t);
        }
    }

    /// The mouse on IR picture `p`.
    pub(super) fn ir_nav(&mut self, p: IrPane, m: IrNavMsg) {
        match m {
            IrNavMsg::Zoom { about_ms, factor } => self.ir_time_zoom(p, Some(about_ms), factor),
            IrNavMsg::Pan { ms } => self.ir_time_pan(p, ms),
            IrNavMsg::ValueZoom { about, factor } => self.ir_value_zoom(p, about, factor),
            IrNavMsg::ValuePan { by } => self.ir_value_pan(p, by),
            IrNavMsg::Cursor { t_ms } => self.ir_cursor_at(p, t_ms),
        }
    }

    /// A navigation key ([`is_nav`]) on the focused IR picture `p`.
    pub(super) fn ir_key(&mut self, p: IrPane, c: CommandId) {
        use CommandId as C;
        let Some((e, t, y)) = self.ir_shown(p) else {
            self.warn(format!("{}: no impulse response shown", Self::ir_title(p)));
            return;
        };
        let cursor = self.view.ir_axes(p).cursor_ms;
        match c {
            C::ZoomIn | C::ZoomOut => {
                // About the cursor while it is in view, as the frequency axis zooms.
                let about = cursor.filter(|x| *x > t.lo && *x < t.hi);
                let f = if c == C::ZoomIn {
                    ZOOM_FACTOR
                } else {
                    1.0 / ZOOM_FACTOR
                };
                self.ir_time_zoom(p, about, f);
            }
            C::PanLeft => self.ir_time_pan(p, -ac2_scene::view::pan_step(t)),
            C::PanRight => self.ir_time_pan(p, ac2_scene::view::pan_step(t)),
            C::ResetView => self.view.ir_axes_mut(p).time_ms = None,
            C::LevelZoomIn => self.ir_value_zoom(p, None, LEVEL_ZOOM_FACTOR),
            C::LevelZoomOut => self.ir_value_zoom(p, None, 1.0 / LEVEL_ZOOM_FACTOR),
            C::LevelPanUp => self.ir_value_pan(p, ac2_scene::view::pan_step(y)),
            C::LevelPanDown => self.ir_value_pan(p, -ac2_scene::view::pan_step(y)),
            C::LevelReset => {
                let axes = self.view.ir_axes_mut(p);
                *axes = IrAxes {
                    cursor_ms: axes.cursor_ms,
                    ..IrAxes::default()
                };
            }
            C::LevelFit => self.ir_fit(p),
            C::ToggleCursor => {
                let axes = self.view.ir_axes_mut(p);
                axes.cursor_ms = match axes.cursor_ms {
                    Some(_) => None,
                    None => Some((t.lo + t.hi) / 2.0),
                };
                if let Some(x) = self.view.ir_axes(p).cursor_ms {
                    self.ir_cursor_at(p, x);
                }
            }
            C::CursorLeft | C::CursorRight => {
                let k = if c == C::CursorLeft { -1.0 } else { 1.0 };
                let from = cursor.unwrap_or((t.lo + t.hi) / 2.0);
                let to = (from + k * e.cursor_step(t)).clamp(t.lo, t.hi);
                self.ir_cursor_at(p, to);
            }
            _ => {}
        }
    }

    /// Shift+Home: the whole IR, its value axis framing the curve: the linear view
    /// symmetric about its peak, the log and ETC views from the noise to the peak.
    fn ir_fit(&mut self, p: IrPane) {
        let mode = self.view.ir.mode;
        let fit = match mode {
            IrMode::Linear => None,
            IrMode::Log | IrMode::Etc => {
                let Some(f) = self.ir_frame_of(p) else {
                    return;
                };
                match ac2_scene::ir::ir_values(&f, mode).and_then(level::fit) {
                    Some(r) => Some(r),
                    None => {
                        self.warn(format!("{}: no curve shown to fit", Self::ir_title(p)));
                        return;
                    }
                }
            }
        };
        let axes = self.view.ir_axes_mut(p);
        axes.time_ms = None;
        match fit {
            None => axes.amplitude = None,
            Some(r) => {
                axes.level_db = r;
                self.toast(format!(
                    "{}: level {} … {} dB",
                    Self::ir_title(p),
                    format::fixed(r.lo, 0),
                    format::fixed(r.hi, 0)
                ));
            }
        }
    }
}
