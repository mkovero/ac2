//! The one binding table: every command, its scopes and its default keys, plus user
//! overrides from `keys.toml`.
//!
//! Rules (PLAN §8.2), enforced by the tests at the bottom:
//! - A binding belongs to a [`Scope`]. The active scope is the focused pane's; a key is looked
//!   up in that scope first, then in `Global`. Within `Global ∪ scope` every chord is unique.
//! - No defaults on `[ ] + - =` or other keys that need AltGr or a dead key on Nordic and
//!   other European layouts.
//! - A command is only bound in scopes where it means something ([`CommandId::scopes`]); the
//!   reducer handles every command, so no key is dead.
//! - The stimulus cluster is reserved: `Space` arm, `Enter` fire, `Esc` stop, `↑/↓` level.
//!   Overrides may not give those chords another meaning. With a window (overlay) open the
//!   window owns the keyboard: its arrows, Enter and Esc never reach the stimulus.
//! - [`STOP_ANYWHERE`] (`Shift+Esc`) stops the stimulus from anywhere, windows included; it
//!   is fixed, so no override can turn a key a dialog types into a stop.

use serde::Deserialize;

mod chord;
mod keymap;
mod keywords;
mod table;
#[cfg(test)]
mod tests;

pub use chord::{Chord, LabelStyle, label_style, layout_unsafe, set_label_style};
pub use keymap::{Keymap, config_path};
pub use table::{Binding, Hint, RESERVED, STOP_ANYWHERE, defaults, hints};

/// Where a binding applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    Transfer,
    Spectrum,
    Ir,
    Spl,
    Distortion,
}

impl Scope {
    pub const ALL: [Scope; 6] = [
        Scope::Global,
        Scope::Transfer,
        Scope::Spectrum,
        Scope::Ir,
        Scope::Spl,
        Scope::Distortion,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Transfer => "transfer",
            Scope::Spectrum => "spectrum",
            Scope::Ir => "ir",
            Scope::Spl => "spl",
            Scope::Distortion => "distortion",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Scope::Global => "Everywhere",
            Scope::Transfer => "Transfer function",
            Scope::Spectrum => "Spectrum / RTA",
            Scope::Ir => "Transfer: impulse response view",
            Scope::Spl => "SPL",
            Scope::Distortion => "Sweep / distortion",
        }
    }

    fn parse(s: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|x| x.name() == s)
    }
}

macro_rules! commands {
    ($($id:ident => $name:literal, $title:literal, [$($scope:ident),*];)*) => {
        /// Every command the UI has; the palette lists all of them.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum CommandId { $($id,)* }

        impl CommandId {
            pub const ALL: &'static [CommandId] = &[$(CommandId::$id,)*];

            /// Name in `keys.toml`.
            pub fn name(self) -> &'static str {
                match self { $(CommandId::$id => $name,)* }
            }

            /// What the palette and the help overlay say.
            pub fn title(self) -> &'static str {
                match self { $(CommandId::$id => $title,)* }
            }

            /// Scopes where a binding of this command means something.
            pub fn scopes(self) -> &'static [Scope] {
                match self { $(CommandId::$id => &[$(Scope::$scope),*],)* }
            }

            pub fn from_name(s: &str) -> Option<CommandId> {
                Self::ALL.iter().copied().find(|c| c.name() == s)
            }
        }
    };
}

commands! {
    Help => "help", "Key bindings", [Global];
    Notifications => "notifications", "Notifications…", [Global];
    Palette => "palette", "Command palette", [Global];
    Settings => "settings", "Settings…", [Global];
    Quit => "quit", "Quit", [Global];
    Fullscreen => "fullscreen", "Full screen", [Global];
    KeyHints => "key_hints", "Key hints", [Global];
    WarningToasts => "warning_toasts", "Warning toasts", [Global];

    StimulusArm => "stimulus_arm", "Arm stimulus", [Global];
    StimulusFire => "stimulus_fire", "Fire stimulus", [Global];
    StimulusStop => "stimulus_stop", "Stop stimulus", [Global];
    StopAnywhere => "stimulus_stop_anywhere", "Stop stimulus anywhere", [Global];
    LevelUp => "level_up", "Stimulus level +1 dB", [Global];
    LevelDown => "level_down", "Stimulus level −1 dB", [Global];
    LevelUpCoarse => "level_up_coarse", "Stimulus level +3 dB", [Global];
    LevelDownCoarse => "level_down_coarse", "Stimulus level −3 dB", [Global];
    StimulusLevel => "stimulus_level", "Set stimulus level…", [Global];
    StimulusOutputs => "stimulus_outputs", "Stimulus outputs…", [Global];
    StimulusTakeOver => "stimulus_take_over", "Take over stimulus", [Global];

    FocusPane1 => "focus_pane_1", "Focus pane 1", [Global];
    FocusPane2 => "focus_pane_2", "Focus pane 2", [Global];
    FocusPane3 => "focus_pane_3", "Focus pane 3", [Global];
    FocusPane4 => "focus_pane_4", "Focus pane 4", [Global];
    FocusPane5 => "focus_pane_5", "Focus pane 5", [Global];
    FocusPane6 => "focus_pane_6", "Focus pane 6", [Global];
    FocusPane7 => "focus_pane_7", "Focus pane 7", [Global];
    FocusPane8 => "focus_pane_8", "Focus pane 8", [Global];
    FocusPane9 => "focus_pane_9", "Focus pane 9", [Global];
    SplitPane => "split_pane", "Split pane", [Global];
    ClosePane => "close_pane", "Close pane", [Global];
    TurnSplit => "turn_split", "Turn split", [Global];
    GrowPane => "grow_pane", "Grow pane", [Global];
    ShrinkPane => "shrink_pane", "Shrink pane", [Global];
    TransferStack => "transfer_stack", "Transfer stack", [Global];
    MaximisePane => "maximise_pane", "Maximise pane", [Global];
    Undo => "undo", "Undo pane change", [Global];
    Redo => "redo", "Redo pane change", [Global];
    NextInTree => "next_in_tree", "Next list row", [Global];
    PrevInTree => "prev_in_tree", "Previous list row", [Global];
    PaneMeasurement => "pane_measurement", "Choose pane measurement…", [Global];
    CycleTheme => "cycle_theme", "Theme", [Global];
    PlotChrome => "plot_chrome", "Plot chrome", [Global];
    ZoomIn => "zoom_in", "Zoom in", [Global];
    ZoomOut => "zoom_out", "Zoom out", [Global];
    PanLeft => "pan_left", "Pan left", [Global];
    PanRight => "pan_right", "Pan right", [Global];
    ResetView => "reset_view", "Reset zoom", [Global];
    LevelZoomIn => "level_zoom_in", "Zoom level in", [Global];
    LevelZoomOut => "level_zoom_out", "Zoom level out", [Global];
    LevelPanUp => "level_pan_up", "Pan level up", [Global];
    LevelPanDown => "level_pan_down", "Pan level down", [Global];
    LevelFit => "level_fit", "Fit level", [Global];
    LevelReset => "level_reset", "Reset axes", [Global];
    ToggleCursor => "toggle_cursor", "Comparison cursor", [Global];
    Compare => "compare", "Compare…", [Global];
    ClearCompare => "clear_compare", "Clear compare", [Global];
    CursorLeft => "cursor_left", "Cursor left", [Global];
    CursorRight => "cursor_right", "Cursor right", [Global];
    Slot1 => "slot_1", "Capture to slot 1", [Global];
    Slot2 => "slot_2", "Capture to slot 2", [Global];
    Slot3 => "slot_3", "Capture to slot 3", [Global];
    Slot4 => "slot_4", "Capture to slot 4", [Global];
    Slot5 => "slot_5", "Capture to slot 5", [Global];
    Slot6 => "slot_6", "Capture to slot 6", [Global];
    Slot7 => "slot_7", "Capture to slot 7", [Global];
    Slot8 => "slot_8", "Capture to slot 8", [Global];
    Slot9 => "slot_9", "Capture to slot 9", [Global];
    ShowSlot1 => "show_slot_1", "Show/hide slot 1", [Global];
    ShowSlot2 => "show_slot_2", "Show/hide slot 2", [Global];
    ShowSlot3 => "show_slot_3", "Show/hide slot 3", [Global];
    ShowSlot4 => "show_slot_4", "Show/hide slot 4", [Global];
    ShowSlot5 => "show_slot_5", "Show/hide slot 5", [Global];
    ShowSlot6 => "show_slot_6", "Show/hide slot 6", [Global];
    ShowSlot7 => "show_slot_7", "Show/hide slot 7", [Global];
    ShowSlot8 => "show_slot_8", "Show/hide slot 8", [Global];
    ShowSlot9 => "show_slot_9", "Show/hide slot 9", [Global];
    NextTrace => "next_trace", "Next trace", [Global];
    PrevTrace => "prev_trace", "Previous trace", [Global];
    NextAnyTrace => "next_any_trace", "Next trace incl. hidden", [Global];
    PrevAnyTrace => "prev_any_trace", "Previous trace incl. hidden", [Global];
    ToggleSelected => "toggle_selected", "Show/hide curve", [Global];
    TraceSlot => "trace_slot", "Move trace to slot…", [Global];
    TraceRename => "trace_rename", "Rename trace…", [Global];
    TraceExport => "trace_export", "Export trace…", [Global];
    ExportPanePng => "pane_export_png", "Export pane as PNG…", [Global];
    SelectLive => "select_live", "Select live", [Global];
    DeleteSelected => "delete_selected", "Delete selected…", [Global];
    OffsetUp => "offset_up", "Offset +1 dB", [Global];
    OffsetDown => "offset_down", "Offset −1 dB", [Global];
    OffsetUpCoarse => "offset_up_coarse", "Offset +3 dB", [Global];
    OffsetDownCoarse => "offset_down_coarse", "Offset −3 dB", [Global];
    OffsetClear => "offset_clear", "Clear offset", [Global];
    ImportTrace => "import_trace", "Import trace…", [Global];
    Sessions => "sessions", "Sessions…", [Global];
    SessionNew => "session_new", "New session…", [Global];
    SessionLoad => "session_load", "Open session…", [Global];
    SessionSave => "session_save", "Save session", [Global];
    SessionSaveAs => "session_save_as", "Save session as…", [Global];
    SessionExport => "session_export", "Export session…", [Global];
    Reconnect => "reconnect", "Reconnect", [Global];
    OpenSession => "session_open", "Open audio I/O…", [Global];
    CloseSession => "session_close", "Close audio I/O", [Global];
    Record => "record", "Record", [Global];
    ReplayRecording => "replay_recording", "Replay recording…", [Global];
    NewMeasurement => "meas_new", "New measurement…", [Global];
    NewTransfer => "meas_new_transfer", "New transfer…", [Global];
    NewSpectrum => "meas_new_spectrum", "New spectrum…", [Global];
    NewRta => "meas_new_rta", "New RTA…", [Global];
    NewSpl => "meas_new_spl", "New SPL meter…", [Global];
    NewMath => "meas_new_math", "New math…", [Global];
    NewAlignment => "meas_new_alignment", "New alignment…", [Global];
    EditMeas => "meas_edit", "Edit measurement…", [Global];
    NextColour => "meas_colour_next", "Next colour", [Global];
    PrevColour => "meas_colour_prev", "Previous colour", [Global];
    HideGroup => "hide_group", "Show/hide measurement", [Global];
    MoveTrace => "move_trace", "Move trace…", [Global];
    ToggleGroup => "toggle_group", "Fold/unfold measurement", [Global];
    InputSetup => "input_setup", "Input setup…", [Global];
    Calibrations => "calibrations", "Calibrations…", [Global];
    InputMics => "input_mics", "Set input mics…", [Global];
    MicCurve => "mic_curve", "Next mic curve", [Global];
    MicCurveInput => "mic_curve_input", "Set input mic curve…", [Global];
    CalDelete => "cal_delete", "Delete calibration…", [Global];
    TraceMicCurve => "trace_mic_curve", "Set trace mic curve…", [Global];
    SweepNew => "sweep_new", "New sweep…", [Global];
    LeqWindows => "leq_windows", "Leq settings…", [Global];
    SplFast => "spl_fast", "Fast time weighting", [Global];
    SplSlow => "spl_slow", "Slow time weighting", [Global];
    SplImpulse => "spl_impulse", "Impulse time weighting", [Global];
    SplA => "spl_a", "A weighting", [Global];
    SplC => "spl_c", "C weighting", [Global];
    SplZ => "spl_z", "Z weighting", [Global];

    ResetAverage => "reset_average", "Reset averaging", [Transfer, Spectrum, Spl];
    StartStop => "start_stop", "Start/stop", [Transfer, Spectrum, Ir, Spl];

    InsertDelay => "insert_delay", "Find first arrival", [Transfer];
    InsertStrongest => "insert_strongest", "Find strongest peak", [Transfer];
    TypeDelay => "type_delay", "Set delay…", [Transfer];
    TrackDelay => "track_delay", "Delay tracking", [Transfer];
    DelayDown => "delay_down", "Delay −1 sample", [Transfer];
    DelayUp => "delay_up", "Delay +1 sample", [Transfer];
    DelayDownFine => "delay_down_fine", "Delay −0.1 sample", [Transfer];
    DelayUpFine => "delay_up_fine", "Delay +0.1 sample", [Transfer];
    FinderAuto => "finder_auto", "Finder band auto", [Transfer];
    FinderFull => "finder_full", "Finder band full", [Transfer];
    FinderMid => "finder_mid", "Finder band mid", [Transfer];
    FinderSub => "finder_sub", "Finder band sub", [Transfer];
    FinderCustom => "finder_custom", "Finder band custom…", [Transfer];
    FinderObservation => "finder_observation", "Finder observation…", [Transfer];
    Invert => "invert", "Invert polarity", [Transfer];
    Offset => "offset", "Set offset…", [Transfer, Spectrum];
    NudgeEarlier => "nudge_earlier", "Delay −0.1 ms", [Transfer];
    NudgeLater => "nudge_later", "Delay +0.1 ms", [Transfer];
    PhaseReference => "phase_reference", "Set phase reference", [Transfer];
    Target => "target", "Import target…", [Transfer];
    CoherenceMask => "coherence_mask", "Coherence mask", [Transfer];
    CoherencePlacement => "coherence_placement", "Coherence placement", [Transfer];
    LegendTopLeft => "legend_top_left", "Legend top-left", [Transfer];
    LegendTopRight => "legend_top_right", "Legend top-right", [Transfer];
    LegendBottomLeft => "legend_bottom_left", "Legend bottom-left", [Transfer];
    LegendBottomRight => "legend_bottom_right", "Legend bottom-right", [Transfer];
    LegendHide => "legend_hide", "Show/hide legend", [Transfer];
    Average => "average", "Average power", [Transfer];
    AverageComplex => "average_complex", "Average complex", [Transfer];
    AverageCoherence => "average_coherence", "Average coherence-weighted", [Transfer];
    MinMax => "min_max", "Min/max tracking", [Transfer];
    PhaseView => "phase_view", "Phase view", [Transfer];
    SmoothCoarser => "smooth_coarser", "Smoothing coarser", [Transfer, Spectrum];
    SmoothFiner => "smooth_finer", "Smoothing finer", [Transfer, Spectrum];
    SmoothOff => "smooth_off", "Smoothing off", [Transfer, Spectrum];
    Smooth48 => "smooth_48", "Smoothing 1/48 oct", [Transfer, Spectrum];
    Smooth24 => "smooth_24", "Smoothing 1/24 oct", [Transfer, Spectrum];
    Smooth12 => "smooth_12", "Smoothing 1/12 oct", [Transfer, Spectrum];
    Smooth6 => "smooth_6", "Smoothing 1/6 oct", [Transfer, Spectrum];
    Smooth3 => "smooth_3", "Smoothing 1/3 oct", [Transfer, Spectrum];

    SpectrumStyle => "spectrum_style", "Spectrum style", [Spectrum];
    PeakHold => "peak_hold", "Peaks", [Spectrum];
    SpectrographSpan => "spectrograph_span", "Spectrograph span", [Spectrum];
    SpectrographColours => "spectrograph_colours", "Next colour map", [Spectrum];
    SpectrographViridis => "spectrograph_colours_viridis", "Colour map viridis", [Spectrum];
    SpectrographMagma => "spectrograph_colours_magma", "Colour map magma", [Spectrum];
    SpectrographInferno => "spectrograph_colours_inferno", "Colour map inferno", [Spectrum];
    SpectrographCividis => "spectrograph_colours_cividis", "Colour map cividis", [Spectrum];
    SpectrographGrey => "spectrograph_colours_grey", "Colour map grey", [Spectrum];

    NextView => "next_view", "Next view", [Transfer, Spectrum, Ir, Spl, Distortion];
    PrevView => "prev_view", "Previous view", [Transfer, Spectrum, Ir, Spl, Distortion];

    SplShowMeter => "spl_show_meter", "Meter view", [Spl];
    SplShowLeq => "spl_show_leq", "Leq view", [Spl];
    SplShowMeterLeq => "spl_show_meter_leq", "Meter + Leq view", [Spl];
    SplShowBands => "spl_show_bands", "Band Leq view", [Spl];
    SplLeqStyle => "spl_leq_style", "Leq style", [Spl];
    SplLeqHistory => "spl_leq_history", "Leq history", [Spl];
    SplNewLog => "spl_new_log", "New SPL log…", [Spl];
    SplLogExport => "spl_log_export", "Export Leq log…", [Spl];
    SplLogExportPrevious => "spl_log_export_previous", "Export previous Leq log…", [Spl];
    SplTimeWeighting => "spl_time_weighting", "Time weighting", [Spl];
    SplWeighting => "spl_weighting", "Frequency weighting", [Spl];

    DistortionUnit => "distortion_unit", "Distortion unit", [Distortion];
    SweepIr => "sweep_ir", "Sweep IR view", [Distortion];
    ReferenceHarmonics => "reference_harmonics", "Reference harmonics", [Distortion];
}
