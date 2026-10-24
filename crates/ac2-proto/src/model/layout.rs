//! A measurement's pane layout (`meas.layout`): the panes a client shows while that
//! measurement is selected. The arrangement, what each pane draws, the measurement each
//! pane was picked to show, the level ranges the operator set on it and where they dropped
//! the plates over its plot (and their text size) travel; which pane has the keyboard, a maximised or full-screen pane and the window stay with each client.

use serde::{Deserialize, Serialize};

use crate::units::MeasId;

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

/// What a pane was picked to show when it is not the measurement whose layout this is: a
/// measurement (its chip, a row dropped on it), or, for a Programme pane, the inputs or the
/// stereo pair it meters. Inputs are zero-based device input channels, as the session's
/// `input_channels` names them; a pane of a session loaded on a device without them shows
/// which are missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PaneContent {
    /// A measurement; its id is the session's, kept by a save and a load.
    Meas { meas: MeasId },
    /// Up to [`PaneContent::MAX_INPUTS`] inputs, metered side by side.
    Inputs { inputs: Vec<u16> },
    /// A stereo pair: its two bars, correlation and goniometer.
    Pair { left: u16, right: u16 },
}

impl PaneContent {
    /// Bars a Programme pane shows at most: more than eight no longer read at a glance,
    /// and a console feed is rarely wider.
    pub const MAX_INPUTS: usize = 8;

    /// The measurement, when that is what the content is.
    pub fn meas(&self) -> Option<MeasId> {
        match self {
            Self::Meas { meas } => Some(*meas),
            _ => None,
        }
    }

    /// The inputs a Programme pane with this content meters, left to right.
    pub fn inputs(&self) -> Vec<u16> {
        match self {
            Self::Meas { .. } => Vec::new(),
            Self::Inputs { inputs } => inputs.clone(),
            Self::Pair { left, right } => vec![*left, *right],
        }
    }

    /// Why the content cannot be shown, if it cannot: no inputs, more than
    /// [`Self::MAX_INPUTS`], one named twice, or a pair of one input.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Self::Meas { .. } => Ok(()),
            Self::Inputs { inputs } if inputs.is_empty() => {
                Err("a Programme pane names at least one input".into())
            }
            Self::Inputs { inputs } if inputs.len() > Self::MAX_INPUTS => Err(format!(
                "a Programme pane shows at most {} inputs, not {}",
                Self::MAX_INPUTS,
                inputs.len()
            )),
            Self::Inputs { inputs } => {
                let mut seen = inputs.clone();
                seen.sort_unstable();
                seen.dedup();
                if seen.len() != inputs.len() {
                    return Err("a Programme pane names each input once".into());
                }
                Ok(())
            }
            Self::Pair { left, right } if left == right => Err(format!(
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
    /// The SPL pane's view.
    pub spl_view: SplView,
    /// The sweep pane's view.
    pub sweep_view: SweepView,
    /// How much of the plot besides the curves.
    pub chrome: PlotChrome,
    /// What the pane was picked to show (its chip, a row dropped on it, a pair row); `None`:
    /// the measurement whose layout this is. A pick of a measurement gone since shows the
    /// layout's own measurement.
    #[serde(default)]
    pub content: Option<PaneContent>,
    /// The level axis ranges the operator set on the pane (zoomed, panned, reset, fitted);
    /// one left `None` is framed by the client when the pane first shows a curve.
    #[serde(default)]
    pub levels: LayoutLevels,
    /// Where the operator dropped the plates over the plot and their text size.
    #[serde(default)]
    pub plates: LayoutPlates,
}

/// A pane's plates over the plot: the legend, the cursor values, the mark's readout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutPlates {
    pub legend: LayoutPlate,
    pub readout: LayoutPlate,
    pub mark: LayoutPlate,
}

impl LayoutPlates {
    fn all(&self) -> [(&'static str, LayoutPlate); 3] {
        [
            ("legend", self.legend),
            ("readout", self.readout),
            ("mark", self.mark),
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
    /// not finite with its top above its bottom (no axis can be drawn on it), and a plate
    /// placed outside 0 … 1 or with a text scale not finite and above 0, a Programme pane
    /// without inputs (or another kind with them), and content [`PaneContent::check`]
    /// refuses.
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
            match (p.kind, &p.content) {
                (PaneKind::Programme, None | Some(PaneContent::Meas { .. })) => {
                    return Err("a Programme pane names the inputs or the pair it meters".into());
                }
                (
                    PaneKind::Transfer | PaneKind::Spectrum | PaneKind::Spl | PaneKind::Distortion,
                    Some(PaneContent::Inputs { .. } | PaneContent::Pair { .. }),
                ) => return Err("only a Programme pane meters inputs".into()),
                _ => {}
            }
            if let Some(c) = &p.content {
                c.check()?;
            }
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

    fn programme(content: Option<PaneContent>) -> PaneLayout {
        let mut l = crate::samples::pane_layout();
        for p in l.panes_mut() {
            if p.kind == PaneKind::Programme {
                p.content = content.clone();
            }
        }
        l
    }

    #[test]
    fn programme_content_is_checked() {
        assert_eq!(crate::samples::pane_layout().check(), Ok(()));
        let inputs = |v: Vec<u16>| Some(PaneContent::Inputs { inputs: v });
        assert_eq!(programme(inputs(vec![0, 1, 2])).check(), Ok(()));
        assert!(programme(None).check().is_err());
        assert!(
            programme(Some(PaneContent::Meas { meas: MeasId(1) }))
                .check()
                .is_err()
        );
        assert!(programme(inputs(vec![])).check().is_err());
        assert!(programme(inputs((0..9).collect())).check().is_err());
        assert!(programme(inputs(vec![1, 1])).check().is_err());
        assert_eq!(
            programme(Some(PaneContent::Pair { left: 3, right: 3 })).check(),
            Err("a pair is two inputs, not input 4 twice".into())
        );
        let mut l = crate::samples::pane_layout();
        l.panes_mut()[0].content = Some(PaneContent::Pair { left: 0, right: 1 });
        assert!(l.check().is_err(), "only a Programme pane meters inputs");
        let p = PaneContent::Pair { left: 2, right: 3 };
        assert_eq!(p.inputs(), vec![2, 3]);
        assert_eq!(p.meas(), None);
    }
}
