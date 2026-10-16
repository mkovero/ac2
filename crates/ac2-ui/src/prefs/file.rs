//! The `ui.toml` file: its tables and how they read into and write from [`UiPrefs`].

use super::layout::*;
use super::*;

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
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bar_ease: Option<bool>,
    /// Written only when off (the default is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spec_ease: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spl_hold_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<ThemeFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    record_limit_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spectrograph_span_s: Option<u32>,
    /// One-based channels per device id.
    #[serde(default)]
    stimulus_outputs: BTreeMap<String, Vec<u32>>,
    /// Per `backend/device id`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sessions: BTreeMap<String, RolesFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leq: Option<LeqFile>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legend: Option<LegendsFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<WindowFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recent_sessions: Vec<RecentSession>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_height: Option<f64>,
}

impl LegendFile {
    fn parse(&self) -> Result<LegendPrefs, String> {
        let d = LegendPrefs::default();
        let limits = ac2_scene::legend::SIZE_LIMITS;
        let (lo, hi) = (f64::from(*limits.start()), f64::from(*limits.end()));
        let num = |name: &str, v: Option<f64>, default: f32, lo: f64, hi: f64| match v {
            None => Ok(default),
            Some(v) if (lo..=hi).contains(&v) => Ok(v as f32),
            Some(_) => Err(format!(
                "ui.toml: legend.transfer.{name} must be {lo} … {hi}"
            )),
        };
        Ok(LegendPrefs {
            hidden: self.hidden.unwrap_or(d.hidden),
            x: num("x", self.x, d.x, 0.0, 1.0)?,
            y: num("y", self.y, d.y, 0.0, 1.0)?,
            max_width: num("max_width", self.max_width, d.max_width, lo, hi)?,
            max_height: num("max_height", self.max_height, d.max_height, lo, hi)?,
        })
    }

    /// Only the choices moved off their defaults, to three decimals (a drag's fraction
    /// has no meaningful digits beyond a pixel).
    fn from_prefs(l: &LegendPrefs) -> Option<LegendsFile> {
        let d = LegendPrefs::default();
        let v =
            |x: f32, default: f32| (x != default).then(|| (f64::from(x) * 1000.0).round() / 1000.0);
        let f = Self {
            hidden: l.hidden.then_some(true),
            x: v(l.x, d.x),
            y: v(l.y, d.y),
            max_width: v(l.max_width, d.max_width),
            max_height: v(l.max_height, d.max_height),
        };
        (*l != d).then_some(LegendsFile { transfer: Some(f) })
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

/// A pane's own level axes (`[layout.panes.levels]` under its `[[layout.panes]]` entry),
/// `[low, high]` dB; one left out is the default.
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
    /// The ranges; one that is no range is the default, logged (a layout is a convenience,
    /// read leniently).
    pub(super) fn parse(f: Option<&Self>, pane: u32) -> PaneLevels {
        let d = PaneLevels::default();
        let Some(f) = f else {
            return d;
        };
        let range = |name: &str, v: Option<[f64; 2]>, default: Range| match v {
            None => default,
            Some(v) => level_range(v).unwrap_or_else(|| {
                tracing::warn!(
                    "ui.toml: pane {pane}'s levels.{name} is not [low, high] in dB; default"
                );
                default
            }),
        };
        PaneLevels {
            transfer: range("transfer", f.transfer, d.transfer),
            spectrum_dbfs: range("spectrum_dbfs", f.spectrum_dbfs, d.spectrum_dbfs),
            spectrum_spl: range("spectrum_spl", f.spectrum_spl, d.spectrum_spl),
            distortion: range("distortion", f.distortion, d.distortion),
        }
    }

    /// Only the ranges moved off their defaults; `None` when all are at them.
    pub(super) fn of(l: &PaneLevels) -> Option<Self> {
        let d = PaneLevels::default();
        let v = |r: Range, default: Range| (r != default).then_some([r.lo, r.hi]);
        (*l != d).then(|| Self {
            transfer: v(l.transfer, d.transfer),
            spectrum_dbfs: v(l.spectrum_dbfs, d.spectrum_dbfs),
            spectrum_spl: v(l.spectrum_spl, d.spectrum_spl),
            distortion: v(l.distortion, d.distortion),
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
}

impl PaneFile {
    pub(super) fn of(p: PaneKind) -> Self {
        match p {
            PaneKind::Transfer => Self::Transfer,
            PaneKind::Spectrum => Self::Spectrum,
            PaneKind::Spl => Self::Spl,
            PaneKind::Distortion => Self::Distortion,
        }
    }

    pub(super) fn pane(self) -> PaneKind {
        match self {
            Self::Transfer => PaneKind::Transfer,
            Self::Spectrum => PaneKind::Spectrum,
            Self::Spl => PaneKind::Spl,
            Self::Distortion => PaneKind::Distortion,
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

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RolesFile {
    #[serde(default)]
    inputs: Vec<u32>,
    #[serde(default)]
    outputs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference: Option<u32>,
    #[serde(default)]
    mics: Vec<u32>,
    #[serde(default)]
    stimulus: Vec<u32>,
    /// One-based channel (as text: TOML keys are strings) → mic name.
    #[serde(default)]
    mic_names: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_device: Option<String>,
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
        Some(DeviceRoles {
            inputs: zeros(&self.inputs)?,
            outputs: u16::try_from(self.outputs).ok()?,
            reference: match self.reference {
                Some(r) => Some(zero(r)?),
                None => None,
            },
            mics: zeros(&self.mics)?,
            stimulus: zeros(&self.stimulus)?,
            mic_names,
            output_device: self.output_device.clone(),
        })
    }

    fn from_roles(r: &DeviceRoles) -> Self {
        Self {
            inputs: ones(&r.inputs),
            outputs: u32::from(r.outputs),
            reference: r.reference.map(|c| u32::from(c) + 1),
            mics: ones(&r.mics),
            stimulus: ones(&r.stimulus),
            mic_names: r
                .mic_names
                .iter()
                .map(|(c, n)| ((u32::from(*c) + 1).to_string(), n.clone()))
                .collect(),
            output_device: r.output_device.clone(),
        }
    }
}

impl UiPrefs {
    /// The outputs remembered for `device`.
    pub fn outputs_for(&self, device: &str) -> Option<&[u16]> {
        self.outputs.get(device).map(Vec::as_slice)
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
                    outputs.insert(device, z);
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
            sessions.insert(device, roles);
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
        if let Some(s) = f.spectrograph_span_s
            && !ac2_scene::view::SPECTROGRAPH_SPANS_S.contains(&s)
        {
            return Err(format!(
                "ui.toml: spectrograph_span_s must be one of {:?}",
                ac2_scene::view::SPECTROGRAPH_SPANS_S
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
        Ok(Self {
            outputs,
            sessions,
            leq,
            key_hints: f.key_hints.unwrap_or(true),
            warning_toasts: f.warning_toasts.unwrap_or(true),
            resolution_marker: f.resolution_marker.unwrap_or(true),
            bar_ease: f.bar_ease.unwrap_or(true),
            spec_ease: f.spec_ease.unwrap_or(true),
            spl_hold_ms: f.spl_hold_ms,
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
            spectrograph_span_s: f.spectrograph_span_s,
            legend: f
                .legend
                .as_ref()
                .and_then(|l| l.transfer.as_ref())
                .map(LegendFile::parse)
                .transpose()?
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
            key_hints: (!self.key_hints).then_some(false),
            unknown: BTreeMap::new(),
            warning_toasts: (!self.warning_toasts).then_some(false),
            resolution_marker: (!self.resolution_marker).then_some(false),
            bar_ease: (!self.bar_ease).then_some(false),
            spec_ease: (!self.spec_ease).then_some(false),
            spl_hold_ms: self.spl_hold_ms,
            theme: self.theme.map(ThemeFile::of),
            record_limit_min: self.record_limit_min,
            spectrograph_span_s: self.spectrograph_span_s,
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
                .map(|(d, o)| (d.clone(), ones(o)))
                .collect(),
            sessions: self
                .sessions
                .iter()
                .map(|(d, r)| (d.clone(), RolesFile::from_roles(r)))
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
