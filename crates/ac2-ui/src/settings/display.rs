//! The Display page: its lines, the steps ←/→ take through each choice, and the text every
//! line shows for the value it holds.

use ac2_scene::theme::ThemeName;
use ac2_scene::view::{
    CoherencePlacement, DistortionUnit, IrMode, LeqLayout, LeqStyle, SpectrumMode, SweepMode,
};

/// A line of the Display page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayRow {
    Theme,
    KeyHints,
    WarningToasts,
    SplHold,
    EmberDecay,
    EmberGlow,
    LeqStyle,
    LeqHistory,
    LeqBars,
    LeqScale,
    SpectrumView,
    SpectrumGlide,
    Quality3d,
    SpikeSpacing3d,
    InvertDrag3d,
    SweepView,
    DistortionUnit,
    Coherence,
    ResolutionMarker,
    RangeBand,
    SimHints,
    PlateBackground,
    CursorJump,
    LevelAxes,
    LevelFollow,
    Advice,
    AdviceRefHeadroom,
    AdviceLowCoherence,
    AdviceLowLevel,
}

impl DisplayRow {
    pub const ALL: [DisplayRow; 29] = [
        DisplayRow::Theme,
        DisplayRow::KeyHints,
        DisplayRow::WarningToasts,
        DisplayRow::SplHold,
        DisplayRow::EmberDecay,
        DisplayRow::EmberGlow,
        DisplayRow::LeqStyle,
        DisplayRow::LeqHistory,
        DisplayRow::LeqBars,
        DisplayRow::LeqScale,
        DisplayRow::SpectrumView,
        DisplayRow::SpectrumGlide,
        DisplayRow::Quality3d,
        DisplayRow::SpikeSpacing3d,
        DisplayRow::InvertDrag3d,
        DisplayRow::SweepView,
        DisplayRow::DistortionUnit,
        DisplayRow::Coherence,
        DisplayRow::ResolutionMarker,
        DisplayRow::RangeBand,
        DisplayRow::SimHints,
        DisplayRow::PlateBackground,
        DisplayRow::CursorJump,
        DisplayRow::LevelAxes,
        DisplayRow::LevelFollow,
        DisplayRow::Advice,
        DisplayRow::AdviceRefHeadroom,
        DisplayRow::AdviceLowCoherence,
        DisplayRow::AdviceLowLevel,
    ];

    pub fn title(self) -> &'static str {
        match self {
            DisplayRow::Theme => "Theme",
            DisplayRow::KeyHints => "Key hints",
            DisplayRow::WarningToasts => "Warning toasts",
            DisplayRow::SplHold => "SPL number holds",
            DisplayRow::EmberDecay => ac2_scene::programme::form::EMBER_TITLE,
            DisplayRow::EmberGlow => ac2_scene::programme::form::GLOW_TITLE,
            DisplayRow::LeqStyle => "Leq windows",
            DisplayRow::LeqHistory => "Leq history strip",
            DisplayRow::LeqBars => "Leq bars",
            DisplayRow::LeqScale => "Shared scale",
            DisplayRow::SpectrumView => "Spectrum pane shows",
            DisplayRow::SpectrumGlide => "Spectrum",
            DisplayRow::Quality3d => "3D quality",
            DisplayRow::SpikeSpacing3d => "3D spike spacing",
            DisplayRow::InvertDrag3d => "Invert vertical drag",
            DisplayRow::SweepView => "Sweep pane shows",
            DisplayRow::DistortionUnit => "Distortion in",
            DisplayRow::Coherence => "Coherence",
            DisplayRow::ResolutionMarker => "Resolution marker",
            DisplayRow::RangeBand => "Range across positions",
            DisplayRow::SimHints => "Simulation overlays",
            DisplayRow::PlateBackground => "Plate background",
            DisplayRow::CursorJump => "Peak / notch jumps",
            DisplayRow::LevelAxes => "Level axes",
            DisplayRow::LevelFollow => "Level axis follows",
            DisplayRow::Advice => "Headroom advice",
            DisplayRow::AdviceRefHeadroom => "Advise lowering at",
            DisplayRow::AdviceLowCoherence => "Advise raising at",
            DisplayRow::AdviceLowLevel => "… and",
        }
    }
}

/// The SPL number's hold choices, ms (`None`: by the meter's time weighting).
pub const SPL_HOLDS_MS: [Option<u32>; 5] = [None, Some(250), Some(500), Some(1000), Some(2000)];

/// The next theme ←/→ choose.
pub fn step_theme(t: ThemeName, d: i32) -> ThemeName {
    const ALL: [ThemeName; 3] = [ThemeName::Dark, ThemeName::Light, ThemeName::HighContrast];
    let i = ALL.iter().position(|x| *x == t).unwrap_or(0) as i32;
    ALL[(i + d).rem_euclid(3) as usize]
}

pub fn theme_name(t: ThemeName) -> &'static str {
    match t {
        ThemeName::Dark => "dark",
        ThemeName::Light => "light",
        ThemeName::HighContrast => "high contrast (sunlight)",
    }
}

/// One ←/→ step of the goniometer's ember decay, seconds.
const EMBER_STEP_S: f32 = 0.1;

/// The ember decay one step from `s`, on the 0.1 s grid, within
/// [`ac2_scene::gonio::EMBER_DECAY_RANGE_S`].
pub fn step_ember_decay(s: f32, d: i32) -> f32 {
    let steps = (ac2_scene::gonio::clamp_decay(s) / EMBER_STEP_S).round() + d.signum() as f32;
    ac2_scene::gonio::clamp_decay(steps * EMBER_STEP_S)
}

/// Every ember decay ←/→ reaches: the [`EMBER_STEP_S`] grid across
/// [`ac2_scene::gonio::EMBER_DECAY_RANGE_S`], in `f32` as the setting is kept, so a stored
/// step compares equal to its entry.
pub fn ember_decay_steps() -> Vec<f32> {
    let (lo, hi) = ac2_scene::gonio::EMBER_DECAY_RANGE_S;
    let (first, last) = (
        (lo / EMBER_STEP_S).round() as i32,
        (hi / EMBER_STEP_S).round() as i32,
    );
    // The grid `step_ember_decay` rounds to: the same product, so the same `f32`.
    (first..=last)
        .map(|k| ac2_scene::gonio::clamp_decay(k as f32 * EMBER_STEP_S))
        .collect()
}

/// `0.4 s: a point fades out that long after it lands`.
pub fn ember_decay_text(s: f32) -> String {
    format!(
        "{} s: a goniometer point fades out that long after it lands",
        ac2_scene::format::fixed(f64::from(ac2_scene::gonio::clamp_decay(s)), 1)
    )
}

/// The ember glows ←/→ step through: closer at the dim end, where a quarter step already
/// shows, and doubling at the hot end, where brightness clips at full and only the longer
/// tail tells steps apart.
const EMBER_GLOWS: [f32; 7] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];

/// The ember glow one step from `g` within [`EMBER_GLOWS`]; a stored value between steps
/// moves to the next step its way.
pub fn step_ember_glow(g: f32, d: i32) -> f32 {
    let g = ac2_scene::gonio::clamp_glow(g);
    let next = match d.signum() {
        1 => EMBER_GLOWS.iter().find(|s| **s > g),
        -1 => EMBER_GLOWS.iter().rev().find(|s| **s < g),
        _ => None,
    };
    next.copied().unwrap_or(g)
}

/// Every ember glow ←/→ reaches.
pub fn ember_glow_steps() -> Vec<f32> {
    EMBER_GLOWS.to_vec()
}

/// `×2: a goniometer point lands that many times as bright`.
pub fn ember_glow_text(g: f32) -> String {
    let n = ac2_scene::format::fixed(f64::from(ac2_scene::gonio::clamp_glow(g)), 2);
    let n = n.trim_end_matches('0').trim_end_matches('.');
    format!("×{n}: a goniometer point lands that many times as bright")
}

pub fn step_hold(h: Option<u32>, d: i32) -> Option<u32> {
    let i = SPL_HOLDS_MS.iter().position(|x| *x == h).unwrap_or(0) as i32;
    SPL_HOLDS_MS[(i + d).clamp(0, SPL_HOLDS_MS.len() as i32 - 1) as usize]
}

/// What the spectrum pane's view is called (G steps it).
pub fn spectrum_view_name(m: SpectrumMode) -> &'static str {
    match m {
        SpectrumMode::Spectrum => "the spectrum",
        SpectrumMode::Split => "the spectrum over its spectrograph",
        SpectrumMode::Spectrograph => "the spectrograph",
        SpectrumMode::Spectrograph3d => "the 3D spectrograph",
    }
}

/// What the sweep pane's view is called (G steps it).
pub fn sweep_view_name(m: SweepMode) -> &'static str {
    match m {
        SweepMode::Response => "response and distortion",
        SweepMode::Ir(IrMode::Linear) => "the impulse response",
        SweepMode::Ir(IrMode::Log) => "the impulse response in dB",
        SweepMode::Ir(IrMode::Etc) => "the energy-time curve",
        SweepMode::Room => "the room parameters",
    }
}

/// The views of the panes the Display page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneViews {
    pub spectrum: SpectrumMode,
    pub sweep: SweepMode,
    pub leq: LeqLayout,
    pub distortion_unit: DistortionUnit,
    pub coherence: CoherencePlacement,
}

/// What the Warning toasts setting says, on the Display page and in its toast.
pub fn warning_toasts_text(on: bool) -> &'static str {
    if on {
        "on: warnings and Leq limit alarms pop up in the corner"
    } else {
        "off: warnings and Leq limit alarms go only to the notification log"
    }
}

/// What the Range across positions setting says, on the Display page and in its toast.
pub fn range_band_text(on: bool) -> &'static str {
    if on {
        "on: shades how far an average's positions spread"
    } else {
        "off"
    }
}

/// What the Simulation overlays setting says, on the Display page and in its toast.
pub fn sim_hints_text(on: bool) -> &'static str {
    if on {
        "on: simulated reflections on IRs, coverage under walks"
    } else {
        "off"
    }
}

/// What the Resolution marker setting says, on the Display page and in its toast.
pub fn resolution_marker_text(on: bool) -> &'static str {
    if on {
        "on: shades where a curve is coarser than its resolution"
    } else {
        "off"
    }
}

/// What the Plate background setting says, on the Display page and in its toast.
pub fn plate_background_text(bg: ac2_scene::plate::PlateBackground) -> &'static str {
    match bg {
        ac2_scene::plate::PlateBackground::SeeThrough => {
            "see-through: the curves show under the plates"
        }
        ac2_scene::plate::PlateBackground::Solid => "solid: the plot's background, opaque",
    }
}

/// What the Leq bars setting says, on the Display page and in its toast.
pub fn bar_ease_text(on: bool) -> &'static str {
    if on {
        "glide to each new reading"
    } else {
        "jump to each new reading"
    }
}

/// What the shared scale setting says, on the Display page and in its toast: the SPL panes'
/// Leq columns, and the spectrum panes whose level axis follows the content, per unit.
pub fn shared_scale_text(on: bool) -> &'static str {
    if on {
        "SPL panes and following spectrum panes on screen share one per unit"
    } else {
        "each pane on its own"
    }
}

/// What the level follow setting says, on the Display page and in its toast.
pub fn level_follow_text(on: bool) -> &'static str {
    if on {
        "the content: a spectrum's automatic range moves with it (a fit re-arms it)"
    } else {
        "nothing: a spectrum's range stays where it was framed"
    }
}

/// What the spectrum glide setting says, on the Display page and in its toast.
pub fn spec_ease_text(on: bool) -> &'static str {
    if on {
        "glides to each new frame"
    } else {
        "jumps to each new frame"
    }
}

/// What the 3D view's vertical drag setting says, on the Display page and in its toast.
pub fn invert_drag_3d_text(on: bool) -> &'static str {
    if on {
        "on: in 3D, drag down to look from higher"
    } else {
        "off: in 3D, drag up to look from higher"
    }
}

/// The 3D quality line on a GPU too slow for any tier.
pub const QUALITY_3D_UNAVAILABLE: &str = "unavailable on this GPU";

/// What the 3D quality setting says, on the Display page and in its toast: by the GPU
/// (naming what it chose here) or as set; on a GPU without the 3D view (`auto` `None`),
/// that, whatever is set.
pub fn quality_3d_text(
    q: ac2_scene::spectrograph_3d::Quality,
    auto: Option<ac2_scene::primitives::SkylineTier>,
) -> String {
    let Some(auto) = auto else {
        return QUALITY_3D_UNAVAILABLE.into();
    };
    use ac2_scene::spectrograph_3d::{Quality, tier_name};
    let what = |t| match t {
        ac2_scene::primitives::SkylineTier::Full => "full: finest grid, full-resolution spikes",
        ac2_scene::primitives::SkylineTier::Lite => "lite: coarser grid, half-resolution spikes",
        ac2_scene::primitives::SkylineTier::Bare => "bare: as lite, never a glow",
    };
    match q {
        Quality::Auto => format!("by the GPU ({} here)", tier_name(auto)),
        Quality::Full => what(ac2_scene::primitives::SkylineTier::Full).into(),
        Quality::Lite => what(ac2_scene::primitives::SkylineTier::Lite).into(),
        Quality::Bare => what(ac2_scene::primitives::SkylineTier::Bare).into(),
    }
}

/// What the 3D spike spacing setting says, on the Display page and in its toast: the step
/// and, off the normal one, how many spikes it draws; a tighter step costs GPU time.
pub fn spike_spacing_text(s: ac2_scene::spectrograph_3d::SpikeSpacing) -> &'static str {
    use ac2_scene::spectrograph_3d::SpikeSpacing;
    match s {
        SpikeSpacing::Tightest => "tightest: 4× the spikes, more GPU time",
        SpikeSpacing::Tight => "tight: 2× the spikes, more GPU time",
        SpikeSpacing::Normal => "normal",
        SpikeSpacing::Loose => "loose: ½ the spikes",
        SpikeSpacing::Loosest => "loosest: ¼ the spikes",
    }
}

/// The Display page's on / off lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplaySwitches {
    pub key_hints: bool,
    pub warning_toasts: bool,
    pub resolution_marker: bool,
    pub range_band: bool,
    pub sim_hints: bool,
    pub bar_ease: bool,
    pub shared_scale: bool,
    pub level_follow: bool,
    pub spec_ease: bool,
    pub quality_3d: ac2_scene::spectrograph_3d::Quality,
    pub spike_spacing: ac2_scene::spectrograph_3d::SpikeSpacing,
    pub invert_drag_3d: bool,
    /// The tier the GPU gets when the quality is auto.
    pub auto_tier: Option<ac2_scene::primitives::SkylineTier>,
    pub plate_background: ac2_scene::plate::PlateBackground,
}

/// What the Display page shows for each line: title, value, whose.
pub fn display_rows(
    theme: ThemeName,
    switches: DisplaySwitches,
    spl_hold_ms: Option<u32>,
    embers: ac2_scene::gonio::Cooling,
    views: PaneViews,
    jump_prominence_db: f64,
    advice: ac2_scene::advice::AdviceSettings,
) -> Vec<(DisplayRow, String)> {
    DisplayRow::ALL
        .iter()
        .map(|r| {
            let v = match r {
                DisplayRow::Theme => theme_name(theme).to_owned(),
                DisplayRow::KeyHints => {
                    if switches.key_hints {
                        "shown under the focused pane".into()
                    } else {
                        "hidden".into()
                    }
                }
                DisplayRow::WarningToasts => warning_toasts_text(switches.warning_toasts).into(),
                DisplayRow::SplHold => match spl_hold_ms {
                    None => "by the time weighting".into(),
                    Some(ms) => {
                        format!("{} s", ac2_scene::format::fixed(f64::from(ms) / 1e3, 2))
                    }
                },
                DisplayRow::EmberDecay => ember_decay_text(embers.decay_s()),
                DisplayRow::EmberGlow => ember_glow_text(embers.glow()),
                DisplayRow::LeqStyle => match views.leq.style {
                    LeqStyle::Columns => "columns".into(),
                    LeqStyle::Tiles => "tiles".into(),
                },
                DisplayRow::LeqHistory => {
                    if views.leq.history {
                        "shown under the windows".into()
                    } else {
                        "hidden".into()
                    }
                }
                DisplayRow::LeqBars => bar_ease_text(switches.bar_ease).into(),
                DisplayRow::LeqScale => shared_scale_text(switches.shared_scale).into(),
                DisplayRow::SpectrumGlide => spec_ease_text(switches.spec_ease).into(),
                DisplayRow::Quality3d => quality_3d_text(switches.quality_3d, switches.auto_tier),
                DisplayRow::SpikeSpacing3d => spike_spacing_text(switches.spike_spacing).into(),
                DisplayRow::InvertDrag3d => invert_drag_3d_text(switches.invert_drag_3d).into(),
                DisplayRow::DistortionUnit => match views.distortion_unit {
                    DistortionUnit::Db => "dB re fundamental".into(),
                    DistortionUnit::Percent => "% of fundamental".into(),
                },
                DisplayRow::Coherence => match views.coherence {
                    CoherencePlacement::Pane => "its own view (G)".into(),
                    CoherencePlacement::OverlayOnMagnitude => "over the magnitude".into(),
                },
                DisplayRow::ResolutionMarker => {
                    resolution_marker_text(switches.resolution_marker).into()
                }
                DisplayRow::RangeBand => range_band_text(switches.range_band).into(),
                DisplayRow::SimHints => sim_hints_text(switches.sim_hints).into(),
                DisplayRow::PlateBackground => {
                    plate_background_text(switches.plate_background).into()
                }
                DisplayRow::CursorJump => {
                    ac2_scene::cursor_jump::prominence_text(jump_prominence_db)
                }
                DisplayRow::SpectrumView => spectrum_view_name(views.spectrum).to_owned(),
                DisplayRow::SweepView => sweep_view_name(views.sweep).to_owned(),
                DisplayRow::LevelAxes => {
                    "each pane's as last left · Enter resets them to the defaults".into()
                }
                DisplayRow::LevelFollow => level_follow_text(switches.level_follow).into(),
                DisplayRow::Advice => ac2_scene::advice::on_text(advice.on).into(),
                DisplayRow::AdviceRefHeadroom => {
                    ac2_scene::advice::ref_headroom_text(advice.ref_headroom_db)
                }
                DisplayRow::AdviceLowCoherence => {
                    ac2_scene::advice::low_coherence_text(advice.low_coherence)
                }
                DisplayRow::AdviceLowLevel => {
                    ac2_scene::advice::low_level_text(advice.low_level_dbfs)
                }
            };
            (*r, v)
        })
        .collect()
}
