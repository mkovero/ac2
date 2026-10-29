//! The rows of the walk setup window and of the plan editor (K9: ↑/↓ rows, ←/→ choices,
//! typed fields, no letter keys), the rules the suggestion relaxed and the rules edited
//! points break, in words.

use ac2_proto::model::{MarkerKind, PlanSpec, WalkState};
use ac2_room::pattern::{Broken, PresetKind, Relaxed};

/// The setup window's title.
pub const SETUP_TITLE: &str = "New spatial walk";

/// The plan row's choice that opens the plan editor.
pub const NEW_PLAN: &str = "New plan…";

/// The plan editor's title.
pub const EDITOR_TITLE: &str = "Room plan";

/// A row of the setup window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetupField {
    Name,
    Plan,
    Zones,
    Preset,
    /// The reverberation time ISO 3382-1's source distance needs.
    ReverbTime,
    Count,
    Pattern,
    /// The walk's k-th mic (a transfer measurement); past the chosen ones, a row to add one.
    Mic(u8),
}

impl SetupField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Plan => "plan",
            Self::Zones => "zones",
            Self::Preset => "preset",
            Self::ReverbTime => "RT",
            Self::Count => "positions",
            Self::Pattern => "pattern",
            Self::Mic(k) => MIC_ROWS.get(usize::from(k)).copied().unwrap_or("mic"),
        }
    }

    /// Whether the row takes typed text.
    pub fn typed(self) -> bool {
        matches!(self, Self::Name | Self::ReverbTime | Self::Count)
    }
}

/// The mic rows' labels: a slot each, the mics themselves named in the values.
const MIC_ROWS: [&str; WalkState::MAX_MICS] = [
    "mic", "mic 2", "mic 3", "mic 4", "mic 5", "mic 6", "mic 7", "mic 8",
];

/// The setup window's rows for `preset`, top to bottom (the RT only for ISO 3382-1), with
/// `mics` mic rows (at least one).
pub fn setup_fields(preset: PresetKind, mics: usize) -> Vec<SetupField> {
    let mut v = vec![
        SetupField::Name,
        SetupField::Plan,
        SetupField::Zones,
        SetupField::Preset,
    ];
    if preset == PresetKind::Iso3382 {
        v.push(SetupField::ReverbTime);
    }
    v.extend([SetupField::Count, SetupField::Pattern]);
    let rows = mics.clamp(1, WalkState::MAX_MICS);
    v.extend((0..rows).map(|k| SetupField::Mic(k as u8)));
    v
}

/// The preset's name.
pub fn preset_name(k: PresetKind) -> &'static str {
    match k {
        PresetKind::Tuning => "Tuning",
        PresetKind::Iso3382 => "ISO 3382-1",
    }
}

/// One row as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub value: String,
    pub focused: bool,
}

/// What the setup window shows, from the setup's values.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupInput<'a> {
    pub name: &'a str,
    /// The plan picked; `None`: the plan row is on [`NEW_PLAN`].
    pub plan: Option<&'a PlanSpec>,
    /// The one zone picked; `None`: every zone.
    pub zone: Option<usize>,
    pub preset: PresetKind,
    pub reverb_s: f64,
    pub n: usize,
    /// The preset's position count; `None` when ISO 3382-1 needs the zones' seats and one
    /// is unset.
    pub default_n: Option<usize>,
    pub seed: u64,
    /// The transfer measurements that capture, one per mic, by name.
    pub mics: &'a [&'a str],
    /// Whether a row to add a mic follows the chosen ones (another transfer on its own
    /// input is there to add).
    pub add_mic: bool,
    pub focus: SetupField,
    /// Text typed into the focused row, not yet taken.
    pub typed: Option<&'a str>,
    /// Rules the suggestion relaxed.
    pub relaxed: &'a [Relaxed],
    /// The points' labels and, per point, the rules it breaks after the operator's edits.
    pub labels: &'a [String],
    pub breaks: &'a [Vec<Broken>],
    /// Why there is no suggestion, or why Enter cannot start the walk.
    pub error: Option<&'a str>,
}

/// The setup window as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupView {
    pub title: &'static str,
    pub rows: Vec<Row>,
    /// Relaxed rules, then each point that breaks one.
    pub notes: Vec<String>,
    pub error: Option<String>,
    pub hint: String,
}

/// Metres to a tenth, without a minus sign on a zero.
pub fn tenths(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    format!("{:.1}", if r == 0.0 { 0.0 } else { r })
}

fn typed_value(t: &str) -> String {
    format!("{t}▏")
}

/// `relaxed: spacing 1.6 m: 8 positions do not fit at 2.0 m`, then per point that breaks a
/// rule `P3: 0.5 m from P1 (spacing 2.0 m)`.
pub fn notes(relaxed: &[Relaxed], labels: &[String], breaks: &[Vec<Broken>]) -> Vec<String> {
    let name = |i: usize| {
        labels
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("P{}", i + 1))
    };
    let mut v: Vec<String> = relaxed.iter().map(|r| format!("relaxed: {r}")).collect();
    for (i, b) in breaks.iter().enumerate() {
        if b.is_empty() {
            continue;
        }
        let texts: Vec<String> = b.iter().map(|r| r.text(name)).collect();
        v.push(format!("{}: {}", name(i), texts.join(", ")));
    }
    v
}

pub fn setup_view(i: &SetupInput<'_>) -> SetupView {
    let mic_rows = i.mics.len() + usize::from(i.add_mic || i.mics.is_empty());
    let rows = setup_fields(i.preset, mic_rows)
        .into_iter()
        .map(|f| {
            let focused = f == i.focus;
            let value = match (focused, i.typed) {
                (true, Some(t)) if f.typed() => typed_value(t),
                _ => match f {
                    SetupField::Name => i.name.to_owned(),
                    SetupField::Plan => match i.plan {
                        Some(p) => format!(
                            "{} · {} zone{}",
                            p.name,
                            p.zones.len(),
                            if p.zones.len() == 1 { "" } else { "s" }
                        ),
                        None => NEW_PLAN.to_owned(),
                    },
                    SetupField::Zones => match (i.plan, i.zone) {
                        (None, _) => "—".to_owned(),
                        (Some(p), None) if p.zones.is_empty() => {
                            "none: the plan has no zone".to_owned()
                        }
                        (Some(p), None) => format!(
                            "all ({})",
                            p.zones
                                .iter()
                                .map(|z| z.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        (Some(p), Some(z)) => p
                            .zones
                            .get(z)
                            .map_or_else(|| "—".to_owned(), |z| z.name.clone()),
                    },
                    SetupField::Preset => preset_name(i.preset).to_owned(),
                    SetupField::ReverbTime => format!("{} s", tenths(i.reverb_s)),
                    SetupField::Count => match i.default_n {
                        Some(_) => i.n.to_string(),
                        None => format!("{} (a zone's seats are unset)", i.n),
                    },
                    SetupField::Pattern => i.seed.to_string(),
                    SetupField::Mic(k) => match i.mics.get(usize::from(k)) {
                        Some(m) => (*m).to_owned(),
                        None if k == 0 => "no transfer measurement".to_owned(),
                        None => "none · →: another mic".to_owned(),
                    },
                },
            };
            Row {
                label: f.label().to_owned(),
                value,
                focused,
            }
        })
        .collect();
    let enter = if i.focus == SetupField::Plan && i.plan.is_none() {
        "Enter opens the plan editor"
    } else {
        "Enter starts the walk"
    };
    SetupView {
        title: SETUP_TITLE,
        rows,
        notes: notes(i.relaxed, i.labels, i.breaks),
        error: i.error.map(str::to_owned),
        hint: format!(
            "↑/↓ row · ←/→ change · type a value · {enter} · Esc closes · on the plan: drag a \
             point, click to add one, right-click to delete"
        ),
    }
}

/// What the plan editor edits on the plan view and in its table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Outline,
    Zone(usize),
    Markers,
}

/// A row of the plan editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorField {
    Name,
    /// Metres; empty: outdoors.
    Ceiling,
    /// Outline, a zone or the markers (←/→).
    Layer,
    ZoneName,
    EarHeight,
    /// A vertex of the outline or the zone, or a marker.
    Row(usize),
}

impl EditorField {
    /// Whether the row takes typed text (a marker row takes `x y heading`, ←/→ its kind).
    pub fn typed(self) -> bool {
        !matches!(self, Self::Layer)
    }
}

/// The editor's rows for `layer` of `plan`, top to bottom.
pub fn editor_fields(plan: &PlanSpec, layer: Layer) -> Vec<EditorField> {
    let mut v = vec![EditorField::Name, EditorField::Ceiling, EditorField::Layer];
    let n = match layer {
        Layer::Outline => plan.outline.len(),
        Layer::Zone(z) => {
            v.extend([EditorField::ZoneName, EditorField::EarHeight]);
            plan.zones.get(z).map_or(0, |z| z.outline.len())
        }
        Layer::Markers => plan.markers.len(),
    };
    v.extend((0..n).map(EditorField::Row));
    v
}

/// The marker kind's word.
pub fn marker_kind(k: MarkerKind) -> &'static str {
    match k {
        MarkerKind::Speaker => "speaker",
        MarkerKind::Foh => "FOH",
        MarkerKind::Point => "point",
    }
}

/// The layer row's value: `outline · 4 vertices`, `zone Stalls · 4 vertices`, `markers · 2`.
pub fn layer_text(plan: &PlanSpec, layer: Layer) -> String {
    let vertices = |n: usize| format!("{n} vert{}", if n == 1 { "ex" } else { "ices" });
    match layer {
        Layer::Outline => format!("outline · {}", vertices(plan.outline.len())),
        Layer::Zone(z) => match plan.zones.get(z) {
            Some(zone) => format!("zone {} · {}", zone.name, vertices(zone.outline.len())),
            None => "zone —".to_owned(),
        },
        Layer::Markers => format!("markers · {}", plan.markers.len()),
    }
}

/// A vertex row's value: `12.0 4.5`.
pub fn vertex_text(v: [f64; 2]) -> String {
    format!("{} {}", tenths(v[0]), tenths(v[1]))
}

/// What the plan editor shows.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorInput<'a> {
    pub plan: &'a PlanSpec,
    pub layer: Layer,
    pub focus: EditorField,
    pub typed: Option<&'a str>,
    /// What `ac2_room::plan::validate` says, in words.
    pub verdict: Result<(), String>,
    /// Why the typed text was not taken.
    pub row_error: Option<&'a str>,
}

/// The plan editor as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorView {
    pub title: &'static str,
    pub rows: Vec<Row>,
    /// *valid · Enter saves it* or *refused: edge 3 crosses edge 6*.
    pub verdict: String,
    pub refused: bool,
    pub row_error: Option<String>,
    pub hint: String,
}

pub fn editor_view(i: &EditorInput<'_>) -> EditorView {
    let p = i.plan;
    let rows = editor_fields(p, i.layer)
        .into_iter()
        .map(|f| {
            let focused = f == i.focus;
            let (label, value) = match f {
                EditorField::Name => ("name".to_owned(), p.name.clone()),
                EditorField::Ceiling => (
                    "ceiling".to_owned(),
                    p.ceiling.map_or_else(
                        || "none (outdoors)".to_owned(),
                        |c| format!("{} m", tenths(c)),
                    ),
                ),
                EditorField::Layer => ("editing".to_owned(), layer_text(p, i.layer)),
                EditorField::ZoneName => (
                    "zone name".to_owned(),
                    match i.layer {
                        Layer::Zone(z) => {
                            p.zones.get(z).map(|z| z.name.clone()).unwrap_or_default()
                        }
                        _ => String::new(),
                    },
                ),
                EditorField::EarHeight => (
                    "ear height".to_owned(),
                    match i.layer {
                        Layer::Zone(z) => p
                            .zones
                            .get(z)
                            .map(|z| format!("{} m", tenths(z.ear_height)))
                            .unwrap_or_default(),
                        _ => String::new(),
                    },
                ),
                EditorField::Row(k) => match i.layer {
                    Layer::Outline => (
                        (k + 1).to_string(),
                        p.outline
                            .get(k)
                            .copied()
                            .map(vertex_text)
                            .unwrap_or_default(),
                    ),
                    Layer::Zone(z) => (
                        (k + 1).to_string(),
                        p.zones
                            .get(z)
                            .and_then(|z| z.outline.get(k))
                            .copied()
                            .map(vertex_text)
                            .unwrap_or_default(),
                    ),
                    Layer::Markers => match p.markers.get(k) {
                        Some(m) => (
                            m.name.clone(),
                            format!(
                                "{} · {}{}",
                                marker_kind(m.kind),
                                vertex_text([m.at[0], m.at[1]]),
                                m.heading_deg
                                    .map(|h| format!(" · heading {h:.0}°"))
                                    .unwrap_or_default()
                            ),
                        ),
                        None => (String::new(), String::new()),
                    },
                },
            };
            let value = match (focused, i.typed) {
                (true, Some(t)) if f.typed() => typed_value(t),
                _ => value,
            };
            Row {
                label,
                value,
                focused,
            }
        })
        .collect();
    let (verdict, refused) = match &i.verdict {
        Ok(()) => (
            "valid · Enter saves it to the plan library".to_owned(),
            false,
        ),
        Err(e) => (format!("refused: {e}"), true),
    };
    let rows_word = match i.layer {
        Layer::Markers => "a marker (x y heading; ←/→ its kind)",
        _ => "a vertex (x y)",
    };
    EditorView {
        title: EDITOR_TITLE,
        rows,
        verdict,
        refused,
        row_error: i.row_error.map(str::to_owned),
        hint: format!(
            "↑/↓ row · ←/→ outline, zones, markers · type {rows_word} · Insert adds a row (on \
             the editing row: a zone) · Delete removes it · on the plan: click adds, drag \
             moves · Enter saves · Esc closes"
        ),
    }
}
