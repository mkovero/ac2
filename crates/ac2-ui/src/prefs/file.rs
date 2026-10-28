//! The `ui.toml` file: its tables and how they read into and write from [`UiPrefs`].

use super::layout::*;
use super::rig_file::RigFile;
use super::*;
use ac2_scene::plate::PlateBackground;

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    /// Written only when off (the default is on). First: plain values precede tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_hints: Option<bool>,
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    warning_toasts: Option<bool>,
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution_marker: Option<bool>,
    /// Written only when solid (the default is see-through).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plate_background: Option<PlateBackgroundFile>,
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bar_ease: Option<bool>,
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared_scale: Option<bool>,
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spec_ease: Option<bool>,
    /// Written only when not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jump_prominence_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spl_hold_ms: Option<u32>,
    /// Written only when not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ember_decay_s: Option<f32>,
    /// Written only when not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ember_glow: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<ThemeFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    record_limit_min: Option<u32>,
    /// One-based channels per `<rig id>/<device id>` ([`rig_key`]).
    #[serde(default)]
    stimulus_outputs: BTreeMap<String, Vec<u32>>,
    /// Per `<rig id>/<backend>/<device id>` ([`rig_key`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sessions: BTreeMap<String, RolesFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leq: Option<LeqFile>,
    /// Written only when not the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    advice: Option<AdviceFile>,
    /// A `[layout]` that does not parse as a pane tree is dropped (with a warning in the
    /// log): the panes are a convenience, so the app starts with one pane instead of
    /// refusing the hold times, sessions and devices the rest of the file keeps.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_layout"
    )]
    layout: Option<LayoutFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ir_levels: Option<IrLevelsFile>,
    /// A `[legend]` that does not parse is dropped (with a warning in the log), as the
    /// layout is: a legend shown again costs a key, a whole file set aside costs much more.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_legend"
    )]
    legend: Option<LegendsFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<WindowFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recent_sessions: Vec<RecentSession>,
    /// The rigs held when the app last ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rigs: Vec<RigFile>,
    /// Top-level keys this version does not know: logged and left out of the next save, so
    /// one stale or mistyped setting costs only itself, never the rest of the file.
    #[serde(flatten, skip_serializing)]
    unknown: BTreeMap<String, toml::Value>,
}

/// Legends per pane; only the transfer pane's moves.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegendsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transfer: Option<LegendFile>,
}

/// A legend's choices; one left out is the default.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegendFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hidden: Option<bool>,
}

fn lenient_legend<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<LegendsFile>, D::Error> {
    let v = toml::Value::deserialize(d)?;
    match LegendsFile::deserialize(v) {
        Ok(l) => Ok(Some(l)),
        Err(e) => {
            tracing::warn!("ui.toml: [legend] dropped, the legend shown: {e}");
            Ok(None)
        }
    }
}

impl LegendFile {
    fn parse(&self) -> LegendPrefs {
        LegendPrefs {
            hidden: self.hidden.unwrap_or(false),
        }
    }

    /// Only the choices moved off their defaults.
    fn from_prefs(l: &LegendPrefs) -> Option<LegendsFile> {
        l.hidden.then_some(LegendsFile {
            transfer: Some(Self { hidden: Some(true) }),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PlateBackgroundFile {
    SeeThrough,
    Solid,
}

impl PlateBackgroundFile {
    fn background(self) -> PlateBackground {
        match self {
            Self::SeeThrough => PlateBackground::SeeThrough,
            Self::Solid => PlateBackground::Solid,
        }
    }

    fn of(b: PlateBackground) -> Self {
        match b {
            PlateBackground::SeeThrough => Self::SeeThrough,
            PlateBackground::Solid => Self::Solid,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ThemeFile {
    Dark,
    Light,
    HighContrast,
}

impl ThemeFile {
    fn name(self) -> ac2_scene::theme::ThemeName {
        use ac2_scene::theme::ThemeName as T;
        match self {
            Self::Dark => T::Dark,
            Self::Light => T::Light,
            Self::HighContrast => T::HighContrast,
        }
    }

    fn of(t: ac2_scene::theme::ThemeName) -> Self {
        use ac2_scene::theme::ThemeName as T;
        match t {
            T::Dark => Self::Dark,
            T::Light => Self::Light,
            T::HighContrast => Self::HighContrast,
        }
    }
}

/// `[low, high]` dB as a range: finite, low below high, within what the axis can show.
fn level_range([lo, hi]: [f64; 2]) -> Option<Range> {
    (lo.is_finite() && hi.is_finite() && lo < hi).then(|| level::clamp(Range::new(lo, hi)))
}

/// `[ir_levels]`: the impulse-response views' dB axes, `[low, high]` dB re peak; one left
/// out is the default.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IrLevelsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transfer: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sweep: Option<[f64; 2]>,
}

impl IrLevelsFile {
    fn parse(&self) -> Result<IrLevelPrefs, String> {
        let d = IrLevelPrefs::default();
        let range = |name: &str, v: Option<[f64; 2]>, default: Range| match v {
            None => Ok(default),
            Some(v) => level_range(v).ok_or_else(|| {
                format!("ui.toml: ir_levels.{name} must be [low, high] in dB, low below high")
            }),
        };
        Ok(IrLevelPrefs {
            transfer: range("transfer", self.transfer, d.transfer)?,
            sweep: range("sweep", self.sweep, d.sweep)?,
        })
    }

    /// Only the ranges moved off their defaults.
    fn from_prefs(l: &IrLevelPrefs) -> Option<Self> {
        let d = IrLevelPrefs::default();
        let v = |r: Range, default: Range| (r != default).then_some([r.lo, r.hi]);
        let f = Self {
            transfer: v(l.transfer, d.transfer),
            sweep: v(l.sweep, d.sweep),
        };
        (*l != d).then_some(f)
    }
}

/// A pane's level axes the operator set (`[layout.panes.levels]` under its
/// `[[layout.panes]]` entry), `[low, high]` dB; one left out is framed when the pane first
/// shows a curve.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PaneLevelsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transfer: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spectrum_dbfs: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spectrum_spl: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    distortion: Option<[f64; 2]>,
}

impl PaneLevelsFile {
    /// The ranges; one that is no range is left out, logged (a layout is a convenience,
    /// read leniently).
    pub(super) fn parse(f: Option<&Self>, pane: u32) -> LayoutLevels {
        let Some(f) = f else {
            return LayoutLevels::default();
        };
        let range = |name: &str, v: Option<[f64; 2]>| {
            let v = v?;
            let r = level_range(v);
            if r.is_none() {
                tracing::warn!(
                    "ui.toml: pane {pane}'s levels.{name} is not [low, high] in dB; left out"
                );
            }
            r.map(|r| LevelRange { lo: r.lo, hi: r.hi })
        };
        LayoutLevels {
            transfer: range("transfer", f.transfer),
            spectrum_dbfs: range("spectrum_dbfs", f.spectrum_dbfs),
            spectrum_spl: range("spectrum_spl", f.spectrum_spl),
            distortion: range("distortion", f.distortion),
        }
    }

    /// The ranges set; `None` when there are none.
    pub(super) fn of(l: &LayoutLevels) -> Option<Self> {
        let v = |r: Option<LevelRange>| r.map(|r| [r.lo, r.hi]);
        (!l.is_empty()).then(|| Self {
            transfer: v(l.transfer),
            spectrum_dbfs: v(l.spectrum_dbfs),
            spectrum_spl: v(l.spectrum_spl),
            distortion: v(l.distortion),
        })
    }
}

/// Where a pane's plates were dropped and their text size (`[layout.panes.plates.readout]`
/// and so on); one left out sits where the scene puts it.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PanePlatesFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legend: Option<PlateFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    readout: Option<PlateFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mark: Option<PlateFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    peaks: Option<PlateFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loudness: Option<PlateFile>,
}

/// One plate: its place, fractions `[x, y]` of the room beside it, and its text scale.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlateFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scale: Option<f64>,
}

impl PanePlatesFile {
    /// The plates; a place or scale out of bounds is left out, logged (a layout is a
    /// convenience, read leniently).
    pub(super) fn parse(f: Option<&Self>, pane: u32) -> Plates {
        let mut out = Plates::default();
        let Some(f) = f else {
            return out;
        };
        let scales = ac2_scene::plate::SCALE_LIMITS;
        for (kind, file) in [
            (PlateKind::Legend, &f.legend),
            (PlateKind::Readout, &f.readout),
            (PlateKind::Mark, &f.mark),
            (PlateKind::Peaks, &f.peaks),
            (PlateKind::Loudness, &f.loudness),
        ] {
            let Some(file) = file else { continue };
            let name = format!("{kind:?}").to_lowercase();
            let p = out.get_mut(kind);
            match file.at {
                Some([x, y]) if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) => {
                    p.move_to(x as f32, y as f32);
                }
                Some(_) => tracing::warn!(
                    "ui.toml: pane {pane}'s plates.{name}.at is not [x, y] within 0 … 1; left out"
                ),
                None => {}
            }
            match file.scale {
                Some(s) if scales.contains(&(s as f32)) => p.set_scale(s as f32),
                Some(_) => tracing::warn!(
                    "ui.toml: pane {pane}'s plates.{name}.scale is not {} … {}; left out",
                    scales.start(),
                    scales.end()
                ),
                None => {}
            }
        }
        out
    }

    /// The plates moved or scaled, to three decimals (a drag's fraction has no meaningful
    /// digits beyond a pixel); `None` when every one sits where the scene puts it.
    pub(super) fn of(p: &Plates) -> Option<Self> {
        let r = |v: f32| (f64::from(v) * 1000.0).round() / 1000.0;
        let one = |p: &PlatePlace| {
            (!p.is_auto()).then(|| PlateFile {
                at: p.at.map(|[x, y]| [r(x), r(y)]),
                scale: (p.scale() != 1.0).then(|| r(p.scale())),
            })
        };
        (!p.is_auto()).then(|| Self {
            legend: one(&p.legend),
            readout: one(&p.readout),
            mark: one(&p.mark),
            peaks: one(&p.peaks),
            loudness: one(&p.loudness),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PaneFile {
    Transfer,
    Spectrum,
    Spl,
    Distortion,
    Programme,
    Walk,
}

impl PaneFile {
    pub(super) fn of(p: PaneKind) -> Self {
        match p {
            PaneKind::Transfer => Self::Transfer,
            PaneKind::Spectrum => Self::Spectrum,
            PaneKind::Spl => Self::Spl,
            PaneKind::Distortion => Self::Distortion,
            PaneKind::Programme => Self::Programme,
            PaneKind::Walk => Self::Walk,
        }
    }

    pub(super) fn pane(self) -> PaneKind {
        match self {
            Self::Transfer => PaneKind::Transfer,
            Self::Spectrum => PaneKind::Spectrum,
            Self::Spl => PaneKind::Spl,
            Self::Distortion => PaneKind::Distortion,
            Self::Programme => PaneKind::Programme,
            Self::Walk => PaneKind::Walk,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SplViewFile {
    Meter,
    Leq,
    MeterLeq,
    Bands,
}

/// A Programme pane's view; absent (an app layout from before the views) reads Meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProgrammeViewFile {
    #[default]
    Meters,
    Goniometer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SpectrumViewFile {
    #[default]
    Spectrum,
    SpectrumSpectrograph,
    Spectrograph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TransferViewFile {
    #[default]
    Magnitude,
    Phase,
    Coherence,
    Alignment,
    IrLinear,
    IrLog,
    Etc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SweepViewFile {
    #[default]
    Response,
    IrLinear,
    IrLog,
    Etc,
    Room,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum UnitFile {
    Db,
    Percent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ChromeFile {
    #[default]
    Full,
    NoGrid,
    TracesOnly,
}

impl ChromeFile {
    pub(super) fn of(c: PlotChrome) -> Self {
        match c {
            PlotChrome::Full => Self::Full,
            PlotChrome::NoGrid => Self::NoGrid,
            PlotChrome::Bare => Self::TracesOnly,
        }
    }

    pub(super) fn chrome(self) -> PlotChrome {
        match self {
            Self::Full => PlotChrome::Full,
            Self::NoGrid => PlotChrome::NoGrid,
            Self::TracesOnly => PlotChrome::Bare,
        }
    }

    pub(super) fn is_full(&self) -> bool {
        *self == Self::Full
    }
}

pub(super) fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowFile {
    width: u32,
    height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    y: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StyleFile {
    Columns,
    Tiles,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeqFile {
    style: StyleFile,
    #[serde(default)]
    history: bool,
}

/// The headroom advice; a missing key keeps its default.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdviceFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    on: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ref_headroom_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    low_coherence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    low_level_dbfs: Option<f64>,
}

impl AdviceFile {
    fn parse(&self) -> Result<ac2_scene::advice::AdviceSettings, String> {
        use ac2_scene::advice::{AdviceSettings, LOW_COHERENCE, LOW_LEVEL_DBFS, REF_HEADROOM_DB};
        let d = AdviceSettings::default();
        let within = |key: &str, v: Option<f64>, choices: &[f64], default: f64| {
            let (lo, hi) = (choices[0], choices[choices.len() - 1]);
            match v {
                None => Ok(default),
                Some(v) if (lo..=hi).contains(&v) => Ok(v),
                Some(_) => Err(format!("ui.toml: advice.{key} must be {lo} … {hi}")),
            }
        };
        Ok(AdviceSettings {
            on: self.on.unwrap_or(d.on),
            ref_headroom_db: within(
                "ref_headroom_db",
                self.ref_headroom_db,
                &REF_HEADROOM_DB,
                d.ref_headroom_db,
            )?,
            low_coherence: within(
                "low_coherence",
                self.low_coherence,
                &LOW_COHERENCE,
                d.low_coherence,
            )?,
            low_level_dbfs: within(
                "low_level_dbfs",
                self.low_level_dbfs,
                &LOW_LEVEL_DBFS,
                d.low_level_dbfs,
            )?,
        })
    }

    fn from_prefs(a: &ac2_scene::advice::AdviceSettings) -> Option<Self> {
        let d = ac2_scene::advice::AdviceSettings::default();
        let changed = |v: f64, d: f64| ((v - d).abs() > 1e-9).then_some(v);
        let f = Self {
            on: (a.on != d.on).then_some(a.on),
            ref_headroom_db: changed(a.ref_headroom_db, d.ref_headroom_db),
            low_coherence: changed(a.low_coherence, d.low_coherence),
            low_level_dbfs: changed(a.low_level_dbfs, d.low_level_dbfs),
        };
        (*a != d).then_some(f)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RolesFile {
    #[serde(default)]
    inputs: Vec<u32>,
    #[serde(default)]
    outputs: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    spare_inputs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference: Option<u32>,
    #[serde(default)]
    mics: Vec<u32>,
    #[serde(default)]
    stimulus: Vec<u32>,
    /// One-based channel (as text: TOML keys are strings) → mic name.
    #[serde(default)]
    mic_names: BTreeMap<String, String>,
    /// One-based channel (as text) → kind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    kinds: BTreeMap<String, KindPref>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pairs: Vec<PairFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_device: Option<String>,
}

/// A left/right pair, one-based channels.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairFile {
    left: u32,
    right: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// One-based → zero-based.
fn zero(c: u32) -> Option<u16> {
    c.checked_sub(1).and_then(|c| u16::try_from(c).ok())
}

fn zeros(v: &[u32]) -> Option<Vec<u16>> {
    v.iter().map(|c| zero(*c)).collect()
}

fn ones(v: &[u16]) -> Vec<u32> {
    v.iter().map(|c| u32::from(*c) + 1).collect()
}

impl RolesFile {
    fn parse(&self) -> Option<DeviceRoles> {
        let mut mic_names = BTreeMap::new();
        for (k, v) in &self.mic_names {
            mic_names.insert(zero(k.trim().parse().ok()?)?, v.clone());
        }
        let mut kinds = BTreeMap::new();
        for (k, v) in &self.kinds {
            kinds.insert(zero(k.trim().parse().ok()?)?, *v);
        }
        let mut pairs = Vec::new();
        for p in &self.pairs {
            pairs.push(ac2_proto::model::Pair {
                left: zero(p.left)?,
                right: zero(p.right)?,
                name: p.name.clone(),
            });
        }
        Some(DeviceRoles {
            inputs: zeros(&self.inputs)?,
            outputs: u16::try_from(self.outputs).ok()?,
            spare_inputs: u16::try_from(self.spare_inputs).ok()?,
            reference: match self.reference {
                Some(r) => Some(zero(r)?),
                None => None,
            },
            mics: zeros(&self.mics)?,
            stimulus: zeros(&self.stimulus)?,
            mic_names,
            kinds,
            pairs,
            output_device: self.output_device.clone(),
        })
    }

    fn from_roles(r: &DeviceRoles) -> Self {
        Self {
            inputs: ones(&r.inputs),
            outputs: u32::from(r.outputs),
            spare_inputs: u32::from(r.spare_inputs),
            reference: r.reference.map(|c| u32::from(c) + 1),
            mics: ones(&r.mics),
            stimulus: ones(&r.stimulus),
            mic_names: r
                .mic_names
                .iter()
                .map(|(c, n)| ((u32::from(*c) + 1).to_string(), n.clone()))
                .collect(),
            kinds: r
                .kinds
                .iter()
                .map(|(c, k)| ((u32::from(*c) + 1).to_string(), *k))
                .collect(),
            pairs: r
                .pairs
                .iter()
                .map(|p| PairFile {
                    left: u32::from(p.left) + 1,
                    right: u32::from(p.right) + 1,
                    name: p.name.clone(),
                })
                .collect(),
            output_device: r.output_device.clone(),
        }
    }
}

impl UiPrefs {
    /// The outputs remembered for `device` of `rig`.
    pub fn outputs_for(&self, rig: RigId, device: &str) -> Option<&[u16]> {
        self.outputs
            .get(&(rig, device.to_owned()))
            .map(Vec::as_slice)
    }

    /// The session dialog choices remembered for `key` ([`Self::device_key`]) of `rig`.
    pub fn roles_for(&self, rig: RigId, key: &str) -> Option<&DeviceRoles> {
        self.sessions.get(&(rig, key.to_owned()))
    }

    /// What is remembered for `rig`'s devices.
    pub fn of_rig(&self, rig: RigId) -> RigPrefs<'_> {
        RigPrefs { prefs: self, rig }
    }

    /// Key of a device in [`UiPrefs::sessions`]: `fake/fake:loop`, `coreaudio/hw:UMC1820`.
    pub fn device_key(backend: ac2_proto::model::BackendKind, device: &str) -> String {
        use ac2_proto::model::BackendKind as B;
        let b = match backend {
            B::Jack => "jack",
            B::CoreAudio => "coreaudio",
            B::Fake => "fake",
            B::Asio => "asio",
            B::Replay => "replay",
        };
        format!("{b}/{device}")
    }

    /// Parses `ui.toml`.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let f: File = toml::from_str(text).map_err(|e| format!("ui.toml: {e}"))?;
        for key in f.unknown.keys() {
            tracing::warn!("ui.toml: unknown setting `{key}` ignored");
        }
        let mut outputs = BTreeMap::new();
        for (device, chans) in f.stimulus_outputs {
            match zeros(&chans) {
                Some(z) if !z.is_empty() => {
                    if let Some(k) = parse_rig_key(&device) {
                        outputs.insert(k, z);
                    }
                }
                _ => {
                    return Err(format!(
                        "ui.toml: stimulus outputs of {device:?} must be channels from 1"
                    ));
                }
            }
        }
        let mut sessions = BTreeMap::new();
        for (device, r) in f.sessions {
            let roles = r.parse().ok_or_else(|| {
                format!("ui.toml: session choices of {device:?} must be channels from 1")
            })?;
            if let Some(k) = parse_rig_key(&device) {
                sessions.insert(k, roles);
            }
        }
        let leq = f.leq.map_or_else(LeqLayout::default, |l| LeqLayout {
            style: match l.style {
                StyleFile::Columns => LeqStyle::Columns,
                StyleFile::Tiles => LeqStyle::Tiles,
            },
            history: l.history,
        });
        if let Some(ms) = f.spl_hold_ms
            && !SPL_HOLD_MS.contains(&ms)
        {
            return Err(format!(
                "ui.toml: spl_hold_ms must be {} … {}",
                SPL_HOLD_MS.start(),
                SPL_HOLD_MS.end()
            ));
        }
        if let Some(m) = f.record_limit_min
            && !RECORD_LIMIT_MIN.contains(&m)
        {
            return Err(format!(
                "ui.toml: record_limit_min must be {} … {}",
                RECORD_LIMIT_MIN.start(),
                RECORD_LIMIT_MIN.end()
            ));
        }
        let window = match f.window {
            Some(w) if w.width >= WindowPrefs::MIN.0 && w.height >= WindowPrefs::MIN.1 => {
                Some(WindowPrefs {
                    width: w.width,
                    height: w.height,
                    pos: w.x.zip(w.y),
                })
            }
            Some(_) => {
                return Err(format!(
                    "ui.toml: the window must be at least {} × {}",
                    WindowPrefs::MIN.0,
                    WindowPrefs::MIN.1
                ));
            }
            None => None,
        };
        let rigs = f.rigs.into_iter().filter_map(RigFile::rig).collect();
        Ok(Self {
            rigs,
            outputs,
            sessions,
            leq,
            key_hints: f.key_hints.unwrap_or(true),
            warning_toasts: f.warning_toasts.unwrap_or(true),
            resolution_marker: f.resolution_marker.unwrap_or(true),
            plate_background: f
                .plate_background
                .map_or(PlateBackground::SeeThrough, PlateBackgroundFile::background),
            bar_ease: f.bar_ease.unwrap_or(true),
            shared_scale: f.shared_scale.unwrap_or(true),
            spec_ease: f.spec_ease.unwrap_or(true),
            // A value no step reaches (hand-edited) is kept when it can be stood out by;
            // anything else is the default.
            jump_prominence_db: f
                .jump_prominence_db
                .filter(|d| d.is_finite() && *d > 0.0 && *d <= 60.0)
                .unwrap_or(ac2_scene::cursor_jump::DEFAULT_PROMINENCE_DB),
            spl_hold_ms: f.spl_hold_ms,
            // A hand-edited value outside the range is clamped into it: how the embers fade
            // is taste, not worth refusing the file over.
            ember_decay_s: f.ember_decay_s.map_or(
                ac2_scene::gonio::EMBER_DECAY_DEFAULT_S,
                ac2_scene::gonio::clamp_decay,
            ),
            ember_glow: f.ember_glow.map_or(
                ac2_scene::gonio::EMBER_GLOW_DEFAULT,
                ac2_scene::gonio::clamp_glow,
            ),
            advice: f
                .advice
                .as_ref()
                .map(AdviceFile::parse)
                .transpose()?
                .unwrap_or_default(),
            layout: f.layout.map(LayoutFile::parse).unwrap_or_default(),
            ir_levels: f
                .ir_levels
                .as_ref()
                .map(IrLevelsFile::parse)
                .transpose()?
                .unwrap_or_default(),
            window,
            theme: f.theme.map(ThemeFile::name),
            record_limit_min: f.record_limit_min,
            legend: f
                .legend
                .as_ref()
                .and_then(|l| l.transfer.as_ref())
                .map(LegendFile::parse)
                .unwrap_or_default(),
            recent_sessions: {
                let mut r = f.recent_sessions;
                r.truncate(MAX_RECENT_SESSIONS);
                r
            },
        })
    }

    /// The file text.
    pub fn to_toml(&self) -> String {
        let f = File {
            rigs: self.rigs.iter().map(RigFile::of).collect(),
            key_hints: (!self.key_hints).then_some(false),
            unknown: BTreeMap::new(),
            warning_toasts: (!self.warning_toasts).then_some(false),
            resolution_marker: (!self.resolution_marker).then_some(false),
            plate_background: (self.plate_background != PlateBackground::SeeThrough)
                .then(|| PlateBackgroundFile::of(self.plate_background)),
            bar_ease: (!self.bar_ease).then_some(false),
            shared_scale: (!self.shared_scale).then_some(false),
            spec_ease: (!self.spec_ease).then_some(false),
            jump_prominence_db: (self.jump_prominence_db
                != ac2_scene::cursor_jump::DEFAULT_PROMINENCE_DB)
                .then_some(self.jump_prominence_db),
            spl_hold_ms: self.spl_hold_ms,
            ember_decay_s: (self.ember_decay_s != ac2_scene::gonio::EMBER_DECAY_DEFAULT_S)
                .then_some(self.ember_decay_s),
            ember_glow: (self.ember_glow != ac2_scene::gonio::EMBER_GLOW_DEFAULT)
                .then_some(self.ember_glow),
            advice: AdviceFile::from_prefs(&self.advice),
            theme: self.theme.map(ThemeFile::of),
            record_limit_min: self.record_limit_min,
            layout: (self.layout != LayoutPrefs::default())
                .then(|| LayoutFile::from_prefs(&self.layout)),
            ir_levels: IrLevelsFile::from_prefs(&self.ir_levels),
            legend: LegendFile::from_prefs(&self.legend),
            window: self.window.map(|w| WindowFile {
                width: w.width,
                height: w.height,
                x: w.pos.map(|p| p.0),
                y: w.pos.map(|p| p.1),
            }),
            recent_sessions: self.recent_sessions.clone(),
            stimulus_outputs: self
                .outputs
                .iter()
                .map(|(k, o)| (rig_key(k), ones(o)))
                .collect(),
            sessions: self
                .sessions
                .iter()
                .map(|(k, r)| (rig_key(k), RolesFile::from_roles(r)))
                .collect(),
            leq: (self.leq != LeqLayout::default()).then_some(LeqFile {
                style: match self.leq.style {
                    LeqStyle::Columns => StyleFile::Columns,
                    LeqStyle::Tiles => StyleFile::Tiles,
                },
                history: self.leq.history,
            }),
        };
        toml::to_string(&f).unwrap_or_default()
    }

    /// Reads `path`. A missing file is the defaults; a damaged one (garbage, a torn write)
    /// is set aside as `ui.toml.damaged-<time>` for inspection and the defaults used, the
    /// reason shown once; one that cannot be read at all is the defaults plus the reason.
    pub fn load(path: Option<&Path>) -> (Self, Option<String>) {
        let Some(path) = path else {
            return (Self::default(), None);
        };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                return (
                    Self::default(),
                    Some(format!("cannot read {}: {e}", path.display())),
                );
            }
        };
        let parsed = std::str::from_utf8(&bytes)
            .map_err(|e| e.to_string())
            .and_then(|t| Self::from_toml(t).map_err(|e| e.to_string()));
        match parsed {
            Ok(p) => (p, None),
            Err(e) => {
                let aside = match ac2_paths::set_aside_damaged(path) {
                    Ok(to) => format!("set aside as {}", to.display()),
                    Err(err) => format!("cannot be set aside ({err})"),
                };
                tracing::error!("{}: {e}; {aside}; using defaults", path.display());
                (
                    Self::default(),
                    Some(format!("{e} ({}); {aside}; using defaults", path.display())),
                )
            }
        }
    }

    /// Writes `path` atomically.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        ac2_paths::write_private_atomic(path, self.to_toml().as_bytes())
            .map_err(|e| format!("cannot save {}: {e}", path.display()))
    }
}

/// The file's key of a device remembered for a rig: the rig's id (16 hex digits) and the
/// device's key, `000000000000fa4e/jack:system`. Keys are strings in TOML.
fn rig_key((rig, device): &(RigId, String)) -> String {
    format!("{rig}/{device}")
}

/// A key written by [`rig_key`]. A key without a rig (written before devices were
/// remembered per rig) names no rig's device, so it is dropped, and the next save leaves
/// it out.
fn parse_rig_key(key: &str) -> Option<(RigId, String)> {
    let (rig, device) = key.split_once('/')?;
    Some((parse_rig_id(rig)?, device.to_owned()))
}

/// A rig id as the file writes it, 16 hex digits; `None` for one no daemon has.
pub(super) fn parse_rig_id(s: &str) -> Option<RigId> {
    Some(s)
        .filter(|s| s.len() == 16)
        .and_then(|s| u64::from_str_radix(s, 16).ok())
        .filter(|n| *n != 0)
        .map(RigId)
}
