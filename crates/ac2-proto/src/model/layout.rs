//! A measurement's pane layout (`meas.layout`): the panes a client shows while that
//! measurement is selected. The arrangement, what each pane draws, the measurement each
//! pane was picked to show and the level ranges the operator set on it travel; which pane
//! has the keyboard, a maximised or full-screen pane and the window stay with each client.

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

/// What a pane draws: its kind and the view of each kind it may be turned into.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
    /// The measurement the pane was picked to show (its chip, a row dropped on it); `None`:
    /// the measurement whose layout this is. Ids are the session's, kept by a save and a
    /// load; a pick of a measurement gone since shows the layout's own measurement.
    #[serde(default)]
    pub meas: Option<MeasId>,
    /// The level axis ranges the operator set on the pane (zoomed, panned, reset, fitted);
    /// one left `None` is framed by the client when the pane first shows a curve.
    #[serde(default)]
    pub levels: LayoutLevels,
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
}

/// The transfer pane's views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferView {
    Magnitude,
    Phase,
    Coherence,
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
    /// and 1 leaves one side no room, more than [`Self::MAX_PANES`] panes, and a level
    /// range not finite with its top above its bottom (no axis can be drawn on it).
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
