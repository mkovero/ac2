//! Hidden palette search words: what an operator may type for a command whose terse title
//! does not say it. The palette matches them like the title but never shows them.

use super::CommandId;

impl CommandId {
    /// Words the palette also matches, never shown. Every command is listed, so a new one
    /// is a compile error until someone decides what it is searched by.
    pub fn keywords(self) -> &'static [&'static str] {
        use CommandId as C;
        match self {
            C::Help => &["keys", "shortcuts", "bindings"],
            C::Notifications => &["toasts", "messages", "warnings", "errors"],
            C::Palette => &["commands", "search"],
            C::Settings => &["preferences", "options", "config"],
            C::Quit => &["exit"],
            C::Fullscreen => &["window"],
            C::KeyHints => &["shortcuts", "help"],
            C::WarningToasts => &["notifications", "messages"],
            C::StimulusArm | C::StimulusFire => &[
                "generator",
                "signal",
                "noise",
                "pink",
                "sine",
                "play",
                "output",
            ],
            C::StimulusStop | C::StopAnywhere => &["generator", "mute", "silence", "output"],
            C::LevelUp | C::LevelDown | C::LevelUpCoarse | C::LevelDownCoarse => {
                &["volume", "gain", "generator"]
            }
            C::StimulusLevel => &["volume", "gain", "generator"],
            C::StimulusOutputs => &["channels", "routing", "generator"],
            C::StimulusTakeOver => &["generator"],
            C::FocusPane1
            | C::FocusPane2
            | C::FocusPane3
            | C::FocusPane4
            | C::FocusPane5
            | C::FocusPane6
            | C::FocusPane7
            | C::FocusPane8
            | C::FocusPane9 => &["select pane"],
            C::SplitPane | C::TurnSplit | C::GrowPane | C::ShrinkPane => &["layout"],
            C::ClosePane => &["layout", "delete pane"],
            C::TransferStack => &["layout", "magnitude", "phase"],
            C::MaximisePane => &["layout", "zoom pane"],
            C::Undo | C::Redo => &["layout"],
            C::NextInTree | C::PrevInTree => &["measurements", "select"],
            C::PaneMeasurement => &["pick", "show", "pane"],
            C::CycleTheme => &["dark", "light", "colours"],
            C::PlotChrome => &[
                "grid",
                "axes",
                "scale",
                "labels",
                "decorations",
                "traces only",
            ],
            C::ZoomIn | C::ZoomOut | C::PanLeft | C::PanRight => &["frequency", "view"],
            C::ResetView => &["zoom", "fit", "frequency"],
            C::LevelZoomIn | C::LevelZoomOut | C::LevelPanUp | C::LevelPanDown => {
                &["range", "dB", "y axis"]
            }
            C::LevelFit => &["auto scale", "range", "y axis"],
            C::LevelReset => &["zoom", "range"],
            C::ToggleCursor => &["readout", "marker"],
            C::Compare => &["overlay", "reference"],
            C::ClearCompare => &["overlay"],
            C::CursorLeft | C::CursorRight => &["readout", "marker"],
            C::Slot1
            | C::Slot2
            | C::Slot3
            | C::Slot4
            | C::Slot5
            | C::Slot6
            | C::Slot7
            | C::Slot8
            | C::Slot9 => &["store", "snapshot", "freeze", "trace"],
            C::ShowSlot1
            | C::ShowSlot2
            | C::ShowSlot3
            | C::ShowSlot4
            | C::ShowSlot5
            | C::ShowSlot6
            | C::ShowSlot7
            | C::ShowSlot8
            | C::ShowSlot9 => &["stored", "visible", "trace"],
            C::NextTrace | C::PrevTrace | C::NextAnyTrace | C::PrevAnyTrace => &["select", "curve"],
            C::ToggleSelected => &["hide", "visible", "trace"],
            C::TraceSlot | C::MoveTrace => &["stored", "slot"],
            C::TraceRename => &["name", "label"],
            C::TraceExport => &["save", "text", "csv", "file"],
            C::ExportPanePng => &["save", "image", "picture", "screenshot", "report", "print"],
            C::SelectLive => &["curve", "trace"],
            C::DeleteSelected => &["remove", "erase", "trace", "measurement"],
            C::OffsetUp | C::OffsetDown | C::OffsetUpCoarse | C::OffsetDownCoarse => {
                &["gain", "level", "shift"]
            }
            C::OffsetClear => &["gain", "remove offset", "reset"],
            C::ImportTrace => &["load", "open", "text", "file", "rew"],
            C::Sessions => &["project", "files"],
            C::SessionNew => &["project"],
            C::SessionLoad => &["project", "load"],
            C::SessionSave | C::SessionSaveAs => &["project", "store"],
            C::SessionExport => &["project", "archive", "ac2session", "share"],
            C::Reconnect => &["daemon", "connection"],
            C::OpenSession => &[
                "device",
                "interface",
                "soundcard",
                "jack",
                "asio",
                "start audio",
            ],
            C::CloseSession => &["device", "interface", "stop audio"],
            C::Record => &["capture", "wav", "raw"],
            C::ReplayRecording => &["playback", "wav", "file"],
            C::NewMeasurement => &["add", "create"],
            C::NewTransfer => &[
                "add",
                "tf",
                "transfer function",
                "frequency response",
                "phase",
                "dual channel",
            ],
            C::NewSpectrum => &["add", "fft"],
            C::NewRta => &["add", "octave", "bands"],
            C::NewSpl => &["add", "level", "leq", "sound level"],
            C::NewMath => &["add", "average", "sum", "difference"],
            C::EditMeas => &["settings", "configure", "change"],
            C::NextColour | C::PrevColour => &["color"],
            C::HideGroup => &["visible", "hide"],
            C::ToggleGroup => &["collapse", "expand"],
            C::InputSetup => &["inputs", "channels", "names"],
            C::Calibrations => &["calibration", "cal", "mic", "microphone", "sensitivity"],
            C::InputMics => &["microphone", "mic", "calibration"],
            C::MicCurve | C::MicCurveInput | C::TraceMicCurve => {
                &["microphone", "mic", "correction", "calibration"]
            }
            C::CalDelete => &["remove", "calibration", "cal"],
            C::SweepNew => &[
                "log sweep",
                "chirp",
                "distortion",
                "thd",
                "harmonics",
                "impulse",
                "IR",
            ],
            C::LeqWindows => &["windows", "limits", "SPL"],
            C::SplFast | C::SplSlow | C::SplImpulse => &["SPL", "time weighting"],
            C::SplA | C::SplC | C::SplZ => &["SPL", "frequency weighting", "dBA", "dBC", "dBZ"],
            C::ResetAverage => &["clear", "restart", "averages"],
            C::StartStop => &["pause", "run", "freeze"],
            C::InsertDelay | C::InsertStrongest => {
                &["delay", "align", "arrival", "impulse", "IR", "delay finder"]
            }
            C::TypeDelay => &["align", "time alignment"],
            C::TrackDelay => &["drift", "follow", "align"],
            C::DelayDown | C::DelayUp | C::DelayDownFine | C::DelayUpFine => &["align", "nudge"],
            C::NudgeEarlier | C::NudgeLater => &["align", "nudge"],
            C::FinderAuto
            | C::FinderFull
            | C::FinderMid
            | C::FinderSub
            | C::FinderCustom
            | C::FinderObservation => &["delay finder", "band", "align"],
            C::Invert => &["polarity", "flip", "phase"],
            C::Offset => &["gain", "level"],
            C::PhaseReference => &["phase"],
            C::EqEditor => &["equaliser", "equalizer", "filter", "what-if", "predict"],
            C::Target => &["curve", "house curve", "reference"],
            C::CoherenceMask => &["coherence", "blank", "gamma"],
            C::CoherencePlacement => &["coherence", "overlay", "gamma"],
            C::LegendTopLeft | C::LegendTopRight | C::LegendBottomLeft | C::LegendBottomRight => {
                &["labels", "position"]
            }
            C::LegendHide => &["hide", "labels"],
            C::Average | C::AverageComplex => &["math", "combine", "mean"],
            C::AverageCoherence => &["math", "combine", "mean", "coherence"],
            C::MinMax => &["min", "max", "peak", "dip", "notch", "comb", "extremes"],
            C::PhaseView => &["unwrap", "unwrapped", "wrapped", "group delay"],
            C::SmoothCoarser
            | C::SmoothFiner
            | C::SmoothOff
            | C::Smooth48
            | C::Smooth24
            | C::Smooth12
            | C::Smooth6
            | C::Smooth3 => &["octave", "resolution"],
            C::SpectrumStyle => &["line", "fill", "bars"],
            C::PeakHold => &["peak", "hold", "max hold", "tracking", "top 5", "max"],
            C::SpectrographSpan => &["waterfall", "history", "time"],
            C::SpectrographColours
            | C::SpectrographViridis
            | C::SpectrographMagma
            | C::SpectrographInferno
            | C::SpectrographCividis
            | C::SpectrographGrey => &["spectrograph", "waterfall", "colormap", "color"],
            C::NextView | C::PrevView => &["spectrograph", "waterfall", "impulse", "IR"],
            C::SplShowMeter => &["SPL", "level"],
            C::SplShowLeq => &["SPL", "history"],
            C::SplShowMeterLeq => &["SPL"],
            C::SplShowBands => &["SPL", "octave", "bands", "1/3"],
            C::SplLeqStyle => &["SPL"],
            C::SplLeqHistory => &["SPL", "graph", "trend"],
            C::SplNewLog => &["SPL", "Leq", "restart"],
            C::SplLogExport | C::SplLogExportPrevious => &["SPL", "csv", "save"],
            C::SplTimeWeighting => &["SPL", "fast", "slow", "impulse"],
            C::SplWeighting => &["SPL", "dBA", "dBC", "dBZ"],
            C::DistortionUnit => &["thd", "harmonics", "percent"],
            C::SweepIr => &["impulse", "IR"],
            C::ReferenceHarmonics => &["thd", "distortion"],
        }
    }
}
