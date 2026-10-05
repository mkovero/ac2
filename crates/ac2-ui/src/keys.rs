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

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use eframe::egui::{Key, Modifiers};
use serde::Deserialize;

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
            Scope::Ir => "Impulse response",
            Scope::Spl => "SPL",
            Scope::Distortion => "Sweep / distortion",
        }
    }

    fn parse(s: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|x| x.name() == s)
    }
}

/// A key with modifiers. `command` is Ctrl on Linux / Windows and ⌘ on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub command: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Punctuation keys: which one arrives already depends on Shift on many layouts (`/` is
/// Shift+7 on Nordic keyboards), so Shift is ignored for them and never part of a chord.
fn is_symbol(k: Key) -> bool {
    matches!(
        k,
        Key::Colon
            | Key::Comma
            | Key::Backslash
            | Key::Slash
            | Key::Pipe
            | Key::Questionmark
            | Key::Exclamationmark
            | Key::OpenBracket
            | Key::CloseBracket
            | Key::OpenCurlyBracket
            | Key::CloseCurlyBracket
            | Key::Backtick
            | Key::Minus
            | Key::Period
            | Key::Plus
            | Key::Equals
            | Key::Semicolon
            | Key::Quote
    )
}

/// Keys never bound by default: unreachable without AltGr / Shift or dead on common
/// European layouts (Nordic `[ ] { } \ |` need AltGr; `+ - =` move around; `` ` `` and `´`
/// are dead keys).
pub fn layout_unsafe(k: Key) -> bool {
    matches!(
        k,
        Key::OpenBracket
            | Key::CloseBracket
            | Key::OpenCurlyBracket
            | Key::CloseCurlyBracket
            | Key::Plus
            | Key::Minus
            | Key::Equals
            | Key::Backtick
            | Key::Quote
            | Key::Backslash
            | Key::Pipe
            | Key::Colon
            | Key::Semicolon
    )
}

impl Chord {
    pub const fn key(key: Key) -> Self {
        Self {
            key,
            command: false,
            alt: false,
            shift: false,
        }
    }

    pub const fn shift(key: Key) -> Self {
        Self {
            shift: true,
            ..Self::key(key)
        }
    }

    pub const fn command(key: Key) -> Self {
        Self {
            command: true,
            ..Self::key(key)
        }
    }

    pub const fn alt(key: Key) -> Self {
        Self {
            alt: true,
            ..Self::key(key)
        }
    }

    /// The chord of a key event.
    pub fn from_event(key: Key, m: Modifiers) -> Self {
        Self {
            key,
            command: m.command,
            alt: m.alt,
            shift: m.shift && !is_symbol(key),
        }
    }

    /// Parses `Ctrl+K`, `Cmd+K`, `Shift+P`, `Alt+Left`, `Space`, `/`, `Up`, `F1`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut c = Chord::key(Key::Space);
        // macOS labels (`⌘⇧P`) parse too, so every label reads back as its chord.
        let mut rest = s.trim();
        loop {
            if let Some(r) = rest.strip_prefix('⌘') {
                c.command = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix('⌥') {
                c.alt = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix('⇧') {
                c.shift = true;
                rest = r;
            } else {
                break;
            }
        }
        let parts: Vec<&str> = rest.split('+').map(str::trim).collect();
        let Some((key, mods)) = parts.split_last() else {
            return Err(format!("empty key {s:?}"));
        };
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "command" => c.command = true,
                "alt" | "option" => c.alt = true,
                "shift" => c.shift = true,
                other => return Err(format!("unknown modifier {other:?} in {s:?}")),
            }
        }
        c.key = match key.to_ascii_lowercase().as_str() {
            "esc" | "escape" => Key::Escape,
            "enter" | "return" => Key::Enter,
            "space" => Key::Space,
            "up" | "↑" => Key::ArrowUp,
            "down" | "↓" => Key::ArrowDown,
            "left" | "←" => Key::ArrowLeft,
            "right" | "→" => Key::ArrowRight,
            "tab" => Key::Tab,
            _ => {
                let k = if key.len() == 1 {
                    key.to_ascii_uppercase()
                } else {
                    (*key).to_owned()
                };
                Key::from_name(&k).ok_or_else(|| format!("unknown key {key:?} in {s:?}"))?
            }
        };
        if c.shift && is_symbol(c.key) {
            return Err(format!(
                "{s:?}: Shift cannot be combined with a punctuation key (layouts differ)"
            ));
        }
        Ok(c)
    }

    /// How the chord is shown: `Ctrl+K` (`⌘K` on macOS), `Shift+P`, `Space`, `↑`.
    pub fn label(&self) -> String {
        self.label_in(label_style())
    }

    /// The label in an explicit style (help, palette and top bar use [`label_style`]).
    pub fn label_in(&self, style: LabelStyle) -> String {
        let key = match self.key {
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            Key::ArrowLeft => "←",
            Key::ArrowRight => "→",
            Key::Escape => "Esc",
            Key::Enter => "Enter",
            Key::Space => "Space",
            k if is_symbol(k) => k.symbol_or_name(),
            k => k.name(),
        };
        let mac = style == LabelStyle::Mac;
        let mut s = String::new();
        if self.command {
            s.push_str(if mac { "⌘" } else { "Ctrl+" });
        }
        if self.alt {
            s.push_str(if mac { "⌥" } else { "Alt+" });
        }
        if self.shift {
            s.push_str(if mac { "⇧" } else { "Shift+" });
        }
        s.push_str(key);
        s
    }
}

/// How chords are written on screen: macOS glyphs (`⌘⇧K`) or PC words (`Ctrl+Shift+K`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    /// `Ctrl+`, `Alt+`, `Shift+`.
    Pc,
    /// `⌘`, `⌥`, `⇧`.
    Mac,
}

impl LabelStyle {
    /// The host platform's convention.
    pub fn platform() -> Self {
        if cfg!(target_os = "macos") {
            LabelStyle::Mac
        } else {
            LabelStyle::Pc
        }
    }
}

/// 0 = platform default, 1 = PC, 2 = Mac.
static LABEL_STYLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Overrides the label style for the whole process (snapshot tests pin `Pc` so screenshots
/// match on every OS).
pub fn set_label_style(style: LabelStyle) {
    let v = match style {
        LabelStyle::Pc => 1,
        LabelStyle::Mac => 2,
    };
    LABEL_STYLE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// The label style in effect.
pub fn label_style() -> LabelStyle {
    match LABEL_STYLE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => LabelStyle::Pc,
        2 => LabelStyle::Mac,
        _ => LabelStyle::platform(),
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
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
    Palette => "palette", "Command palette", [Global];
    Quit => "quit", "Quit", [Global];
    Fullscreen => "fullscreen", "Window full screen on / off", [Global];
    KeyHints => "key_hints", "Key hints on / off", [Global];

    StimulusArm => "stimulus_arm", "Stimulus: arm (needs a typed level)", [Global];
    StimulusFire => "stimulus_fire", "Stimulus: fire (when armed)", [Global];
    StimulusStop => "stimulus_stop", "Stimulus: stop and disarm (no window open)", [Global];
    StopAnywhere => "stimulus_stop_anywhere", "Stimulus: stop and disarm, also with a window open", [Global];
    LevelUp => "level_up", "Stimulus level +1 dB", [Global];
    LevelDown => "level_down", "Stimulus level −1 dB", [Global];
    LevelUpCoarse => "level_up_coarse", "Stimulus level +3 dB", [Global];
    LevelDownCoarse => "level_down_coarse", "Stimulus level −3 dB", [Global];
    StimulusLevel => "stimulus_level", "Stimulus: type level (dBFS)…", [Global];
    StimulusOutputs => "stimulus_outputs", "Stimulus: type output channels…", [Global];
    StimulusTakeOver => "stimulus_take_over", "Stimulus: take over the lease from another client and arm", [Global];

    FocusTransfer => "focus_transfer", "Focus transfer-function pane", [Global];
    FocusSpectrum => "focus_spectrum", "Focus spectrum / RTA pane", [Global];
    FocusIr => "focus_ir", "Focus impulse-response pane", [Global];
    FocusSpl => "focus_spl", "Focus SPL pane", [Global];
    FocusDistortion => "focus_distortion", "Focus (and show) the sweep / distortion pane", [Global];
    NextPane => "next_pane", "Focus next pane", [Global];
    PrevPane => "prev_pane", "Focus previous pane", [Global];
    MaximizePane => "maximize_pane", "Layout: split → one pane → full screen", [Global];
    NextMeasurement => "next_measurement", "Select next measurement of the focused pane", [Global];
    PrevMeasurement => "prev_measurement", "Select previous measurement of the focused pane", [Global];
    PaneMeasurement => "pane_measurement", "Choose the measurement the focused pane shows…", [Global];
    CycleTheme => "cycle_theme", "Theme: dark → light → high contrast", [Global];
    ZoomIn => "zoom_in", "Zoom frequency in", [Global];
    ZoomOut => "zoom_out", "Zoom frequency out", [Global];
    PanLeft => "pan_left", "Pan frequency down", [Global];
    PanRight => "pan_right", "Pan frequency up", [Global];
    ResetView => "reset_view", "Reset zoom (20 Hz – 20 kHz)", [Global];
    LevelZoomIn => "level_zoom_in", "Zoom level axis in (vertical)", [Global];
    LevelZoomOut => "level_zoom_out", "Zoom level axis out (vertical)", [Global];
    LevelPanUp => "level_pan_up", "Pan level axis up (towards higher levels)", [Global];
    LevelPanDown => "level_pan_down", "Pan level axis down (towards lower levels)", [Global];
    LevelFit => "level_fit", "Fit level axis to the shown curves, frequency to 20 Hz – 20 kHz", [Global];
    LevelReset => "level_reset", "Level and frequency axes back to the pane's default", [Global];
    ToggleCursor => "toggle_cursor", "Comparison cursor on / off", [Global];
    CursorLeft => "cursor_left", "Cursor 1/12 octave down", [Global];
    CursorRight => "cursor_right", "Cursor 1/12 octave up", [Global];
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
    ToggleTrace => "toggle_trace", "Show / hide the selected stored trace", [Global];
    TraceSlot => "trace_slot", "Move the selected trace to slot… (1 … 9, none frees its slot)", [Global];
    TraceRename => "trace_rename", "Rename the selected trace…", [Global];
    TraceExport => "trace_export", "Export the selected trace (ac2 CSV) to a file…", [Global];
    SelectLive => "select_live", "Deselect the stored trace: keys act on the live measurement again", [Global];
    DeleteTrace => "trace_delete", "Delete selected trace… (asks first)", [Global];
    OffsetUp => "offset_up", "Display offset +1 dB of the selected curve", [Global];
    OffsetDown => "offset_down", "Display offset −1 dB of the selected curve", [Global];
    OffsetUpCoarse => "offset_up_coarse", "Display offset +3 dB of the selected curve", [Global];
    OffsetDownCoarse => "offset_down_coarse", "Display offset −3 dB of the selected curve", [Global];
    OffsetClear => "offset_clear", "Display offset of the selected curve back to 0", [Global];
    ImportTrace => "import_trace", "Import a trace file (CSV / analyzer text)…", [Global];
    SessionSave => "session_save", "Session: save (name or path)…", [Global];
    SessionLoad => "session_load", "Session: load, disarmed (name or path)…", [Global];
    Reconnect => "reconnect", "Reconnect to the daemon now", [Global];
    OpenSession => "session_open", "Open audio session…", [Global];
    CloseSession => "session_close", "Close audio session", [Global];
    Record => "record", "Record: raw audio of every input on / off (stops by itself after 1 h)", [Global];
    ReplayRecording => "replay_recording", "Replay a recording as the session (name or path)…", [Global];
    NewTransfer => "meas_new_transfer", "New transfer measurement…", [Global];
    NewSpectrum => "meas_new_spectrum", "New spectrum…", [Global];
    NewRta => "meas_new_rta", "New RTA…", [Global];
    NewSpl => "meas_new_spl", "New SPL meter…", [Global];
    DeleteMeasurement => "meas_delete", "Delete selected measurement", [Global];
    InputSetup => "input_setup", "Input setup: mic, mic curve and calibration of each input…", [Global];
    Calibrations => "calibrations", "Calibrations: mics, curves and sensitivity calibrations…", [Global];
    InputMics => "input_mics", "Input setup: type mic names (3=M30, 4=ECM)…", [Global];
    MicCurve => "mic_curve", "Mic curve: next curve on the selected measurement's input (off → 0° → 90° …)", [Global];
    MicCurveInput => "mic_curve_input", "Mic curve on input N… (e.g. 2=90°, 2=off)", [Global];
    CalDelete => "cal_delete", "Calibration: delete a sensitivity calibration (input=mic)…", [Global];
    TraceMicCurve => "trace_mic_curve", "Mic curve on the selected trace (e.g. MM1 34804 90°; none removes)…", [Global];
    SweepNew => "sweep_new", "Sweep measurement: response and harmonic distortion…", [Global];
    LeqWindows => "leq_windows", "Leq windows and limits of the SPL meter…", [Global];
    SplFast => "spl_fast", "SPL meter: Fast time weighting (125 ms)", [Global];
    SplSlow => "spl_slow", "SPL meter: Slow time weighting (1 s)", [Global];
    SplImpulse => "spl_impulse", "SPL meter: Impulse time weighting (35 ms / 1.5 s)", [Global];
    SplA => "spl_a", "SPL meter: A weighting", [Global];
    SplC => "spl_c", "SPL meter: C weighting", [Global];
    SplZ => "spl_z", "SPL meter: Z weighting (flat)", [Global];

    Freeze => "freeze", "Freeze / unfreeze selected measurement", [Transfer, Spectrum];
    ResetAverage => "reset_average", "Reset averaging of selected measurement", [Transfer, Spectrum, Spl];
    StartStop => "start_stop", "Start / stop selected measurement", [Transfer, Spectrum, Spl];

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
    NudgeEarlier => "nudge_earlier", "Nudge selected trace 0.1 ms earlier", [Transfer];
    NudgeLater => "nudge_later", "Nudge selected trace 0.1 ms later", [Transfer];
    PhaseReference => "phase_reference", "Make selected trace the phase reference", [Transfer];
    Target => "target", "Load a target curve file…", [Transfer];
    ToggleIr => "toggle_ir", "Show / hide IR pane", [Transfer, Ir];
    CoherenceMask => "coherence_mask", "Coherence mask: off → 0.3 → 0.5 → 0.7 → 0.9", [Transfer];
    CoherencePlacement => "coherence_placement", "Coherence: own pane / over magnitude", [Transfer];
    Average => "average", "Average shown stored traces (power)", [Transfer];
    AverageComplex => "average_complex", "Average shown stored traces (complex)", [Transfer];
    AverageCoherence => "average_coherence", "Average shown stored traces (coherence-weighted)", [Transfer];
    MathDifference => "math_difference", "A − B: dB difference of the two lowest shown slots", [Transfer];
    MathDivide => "math_divide", "A / B: complex division of the two lowest shown slots", [Transfer];
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

    SpectrumStyle => "spectrum_style", "RTA: bars / line", [Spectrum];
    PeakHold => "peak_hold", "Peak hold on / off", [Spectrum];
    Spectrograph => "spectrograph", "Spectrograph under the spectrum on / off", [Spectrum];
    SpectrographSpan => "spectrograph_span", "Spectrograph history: 10 → 30 → 60 → 120 s", [Spectrum];

    IrMode => "ir_mode", "IR: linear → log → ETC", [Ir, Distortion];

    SplLeqView => "spl_leq_view", "SPL: meter → Leq windows → meter + Leq", [Spl];
    SplShowMeter => "spl_show_meter", "SPL pane: the meter", [Spl];
    SplShowLeq => "spl_show_leq", "SPL pane: the Leq windows", [Spl];
    SplShowMeterLeq => "spl_show_meter_leq", "SPL pane: meter + Leq windows", [Spl];
    SplLeqStyle => "spl_leq_style", "SPL Leq windows: columns / tiles", [Spl];
    SplLeqHistory => "spl_leq_history", "SPL Leq windows: history strip on / off", [Spl];
    SplNewLog => "spl_new_log", "Start a new SPL log…", [Spl];
    SplTimeWeighting => "spl_time_weighting", "SPL meter: time weighting Fast → Slow → Impulse", [Spl];
    SplWeighting => "spl_weighting", "SPL meter: frequency weighting A → C → Z", [Spl];

    DistortionUnit => "distortion_unit", "Distortion in dB re fundamental / percent", [Distortion];
    SweepIr => "sweep_ir", "Sweep: distortion / impulse response", [Distortion];
    HideDistortion => "hide_distortion", "Hide the sweep / distortion pane", [Distortion];
}

/// One key in one scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub command: CommandId,
    pub scope: Scope,
    pub chord: Chord,
}

/// The stop that works from anywhere: handled before an open window gets the key. Esc with
/// Shift: the stop key operators already know, made deliberate by the held Shift so that
/// closing a window never stops by accident; the same two keys on every layout (no AltGr,
/// no dead key), never typed text, and taken by no desktop (Ctrl+Esc opens the Windows Start
/// menu, Ctrl/⌘+Space switches input methods or opens Spotlight, F-keys are media keys on
/// many laptops).
pub const STOP_ANYWHERE: Chord = Chord::shift(Key::Escape);

/// The stimulus cluster: these chords mean exactly these commands, in every keymap.
pub const RESERVED: [(Chord, CommandId); 8] = [
    (Chord::key(Key::Space), CommandId::StimulusArm),
    (Chord::key(Key::Enter), CommandId::StimulusFire),
    (Chord::key(Key::Escape), CommandId::StimulusStop),
    (STOP_ANYWHERE, CommandId::StopAnywhere),
    (Chord::key(Key::ArrowUp), CommandId::LevelUp),
    (Chord::key(Key::ArrowDown), CommandId::LevelDown),
    (Chord::shift(Key::ArrowUp), CommandId::LevelUpCoarse),
    (Chord::shift(Key::ArrowDown), CommandId::LevelDownCoarse),
];

/// Default bindings: `(command, scope, chord)`.
pub fn defaults() -> Vec<Binding> {
    use CommandId as C;
    use Key as K;
    use Scope as S;
    let k = Chord::key;
    let sh = Chord::shift;
    let cmd = Chord::command;
    let alt = Chord::alt;
    let alt_sh = |key| Chord {
        shift: true,
        ..Chord::alt(key)
    };
    let mut v: Vec<(C, S, Chord)> = vec![
        // H, not `/`: `/` is Shift+7 on Nordic and German layouts. Shift+H, one step
        // further, hides or shows the panes' key hints.
        (C::Help, S::Global, k(K::H)),
        (C::Help, S::Global, k(K::F1)),
        (C::KeyHints, S::Global, sh(K::H)),
        (C::Palette, S::Global, cmd(K::K)),
        (C::Quit, S::Global, cmd(K::Q)),
        (C::Fullscreen, S::Global, k(K::F11)),
        (C::StimulusLevel, S::Global, k(K::L)),
        // Plain digits are the slots' (as in `ac`); panes take Alt+digit.
        (C::FocusTransfer, S::Global, alt(K::Num1)),
        (C::FocusSpectrum, S::Global, alt(K::Num2)),
        (C::FocusIr, S::Global, alt(K::Num3)),
        (C::FocusSpl, S::Global, alt(K::Num4)),
        (C::FocusDistortion, S::Global, alt(K::Num5)),
        (C::SweepNew, S::Global, sh(K::S)),
        (C::NextPane, S::Global, k(K::Tab)),
        (C::PrevPane, S::Global, sh(K::Tab)),
        (C::MaximizePane, S::Global, k(K::W)),
        (C::NextMeasurement, S::Global, k(K::N)),
        (C::PrevMeasurement, S::Global, sh(K::N)),
        (C::CycleTheme, S::Global, k(K::T)),
        (C::ZoomIn, S::Global, k(K::I)),
        (C::ZoomOut, S::Global, k(K::O)),
        // Plain O zooms out; the session dialog takes Shift+O.
        (C::OpenSession, S::Global, sh(K::O)),
        (C::PanLeft, S::Global, k(K::ArrowLeft)),
        (C::PanRight, S::Global, k(K::ArrowRight)),
        (C::ResetView, S::Global, k(K::Home)),
        // The level (vertical) axis of the focused pane, beside the frequency axis's keys:
        // Ctrl+I / Ctrl+O zoom it as I / O zoom frequency, Ctrl on the arrows that move the
        // stimulus level alone pans it (a plain ↑/↓ never pans by mistake), and Home with a
        // modifier frames or resets it as plain Home resets frequency.
        (C::LevelZoomIn, S::Global, cmd(K::I)),
        (C::LevelZoomOut, S::Global, cmd(K::O)),
        (C::LevelPanUp, S::Global, cmd(K::ArrowUp)),
        (C::LevelPanDown, S::Global, cmd(K::ArrowDown)),
        (C::LevelFit, S::Global, sh(K::Home)),
        (C::LevelReset, S::Global, cmd(K::Home)),
        // A display offset spreads traces apart: Alt on the level arrows, Shift for 3 dB as
        // for the stimulus level; Alt+Home back to none.
        (C::OffsetUp, S::Global, alt(K::ArrowUp)),
        (C::OffsetDown, S::Global, alt(K::ArrowDown)),
        (C::OffsetUpCoarse, S::Global, alt_sh(K::ArrowUp)),
        (C::OffsetDownCoarse, S::Global, alt_sh(K::ArrowDown)),
        (C::OffsetClear, S::Global, alt(K::Home)),
        // Delete asks first (Delete again or Enter deletes).
        (C::DeleteTrace, S::Global, k(K::Delete)),
        (C::TraceRename, S::Global, k(K::F2)),
        (C::ToggleCursor, S::Global, k(K::C)),
        (C::CursorLeft, S::Global, sh(K::ArrowLeft)),
        (C::CursorRight, S::Global, sh(K::ArrowRight)),
        (C::Slot1, S::Global, cmd(K::Num1)),
        (C::Slot2, S::Global, cmd(K::Num2)),
        (C::Slot3, S::Global, cmd(K::Num3)),
        (C::Slot4, S::Global, cmd(K::Num4)),
        (C::Slot5, S::Global, cmd(K::Num5)),
        (C::Slot6, S::Global, cmd(K::Num6)),
        (C::Slot7, S::Global, cmd(K::Num7)),
        (C::Slot8, S::Global, cmd(K::Num8)),
        (C::Slot9, S::Global, cmd(K::Num9)),
        (C::ShowSlot1, S::Global, k(K::Num1)),
        (C::ShowSlot2, S::Global, k(K::Num2)),
        (C::ShowSlot3, S::Global, k(K::Num3)),
        (C::ShowSlot4, S::Global, k(K::Num4)),
        (C::ShowSlot5, S::Global, k(K::Num5)),
        (C::ShowSlot6, S::Global, k(K::Num6)),
        (C::ShowSlot7, S::Global, k(K::Num7)),
        (C::ShowSlot8, S::Global, k(K::Num8)),
        (C::ShowSlot9, S::Global, k(K::Num9)),
        // Shift+digit is punctuation on most layouts, so trace selection steps with V; with
        // Alt it reaches the hidden traces too.
        (C::NextTrace, S::Global, k(K::V)),
        (C::PrevTrace, S::Global, sh(K::V)),
        (C::NextAnyTrace, S::Global, alt(K::V)),
        (
            C::PrevAnyTrace,
            S::Global,
            Chord {
                shift: true,
                ..alt(K::V)
            },
        ),
        // A is free in every pane; the digits stay the slots' show / hide.
        (C::ToggleTrace, S::Global, k(K::A)),
        (C::InsertDelay, S::Transfer, k(K::X)),
        (C::InsertStrongest, S::Transfer, sh(K::X)),
        (C::TypeDelay, S::Transfer, k(K::D)),
        (C::TrackDelay, S::Transfer, k(K::Y)),
        (C::Invert, S::Transfer, k(K::U)),
        (C::Offset, S::Transfer, k(K::J)),
        (C::Offset, S::Spectrum, k(K::J)),
        (C::NudgeEarlier, S::Transfer, k(K::Comma)),
        (C::NudgeLater, S::Transfer, k(K::Period)),
        // The measurement's own delay on the same two keys as the display nudge: Ctrl for a
        // whole sample, Alt for a tenth (Shift is never combined with punctuation).
        (C::DelayDown, S::Transfer, cmd(K::Comma)),
        (C::DelayUp, S::Transfer, cmd(K::Period)),
        (C::DelayDownFine, S::Transfer, alt(K::Comma)),
        (C::DelayUpFine, S::Transfer, alt(K::Period)),
        (C::PhaseReference, S::Transfer, k(K::E)),
        (C::Target, S::Transfer, k(K::Z)),
        // Shift+I shows or hides the impulse response wherever there is one (plain I zooms).
        (C::ToggleIr, S::Transfer, sh(K::I)),
        (C::ToggleIr, S::Ir, sh(K::I)),
        (C::CoherenceMask, S::Transfer, k(K::B)),
        (C::CoherencePlacement, S::Transfer, sh(K::C)),
        (C::Average, S::Transfer, k(K::M)),
        (C::PhaseUnwrap, S::Transfer, k(K::P)),
        // K steps smoothing coarser, Shift+K finer (off → 1/48 … 1/3 octave).
        (C::SmoothCoarser, S::Transfer, k(K::K)),
        (C::SmoothFiner, S::Transfer, sh(K::K)),
        (C::SmoothCoarser, S::Spectrum, k(K::K)),
        (C::SmoothFiner, S::Spectrum, sh(K::K)),
        (C::GroupDelay, S::Transfer, sh(K::P)),
        (C::Freeze, S::Transfer, k(K::F)),
        (C::Freeze, S::Spectrum, k(K::F)),
        (C::ResetAverage, S::Transfer, k(K::R)),
        (C::ResetAverage, S::Spectrum, k(K::R)),
        (C::ResetAverage, S::Spl, k(K::R)),
        (C::StartStop, S::Transfer, k(K::S)),
        (C::StartStop, S::Spl, k(K::S)),
        (C::SpectrumStyle, S::Spectrum, k(K::B)),
        (C::StartStop, S::Spectrum, k(K::S)),
        (C::PeakHold, S::Spectrum, k(K::P)),
        // G is the view key of every pane (IR mode, SPL view): here the spectrograph, and
        // with Shift how much history it shows.
        (C::Spectrograph, S::Spectrum, k(K::G)),
        (C::SpectrographSpan, S::Spectrum, sh(K::G)),
        (C::IrMode, S::Ir, k(K::G)),
        (C::SplLeqView, S::Spl, k(K::G)),
        // B as the RTA's bars / line (C is the global cursor); Shift+B the other change of
        // the Leq windows' layout.
        (C::SplLeqStyle, S::Spl, k(K::B)),
        (C::SplLeqHistory, S::Spl, sh(K::B)),
        // R resets the meter's display; Shift+R, a step further, starts a new log (after a
        // confirmation: it discards show data).
        (C::SplNewLog, S::Spl, sh(K::R)),
        // The meter's weightings: F for the time weighting (Fast → Slow → Impulse; there is
        // nothing to freeze in this pane), Z for the frequency weighting (A → C → Z). Letters
        // only: the same keys on every layout.
        (C::SplTimeWeighting, S::Spl, k(K::F)),
        (C::SplWeighting, S::Spl, k(K::Z)),
        // Plain L types the stimulus level; Shift+L is the Leq windows.
        (C::LeqWindows, S::Global, sh(K::L)),
        (C::IrMode, S::Distortion, k(K::G)),
        (C::DistortionUnit, S::Distortion, k(K::U)),
        (C::SweepIr, S::Distortion, sh(K::I)),
        // W is the layout key: maximise, and with Shift this pane away.
        (C::HideDistortion, S::Distortion, sh(K::W)),
    ];
    v.extend(RESERVED.iter().map(|(c, id)| (*id, S::Global, *c)));
    v.into_iter()
        .map(|(command, scope, chord)| Binding {
            command,
            scope,
            chord,
        })
        .collect()
}

/// One entry of a pane's key-hint line: a command operators reach for most in that pane,
/// its short name on the line, and how long it stays as the line narrows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub command: CommandId,
    /// `select trace`, `dB/%`: a few words, the line has little room.
    pub name: &'static str,
    /// Higher stays longer: the lowest goes first when the line does not fit.
    pub priority: u8,
}

const fn hint(command: CommandId, name: &'static str, priority: u8) -> Hint {
    Hint {
        command,
        name,
        priority,
    }
}

/// The most used commands of each pane, in the order its hint line shows them (curated, not
/// counted: everything else is in the help overlay and the palette). Global commands appear
/// where they are what the pane is used for.
pub fn hints(scope: Scope) -> &'static [Hint] {
    use CommandId as C;
    match scope {
        Scope::Global => &[],
        Scope::Transfer => {
            const {
                &[
                    hint(C::NextTrace, "select trace", 90),
                    hint(C::ToggleTrace, "show/hide", 80),
                    hint(C::Slot1, "capture", 85),
                    hint(C::InsertDelay, "find delay", 70),
                    hint(C::SmoothCoarser, "smoothing", 60),
                    hint(C::ToggleIr, "IR", 40),
                    hint(C::MaximizePane, "maximise", 50),
                    hint(C::OffsetUp, "offset", 45),
                ]
            }
        }
        Scope::Spectrum => {
            const {
                &[
                    hint(C::StartStop, "start/stop", 70),
                    hint(C::Freeze, "freeze", 80),
                    hint(C::PeakHold, "peak hold", 75),
                    // The RTA's bars / line (B) is in the help and the palette: the line
                    // holds eight.
                    hint(C::Spectrograph, "spectrograph", 65),
                    hint(C::SmoothCoarser, "smoothing", 60),
                    hint(C::LevelFit, "fit level", 55),
                    hint(C::Slot1, "capture", 85),
                    hint(C::MaximizePane, "maximise", 40),
                ]
            }
        }
        Scope::Ir => {
            const {
                &[
                    hint(C::IrMode, "linear/log/ETC", 90),
                    hint(C::NextMeasurement, "next measurement", 60),
                    hint(C::ToggleIr, "hide pane", 70),
                    hint(C::MaximizePane, "maximise", 50),
                ]
            }
        }
        Scope::Spl => {
            const {
                &[
                    hint(C::SplLeqView, "meter/Leq/both", 90),
                    hint(C::SplTimeWeighting, "F/S/I", 88),
                    hint(C::SplWeighting, "A/C/Z", 86),
                    hint(C::SplLeqStyle, "columns/tiles", 70),
                    hint(C::SplLeqHistory, "history", 60),
                    hint(C::LeqWindows, "windows", 80),
                    hint(C::SplNewLog, "new log", 50),
                    hint(C::MaximizePane, "maximise", 40),
                ]
            }
        }
        Scope::Distortion => {
            const {
                &[
                    hint(C::SweepNew, "new sweep", 90),
                    hint(C::NextMeasurement, "next sweep", 80),
                    hint(C::DistortionUnit, "dB/%", 70),
                    hint(C::IrMode, "linear/log/ETC", 70),
                    hint(C::SweepIr, "IR/distortion", 60),
                    hint(C::MaximizePane, "maximise", 50),
                    hint(C::HideDistortion, "hide pane", 40),
                ]
            }
        }
    }
}

/// The effective binding table.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: defaults(),
        }
    }
}

/// `keys.toml`: `[scope] command = "Key"` or `command = ["Key", "Key"]`; `[]` unbinds.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ChordList {
    One(String),
    Many(Vec<String>),
}

/// Where user overrides live: `keys.toml` in the ac2 config directory
/// (`ac2_paths::config_dir`; `~/.config/ac2` on Linux).
pub fn config_path() -> PathBuf {
    ac2_paths::keymap()
}

impl Keymap {
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Defaults with the overrides in `toml` applied, validated.
    pub fn from_toml(toml: &str) -> Result<Self, String> {
        let table: BTreeMap<String, BTreeMap<String, ChordList>> =
            toml::from_str(toml).map_err(|e| format!("keys.toml: {e}"))?;
        let mut bindings = defaults();
        for (scope_name, cmds) in table {
            let scope = Scope::parse(&scope_name)
                .ok_or_else(|| format!("keys.toml: unknown scope [{scope_name}]"))?;
            for (cmd_name, list) in cmds {
                let command = CommandId::from_name(&cmd_name).ok_or_else(|| {
                    format!("keys.toml: unknown command {cmd_name:?} in [{scope_name}]")
                })?;
                if !command.scopes().contains(&scope) {
                    return Err(format!(
                        "keys.toml: {cmd_name} does nothing in [{scope_name}] (valid: {})",
                        command
                            .scopes()
                            .iter()
                            .map(|s| s.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                let chords = match list {
                    ChordList::One(s) => vec![s],
                    ChordList::Many(v) => v,
                };
                bindings.retain(|b| !(b.command == command && b.scope == scope));
                for s in chords {
                    let chord = Chord::parse(&s).map_err(|e| format!("keys.toml: {e}"))?;
                    bindings.push(Binding {
                        command,
                        scope,
                        chord,
                    });
                }
            }
        }
        let map = Self { bindings };
        map.validate()?;
        Ok(map)
    }

    /// Loads `path`; a missing file means defaults. On error the defaults are returned with
    /// the message, so a typo never leaves the operator without keys.
    pub fn load(path: Option<&Path>) -> (Self, Option<String>) {
        let Some(path) = path else {
            return (Self::default(), None);
        };
        match std::fs::read_to_string(path) {
            Ok(s) => match Self::from_toml(&s) {
                Ok(m) => (m, None),
                Err(e) => (Self::default(), Some(format!("{e}; using default keys"))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!("{}: {e}; using default keys", path.display())),
            ),
        }
    }

    /// Conflicts, reserved chords and scope rules.
    pub fn validate(&self) -> Result<(), String> {
        for b in &self.bindings {
            if !b.command.scopes().contains(&b.scope) {
                return Err(format!(
                    "{} bound in {} where it does nothing",
                    b.command.name(),
                    b.scope.name()
                ));
            }
            if let Some((_, owner)) = RESERVED.iter().find(|(c, _)| *c == b.chord)
                && *owner != b.command
            {
                return Err(format!(
                    "{} is reserved for {}; cannot bind it to {}",
                    b.chord,
                    owner.name(),
                    b.command.name()
                ));
            }
        }
        let esc = Chord::key(Key::Escape);
        if !self
            .bindings
            .iter()
            .any(|b| b.chord == esc && b.command == CommandId::StimulusStop)
        {
            return Err("Esc must stay bound to stimulus_stop".into());
        }
        let anywhere: Vec<Chord> = self
            .bindings
            .iter()
            .filter(|b| b.command == CommandId::StopAnywhere)
            .map(|b| b.chord)
            .collect();
        if anywhere != [STOP_ANYWHERE] {
            return Err(format!(
                "stimulus_stop_anywhere is fixed to {STOP_ANYWHERE}: it works inside dialogs, \
                 so it may not be a key they use"
            ));
        }
        for scope in Scope::ALL {
            let mut seen: std::collections::HashMap<Chord, CommandId> =
                std::collections::HashMap::new();
            for b in self
                .bindings
                .iter()
                .filter(|b| b.scope == Scope::Global || b.scope == scope)
            {
                if let Some(prev) = seen.insert(b.chord, b.command)
                    && prev != b.command
                {
                    return Err(format!(
                        "{} is bound to both {} and {} in {}",
                        b.chord,
                        prev.name(),
                        b.command.name(),
                        scope.name()
                    ));
                }
            }
        }
        Ok(())
    }

    /// The command `chord` runs with `scope` active.
    pub fn lookup(&self, scope: Scope, chord: Chord) -> Option<CommandId> {
        let find = |s: Scope| {
            self.bindings
                .iter()
                .find(|b| b.scope == s && b.chord == chord)
                .map(|b| b.command)
        };
        find(scope).or_else(|| find(Scope::Global))
    }

    /// Chords of `command` in `scope`.
    pub fn chords(&self, command: CommandId, scope: Scope) -> Vec<Chord> {
        self.bindings
            .iter()
            .filter(|b| b.command == command && b.scope == scope)
            .map(|b| b.chord)
            .collect()
    }

    /// The first chord that runs `command` with `scope` active: the scope's own, else the
    /// global one.
    pub fn first_chord(&self, command: CommandId, scope: Scope) -> Option<Chord> {
        [scope, Scope::Global]
            .into_iter()
            .find_map(|s| self.chords(command, s).first().copied())
    }

    /// Key text for the palette: the chord in `active` scope (or global), else the chord in
    /// the command's own scope with that scope named.
    pub fn key_hint(&self, command: CommandId, active: Scope) -> Option<String> {
        let join = |v: Vec<Chord>| v.iter().map(Chord::label).collect::<Vec<_>>().join(" · ");
        for s in [active, Scope::Global] {
            let v = self.chords(command, s);
            if !v.is_empty() {
                return Some(join(v));
            }
        }
        command.scopes().iter().find_map(|s| {
            let v = self.chords(command, *s);
            (!v.is_empty()).then(|| format!("{} ({})", join(v), s.name()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Keymap::default().validate().expect("default keymap");
    }

    #[test]
    fn no_conflicts_in_any_scope() {
        let m = Keymap::default();
        for scope in Scope::ALL {
            let mut seen = std::collections::HashMap::new();
            for b in m
                .bindings()
                .iter()
                .filter(|b| b.scope == Scope::Global || b.scope == scope)
            {
                if let Some(prev) = seen.insert(b.chord, b.command) {
                    assert_eq!(prev, b.command, "{} in {scope:?}", b.chord);
                }
            }
        }
    }

    #[test]
    fn no_layout_unsafe_defaults() {
        for b in defaults() {
            assert!(
                !layout_unsafe(b.chord.key),
                "{:?} uses {:?}",
                b.command,
                b.chord.key
            );
            assert!(!(b.chord.shift && is_symbol(b.chord.key)));
        }
        for s in ["[", "]", "+", "-"] {
            assert!(
                defaults().iter().all(|b| b.chord.key.symbol_or_name() != s),
                "{s}"
            );
        }
    }

    #[test]
    fn every_binding_is_in_a_scope_where_it_acts() {
        for b in defaults() {
            assert!(b.command.scopes().contains(&b.scope), "{b:?}");
        }
    }

    #[test]
    fn every_command_is_reachable() {
        // Every command has a key or is deliberately palette-only.
        let palette_only = [
            CommandId::StimulusOutputs,
            CommandId::StimulusTakeOver,
            CommandId::Reconnect,
            CommandId::CloseSession,
            CommandId::Record,
            CommandId::ReplayRecording,
            CommandId::NewTransfer,
            CommandId::NewSpectrum,
            CommandId::NewRta,
            CommandId::NewSpl,
            CommandId::DeleteMeasurement,
            CommandId::AverageComplex,
            CommandId::AverageCoherence,
            CommandId::MathDifference,
            CommandId::MathDivide,
            CommandId::ImportTrace,
            CommandId::SessionSave,
            CommandId::SessionLoad,
            CommandId::InputSetup,
            CommandId::Calibrations,
            CommandId::InputMics,
            CommandId::MicCurve,
            CommandId::MicCurveInput,
            CommandId::CalDelete,
            CommandId::TraceMicCurve,
            CommandId::TraceSlot,
            CommandId::TraceRename,
            CommandId::TraceExport,
            CommandId::FinderAuto,
            CommandId::FinderFull,
            CommandId::FinderMid,
            CommandId::FinderSub,
            CommandId::FinderCustom,
            CommandId::FinderObservation,
            CommandId::PaneMeasurement,
            // Esc also deselects (when no dialog is open); this is its palette entry.
            CommandId::SelectLive,
            CommandId::SplFast,
            CommandId::SplSlow,
            CommandId::SplImpulse,
            CommandId::SplA,
            CommandId::SplC,
            CommandId::SplZ,
            CommandId::SmoothOff,
            CommandId::Smooth48,
            CommandId::Smooth24,
            CommandId::Smooth12,
            CommandId::Smooth6,
            CommandId::Smooth3,
            CommandId::SplShowMeter,
            CommandId::SplShowLeq,
            CommandId::SplShowMeterLeq,
        ];
        let m = Keymap::default();
        for c in CommandId::ALL {
            let bound = m.bindings().iter().any(|b| b.command == *c);
            assert!(bound || palette_only.contains(c), "{c:?} has no key");
        }
    }

    #[test]
    fn learned_bindings_carry_over() {
        let m = Keymap::default();
        let t = Scope::Transfer;
        let c = |s: &str| Chord::parse(s).expect(s);
        for (chord, cmd) in [
            ("X", CommandId::InsertDelay),
            ("Y", CommandId::TrackDelay),
            ("U", CommandId::Invert),
            ("J", CommandId::Offset),
            ("Z", CommandId::Target),
            ("Shift+I", CommandId::ToggleIr),
            ("B", CommandId::CoherenceMask),
            ("M", CommandId::Average),
            ("Shift+P", CommandId::GroupDelay),
            ("Ctrl+1", CommandId::Slot1),
            ("Ctrl+9", CommandId::Slot9),
            ("1", CommandId::ShowSlot1),
            ("9", CommandId::ShowSlot9),
            ("Alt+2", CommandId::FocusSpectrum),
            ("Space", CommandId::StimulusArm),
            ("Enter", CommandId::StimulusFire),
            ("Esc", CommandId::StimulusStop),
            ("Shift+Esc", CommandId::StopAnywhere),
            ("Up", CommandId::LevelUp),
            ("Down", CommandId::LevelDown),
            ("H", CommandId::Help),
            ("F1", CommandId::Help),
            ("Shift+H", CommandId::KeyHints),
            ("Ctrl+K", CommandId::Palette),
            ("Shift+O", CommandId::OpenSession),
            ("O", CommandId::ZoomOut),
            ("K", CommandId::SmoothCoarser),
            ("Shift+K", CommandId::SmoothFiner),
            ("V", CommandId::NextTrace),
            ("Shift+V", CommandId::PrevTrace),
            ("Alt+V", CommandId::NextAnyTrace),
            ("Alt+Shift+V", CommandId::PrevAnyTrace),
            ("A", CommandId::ToggleTrace),
        ] {
            assert_eq!(m.lookup(t, c(chord)), Some(cmd), "{chord}");
        }
        // Scoped lookup: P means peak hold in the spectrum pane, phase in the transfer pane.
        assert_eq!(m.lookup(Scope::Spectrum, c("P")), Some(CommandId::PeakHold));
        assert_eq!(
            m.lookup(Scope::Transfer, c("P")),
            Some(CommandId::PhaseUnwrap)
        );
        // H is help in every pane; Shift+I is the impulse response wherever there is one.
        for scope in Scope::ALL {
            assert_eq!(m.lookup(scope, c("H")), Some(CommandId::Help), "{scope:?}");
        }
        assert_eq!(m.lookup(Scope::Ir, c("Shift+I")), Some(CommandId::ToggleIr));
        assert_eq!(
            m.lookup(Scope::Distortion, c("Shift+I")),
            Some(CommandId::SweepIr)
        );
        assert_eq!(m.lookup(Scope::Global, c("/")), None);
        assert_eq!(m.lookup(Scope::Spl, c("X")), None);
        // The SPL meter's weightings; F freezes elsewhere, Z loads a target in transfer.
        assert_eq!(
            m.lookup(Scope::Spl, c("F")),
            Some(CommandId::SplTimeWeighting)
        );
        assert_eq!(m.lookup(Scope::Spl, c("Z")), Some(CommandId::SplWeighting));
        assert_eq!(m.lookup(Scope::Spectrum, c("F")), Some(CommandId::Freeze));
    }

    /// Offsets, the level axis and Delete: modifiers on the stimulus arrows (which stay the
    /// stimulus level's alone) and beside the frequency axis's I / O / Home.
    #[test]
    fn offset_level_and_delete_keys() {
        let m = Keymap::default();
        let c = |s: &str| Chord::parse(s).expect(s);
        for scope in Scope::ALL {
            assert_eq!(m.lookup(scope, c("Up")), Some(CommandId::LevelUp));
            assert_eq!(
                m.lookup(scope, c("Shift+Down")),
                Some(CommandId::LevelDownCoarse)
            );
            assert_eq!(m.lookup(scope, c("Alt+Up")), Some(CommandId::OffsetUp));
            assert_eq!(m.lookup(scope, c("Alt+Down")), Some(CommandId::OffsetDown));
            assert_eq!(
                m.lookup(scope, c("Alt+Shift+Up")),
                Some(CommandId::OffsetUpCoarse)
            );
            assert_eq!(
                m.lookup(scope, c("Alt+Shift+Down")),
                Some(CommandId::OffsetDownCoarse)
            );
            assert_eq!(m.lookup(scope, c("Alt+Home")), Some(CommandId::OffsetClear));
            assert_eq!(m.lookup(scope, c("Delete")), Some(CommandId::DeleteTrace));
            assert_eq!(m.lookup(scope, c("Home")), Some(CommandId::ResetView));
            for (k, cmd) in [
                ("Ctrl+Up", CommandId::LevelPanUp),
                ("Ctrl+Down", CommandId::LevelPanDown),
                ("Ctrl+I", CommandId::LevelZoomIn),
                ("Ctrl+O", CommandId::LevelZoomOut),
                ("Shift+Home", CommandId::LevelFit),
                ("Ctrl+Home", CommandId::LevelReset),
                ("I", CommandId::ZoomIn),
                ("O", CommandId::ZoomOut),
            ] {
                assert_eq!(m.lookup(scope, c(k)), Some(cmd), "{k} {scope:?}");
            }
        }
        assert_eq!(m.lookup(Scope::Spectrum, c("J")), Some(CommandId::Offset));
    }

    #[test]
    fn symbols_ignore_shift() {
        // `/` arrives as Shift+7 → Slash with Shift on Nordic layouts.
        let shifted = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        let ch = Chord::from_event(Key::Slash, shifted);
        // Bound in keys.toml, `/` matches however the layout produces it.
        let m = Keymap::from_toml("[global]\nhelp = [\"H\", \"/\"]").expect("valid");
        assert_eq!(m.lookup(Scope::Global, ch), Some(CommandId::Help));
        assert!(Chord::parse("Shift+/").is_err());
        // Letters keep Shift.
        assert!(Chord::from_event(Key::P, shifted).shift);
    }

    #[test]
    fn parse_and_label() {
        for (s, label) in [
            ("ctrl+k", "Ctrl+K"),
            ("Cmd+K", "Ctrl+K"),
            ("Shift+P", "Shift+P"),
            ("space", "Space"),
            ("Up", "↑"),
            ("/", "/"),
            (",", ","),
            ("F1", "F1"),
            ("Alt+Left", "Alt+←"),
            ("Home", "Home"),
        ] {
            let c = Chord::parse(s).expect(s);
            assert_eq!(c.label_in(LabelStyle::Pc), label, "{s}");
            assert_eq!(
                Chord::parse(&c.label_in(LabelStyle::Pc)).ok(),
                Some(c),
                "{s}"
            );
            assert_eq!(
                Chord::parse(&c.label_in(LabelStyle::Mac)).ok(),
                Some(c),
                "{s}"
            );
        }
        assert_eq!(Chord::parse("⌘⇧K"), Chord::parse("Ctrl+Shift+K"));
        assert_eq!(Chord::parse("⌥←"), Chord::parse("Alt+Left"));
        assert!(Chord::parse("Hyper+K").is_err());
        assert!(Chord::parse("Ctrl+Nope").is_err());
    }

    #[test]
    fn toml_overrides() {
        let m = Keymap::from_toml(
            r#"
            [transfer]
            insert_delay = "Q"
            invert = ["U", "I"]

            [global]
            zoom_in = "Alt+I"
            "#,
        );
        // I is zoom_in globally by default... but zoom_in moved to Alt+I, so I is free.
        let m = m.expect("valid");
        let c = |s: &str| Chord::parse(s).expect(s);
        assert_eq!(
            m.lookup(Scope::Transfer, c("Q")),
            Some(CommandId::InsertDelay)
        );
        assert_eq!(m.lookup(Scope::Transfer, c("X")), None);
        assert_eq!(m.lookup(Scope::Transfer, c("I")), Some(CommandId::Invert));
        assert_eq!(
            m.lookup(Scope::Spectrum, c("Alt+I")),
            Some(CommandId::ZoomIn)
        );
        // Unbinding with an empty list.
        let m = Keymap::from_toml("[transfer]\ntarget = []").expect("valid");
        assert_eq!(m.lookup(Scope::Transfer, c("Z")), None);
    }

    #[test]
    fn toml_errors() {
        let bad = [
            ("[transfer]\ninsert_delay = \"U\"", "bound to both"),
            ("[global]\nhelp = \"Space\"", "reserved"),
            ("[global]\nstimulus_stop = \"Q\"", "Esc must stay"),
            ("[global]\nstimulus_stop_anywhere = \"Q\"", "is fixed"),
            (
                "[global]\nstimulus_stop_anywhere = [\"Shift+Esc\", \"Ctrl+S\"]",
                "is fixed",
            ),
            ("[global]\nhelp = \"Shift+Esc\"", "reserved"),
            ("[spectrum]\ninsert_delay = \"Q\"", "does nothing"),
            ("[nowhere]\nhelp = \"Q\"", "unknown scope"),
            ("[global]\nfly = \"Q\"", "unknown command"),
            ("[global]\nhelp = \"Ctrl+Nope\"", "unknown key"),
            ("[global]\nhelp = 3", "keys.toml"),
            // H is help in every pane: a pane's command on H conflicts with it there.
            ("[spectrum]\npeak_hold = \"H\"", "bound to both"),
        ];
        for (src, want) in bad {
            let e = Keymap::from_toml(src).expect_err(src);
            assert!(e.contains(want), "{src}: {e}");
        }
        // A global override must not collide with any scope's binding either.
        let e = Keymap::from_toml("[global]\nzoom_in = \"X\"").expect_err("conflict");
        assert!(e.contains("transfer"), "{e}");
    }

    #[test]
    fn load_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("ac2-ui-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let p = dir.join("keys.toml");
        let (m, e) = Keymap::load(Some(&p));
        assert_eq!(m, Keymap::default());
        assert!(e.is_none());
        std::fs::write(&p, "[global]\nhelp = \"Space\"").expect("write");
        let (m, e) = Keymap::load(Some(&p));
        assert_eq!(m, Keymap::default());
        assert!(e.expect("error").contains("using default keys"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_hints() {
        let m = Keymap::default();
        assert_eq!(
            m.key_hint(CommandId::InsertDelay, Scope::Transfer)
                .as_deref(),
            Some("X")
        );
        assert_eq!(
            m.key_hint(CommandId::InsertDelay, Scope::Spectrum)
                .as_deref(),
            Some("X (transfer)")
        );
        assert_eq!(m.key_hint(CommandId::Reconnect, Scope::Global), None);
        assert_eq!(
            m.key_hint(CommandId::Help, Scope::Ir).as_deref(),
            Some("H · F1")
        );
        assert_eq!(
            m.first_chord(CommandId::ToggleIr, Scope::Ir),
            Chord::parse("Shift+I").ok()
        );
        assert_eq!(
            m.first_chord(CommandId::MaximizePane, Scope::Spl),
            Chord::parse("W").ok()
        );
        assert_eq!(m.first_chord(CommandId::ToggleIr, Scope::Spl), None);
    }

    #[test]
    fn pane_hints_are_bound_where_they_are_shown() {
        let m = Keymap::default();
        for scope in Scope::ALL {
            let h = hints(scope);
            if scope == Scope::Global {
                assert!(h.is_empty());
                continue;
            }
            assert!((4..=8).contains(&h.len()), "{scope:?}: {} hints", h.len());
            for x in h {
                assert!(
                    x.command.scopes().contains(&scope)
                        || x.command.scopes().contains(&Scope::Global),
                    "{:?} does nothing in {scope:?}",
                    x.command
                );
                assert!(
                    m.first_chord(x.command, scope).is_some(),
                    "{:?} has no key in {scope:?}",
                    x.command
                );
                assert!(x.priority < u8::MAX, "the help hint keeps the top priority");
            }
            let mut cmds: Vec<_> = h.iter().map(|x| x.command).collect();
            cmds.sort();
            cmds.dedup();
            assert_eq!(cmds.len(), h.len(), "{scope:?} lists a command twice");
        }
    }

    /// The stop chord: the same in every scope, unique, layout-safe and never typed text.
    #[test]
    fn stop_anywhere_is_safe_on_every_layout() {
        let m = Keymap::default();
        assert_eq!(Chord::parse("Shift+Esc"), Ok(STOP_ANYWHERE));
        assert!(!layout_unsafe(STOP_ANYWHERE.key) && !is_symbol(STOP_ANYWHERE.key));
        const { assert!(!(STOP_ANYWHERE.command || STOP_ANYWHERE.alt)) };
        for scope in Scope::ALL {
            assert_eq!(
                m.lookup(scope, STOP_ANYWHERE),
                Some(CommandId::StopAnywhere),
                "{scope:?}"
            );
            let holders: Vec<_> = m
                .bindings()
                .iter()
                .filter(|b| {
                    (b.scope == Scope::Global || b.scope == scope) && b.chord == STOP_ANYWHERE
                })
                .collect();
            assert_eq!(holders.len(), 1, "{scope:?}");
        }
        // Nordic layouts send Esc and Shift as such: the chord of the event is the stop.
        let shifted = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        assert_eq!(Chord::from_event(Key::Escape, shifted), STOP_ANYWHERE);
        assert_eq!(STOP_ANYWHERE.label_in(LabelStyle::Pc), "Shift+Esc");
        // Keeping it and adding nothing is fine.
        Keymap::from_toml("[global]\nstimulus_stop_anywhere = \"Shift+Esc\"").expect("valid");
    }
}
