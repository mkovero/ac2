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
    Help => "help", "Show / hide key bindings", [Global];
    Notifications => "notifications", "Recent notifications… (the messages that went by in the corner)", [Global];
    Palette => "palette", "Command palette", [Global];
    Settings => "settings", "Settings: inputs & outputs, audio, calibration, Leq, recording, display, connection…", [Global];
    Quit => "quit", "Quit", [Global];
    Fullscreen => "fullscreen", "Full screen: window → panes only → off", [Global];
    KeyHints => "key_hints", "Key hints on / off", [Global];
    WarningToasts => "warning_toasts", "Warning toasts on / off (off: warnings and Leq limit alarms go only to the notification log)", [Global];

    StimulusArm => "stimulus_arm", "Stimulus: arm what the view plays (sweep view: a run of the selected sweep measurement; others: the generator)", [Global];
    StimulusFire => "stimulus_fire", "Stimulus: fire what is armed (named in the top bar)", [Global];
    StimulusStop => "stimulus_stop", "Stimulus: stop and disarm (no window open)", [Global];
    StopAnywhere => "stimulus_stop_anywhere", "Stimulus: stop and disarm, also with a window open", [Global];
    LevelUp => "level_up", "Stimulus level +1 dB", [Global];
    LevelDown => "level_down", "Stimulus level −1 dB", [Global];
    LevelUpCoarse => "level_up_coarse", "Stimulus level +3 dB", [Global];
    LevelDownCoarse => "level_down_coarse", "Stimulus level −3 dB", [Global];
    StimulusLevel => "stimulus_level", "Stimulus: type level (dBFS)…", [Global];
    StimulusOutputs => "stimulus_outputs", "Stimulus outputs: tick them in Settings › Audio & I/O…", [Global];
    StimulusTakeOver => "stimulus_take_over", "Stimulus: take over the lease from another client and arm", [Global];

    FocusPane1 => "focus_pane_1", "Focus pane 1 (panes count in reading order)", [Global];
    FocusPane2 => "focus_pane_2", "Focus pane 2", [Global];
    FocusPane3 => "focus_pane_3", "Focus pane 3", [Global];
    FocusPane4 => "focus_pane_4", "Focus pane 4", [Global];
    FocusPane5 => "focus_pane_5", "Focus pane 5", [Global];
    FocusPane6 => "focus_pane_6", "Focus pane 6", [Global];
    FocusPane7 => "focus_pane_7", "Focus pane 7", [Global];
    FocusPane8 => "focus_pane_8", "Focus pane 8", [Global];
    FocusPane9 => "focus_pane_9", "Focus pane 9", [Global];
    SplitPane => "split_pane", "Split the focused pane in two (along its longer side)", [Global];
    ClosePane => "close_pane", "Close the focused pane", [Global];
    TurnSplit => "turn_split", "Turn the focused pane's split (side by side ↔ stacked)", [Global];
    GrowPane => "grow_pane", "Grow the focused pane (within its split)", [Global];
    ShrinkPane => "shrink_pane", "Shrink the focused pane (within its split)", [Global];
    TransferStack => "transfer_stack", "Transfer stack: magnitude / phase / coherence (in place of the focused pane)", [Global];
    MaximizePane => "maximize_pane", "Layout: split → one pane → full screen", [Global];
    Undo => "undo", "Undo the last change to the panes (split, close, drop, pick, view, size, W / F, compare)", [Global];
    Redo => "redo", "Redo the pane change last undone", [Global];
    NextInTree => "next_in_tree", "Select the next row in the measurement list (a measurement row brings its panes, as a click does)", [Global];
    PrevInTree => "prev_in_tree", "Select the previous row in the measurement list (a measurement row brings its panes, as a click does)", [Global];
    PaneMeasurement => "pane_measurement", "Choose the measurement the focused pane shows…", [Global];
    CycleTheme => "cycle_theme", "Theme: dark → light → high contrast", [Global];
    PlotChrome => "plot_chrome", "Plot of the focused pane: grid, labels and cursor → no grid or marks → traces only (SPL: the limit text goes with the grid, the scale with the labels)", [Global];
    ZoomIn => "zoom_in", "Zoom frequency in (IR: time)", [Global];
    ZoomOut => "zoom_out", "Zoom frequency out (IR: time)", [Global];
    PanLeft => "pan_left", "Pan frequency down (IR: earlier)", [Global];
    PanRight => "pan_right", "Pan frequency up (IR: later)", [Global];
    ResetView => "reset_view", "Reset zoom (20 Hz – 20 kHz; IR: the whole IR)", [Global];
    LevelZoomIn => "level_zoom_in", "Zoom level axis in (vertical; IR: amplitude or dB)", [Global];
    LevelZoomOut => "level_zoom_out", "Zoom level axis out (vertical; IR: amplitude or dB)", [Global];
    LevelPanUp => "level_pan_up", "Pan level axis up (towards higher levels)", [Global];
    LevelPanDown => "level_pan_down", "Pan level axis down (towards lower levels)", [Global];
    LevelFit => "level_fit", "Fit level axis to the shown curves, frequency to 20 Hz – 20 kHz (IR: the whole IR)", [Global];
    LevelReset => "level_reset", "Level and frequency axes back to the pane's default", [Global];
    ToggleCursor => "toggle_cursor", "Comparison cursor on / off (IR: a time cursor)", [Global];
    Compare => "compare", "Compare the selected measurement or trace over the panes that draw its kind (on / off)", [Global];
    ClearCompare => "clear_compare", "Clear compare: no compared curves", [Global];
    CursorLeft => "cursor_left", "Cursor 1/12 octave down (IR: earlier)", [Global];
    CursorRight => "cursor_right", "Cursor 1/12 octave up (IR: later)", [Global];
    Slot1 => "slot_1", "Capture selected measurement to slot 1", [Global];
    Slot2 => "slot_2", "Capture selected measurement to slot 2", [Global];
    Slot3 => "slot_3", "Capture selected measurement to slot 3", [Global];
    Slot4 => "slot_4", "Capture selected measurement to slot 4", [Global];
    Slot5 => "slot_5", "Capture selected measurement to slot 5", [Global];
    Slot6 => "slot_6", "Capture selected measurement to slot 6", [Global];
    Slot7 => "slot_7", "Capture selected measurement to slot 7", [Global];
    Slot8 => "slot_8", "Capture selected measurement to slot 8", [Global];
    Slot9 => "slot_9", "Capture selected measurement to slot 9", [Global];
    ShowSlot1 => "show_slot_1", "Show / hide slot 1", [Global];
    ShowSlot2 => "show_slot_2", "Show / hide slot 2", [Global];
    ShowSlot3 => "show_slot_3", "Show / hide slot 3", [Global];
    ShowSlot4 => "show_slot_4", "Show / hide slot 4", [Global];
    ShowSlot5 => "show_slot_5", "Show / hide slot 5", [Global];
    ShowSlot6 => "show_slot_6", "Show / hide slot 6", [Global];
    ShowSlot7 => "show_slot_7", "Show / hide slot 7", [Global];
    ShowSlot8 => "show_slot_8", "Show / hide slot 8", [Global];
    ShowSlot9 => "show_slot_9", "Show / hide slot 9", [Global];
    NextTrace => "next_trace", "Select next shown stored trace (then live)", [Global];
    PrevTrace => "prev_trace", "Select previous shown stored trace (then live)", [Global];
    NextAnyTrace => "next_any_trace", "Select next trace incl. hidden (then live)", [Global];
    PrevAnyTrace => "prev_any_trace", "Select previous trace incl. hidden (then live)", [Global];
    ToggleSelected => "toggle_selected", "Show / hide the selected curve", [Global];
    TraceSlot => "trace_slot", "Move the selected trace to slot… (1 … 9, none frees its slot)", [Global];
    TraceRename => "trace_rename", "Rename the selected trace…", [Global];
    TraceExport => "trace_export", "Export the selected trace (CSV or IR WAV) to a file…", [Global];
    SelectLive => "select_live", "Deselect the stored trace: keys act on the live measurement again", [Global];
    DeleteSelected => "delete_selected", "Delete selected measurement or trace…", [Global];
    OffsetUp => "offset_up", "Display offset +1 dB of the selected curve", [Global];
    OffsetDown => "offset_down", "Display offset −1 dB of the selected curve", [Global];
    OffsetUpCoarse => "offset_up_coarse", "Display offset +3 dB of the selected curve", [Global];
    OffsetDownCoarse => "offset_down_coarse", "Display offset −3 dB of the selected curve", [Global];
    OffsetClear => "offset_clear", "Display offset of the selected curve back to 0", [Global];
    ImportTrace => "import_trace", "Import a trace file (CSV, analyzer text, IR WAV)…", [Global];
    Sessions => "sessions", "Sessions: new, open, save, recent sessions…", [Global];
    SessionNew => "session_new", "New session (asks about unsaved changes), then the audio settings", [Global];
    SessionLoad => "session_load", "Sessions: open a file or folder on this computer, disarmed…", [Global];
    SessionSave => "session_save", "Save session (where it came from; untitled: Save as)", [Global];
    SessionSaveAs => "session_save_as", "Save session as: a new session on the daemon…", [Global];
    SessionExport => "session_export", "Export session: an .ac2session file on this computer…", [Global];
    Reconnect => "reconnect", "Reconnect to the daemon now", [Global];
    OpenSession => "session_open", "Open audio session: Settings › Audio & I/O…", [Global];
    CloseSession => "session_close", "Close audio session", [Global];
    Record => "record", "Record: raw audio of every input on / off (stops by itself at the limit in Settings › Recording)", [Global];
    ReplayRecording => "replay_recording", "Replay a recording as the session (name or path)…", [Global];
    NewMeasurement => "meas_new", "New measurement…", [Global];
    NewTransfer => "meas_new_transfer", "New transfer measurement…", [Global];
    NewSpectrum => "meas_new_spectrum", "New spectrum…", [Global];
    NewRta => "meas_new_rta", "New RTA…", [Global];
    NewSpl => "meas_new_spl", "New SPL meter…", [Global];
    NewMath => "meas_new_math", "New math channel: A ÷ × + − B, or the average of several (mic positions)…", [Global];
    EditMeas => "meas_edit", "Edit measurement…", [Global];
    NextColour => "meas_colour_next", "Next colour for the selected measurement (one no other measurement has; grey last)", [Global];
    PrevColour => "meas_colour_prev", "Previous colour for the selected measurement", [Global];
    HideGroup => "hide_group", "Show / hide the selected measurement with every trace under it", [Global];
    MoveTrace => "move_trace", "Move the selected trace or math channel to another measurement (or Imported)…", [Global];
    ToggleGroup => "toggle_group", "Fold / unfold the selected measurement in the list", [Global];
    InputSetup => "input_setup", "Input setup: Settings › Audio & I/O (names, roles, mics, max level)…", [Global];
    Calibrations => "calibrations", "Calibrations: Settings › Calibration (mics, curves, sensitivities)…", [Global];
    InputMics => "input_mics", "Input setup: type mic names (3=M30, 4=ECM)…", [Global];
    MicCurve => "mic_curve", "Mic curve: next curve on the selected measurement's input (off → 0° → 90° …)", [Global];
    MicCurveInput => "mic_curve_input", "Mic curve on input N… (e.g. 2=90°, 2=off)", [Global];
    CalDelete => "cal_delete", "Calibration: delete a sensitivity calibration (input=mic)…", [Global];
    TraceMicCurve => "trace_mic_curve", "Mic curve on the selected trace (e.g. MM1 34804 90°; none removes)…", [Global];
    SweepNew => "sweep_new", "New sweep measurement: response and harmonic distortion…", [Global];
    LeqWindows => "leq_windows", "Leq windows and limits of the SPL meter: Settings › SPL / Leq…", [Global];
    SplFast => "spl_fast", "SPL meter: Fast time weighting (125 ms)", [Global];
    SplSlow => "spl_slow", "SPL meter: Slow time weighting (1 s)", [Global];
    SplImpulse => "spl_impulse", "SPL meter: Impulse time weighting (35 ms / 1.5 s)", [Global];
    SplA => "spl_a", "SPL meter: A weighting", [Global];
    SplC => "spl_c", "SPL meter: C weighting", [Global];
    SplZ => "spl_z", "SPL meter: Z weighting (flat)", [Global];

    ResetAverage => "reset_average", "Reset averaging of selected measurement", [Transfer, Spectrum, Spl];
    StartStop => "start_stop", "Start / stop selected measurement", [Transfer, Spectrum, Ir, Spl];

    InsertDelay => "insert_delay", "Delay: find and insert first arrival", [Transfer];
    InsertStrongest => "insert_strongest", "Delay: find and insert strongest peak", [Transfer];
    TypeDelay => "type_delay", "Delay: type value (ms)…", [Transfer];
    TrackDelay => "track_delay", "Delay tracking on / off", [Transfer];
    DelayDown => "delay_down", "Delay of the measurement −1 sample (the curve moves at once)", [Transfer];
    DelayUp => "delay_up", "Delay of the measurement +1 sample (the curve moves at once)", [Transfer];
    DelayDownFine => "delay_down_fine", "Delay of the measurement −0.1 sample", [Transfer];
    DelayUpFine => "delay_up_fine", "Delay of the measurement +0.1 sample", [Transfer];
    FinderAuto => "finder_auto", "Delay finder: auto band (full → mid → sub)", [Transfer];
    FinderFull => "finder_full", "Delay finder: full band (2–16 kHz)", [Transfer];
    FinderMid => "finder_mid", "Delay finder: mid band (300 Hz – 3 kHz)", [Transfer];
    FinderSub => "finder_sub", "Delay finder: sub band (20–120 Hz)", [Transfer];
    FinderCustom => "finder_custom", "Delay finder: custom band (Hz)…", [Transfer];
    FinderObservation => "finder_observation", "Delay finder: observation length (s)…", [Transfer];
    Invert => "invert", "Invert polarity of selected trace (display)", [Transfer];
    Offset => "offset", "Type dB offset of selected trace…", [Transfer, Spectrum];
    NudgeEarlier => "nudge_earlier", "Delay −0.1 ms of the measurement, or of the selected stored trace", [Transfer];
    NudgeLater => "nudge_later", "Delay +0.1 ms of the measurement, or of the selected stored trace", [Transfer];
    PhaseReference => "phase_reference", "Make selected trace the phase reference", [Transfer];
    Target => "target", "Load a target curve file…", [Transfer];
    CoherenceMask => "coherence_mask", "Coherence mask: off → 0.3 → 0.5 → 0.7 → 0.9", [Transfer];
    CoherencePlacement => "coherence_placement", "Coherence: its own view / over the magnitude", [Transfer];
    LegendTopLeft => "legend_top_left", "Legend: top-left corner (drag it anywhere; its corner grip resizes)", [Transfer];
    LegendTopRight => "legend_top_right", "Legend: top-right corner", [Transfer];
    LegendBottomLeft => "legend_bottom_left", "Legend: bottom-left corner", [Transfer];
    LegendBottomRight => "legend_bottom_right", "Legend: bottom-right corner", [Transfer];
    LegendHide => "legend_hide", "Legend: hide / show", [Transfer];
    Average => "average", "Average shown stored traces (power)", [Transfer];
    AverageComplex => "average_complex", "Average shown stored traces (complex)", [Transfer];
    AverageCoherence => "average_coherence", "Average shown stored traces (coherence-weighted)", [Transfer];
    PhaseUnwrap => "phase_unwrap", "Phase wrapped / unwrapped", [Transfer];
    SmoothCoarser => "smooth_coarser", "Smoothing coarser (selected trace or pane's measurement)", [Transfer, Spectrum];
    SmoothFiner => "smooth_finer", "Smoothing finer (selected trace or pane's measurement)", [Transfer, Spectrum];
    SmoothOff => "smooth_off", "Smoothing: off", [Transfer, Spectrum];
    Smooth48 => "smooth_48", "Smoothing: 1/48 oct", [Transfer, Spectrum];
    Smooth24 => "smooth_24", "Smoothing: 1/24 oct", [Transfer, Spectrum];
    Smooth12 => "smooth_12", "Smoothing: 1/12 oct", [Transfer, Spectrum];
    Smooth6 => "smooth_6", "Smoothing: 1/6 oct", [Transfer, Spectrum];
    Smooth3 => "smooth_3", "Smoothing: 1/3 oct", [Transfer, Spectrum];
    GroupDelay => "group_delay", "Phase / group delay", [Transfer];

    SpectrumStyle => "spectrum_style", "Spectrum: line / fill / bars", [Spectrum];
    PeakHold => "peak_hold", "Peak hold on / off", [Spectrum];
    SpectrographSpan => "spectrograph_span", "Spectrograph history: 10 → 30 → 60 → 120 s", [Spectrum];
    SpectrographColours => "spectrograph_colours", "Spectrograph colours: the next colour map", [Spectrum];
    SpectrographViridis => "spectrograph_colours_viridis", "Spectrograph colours: viridis (blue, green, yellow)", [Spectrum];
    SpectrographMagma => "spectrograph_colours_magma", "Spectrograph colours: magma (black, purple, orange; dark)", [Spectrum];
    SpectrographInferno => "spectrograph_colours_inferno", "Spectrograph colours: inferno (black, purple, red, yellow; dark)", [Spectrum];
    SpectrographCividis => "spectrograph_colours_cividis", "Spectrograph colours: cividis (dark blue to yellow)", [Spectrum];
    SpectrographGrey => "spectrograph_colours_grey", "Spectrograph colours: grey (black to light grey)", [Spectrum];

    NextView => "next_view", "Next view", [Transfer, Spectrum, Ir, Spl, Distortion];
    PrevView => "prev_view", "Previous view", [Transfer, Spectrum, Ir, Spl, Distortion];

    SplShowMeter => "spl_show_meter", "SPL pane: the meter", [Spl];
    SplShowLeq => "spl_show_leq", "SPL pane: the Leq windows", [Spl];
    SplShowMeterLeq => "spl_show_meter_leq", "SPL pane: meter + Leq windows", [Spl];
    SplShowBands => "spl_show_bands", "SPL pane: the band Leq bars (20 Hz … 10 kHz)", [Spl];
    SplLeqStyle => "spl_leq_style", "SPL Leq windows: columns / tiles", [Spl];
    SplLeqHistory => "spl_leq_history", "SPL Leq windows: history strip on / off", [Spl];
    SplNewLog => "spl_new_log", "Start a new SPL log…", [Spl];
    SplLogExport => "spl_log_export", "Export Leq log (CSV, on this computer)…", [Spl];
    SplLogExportPrevious => "spl_log_export_previous", "Export the previous Leq log (CSV, on this computer)…", [Spl];
    SplTimeWeighting => "spl_time_weighting", "SPL meter: time weighting Fast → Slow → Impulse", [Spl];
    SplWeighting => "spl_weighting", "SPL meter: frequency weighting A → C → Z", [Spl];

    DistortionUnit => "distortion_unit", "Distortion in dB re fundamental / percent", [Distortion];
    SweepIr => "sweep_ir", "Sweep pane: impulse response (again: response & distortion)", [Distortion];
    ReferenceHarmonics => "reference_harmonics", "Distortion: the reference input's own harmonics shown / hidden", [Distortion];
}
