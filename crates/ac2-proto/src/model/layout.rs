//! A measurement's pane layout (`meas.layout`): the panes a client shows while that
//! measurement is selected. The arrangement, what each pane draws, the measurement each
//! pane was picked to show, the level ranges the operator set on it and where they dropped
//! the plates over its plot (and their text size) travel; which pane has the keyboard, a maximised or full-screen pane and the window stay with each client.

use serde::{Deserialize, Serialize};

use crate::units::{MeasId, RigId, TraceId, WalkId};

/// The pane tree: a leaf is a pane, a split shares its rectangle between two subtrees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PaneLayout {
    /// One pane.
    Pane {
        /// What it draws (boxed: a pane's level ranges make it far larger than a split,
        /// and every measurement carries a layout).
        pane: Box<LayoutPane>,
    },
    /// Two subtrees side by side or stacked.
    Split {
        /// How the rectangle is divided.
        axis: SplitAxis,
        /// The share `a` takes, strictly between 0 and 1.
        ratio: f64,
        /// Left or top.
        a: Box<PaneLayout>,
        /// Right or bottom.
        b: Box<PaneLayout>,
    },
}

/// How a split divides its rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    /// Side by side: `a` left.
    Row,
    /// Stacked: `a` on top.
    Column,
}

/// What a pane shows. Every reference names the rig whose id it is, because ids are per
/// daemon: a pane may draw this rig's curves beside another rig's. A pane with no pick holds
/// [`PaneContent::Nothing`] and keeps its kind; it follows no selection.
///
/// Inputs are zero-based device input channels, as the session's `input_channels` names
/// them; a pane of a session loaded on a device without them shows which are missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PaneContent {
    /// Nothing picked, on any pane kind.
    Nothing,
    /// A graph pane's curves (Transfer, Spectrum / RTA, Sweep / distortion), the lead first:
    /// it names the pane and owns its readouts. At least one, at most
    /// [`PaneContent::MAX_ENTRIES`].
    Curves { entries: Vec<PaneEntry> },
    /// An SPL pane's one meter: a meter is a number, read side by side, never overlaid.
    Meter { rig: RigRef, meas: MeasId },
    /// Up to [`PaneContent::MAX_INPUTS`] inputs, metered side by side.
    Inputs { rig: RigRef, inputs: Vec<u16> },
    /// A stereo pair: its two bars, correlation and goniometer.
    Pair { rig: RigRef, left: u16, right: u16 },
    /// A spatial walk: its plan, points and step (a Walk pane's only content).
    Walk { rig: RigRef, walk: WalkId },
}

/// One curve reference of a graph pane, with the rig whose id it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneEntry {
    /// Whose id `what` is.
    pub rig: RigRef,
    /// What is drawn.
    pub what: EntryRef,
}

/// What a graph pane entry draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EntryRef {
    /// A measurement's group: its live curve, its math channels, its shown traces, what is
    /// filed under it now and later.
    Group { meas: MeasId },
    /// One live curve: a measurement's or a math channel's.
    Live { meas: MeasId },
    /// One stored trace.
    Trace { trace: TraceId },
}

impl EntryRef {
    /// The measurement it names, if it names one.
    pub fn meas(self) -> Option<MeasId> {
        match self {
            Self::Group { meas } | Self::Live { meas } => Some(meas),
            Self::Trace { .. } => None,
        }
    }
}

impl PaneContent {
    /// Bars a Programme pane shows at most: more than eight no longer read at a glance,
    /// and a console feed is rarely wider.
    pub const MAX_INPUTS: usize = 8;

    /// Entries a graph pane holds at most: a group and 32 more, the most a measurement's
    /// compare list held, so that salvage never cuts a list that was legal.
    pub const MAX_ENTRIES: usize = 33;

    /// A graph pane drawing measurement `meas`'s group of this rig.
    pub fn group(meas: MeasId) -> Self {
        Self::Curves {
            entries: vec![PaneEntry {
                rig: RigRef::ThisRig,
                what: EntryRef::Group { meas },
            }],
        }
    }

    /// The lead entry of a graph pane.
    pub fn lead(&self) -> Option<&PaneEntry> {
        match self {
            Self::Curves { entries } => entries.first(),
            _ => None,
        }
    }

    /// The inputs a Programme pane with this content meters, left to right.
    pub fn inputs(&self) -> Vec<u16> {
        match self {
            Self::Inputs { inputs, .. } => inputs.clone(),
            Self::Pair { left, right, .. } => vec![*left, *right],
            Self::Nothing | Self::Curves { .. } | Self::Meter { .. } | Self::Walk { .. } => {
                Vec::new()
            }
        }
    }

    /// Every rig reference it holds.
    pub fn rigs(&self) -> Vec<&RigRef> {
        match self {
            Self::Nothing => Vec::new(),
            Self::Curves { entries } => entries.iter().map(|e| &e.rig).collect(),
            Self::Meter { rig, .. }
            | Self::Inputs { rig, .. }
            | Self::Pair { rig, .. }
            | Self::Walk { rig, .. } => vec![rig],
        }
    }

    /// Every rig reference it holds, to rewrite (renumbering, pruning).
    pub fn refs_mut(&mut self) -> Vec<&mut RigRef> {
        match self {
            Self::Nothing => Vec::new(),
            Self::Curves { entries } => entries.iter_mut().map(|e| &mut e.rig).collect(),
            Self::Meter { rig, .. }
            | Self::Inputs { rig, .. }
            | Self::Pair { rig, .. }
            | Self::Walk { rig, .. } => vec![rig],
        }
    }

    /// Why the content cannot be shown, if it cannot: no inputs, more than
    /// [`Self::MAX_INPUTS`], one named twice, a pair of one input; no entries, more than
    /// [`Self::MAX_ENTRIES`], one twice.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Self::Nothing | Self::Meter { .. } | Self::Walk { .. } => Ok(()),
            Self::Curves { entries } if entries.is_empty() => {
                Err("a pane with nothing picked holds nothing, not an empty list".into())
            }
            Self::Curves { entries } if entries.len() > Self::MAX_ENTRIES => Err(format!(
                "a pane draws at most {} entries, not {}",
                Self::MAX_ENTRIES,
                entries.len()
            )),
            Self::Curves { entries } => {
                for (i, e) in entries.iter().enumerate() {
                    if entries[..i].contains(e) {
                        return Err(format!("{:?} is in the pane twice", e.what));
                    }
                }
                Ok(())
            }
            Self::Inputs { inputs, .. } if inputs.is_empty() => {
                Err("a Programme pane names at least one input".into())
            }
            Self::Inputs { inputs, .. } if inputs.len() > Self::MAX_INPUTS => Err(format!(
                "a Programme pane shows at most {} inputs, not {}",
                Self::MAX_INPUTS,
                inputs.len()
            )),
            Self::Inputs { inputs, .. } => {
                let mut seen = inputs.clone();
                seen.sort_unstable();
                seen.dedup();
                if seen.len() != inputs.len() {
                    return Err("a Programme pane names each input once".into());
                }
                Ok(())
            }
            Self::Pair { left, right, .. } if left == right => Err(format!(
                "a pair is two inputs, not input {} twice",
                left + 1
            )),
            Self::Pair { .. } => Ok(()),
        }
    }
}

/// What a pane draws: its kind and the view of each kind it may be turned into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutPane {
    /// The kind of plot.
    pub kind: PaneKind,
    /// The transfer pane's view.
    pub transfer_view: TransferView,
    /// The spectrum pane's view.
    pub spectrum_view: SpectrumView,
    /// Where the spectrum pane's 3D spectrograph is seen from: a view of the pane, kept
    /// while the pane shows another view.
    pub spectrum_camera: SpectrumCamera,
    /// The SPL pane's view.
    pub spl_view: SplView,
    /// The sweep pane's view.
    pub sweep_view: SweepView,
    /// The Programme pane's view; `goniometer` on a pane that meters no pair draws its meters.
    pub programme_view: ProgrammeView,
    /// How much of the plot besides the curves.
    pub chrome: PlotChrome,
    /// What the pane shows, each reference with its rig. A reference naming something its
    /// rig does not hold (yet) waits: it is never swapped for a stand-in.
    pub content: PaneContent,
    /// The level axis ranges the operator set on the pane (zoomed, panned, reset, fitted);
    /// one left `None` is framed by the client when the pane first shows a curve.
    #[serde(default)]
    pub levels: LayoutLevels,
    /// Where the operator dropped the plates over the plot and their text size.
    #[serde(default)]
    pub plates: LayoutPlates,
}

/// The rig whose id a pane reference is. A layout is stored by one daemon (a measurement's,
/// a rig meter's), so its own references name [`RigRef::ThisRig`]: the rig a client shows
/// when it is loaded may be another, whose ids would be misread as this one's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RigRef {
    /// The daemon storing the layout.
    ThisRig,
    /// A pinned rig, by its id (never the CURVE key, which a local rig lacks and a rig
    /// moving to network mode gains); the name is for the operator when it is absent.
    Rig {
        /// The pinned rig.
        id: RigId,
        /// Its name when pinned.
        name: String,
    },
}

/// A pane's five plates: the legend, the cursor values, the mark's readout, the
/// spectrum's list of tracked peaks and the Programme pane's loudness readout. A pane kind
/// without one of them keeps that slot at its default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutPlates {
    pub legend: LayoutPlate,
    pub readout: LayoutPlate,
    pub mark: LayoutPlate,
    pub peaks: LayoutPlate,
    /// `at: None` docks it: the pane reserves room for it beside or above the bars.
    pub loudness: LayoutPlate,
}

impl LayoutPlates {
    fn all(&self) -> [(&'static str, LayoutPlate); 5] {
        [
            ("legend", self.legend),
            ("readout", self.readout),
            ("mark", self.mark),
            ("peaks", self.peaks),
            ("loudness", self.loudness),
        ]
    }
}

/// Where one plate sits and how large its text is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutPlate {
    /// Its place as fractions of the room the plot leaves beside it, `[x, y]`, each 0 … 1
    /// (0 at the left / top edge): a fraction keeps the plate inside a plot of any size.
    /// `None`: where the client puts it.
    pub at: Option<[f64; 2]>,
    /// Its text size, times the plot's.
    pub scale: f64,
}

impl Default for LayoutPlate {
    fn default() -> Self {
        Self {
            at: None,
            scale: 1.0,
        }
    }
}

/// A pane's operator-set level axis ranges, one per kind the pane may show (it keeps the
/// others while it shows another kind), the spectrum's one per scale: a dBFS range means
/// nothing on an SPL axis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutLevels {
    /// The transfer magnitude axis, dB.
    pub transfer: Option<LevelRange>,
    /// The spectrum axis in dBFS.
    pub spectrum_dbfs: Option<LevelRange>,
    /// The spectrum axis in dB SPL.
    pub spectrum_spl: Option<LevelRange>,
    /// The sweep response (harmonics, THD) axis, dB.
    pub distortion: Option<LevelRange>,
}

impl LayoutLevels {
    fn all(&self) -> [(&'static str, Option<LevelRange>); 4] {
        [
            ("transfer", self.transfer),
            ("spectrum_dbfs", self.spectrum_dbfs),
            ("spectrum_spl", self.spectrum_spl),
            ("distortion", self.distortion),
        ]
    }

    /// No range set.
    pub fn is_empty(&self) -> bool {
        self.all().iter().all(|(_, r)| r.is_none())
    }
}

/// A level axis range, dB: `lo` at the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelRange {
    /// Bottom, dB.
    pub lo: f64,
    /// Top, dB.
    pub hi: f64,
}

/// The kinds of pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    /// Live transfer curves and their impulse response.
    Transfer,
    /// Spectrum / RTA.
    Spectrum,
    /// SPL meter and Leq.
    Spl,
    /// Sweep results: response, distortion, the sweep's IR.
    Distortion,
    /// Programme meters of inputs or a stereo pair: PPM bars, correlation, goniometer.
    Programme,
    /// A spatial walk: the plan, the current point and its readout, the step.
    Walk,
}

impl PaneKind {
    /// A graph pane: it draws curves, several of them over each other.
    pub fn draws_curves(self) -> bool {
        matches!(self, Self::Transfer | Self::Spectrum | Self::Distortion)
    }
}

/// The transfer pane's views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferView {
    Magnitude,
    Phase,
    Coherence,
    /// An alignment's phase difference and summation (`docs/design/alignment.md`).
    Alignment,
    IrLinear,
    IrLog,
    Etc,
}

/// The spectrum pane's views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpectrumView {
    Spectrum,
    SpectrumSpectrograph,
    Spectrograph,
    /// Level as height over frequency and time (`docs/design/spectrograph-3d.md`).
    #[serde(rename = "spectrograph_3d")]
    Spectrograph3d,
}

/// The 3D spectrograph's camera, orbiting the floor's centre: whole degrees and zoom
/// steps, so a stored camera reads back equal and an unchanged one is not sent again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpectrumCamera {
    /// Around the vertical axis, degrees; 0 looks along the time axis, positive from the
    /// right.
    pub yaw_deg: i8,
    /// Above the floor, degrees.
    pub pitch_deg: u8,
    /// Dolly steps from the fitted distance; positive is closer.
    pub zoom_step: i8,
}

impl SpectrumCamera {
    /// Past 90° frequency would read right to left; 80 leaves the axis readable.
    pub const YAW_MAX_DEG: i8 = 80;
    /// Edge-on spikes vanish below the low limit; top-down is the 2D view.
    pub const PITCH_DEG: std::ops::RangeInclusive<u8> = 2..=75;
    pub const ZOOM_STEPS: std::ops::RangeInclusive<i8> = -8..=4;

    /// Frequency left to right, seen from just above the floor.
    pub const DEFAULT: Self = Self {
        yaw_deg: 0,
        pitch_deg: 8,
        zoom_step: 0,
    };

    /// Why the camera is not one a pane may hold: each field within its range.
    pub fn check(&self) -> Result<(), String> {
        if self.yaw_deg.unsigned_abs() > Self::YAW_MAX_DEG.unsigned_abs() {
            return Err(format!(
                "a 3D spectrograph's yaw is within ±{}°, not {}°",
                Self::YAW_MAX_DEG,
                self.yaw_deg
            ));
        }
        if !Self::PITCH_DEG.contains(&self.pitch_deg) {
            return Err(format!(
                "a 3D spectrograph's pitch is within {} … {}°, not {}°",
                Self::PITCH_DEG.start(),
                Self::PITCH_DEG.end(),
                self.pitch_deg
            ));
        }
        if !Self::ZOOM_STEPS.contains(&self.zoom_step) {
            return Err(format!(
                "a 3D spectrograph's zoom step is within {} … {}, not {}",
                Self::ZOOM_STEPS.start(),
                Self::ZOOM_STEPS.end(),
                self.zoom_step
            ));
        }
        Ok(())
    }
}

impl Default for SpectrumCamera {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The SPL pane's views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplView {
    Meter,
    Leq,
    MeterLeq,
    Bands,
}

/// The sweep pane's views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepView {
    Response,
    IrLinear,
    IrLog,
    Etc,
    Room,
}

/// The Programme pane's views: its bars and loudness plate, or the goniometer of its pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgrammeView {
    Meters,
    Goniometer,
}

/// Grid, labels and cursor around the curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlotChrome {
    Full,
    NoGrid,
    TracesOnly,
}

impl PaneLayout {
    /// Most panes a layout holds: past this the plots are too small to read on any screen.
    pub const MAX_PANES: usize = 16;

    /// The panes, left or top first.
    pub fn panes(&self) -> Vec<&LayoutPane> {
        let mut v = Vec::new();
        self.collect(&mut v);
        v
    }

    fn collect<'a>(&'a self, v: &mut Vec<&'a LayoutPane>) {
        match self {
            Self::Pane { pane } => v.push(pane),
            Self::Split { a, b, .. } => {
                a.collect(v);
                b.collect(v);
            }
        }
    }

    /// The panes, left or top first, to change.
    pub fn panes_mut(&mut self) -> Vec<&mut LayoutPane> {
        match self {
            Self::Pane { pane } => vec![&mut **pane],
            Self::Split { a, b, .. } => {
                let mut v = a.panes_mut();
                v.extend(b.panes_mut());
                v
            }
        }
    }

    /// Why the layout cannot be drawn, if it cannot: a split ratio not strictly between 0
    /// and 1 leaves one side no room, more than [`Self::MAX_PANES`] panes, a level range
    /// not finite with its top above its bottom (no axis can be drawn on it), a plate
    /// placed outside 0 … 1 or with a text scale not finite and above 0, content
    /// [`PaneContent::check`] refuses, and content of another kind than the pane's:
    /// [`PaneContent::Nothing`] on any pane, curves only on a Transfer, Spectrum / RTA or
    /// Sweep / distortion pane, a meter only on an SPL pane, inputs or a pair only on a
    /// Programme pane, a walk only on a Walk pane, and a 3D spectrograph camera outside
    /// [`SpectrumCamera::check`]'s ranges (refused, never clamped: it came from another
    /// client or a file).
    pub fn check(&self) -> Result<(), String> {
        let n = self.panes().len();
        if n > Self::MAX_PANES {
            return Err(format!(
                "a layout holds at most {} panes, not {n}",
                Self::MAX_PANES
            ));
        }
        for (name, r) in self
            .panes()
            .iter()
            .flat_map(|p| p.levels.all())
            .filter_map(|(n, r)| Some((n, r?)))
        {
            if !(r.lo.is_finite() && r.hi.is_finite() && r.lo < r.hi) {
                return Err(format!(
                    "a pane's {name} level range is finite with its top above its bottom, \
                     not {} … {}",
                    r.lo, r.hi
                ));
            }
        }
        for p in self.panes() {
            p.spectrum_camera.check()?;
            let fits = match (&p.content, p.kind) {
                (PaneContent::Nothing, _) => true,
                (PaneContent::Curves { .. }, k) => k.draws_curves(),
                (PaneContent::Meter { .. }, k) => k == PaneKind::Spl,
                (PaneContent::Inputs { .. } | PaneContent::Pair { .. }, k) => {
                    k == PaneKind::Programme
                }
                (PaneContent::Walk { .. }, k) => k == PaneKind::Walk,
            };
            if !fits {
                return Err(match &p.content {
                    PaneContent::Curves { .. } => {
                        "curves are drawn on a Transfer, Spectrum / RTA or Sweep / distortion \
                         pane only"
                    }
                    PaneContent::Meter { .. } => "only an SPL pane shows a meter",
                    PaneContent::Inputs { .. } | PaneContent::Pair { .. } => {
                        "only a Programme pane meters inputs"
                    }
                    PaneContent::Walk { .. } => "only a Walk pane shows a walk",
                    PaneContent::Nothing => unreachable!("nothing fits every pane"),
                }
                .into());
            }
            p.content.check()?;
        }
        for (name, p) in self.panes().iter().flat_map(|p| p.plates.all()) {
            if p.at
                .is_some_and(|a| !a.iter().all(|v| (0.0..=1.0).contains(v)))
            {
                return Err(format!(
                    "a pane's {name} plate is placed within 0 … 1, not {:?}",
                    p.at
                ));
            }
            if !(p.scale.is_finite() && p.scale > 0.0) {
                return Err(format!(
                    "a pane's {name} plate's text scale is finite and above 0, not {}",
                    p.scale
                ));
            }
        }
        self.check_ratios()
    }

    /// Why the layout cannot be stored by the daemon holding it, given `group`, the
    /// measurement whose group draws a curve of this rig (`None`: no group's): a [`EntryRef::Live`]
    /// or [`EntryRef::Trace`] of this rig in a pane whose [`EntryRef::Group`] draws it already.
    pub fn check_groups(&self, group: impl Fn(EntryRef) -> Option<MeasId>) -> Result<(), String> {
        for p in self.panes() {
            let PaneContent::Curves { entries } = &p.content else {
                continue;
            };
            let groups: Vec<MeasId> = entries
                .iter()
                .filter(|e| e.rig == RigRef::ThisRig)
                .filter_map(|e| match e.what {
                    EntryRef::Group { meas } => Some(meas),
                    _ => None,
                })
                .collect();
            for e in entries.iter().filter(|e| e.rig == RigRef::ThisRig) {
                if matches!(e.what, EntryRef::Group { .. }) {
                    continue;
                }
                if let Some(g) = group(e.what).filter(|g| groups.contains(g)) {
                    return Err(format!(
                        "{:?} is drawn by the group of measurement {g} in the same pane",
                        e.what
                    ));
                }
            }
        }
        Ok(())
    }

    /// Every reference of every pane, to read or rewrite.
    pub fn contents_mut(&mut self) -> Vec<&mut PaneContent> {
        self.panes_mut()
            .into_iter()
            .map(|p| &mut p.content)
            .collect()
    }

    fn check_ratios(&self) -> Result<(), String> {
        match self {
            Self::Pane { .. } => Ok(()),
            Self::Split { ratio, a, b, .. } => {
                if !(ratio.is_finite() && *ratio > 0.0 && *ratio < 1.0) {
                    return Err(format!(
                        "a split's ratio is strictly between 0 and 1, not {ratio}"
                    ));
                }
                a.check_ratios()?;
                b.check_ratios()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programme(content: PaneContent) -> PaneLayout {
        let mut l = crate::samples::pane_layout();
        for p in l.panes_mut() {
            if p.kind == PaneKind::Programme {
                p.content = content.clone();
            }
        }
        l
    }

    fn this(what: EntryRef) -> PaneEntry {
        PaneEntry {
            rig: RigRef::ThisRig,
            what,
        }
    }

    #[test]
    fn programme_content_is_checked() {
        assert_eq!(crate::samples::pane_layout().check(), Ok(()));
        let rig = RigRef::ThisRig;
        let inputs = |v: Vec<u16>| PaneContent::Inputs {
            rig: RigRef::ThisRig,
            inputs: v,
        };
        assert_eq!(programme(inputs(vec![0, 1, 2])).check(), Ok(()));
        assert_eq!(programme(PaneContent::Nothing).check(), Ok(()));
        assert_eq!(
            programme(PaneContent::group(MeasId(1))).check(),
            Err(
                "curves are drawn on a Transfer, Spectrum / RTA or Sweep / distortion pane only"
                    .into()
            )
        );
        assert!(programme(inputs(vec![])).check().is_err());
        assert!(programme(inputs((0..9).collect())).check().is_err());
        assert!(programme(inputs(vec![1, 1])).check().is_err());
        assert_eq!(
            programme(PaneContent::Pair {
                rig: rig.clone(),
                left: 3,
                right: 3
            })
            .check(),
            Err("a pair is two inputs, not input 4 twice".into())
        );
        let mut l = crate::samples::pane_layout();
        l.panes_mut()[0].content = PaneContent::Pair {
            rig: rig.clone(),
            left: 0,
            right: 1,
        };
        assert_eq!(l.check(), Err("only a Programme pane meters inputs".into()));
        let p = PaneContent::Pair {
            rig,
            left: 2,
            right: 3,
        };
        assert_eq!(p.inputs(), vec![2, 3]);
        assert_eq!(p.lead(), None);
    }

    #[test]
    fn each_content_goes_on_the_pane_kind_that_draws_it() {
        let mut l = crate::samples::pane_layout();
        let walk = PaneContent::Walk {
            rig: RigRef::ThisRig,
            walk: WalkId(2),
        };
        let meter = PaneContent::Meter {
            rig: RigRef::ThisRig,
            meas: MeasId(1),
        };
        let cases = [
            (PaneKind::Walk, walk.clone(), Ok(())),
            (PaneKind::Walk, PaneContent::Nothing, Ok(())),
            (
                PaneKind::Transfer,
                walk,
                Err("only a Walk pane shows a walk"),
            ),
            (PaneKind::Spl, meter.clone(), Ok(())),
            (PaneKind::Spl, PaneContent::Nothing, Ok(())),
            (
                PaneKind::Spectrum,
                meter,
                Err("only an SPL pane shows a meter"),
            ),
            (PaneKind::Distortion, PaneContent::group(MeasId(4)), Ok(())),
            (PaneKind::Spectrum, PaneContent::group(MeasId(4)), Ok(())),
            (
                PaneKind::Spl,
                PaneContent::group(MeasId(4)),
                Err(
                    "curves are drawn on a Transfer, Spectrum / RTA or Sweep / distortion pane only",
                ),
            ),
        ];
        for (kind, content, want) in cases {
            let p = &mut l.panes_mut()[0];
            p.kind = kind;
            p.content = content.clone();
            assert_eq!(
                l.check(),
                want.map_err(str::to_owned),
                "{kind:?} with {content:?}"
            );
        }
    }

    #[test]
    fn a_pane_holds_a_group_and_up_to_32_more_each_once() {
        let mut l = crate::samples::pane_layout();
        let p = &mut l.panes_mut()[0];
        p.content = PaneContent::Curves { entries: vec![] };
        assert_eq!(
            l.check(),
            Err("a pane with nothing picked holds nothing, not an empty list".into())
        );
        let mut entries = vec![this(EntryRef::Group { meas: MeasId(1) })];
        entries.extend((0..32).map(|i| this(EntryRef::Trace { trace: TraceId(i) })));
        l.panes_mut()[0].content = PaneContent::Curves {
            entries: entries.clone(),
        };
        assert_eq!(l.check(), Ok(()));
        entries.push(this(EntryRef::Live { meas: MeasId(9) }));
        l.panes_mut()[0].content = PaneContent::Curves {
            entries: entries.clone(),
        };
        assert_eq!(
            l.check(),
            Err("a pane draws at most 33 entries, not 34".into())
        );
        entries.truncate(3);
        entries.push(this(EntryRef::Trace { trace: TraceId(1) }));
        l.panes_mut()[0].content = PaneContent::Curves {
            entries: entries.clone(),
        };
        assert_eq!(
            l.check(),
            Err("Trace { trace: TraceId(1) } is in the pane twice".into())
        );
        // The same id of another rig is another curve.
        entries.pop();
        entries.push(PaneEntry {
            rig: RigRef::Rig {
                id: RigId(7),
                name: "ketunkolo".into(),
            },
            what: EntryRef::Trace { trace: TraceId(1) },
        });
        l.panes_mut()[0].content = PaneContent::Curves { entries };
        assert_eq!(l.check(), Ok(()));
    }

    #[test]
    fn a_curve_its_group_draws_is_not_listed_again() {
        let mut l = crate::samples::pane_layout();
        let other = RigRef::Rig {
            id: RigId(7),
            name: "ketunkolo".into(),
        };
        l.panes_mut()[0].content = PaneContent::Curves {
            entries: vec![
                this(EntryRef::Group { meas: MeasId(1) }),
                this(EntryRef::Trace { trace: TraceId(5) }),
                PaneEntry {
                    rig: other,
                    what: EntryRef::Live { meas: MeasId(2) },
                },
            ],
        };
        // Trace 5 is filed under measurement 3, live 2 is another rig's.
        let group = |w: EntryRef| match w {
            EntryRef::Trace { .. } => Some(MeasId(3)),
            _ => Some(MeasId(1)),
        };
        assert_eq!(l.check_groups(group), Ok(()));
        let group = |_: EntryRef| Some(MeasId(1));
        assert_eq!(
            l.check_groups(group),
            Err("Trace { trace: TraceId(5) } is drawn by the group of measurement 1 in the same pane".into())
        );
    }

    #[test]
    fn a_camera_outside_its_ranges_is_refused_not_clamped() {
        let with = |c: SpectrumCamera| {
            let mut l = crate::samples::pane_layout();
            l.panes_mut()[0].spectrum_camera = c;
            l.check()
        };
        let d = SpectrumCamera::DEFAULT;
        for ok in [
            d,
            SpectrumCamera { yaw_deg: -80, ..d },
            SpectrumCamera { yaw_deg: 80, ..d },
            SpectrumCamera { pitch_deg: 2, ..d },
            SpectrumCamera { pitch_deg: 75, ..d },
            SpectrumCamera { zoom_step: -8, ..d },
            SpectrumCamera { zoom_step: 4, ..d },
        ] {
            assert_eq!(with(ok), Ok(()), "{ok:?}");
        }
        for bad in [
            SpectrumCamera { yaw_deg: -81, ..d },
            SpectrumCamera { yaw_deg: 81, ..d },
            SpectrumCamera {
                yaw_deg: i8::MIN,
                ..d
            },
            SpectrumCamera { pitch_deg: 1, ..d },
            SpectrumCamera { pitch_deg: 76, ..d },
            SpectrumCamera { zoom_step: -9, ..d },
            SpectrumCamera { zoom_step: 5, ..d },
        ] {
            assert!(with(bad).is_err(), "{bad:?}");
        }
        let mut l = crate::samples::pane_layout();
        l.panes_mut()[0].spectrum_camera = SpectrumCamera { pitch_deg: 90, ..d };
        let before = l.clone();
        assert!(l.check().is_err());
        assert_eq!(l, before);
    }

    #[test]
    fn a_camera_round_trips_exactly() {
        let c = SpectrumCamera {
            yaw_deg: -35,
            pitch_deg: 17,
            zoom_step: -6,
        };
        let raw = rmp_serde::to_vec_named(&c).expect("msgpack");
        assert_eq!(
            rmp_serde::from_slice::<SpectrumCamera>(&raw).expect("read"),
            c
        );
    }
}
