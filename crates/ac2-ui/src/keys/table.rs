//! The default bindings, the reserved stimulus cluster and each pane's key hints.

use eframe::egui::Key;

use super::{Chord, CommandId, Scope};

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
        // H, not `/`: `/` is Shift+7 on Nordic and German layouts.
        (C::Help, S::Global, k(K::H)),
        (C::Help, S::Global, k(K::F1)),
        (C::Palette, S::Global, cmd(K::K)),
        // Ctrl+P(references): the desktop's Ctrl+, is the transfer pane's whole-sample delay
        // step here, and a letter is on every layout.
        (C::Settings, S::Global, cmd(K::P)),
        (C::Quit, S::Global, cmd(K::Q)),
        // F is full screen in every pane, so no pane binds plain F; F11 as elsewhere.
        (C::Fullscreen, S::Global, k(K::F)),
        (C::Fullscreen, S::Global, k(K::F11)),
        (C::StimulusLevel, S::Global, k(K::L)),
        // Plain digits are the slots' (as in `ac`); panes take Alt+digit.
        (C::FocusPane1, S::Global, alt(K::Num1)),
        (C::FocusPane2, S::Global, alt(K::Num2)),
        (C::FocusPane3, S::Global, alt(K::Num3)),
        (C::FocusPane4, S::Global, alt(K::Num4)),
        (C::FocusPane5, S::Global, alt(K::Num5)),
        (C::FocusPane6, S::Global, alt(K::Num6)),
        (C::FocusPane7, S::Global, alt(K::Num7)),
        (C::FocusPane8, S::Global, alt(K::Num8)),
        (C::FocusPane9, S::Global, alt(K::Num9)),
        // Plain letters: desktops take Ctrl+Tab and Ctrl+D for themselves, and letters sit
        // where they are on every layout. Q closes a pane; Ctrl+Q, a step further, quits.
        (C::SplitPane, S::Global, k(K::N)),
        (C::ClosePane, S::Global, k(K::Q)),
        // Shift+N, a step on from N: the split just made turns the other way. Alt on the
        // side arrows sizes the pane, as Alt on the level arrows offsets the traces.
        (C::TurnSplit, S::Global, sh(K::N)),
        (C::GrowPane, S::Global, alt(K::ArrowRight)),
        (C::ShrinkPane, S::Global, alt(K::ArrowLeft)),
        (C::SweepNew, S::Global, sh(K::S)),
        // Ctrl+M (the operator's choice): every kind of measurement in one list, so the
        // kinds without a key of their own are found without the palette's words.
        (C::NewMeasurement, S::Global, cmd(K::M)),
        // Shift+M, a step on from M (average the shown traces): a math channel by name.
        (C::NewMath, S::Global, sh(K::M)),
        // Tab walks the measurement list row by row, the way it walks a form's fields, and
        // the focused pane shows the measurement of each row it reaches.
        (C::NextInTree, S::Global, k(K::Tab)),
        (C::PrevInTree, S::Global, sh(K::Tab)),
        (C::MaximizePane, S::Global, k(K::W)),
        // Ctrl+Z / Ctrl+Shift+Z as in every editor; both reach the app on the desktops it
        // runs on. No Ctrl+Y: Y and Z swap places on German layouts, and one redo is enough.
        (C::Undo, S::Global, cmd(K::Z)),
        (
            C::Redo,
            S::Global,
            Chord {
                shift: true,
                ..cmd(K::Z)
            },
        ),
        // T strips the focused plot step by step (grid, then labels and cursor) for a clean
        // picture; the theme is in Settings › Display and the palette.
        (C::PlotChrome, S::Global, k(K::T)),
        (C::ZoomIn, S::Global, k(K::I)),
        (C::ZoomOut, S::Global, k(K::O)),
        // Plain O zooms out; the session dialog takes Shift+O.
        (C::OpenSession, S::Global, sh(K::O)),
        // Alt+S: the saved sessions (S alone is start/stop, Shift+S a sweep).
        (C::Sessions, S::Global, alt(K::S)),
        // Ctrl+S: save is rare enough for a Ctrl chord, and it is the key every editor uses.
        (C::SessionSave, S::Global, cmd(K::S)),
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
        // Delete asks first (Delete again or Enter deletes). Backspace too: Mac keyboards
        // and many laptops have no Delete key.
        (C::DeleteSelected, S::Global, k(K::Delete)),
        (C::DeleteSelected, S::Global, k(K::Backspace)),
        (C::TraceRename, S::Global, k(K::F2)),
        (C::Compare, S::Global, k(K::C)),
        // Shift+C steps the selected measurement's colour (C itself compares).
        (C::NextColour, S::Global, sh(K::C)),
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
        // A is free in every pane; the digits stay the slots' show / hide. It acts on what
        // was selected last: a measurement or a stored trace.
        (C::ToggleSelected, S::Global, k(K::A)),
        // Shift+A, a step on from A: the whole group (the measurement and its traces).
        (C::HideGroup, S::Global, sh(K::A)),
        // Shift+F2, beside F2 (rename): where the trace is listed.
        (C::MoveTrace, S::Global, sh(K::F2)),
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
        (C::CoherenceMask, S::Transfer, k(K::B)),
        (C::Average, S::Transfer, k(K::M)),
        // P tracks extremes in both plots: min / max here, peak hold and peaks on the
        // spectrum. The phase views share Shift+P.
        (C::MinMax, S::Transfer, k(K::P)),
        // K steps smoothing coarser, Shift+K finer (off → 1/48 … 1/3 octave).
        (C::SmoothCoarser, S::Transfer, k(K::K)),
        (C::SmoothFiner, S::Transfer, sh(K::K)),
        (C::SmoothCoarser, S::Spectrum, k(K::K)),
        (C::SmoothFiner, S::Spectrum, sh(K::K)),
        (C::PhaseView, S::Transfer, sh(K::P)),
        (C::ResetAverage, S::Transfer, k(K::R)),
        (C::ResetAverage, S::Spectrum, k(K::R)),
        (C::ResetAverage, S::Spl, k(K::R)),
        (C::StartStop, S::Transfer, k(K::S)),
        (C::StartStop, S::Spl, k(K::S)),
        (C::SpectrumStyle, S::Spectrum, k(K::B)),
        (C::StartStop, S::Spectrum, k(K::S)),
        // The IR view shows the transfer measurement: S starts and stops it there too.
        (C::StartStop, S::Ir, k(K::S)),
        (C::PeakHold, S::Spectrum, k(K::P)),
        // G steps the focused pane's views, Shift+G back: one key pair for every pane, the
        // IR's displays among the views, so no view needs a second key to reach.
        (C::NextView, S::Transfer, k(K::G)),
        (C::PrevView, S::Transfer, sh(K::G)),
        (C::NextView, S::Ir, k(K::G)),
        (C::PrevView, S::Ir, sh(K::G)),
        (C::NextView, S::Spectrum, k(K::G)),
        (C::PrevView, S::Spectrum, sh(K::G)),
        (C::NextView, S::Spl, k(K::G)),
        (C::PrevView, S::Spl, sh(K::G)),
        (C::NextView, S::Distortion, k(K::G)),
        (C::PrevView, S::Distortion, sh(K::G)),
        // Shift+B, beside B (line / fill / bars): the spectrograph's history length.
        (C::SpectrographSpan, S::Spectrum, sh(K::B)),
        // R resets the meter's display; Shift+R, a step further, starts a new log (after a
        // confirmation: it discards show data).
        (C::SplNewLog, S::Spl, sh(K::R)),
        // The meter's weightings: Shift+F for the time weighting (Fast → Slow → Impulse;
        // plain F is full screen everywhere), Z for the frequency weighting (A → C → Z).
        // Letters only: the same keys on every layout.
        (C::SplTimeWeighting, S::Spl, sh(K::F)),
        (C::SplWeighting, S::Spl, k(K::Z)),
        // Plain L types the stimulus level; Shift+L is the Leq windows.
        (C::LeqWindows, S::Global, sh(K::L)),
        (C::SweepIr, S::Distortion, sh(K::I)),
        // E, as the transfer pane's phase reference: the reference input's own harmonics.
        (C::ReferenceHarmonics, S::Distortion, k(K::E)),
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
                    hint(C::NextView, "view", 75),
                    hint(C::ToggleSelected, "show/hide", 80),
                    hint(C::Slot1, "capture", 85),
                    hint(C::InsertDelay, "find delay", 70),
                    hint(C::CoherenceMask, "coherence mask", 65),
                    hint(C::MinMax, "min/max", 55),
                    hint(C::SmoothCoarser, "smoothing", 60),
                ]
            }
        }
        Scope::Spectrum => {
            const {
                &[
                    hint(C::StartStop, "start/stop", 70),
                    hint(C::PeakHold, "peaks", 75),
                    // The spectrum style (B) is in the help and the palette: the line
                    // holds eight.
                    hint(C::NextView, "view", 65),
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
                    hint(C::NextView, "view", 92),
                    hint(C::ZoomIn, "zoom time", 80),
                    hint(C::LevelZoomIn, "zoom level", 65),
                    hint(C::LevelFit, "fit", 55),
                    hint(C::NextInTree, "next", 60),
                    hint(C::ClosePane, "close pane", 45),
                    hint(C::MaximizePane, "maximise", 50),
                ]
            }
        }
        Scope::Spl => {
            const {
                &[
                    hint(C::NextView, "view", 90),
                    hint(C::SplTimeWeighting, "F/S/I", 88),
                    hint(C::SplWeighting, "A/C/Z", 86),
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
                    hint(C::NextTrace, "next sweep", 80),
                    hint(C::NextView, "view", 75),
                    hint(C::ReferenceHarmonics, "reference", 60),
                    hint(C::MaximizePane, "maximise", 50),
                    hint(C::ClosePane, "close pane", 40),
                ]
            }
        }
    }
}
