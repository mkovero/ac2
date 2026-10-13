//! UI preferences kept between runs (`ui.toml` in the ac2 config directory): the stimulus
//! outputs last used on each output device (decision K4), the session dialog's choices
//! per device — which inputs and outputs were in the session, their roles and the mic
//! names — the Leq view's layout, whether the panes show their key hints, how long the SPL
//! meter's number holds a reading, the theme, the record toggle's time limit, the
//! spectrograph's history span, the layout and window as last left: the pane tree, the
//! focused pane, maximised or full screen, what each pane shows (its kind, measurement,
//! modes and how much of its plot: grid, labels, cursor, and its own level axis range: a fit
//! made for one show is a fair start for the next), the window's size and position, the
//! impulse-response views' dB axes, where the transfer pane's legend sat and how large it may be, and the session files
//! this app opened or exported (the Sessions dialog's recents).
//!
//! ```toml
//! key_hints = false
//! spl_hold_ms = 250
//! theme = "light"
//! record_limit_min = 90
//! spectrograph_span_s = 30
//!
//! [stimulus_outputs]
//! "hw:UMC1820" = [1, 2]
//!
//! [sessions."coreaudio/hw:UMC1820"]
//! inputs = [1, 2, 3]
//! outputs = 2
//! reference = 1
//! mics = [2, 3]
//! stimulus = [1]
//! mic_names = { 2 = "M30 FOH", 3 = "ECM8000" }
//!
//! [leq]
//! style = "tiles"
//! history = true
//!
//! [layout]
//! focus = 2
//! maximized = true
//! fullscreen = true
//! distortion_unit = "percent"
//! hidden = ["TF 2"]
//! compared = ["Main R"]
//!
//! [layout.tree]
//! split = "row"
//! ratio = 0.5
//! a = { pane = 1 }
//! b = { pane = 2 }
//!
//! [[layout.panes]]
//! id = 1
//! kind = "transfer"
//! measurement = "Main L"
//!
//! [layout.panes.levels]
//! transfer = [-24.0, 12.0]
//!
//! [[layout.panes]]
//! id = 2
//! kind = "spl"
//! measurement = "FOH SPL"
//! spl_view = "meter_leq"
//! chrome = "no_grid"
//!
//! [ir_levels]
//! transfer = [-80.0, 3.0]
//! sweep = [-90.0, 0.0]
//!
//! [legend.transfer]
//! x = 1.0
//! y = 0.0
//! max_height = 0.3
//!
//! [window]
//! width = 1600
//! height = 900
//! x = 80
//! y = 40
//!
//! [[recent_sessions]]
//! path = "/run/media/foh/STICK/friday.ac2session"
//! opened_at = 1789500000000000000
//! ```
//!
//! Channels are one-based in the file, as everywhere the operator reads or types them.
//! Measurements are remembered by name: ids do not outlive the daemon's state. The file is
//! UI state, not configuration: one that cannot be read is reported once and replaced by
//! the next save, and a write is atomic (`ac2_paths::write_private_atomic`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ac2_scene::axis::Range;
use ac2_scene::legend::LegendView;
use ac2_scene::view::{
    DistortionUnit, IrMode, LeqLayout, LeqStyle, PlotChrome, SpectrumMode, SplMode, SweepMode,
    ViewState, level,
};
use serde::{Deserialize, Serialize};

use crate::state::{Axis, PaneId, PaneKind, PaneLevels, PaneModes, PaneNode, TransferView};

mod file;
mod layout;
#[cfg(test)]
mod tests;

/// The session dialog's choices for one device (zero-based channels).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRoles {
    /// Inputs in the session.
    pub inputs: Vec<u16>,
    /// Output channels of the session (a count: outputs `0 .. outputs`).
    pub outputs: u16,
    /// The loopback return.
    pub reference: Option<u16>,
    /// Measurement mics.
    pub mics: Vec<u16>,
    /// Stimulus outputs.
    pub stimulus: Vec<u16>,
    /// Mic name per input.
    pub mic_names: BTreeMap<u16, String>,
    /// The device the session plays on when it is not the input device (its id); `None`:
    /// the input device's own outputs, or the system default where it has none.
    pub output_device: Option<String>,
}

/// The layout as the operator left it.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutPrefs {
    /// The pane tree and what each pane shows; `None`: none remembered, the app starts with
    /// one pane.
    pub panes: Option<PanesPrefs>,
    /// Only the focused pane (W).
    pub maximized: bool,
    /// The window fills the screen (F or F11; with `maximized`, the full-screen pane).
    pub fullscreen: bool,
    /// With `fullscreen`: the panes alone, no top bar or lists (F's second step).
    pub panes_only: bool,
    pub distortion_unit: DistortionUnit,
    /// Measurements whose live curves are hidden (A), by name.
    pub hidden: BTreeSet<String>,
    /// Measurements whose live curves every transfer pane draws besides its own group
    /// (compare, C), by name.
    pub compared: BTreeSet<String>,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        Self {
            panes: None,
            maximized: false,
            fullscreen: false,
            panes_only: false,
            distortion_unit: DistortionUnit::Db,
            hidden: BTreeSet::new(),
            compared: BTreeSet::new(),
        }
    }
}

/// The pane tree as left.
#[derive(Clone, Debug, PartialEq)]
pub struct PanesPrefs {
    pub root: PaneNode,
    pub focus: PaneId,
    /// One per leaf.
    pub views: Vec<PanePrefs>,
}

/// What one pane showed.
#[derive(Clone, Debug, PartialEq)]
pub struct PanePrefs {
    pub id: PaneId,
    pub kind: PaneKind,
    /// The measurement it showed, by name.
    pub measurement: Option<String>,
    pub modes: PaneModes,
    /// Its own level axis ranges.
    pub levels: PaneLevels,
}

/// The impulse-response views' dB axes (log / ETC), dB re peak: one for the transfer
/// panes' IR views, one for the sweep panes'. The panes' level axes are each pane's own
/// ([`PanePrefs::levels`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IrLevelPrefs {
    pub transfer: Range,
    pub sweep: Range,
}

impl IrLevelPrefs {
    /// The ranges `view` shows.
    pub fn of(view: &ViewState) -> Self {
        Self {
            transfer: view.ir.axes.level_db,
            sweep: view.distortion.ir.level_db,
        }
    }

    /// Puts the ranges on `view`.
    pub fn apply(&self, view: &mut ViewState) {
        view.ir.axes.level_db = self.transfer;
        view.distortion.ir.level_db = self.sweep;
    }
}

impl Default for IrLevelPrefs {
    fn default() -> Self {
        Self::of(&ViewState::default())
    }
}

/// The window's size and position, logical points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowPrefs {
    pub width: u32,
    pub height: u32,
    /// Where its outer top-left was, when the system said (Wayland does not).
    pub pos: Option<(i32, i32)>,
}

impl WindowPrefs {
    /// Smallest window the app lays out.
    pub const MIN: (u32, u32) = (720, 480);
}

/// Bounds of the SPL meter's display hold, ms.
pub const SPL_HOLD_MS: std::ops::RangeInclusive<u32> = 100..=10_000;

/// What the UI remembers.
#[derive(Clone, Debug, PartialEq)]
pub struct UiPrefs {
    /// Zero-based stimulus outputs per output device id.
    pub outputs: BTreeMap<String, Vec<u16>>,
    /// Session dialog choices per `backend/device id` ([`UiPrefs::device_key`]).
    pub sessions: BTreeMap<String, DeviceRoles>,
    /// How the SPL pane lays the Leq windows out.
    pub leq: LeqLayout,
    /// The focused pane's line of its most used keys (on until the operator turns it off).
    pub key_hints: bool,
    /// Warnings and Leq limit alarms pop up in the corner (on until the operator turns it
    /// off); off, they go only to the notification log.
    pub warning_toasts: bool,
    /// The transfer and sweep panes shade the ranges a curve's analysis does not resolve
    /// at its grid (on until the operator turns it off).
    pub resolution_marker: bool,
    /// The Leq and band Leq bars glide to each new reading (on until the operator turns it
    /// off); off, they jump to it as received.
    pub bar_ease: bool,
    /// The live spectrum and RTA curves glide to each new frame (on until the operator
    /// turns it off); off, they jump to it as received.
    pub spec_ease: bool,
    /// How long the SPL meter's number holds a reading; `None`: by its time weighting
    /// (`ac2_scene::spl::display_period_s`).
    pub spl_hold_ms: Option<u32>,
    /// The layout as last left.
    pub layout: LayoutPrefs,
    /// The impulse-response dB axes as last left.
    pub ir_levels: IrLevelPrefs,
    /// The window as last left (`None`: never saved).
    pub window: Option<WindowPrefs>,
    /// The theme last chosen (`None`: never chosen; `ac2-ui --theme` overrides it for a run).
    pub theme: Option<ac2_scene::theme::ThemeName>,
    /// How long the record toggle records before it stops by itself, minutes (`None`: an
    /// hour, [`crate::state::RECORD_MAX_S`]).
    pub record_limit_min: Option<u32>,
    /// The spectrograph's history span, s (one of
    /// [`ac2_scene::view::SPECTROGRAPH_SPANS_S`]; `None`: its default).
    pub spectrograph_span_s: Option<u32>,
    /// The transfer pane's legend as last left.
    pub legend: LegendPrefs,
    /// Session files and folders on this machine the app opened or exported, most recent
    /// first, at most [`MAX_RECENT_SESSIONS`].
    pub recent_sessions: Vec<RecentSession>,
}

/// How many session files the recents keep.
pub const MAX_RECENT_SESSIONS: usize = 12;

/// A session file or folder on this machine the app opened or exported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentSession {
    pub path: String,
    /// When (this machine's wall clock, ns).
    pub opened_at: u64,
}

impl UiPrefs {
    /// `path` used now: first in the recents, once.
    pub fn note_recent_session(&mut self, path: &str, at: ac2_proto::units::WallNs) {
        self.recent_sessions.retain(|r| r.path != path);
        self.recent_sessions.insert(
            0,
            RecentSession {
                path: path.to_owned(),
                opened_at: at.0,
            },
        );
        self.recent_sessions.truncate(MAX_RECENT_SESSIONS);
    }
}

/// Bounds of the record toggle's limit, minutes: a whole day of every input of a large
/// interface would fill a disk unattended.
pub const RECORD_LIMIT_MIN: std::ops::RangeInclusive<u32> = 1..=480;

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            outputs: BTreeMap::new(),
            sessions: BTreeMap::new(),
            leq: LeqLayout::default(),
            key_hints: true,
            warning_toasts: true,
            resolution_marker: true,
            bar_ease: true,
            spec_ease: true,
            spl_hold_ms: None,
            layout: LayoutPrefs::default(),
            ir_levels: IrLevelPrefs::default(),
            window: None,
            theme: None,
            record_limit_min: None,
            spectrograph_span_s: None,
            legend: LegendPrefs::default(),
            recent_sessions: Vec::new(),
        }
    }
}

/// A legend as last left: hidden or not, where it sat, how large it may be (the scroll and
/// the pointer are not kept).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegendPrefs {
    pub hidden: bool,
    pub x: f32,
    pub y: f32,
    pub max_width: f32,
    pub max_height: f32,
}

impl LegendPrefs {
    pub fn of(v: &LegendView) -> Self {
        Self {
            hidden: v.hidden,
            x: v.x,
            y: v.y,
            max_width: v.max_width,
            max_height: v.max_height,
        }
    }

    /// Puts the choices on `v`.
    pub fn apply(&self, v: &mut LegendView) {
        v.hidden = self.hidden;
        v.move_to(self.x, self.y);
        v.resize(self.max_width, self.max_height);
    }
}

impl Default for LegendPrefs {
    fn default() -> Self {
        Self::of(&LegendView::default())
    }
}
