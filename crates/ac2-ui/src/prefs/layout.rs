//! The `[layout]` table: the pane tree and what each pane shows.

use super::file::*;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AxisFile {
    Row,
    Column,
}

/// A node of the pane tree: a leaf names its pane, a split its two halves.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum NodeFile {
    Leaf {
        pane: u32,
    },
    Split {
        split: AxisFile,
        ratio: f32,
        a: Box<NodeFile>,
        b: Box<NodeFile>,
    },
}

impl NodeFile {
    fn of(n: &PaneNode) -> Self {
        match n {
            PaneNode::Leaf(id) => Self::Leaf { pane: id.0 },
            PaneNode::Split { axis, ratio, a, b } => Self::Split {
                split: match axis {
                    Axis::Row => AxisFile::Row,
                    Axis::Column => AxisFile::Column,
                },
                ratio: *ratio,
                a: Box::new(Self::of(a)),
                b: Box::new(Self::of(b)),
            },
        }
    }

    /// The tree; `None` when a ratio is not a number (NaN passes a clamp, and a split by
    /// it has no place for either side).
    fn node(&self) -> Option<PaneNode> {
        Some(match self {
            Self::Leaf { pane } => PaneNode::Leaf(PaneId(*pane)),
            Self::Split { split, ratio, a, b } => PaneNode::Split {
                axis: match split {
                    AxisFile::Row => Axis::Row,
                    AxisFile::Column => Axis::Column,
                },
                ratio: ratio.is_finite().then(|| ratio.clamp(0.0, 1.0))?,
                a: Box::new(a.node()?),
                b: Box::new(b.node()?),
            },
        })
    }
}

/// What a Programme pane meters, one-based as the operator numbers inputs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum MetersFile {
    Inputs(Vec<u32>),
    Pair([u32; 2]),
}

impl MetersFile {
    fn of(m: &crate::state::Metered) -> Self {
        let one = |c: u16| u32::from(c) + 1;
        match m.pair() {
            Some((l, r)) => Self::Pair([one(l), one(r)]),
            None => Self::Inputs(m.inputs().into_iter().map(one).collect()),
        }
    }

    /// `None` for what no pane can meter (input 0, a repeat, more than eight): the pane
    /// then comes back empty rather than the file being refused.
    fn parse(&self) -> Option<crate::state::Metered> {
        let zero = |c: u32| c.checked_sub(1).and_then(|c| u16::try_from(c).ok());
        let content = match self {
            Self::Inputs(v) => ac2_proto::model::layout::PaneContent::Inputs {
                inputs: v.iter().map(|c| zero(*c)).collect::<Option<Vec<_>>>()?,
            },
            Self::Pair([l, r]) => ac2_proto::model::layout::PaneContent::Pair {
                left: zero(*l)?,
                right: zero(*r)?,
            },
        };
        crate::state::Metered::from_wire(&content)
    }
}

/// One pane: its kind, measurement and the modes of each kind it may show.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneEntryFile {
    id: u32,
    kind: PaneFile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    measurement: Option<String>,
    /// A Programme pane's one-based inputs, or its pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    meters: Option<MetersFile>,
    #[serde(default)]
    transfer_view: TransferViewFile,
    #[serde(default = "spl_meter_leq")]
    spl_view: SplViewFile,
    #[serde(default)]
    spectrum_view: SpectrumViewFile,
    #[serde(default)]
    sweep_view: SweepViewFile,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    sweep_reference: bool,
    #[serde(default, skip_serializing_if = "ChromeFile::is_full")]
    chrome: ChromeFile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    levels: Option<PaneLevelsFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plates: Option<PanePlatesFile>,
}

impl PaneEntryFile {
    fn of(p: &PanePrefs) -> Self {
        let m = &p.modes;
        Self {
            id: p.id.0,
            kind: PaneFile::of(p.kind),
            measurement: p.measurement.clone(),
            meters: p.meters.as_ref().map(MetersFile::of),
            transfer_view: match m.transfer {
                TransferView::Magnitude => TransferViewFile::Magnitude,
                TransferView::Phase => TransferViewFile::Phase,
                TransferView::Coherence => TransferViewFile::Coherence,
                TransferView::Alignment => TransferViewFile::Alignment,
                TransferView::Ir(IrMode::Linear) => TransferViewFile::IrLinear,
                TransferView::Ir(IrMode::Log) => TransferViewFile::IrLog,
                TransferView::Ir(IrMode::Etc) => TransferViewFile::Etc,
            },
            spl_view: match m.spl {
                SplMode::Meter => SplViewFile::Meter,
                SplMode::Leq => SplViewFile::Leq,
                SplMode::MeterLeq => SplViewFile::MeterLeq,
                SplMode::Bands => SplViewFile::Bands,
            },
            spectrum_view: match m.spectrum {
                SpectrumMode::Spectrum => SpectrumViewFile::Spectrum,
                SpectrumMode::Split => SpectrumViewFile::SpectrumSpectrograph,
                SpectrumMode::Spectrograph => SpectrumViewFile::Spectrograph,
            },
            sweep_view: match m.sweep {
                SweepMode::Response => SweepViewFile::Response,
                SweepMode::Ir(IrMode::Linear) => SweepViewFile::IrLinear,
                SweepMode::Ir(IrMode::Log) => SweepViewFile::IrLog,
                SweepMode::Ir(IrMode::Etc) => SweepViewFile::Etc,
                SweepMode::Room => SweepViewFile::Room,
            },
            sweep_reference: m.sweep_reference,
            chrome: ChromeFile::of(m.chrome),
            levels: PaneLevelsFile::of(&p.levels),
            plates: PanePlatesFile::of(&p.plates),
        }
    }

    fn parse(&self) -> PanePrefs {
        PanePrefs {
            id: PaneId(self.id),
            kind: self.kind.pane(),
            measurement: self.measurement.clone(),
            meters: self.meters.as_ref().and_then(MetersFile::parse),
            modes: PaneModes {
                transfer: match self.transfer_view {
                    TransferViewFile::Magnitude => TransferView::Magnitude,
                    TransferViewFile::Phase => TransferView::Phase,
                    TransferViewFile::Coherence => TransferView::Coherence,
                    TransferViewFile::Alignment => TransferView::Alignment,
                    TransferViewFile::IrLinear => TransferView::Ir(IrMode::Linear),
                    TransferViewFile::IrLog => TransferView::Ir(IrMode::Log),
                    TransferViewFile::Etc => TransferView::Ir(IrMode::Etc),
                },
                spl: match self.spl_view {
                    SplViewFile::Meter => SplMode::Meter,
                    SplViewFile::Leq => SplMode::Leq,
                    SplViewFile::MeterLeq => SplMode::MeterLeq,
                    SplViewFile::Bands => SplMode::Bands,
                },
                spectrum: match self.spectrum_view {
                    SpectrumViewFile::Spectrum => SpectrumMode::Spectrum,
                    SpectrumViewFile::SpectrumSpectrograph => SpectrumMode::Split,
                    SpectrumViewFile::Spectrograph => SpectrumMode::Spectrograph,
                },
                sweep: match self.sweep_view {
                    SweepViewFile::Response => SweepMode::Response,
                    SweepViewFile::IrLinear => SweepMode::Ir(IrMode::Linear),
                    SweepViewFile::IrLog => SweepMode::Ir(IrMode::Log),
                    SweepViewFile::Etc => SweepMode::Ir(IrMode::Etc),
                    SweepViewFile::Room => SweepMode::Room,
                },
                sweep_reference: self.sweep_reference,
                chrome: self.chrome.chrome(),
            },
            levels: PaneLevelsFile::parse(self.levels.as_ref(), self.id),
            plates: PanePlatesFile::parse(self.plates.as_ref(), self.id),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct LayoutFile {
    /// The focused pane's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    focus: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    maximized: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    fullscreen: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    panes_only: bool,
    #[serde(default = "unit_db")]
    distortion_unit: UnitFile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden: Vec<String>,
    /// Last: tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tree: Option<NodeFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    panes: Vec<PaneEntryFile>,
    /// Keys this version does not know: logged and left out of the next save, so one stale
    /// key costs only itself, never the pane tree.
    #[serde(flatten, skip_serializing)]
    unknown: BTreeMap<String, toml::Value>,
}

fn spl_meter_leq() -> SplViewFile {
    SplViewFile::MeterLeq
}

fn unit_db() -> UnitFile {
    UnitFile::Db
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// The `[layout]` table if it parses, else none and a warning naming why.
pub(super) fn lenient_layout<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<LayoutFile>, D::Error> {
    let v = toml::Value::deserialize(d)?;
    match LayoutFile::deserialize(v) {
        Ok(l) => Ok(Some(l)),
        Err(e) => {
            tracing::warn!("ui.toml: [layout] dropped, one pane instead: {e}");
            Ok(None)
        }
    }
}

impl LayoutFile {
    pub(super) fn parse(self) -> LayoutPrefs {
        for key in self.unknown.keys() {
            tracing::warn!("ui.toml: unknown [layout] setting `{key}` ignored");
        }
        let panes = self.tree.as_ref().and_then(|t| {
            let Some(root) = t.node() else {
                tracing::warn!(
                    "ui.toml: pane tree dropped, one pane instead: a ratio is not a number"
                );
                return None;
            };
            let leaves = root.leaves();
            let mut seen = BTreeSet::new();
            // A pane twice in the tree has no place to be drawn, and two entries of one id
            // say two things of one pane: either way the tree is dropped.
            if !leaves.iter().all(|id| seen.insert(*id)) {
                tracing::warn!(
                    "ui.toml: pane tree dropped, one pane instead: a pane twice in the tree"
                );
                return None;
            }
            let mut seen = BTreeSet::new();
            if !self.panes.iter().all(|p| seen.insert(p.id)) {
                tracing::warn!(
                    "ui.toml: pane tree dropped, one pane instead: two [[layout.panes]] of one id"
                );
                return None;
            }
            let views: Vec<PanePrefs> = self
                .panes
                .iter()
                .map(PaneEntryFile::parse)
                .filter(|p| leaves.contains(&p.id))
                .collect();
            Some(PanesPrefs {
                focus: self.focus.map_or(leaves[0], PaneId),
                root,
                views,
            })
        });
        LayoutPrefs {
            panes,
            maximized: self.maximized,
            fullscreen: self.fullscreen,
            panes_only: self.panes_only,
            distortion_unit: match self.distortion_unit {
                UnitFile::Db => DistortionUnit::Db,
                UnitFile::Percent => DistortionUnit::Percent,
            },
            hidden: self.hidden.into_iter().collect(),
        }
    }

    pub(super) fn from_prefs(l: &LayoutPrefs) -> Self {
        Self {
            focus: l.panes.as_ref().map(|p| p.focus.0),
            maximized: l.maximized,
            fullscreen: l.fullscreen,
            panes_only: l.panes_only,
            distortion_unit: match l.distortion_unit {
                DistortionUnit::Db => UnitFile::Db,
                DistortionUnit::Percent => UnitFile::Percent,
            },
            hidden: l.hidden.iter().cloned().collect(),
            tree: l.panes.as_ref().map(|p| NodeFile::of(&p.root)),
            panes: l
                .panes
                .as_ref()
                .map(|p| p.views.iter().map(PaneEntryFile::of).collect())
                .unwrap_or_default(),
            unknown: BTreeMap::new(),
        }
    }
}
