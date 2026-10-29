//! The spatial walk's setup window and plan editor as the reducer holds them: what is chosen,
//! the row in focus, the typed text, the points the suggestion placed and the operator moved.
//! No key or egui here; the texts are `ac2_scene::walk::setup`'s, the rules `ac2_room`'s.

use ac2_proto::model::{Marker, MarkerKind, PlanSpec, WalkPointSpec, WalkState, ZoneSpec};
use ac2_proto::units::MeasId;
use ac2_room::pattern::{Broken, Preset, PresetKind, Relaxed, breaks, suggest};
use ac2_scene::walk::setup::{EditorField, Layer, SetupField, editor_fields, setup_fields};
use ac2_scene::walk::snap;

/// The ear height of a point added outside every zone: a seated listener's.
const SEATED_EAR_M: f64 = 1.2;

/// The height a new speaker marker is given: a typical flown or stacked box's centre.
const SPEAKER_HEIGHT_M: f64 = 3.0;

/// The reverberation time the ISO 3382-1 row starts at, seconds.
pub const DEFAULT_RT_S: f64 = 1.0;

/// A mouse step on a plan view, in plan metres (the view reads them through its
/// `PlanMap`); `reach_m` is how near a point must be to be the one pressed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlanMsg {
    /// The button went down: on a point, it is dragged; elsewhere a point is added there.
    Press {
        at: [f64; 2],
        reach_m: f64,
    },
    /// The pointer moved with the button held.
    Drag {
        at: [f64; 2],
    },
    Release,
    /// The secondary button: the point under it goes.
    Remove {
        at: [f64; 2],
        reach_m: f64,
    },
}

/// The point of `points` nearest `at` within `reach_m`.
fn nearest(points: impl Iterator<Item = [f64; 2]>, at: [f64; 2], reach_m: f64) -> Option<usize> {
    points
        .enumerate()
        .map(|(i, p)| (i, (p[0] - at[0]).hypot(p[1] - at[1])))
        .filter(|(_, d)| *d <= reach_m)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A number as typed: `1.5`, `1,5`, with or without a trailing unit.
fn number(t: &str, unit: &str) -> Option<f64> {
    let t = t.trim();
    let t = t.strip_suffix(unit).unwrap_or(t).trim().replace(',', ".");
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `x y` (or `x y heading`), separated by spaces or a semicolon.
fn numbers(t: &str) -> Option<Vec<f64>> {
    t.split(|c: char| c.is_whitespace() || c == ';')
        .filter(|s| !s.is_empty())
        .map(|s| number(s, ""))
        .collect()
}

/// The setup window (§4): a plan, the zones, the preset's rules and the count give the
/// suggested points; the operator may move, add and delete them before the walk starts.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkSetup {
    pub name: String,
    /// The plan's name; `None`: *New plan…*.
    pub plan: Option<String>,
    /// A plan just saved from the editor, until the daemon's library shows it.
    pub saved: Option<PlanSpec>,
    /// One zone; `None`: every zone.
    pub zone: Option<usize>,
    pub preset: PresetKind,
    pub reverb_s: f64,
    /// The count asked for; `None`: the preset's default.
    pub n: Option<usize>,
    pub seed: u64,
    /// The transfer measurements that capture, one per mic, in mic order.
    pub mics: Vec<MeasId>,
    /// Whether another offered transfer could be added (on an input none chosen measures).
    pub add_mic: bool,
    pub focus: SetupField,
    pub typed: Option<String>,
    /// Why the typed text or the suggestion was refused.
    pub error: Option<String>,
    /// The points, suggested then edited.
    pub points: Vec<[f64; 3]>,
    pub relaxed: Vec<Relaxed>,
    pub default_n: Option<usize>,
    /// The point being dragged.
    pub drag: Option<usize>,
    /// `walk.new` was sent: Enter waits for its answer.
    pub sent: bool,
}

impl WalkSetup {
    /// A setup over the first plan of `plans`, if any, and the first of `mics` (the session's
    /// transfer measurements and the input each measures).
    pub fn new(name: String, plans: &[PlanSpec], mics: &[(MeasId, u16)]) -> Self {
        let mut s = Self {
            name,
            plan: plans.first().map(|p| p.name.clone()),
            saved: None,
            zone: None,
            preset: PresetKind::Tuning,
            reverb_s: DEFAULT_RT_S,
            n: None,
            seed: 1,
            mics: mics.first().map(|m| m.0).into_iter().collect(),
            add_mic: false,
            focus: SetupField::Plan,
            typed: None,
            error: None,
            points: Vec::new(),
            relaxed: Vec::new(),
            default_n: None,
            drag: None,
            sent: false,
        };
        s.add_mic = !s.mic_choices(s.mics.len(), mics).is_empty();
        let plan = s.plan_in(plans).cloned();
        s.refresh(plan.as_ref());
        s
    }

    /// What mic row `k` may take: the offered transfers no other row chose, on an input no
    /// other row measures (one mic stands at one point).
    fn mic_choices(&self, k: usize, offered: &[(MeasId, u16)]) -> Vec<MeasId> {
        let others: Vec<(MeasId, u16)> = self
            .mics
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != k)
            .filter_map(|(_, m)| offered.iter().find(|o| o.0 == *m).copied())
            .collect();
        offered
            .iter()
            .filter(|o| !others.iter().any(|x| x.0 == o.0 || x.1 == o.1))
            .map(|o| o.0)
            .collect()
    }

    /// The chosen plan among `plans` (the library and the session's copies), or the one
    /// just saved.
    pub fn plan_in<'a>(&'a self, plans: &'a [PlanSpec]) -> Option<&'a PlanSpec> {
        let name = self.plan.as_ref()?;
        plans
            .iter()
            .find(|p| &p.name == name)
            .or(self.saved.as_ref().filter(|p| &p.name == name))
    }

    /// The zones the suggestion covers.
    pub fn zones(&self, plan: &PlanSpec) -> Vec<usize> {
        match self.zone {
            Some(z) if z < plan.zones.len() => vec![z],
            _ => (0..plan.zones.len()).collect(),
        }
    }

    /// The preset's rules for `plan`, or why not.
    pub fn rules(&self, plan: &PlanSpec) -> Result<Preset, String> {
        match self.preset {
            PresetKind::Tuning => Ok(Preset::tuning()),
            PresetKind::Iso3382 => Preset::iso3382(plan, self.reverb_s).map_err(|e| e.to_string()),
        }
    }

    /// The count the suggestion places.
    pub fn count(&self) -> usize {
        self.n.or(self.default_n).unwrap_or(8)
    }

    /// A new suggestion for the choices made: any edit to the points is replaced.
    pub fn refresh(&mut self, plan: Option<&PlanSpec>) {
        self.points.clear();
        self.relaxed.clear();
        self.drag = None;
        self.default_n = None;
        let Some(plan) = plan else {
            return;
        };
        let zones = self.zones(plan);
        let preset = match self.rules(plan) {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        self.default_n = preset.default_n(plan, &zones);
        match suggest(plan, &zones, self.count(), &preset, self.seed) {
            Ok(s) => {
                self.points = s.points.iter().map(|p| p.at).collect();
                self.relaxed = s.relaxed;
            }
            Err(e) => self.error = Some(format!("no positions: {e}")),
        }
    }

    /// The rows shown: a mic row per chosen mic, and one to add another while one is there.
    pub fn fields(&self) -> Vec<SetupField> {
        let add = self.add_mic && self.mics.len() < WalkState::MAX_MICS;
        setup_fields(self.preset, self.mics.len() + usize::from(add))
    }

    /// The labels of the points, in walking order.
    pub fn labels(&self) -> Vec<String> {
        (1..=self.points.len()).map(|i| format!("P{i}")).collect()
    }

    /// The rules each point breaks.
    pub fn breaks(&self, plan: &PlanSpec) -> Vec<Vec<Broken>> {
        match self.rules(plan) {
            Ok(p) => breaks(plan, &self.points, &p),
            Err(_) => vec![Vec::new(); self.points.len()],
        }
    }

    /// ↑/↓: the next or previous row, the typed text taken first.
    pub fn step_row(&mut self, d: isize, plan: Option<&PlanSpec>) {
        if !self.commit(plan) {
            return;
        }
        let f = self.fields();
        let i = f.iter().position(|x| *x == self.focus).unwrap_or(0) as isize;
        self.focus = f[(i + d).clamp(0, f.len() as isize - 1) as usize];
    }

    /// ←/→ on the focused row. `plans` are the names offered, `mics` the transfer
    /// measurements and their inputs; the caller brings the zone into range and refreshes
    /// the suggestion.
    pub fn change(&mut self, d: isize, plans: &[String], mics: &[(MeasId, u16)]) {
        self.typed = None;
        self.error = None;
        let cycle = |n: usize, i: usize| ((i as isize + d).rem_euclid(n as isize)) as usize;
        match self.focus {
            SetupField::Name => {}
            SetupField::Plan => {
                // The plans, then *New plan…*.
                let n = plans.len() + 1;
                let i = self
                    .plan
                    .as_ref()
                    .and_then(|p| plans.iter().position(|x| x == p))
                    .unwrap_or(plans.len());
                let j = cycle(n, i);
                self.plan = plans.get(j).cloned();
                self.zone = None;
            }
            SetupField::Zones => {
                // Every zone, then each alone; the count is the plan's.
                self.zone = match self.zone {
                    None if d > 0 => Some(0),
                    None => Some(usize::MAX),
                    Some(z) if d > 0 => z.checked_add(1),
                    Some(0) => None,
                    Some(z) => Some(z - 1),
                };
            }
            SetupField::Preset => {
                self.preset = match self.preset {
                    PresetKind::Tuning => PresetKind::Iso3382,
                    PresetKind::Iso3382 => PresetKind::Tuning,
                };
                self.n = None;
            }
            SetupField::ReverbTime => {
                self.reverb_s = ((self.reverb_s * 10.0).round() + d as f64).max(1.0) / 10.0;
            }
            SetupField::Count => {
                let n = self.count() as isize + d;
                self.n = Some(n.clamp(1, WalkState::MAX_POINTS as isize) as usize);
            }
            SetupField::Pattern => {
                self.seed = (self.seed as i64 + d as i64).max(1) as u64;
            }
            SetupField::Mic(k) => {
                let k = usize::from(k);
                // The first mic is always one; a later row cycles through none as well,
                // and none drops it and the rows after it.
                let mut choices: Vec<Option<MeasId>> =
                    self.mic_choices(k, mics).into_iter().map(Some).collect();
                if k > 0 {
                    choices.insert(0, None);
                }
                if !choices.is_empty() {
                    let now = self.mics.get(k).copied();
                    let i = choices.iter().position(|c| *c == now).unwrap_or(0);
                    match choices[cycle(choices.len(), i)] {
                        Some(m) if k < self.mics.len() => self.mics[k] = m,
                        Some(m) => self.mics.push(m),
                        None => self.mics.truncate(k),
                    }
                }
                self.add_mic = !self.mic_choices(self.mics.len(), mics).is_empty();
            }
        }
    }

    /// The zone row's choice brought into the plan's range: past the last zone is every
    /// zone again, before the first the last.
    pub fn clamp_zone(&mut self, plan: Option<&PlanSpec>) {
        let n = plan.map_or(0, |p| p.zones.len());
        self.zone = match self.zone {
            Some(z) if z == usize::MAX => n.checked_sub(1),
            Some(z) if z >= n => None,
            z => z,
        };
    }

    /// Typed text on a typed row.
    pub fn type_text(&mut self, t: &str) {
        if self.focus.typed() {
            self.typed.get_or_insert_with(String::new).push_str(t);
            self.error = None;
        }
    }

    pub fn backspace(&mut self) {
        if let Some(t) = &mut self.typed {
            t.pop();
        }
    }

    /// Takes the typed text into its row; `false` (and the reason in `error`) when it does
    /// not read. A changed choice makes a new suggestion.
    pub fn commit(&mut self, plan: Option<&PlanSpec>) -> bool {
        let Some(t) = self.typed.take() else {
            return true;
        };
        let refused = match self.focus {
            SetupField::Name => match t.trim() {
                "" => Some("a walk needs a name".to_owned()),
                n => {
                    self.name = n.to_owned();
                    None
                }
            },
            SetupField::ReverbTime => match number(&t, "s").filter(|v| *v > 0.0) {
                Some(v) => {
                    self.reverb_s = v;
                    self.refresh(plan);
                    None
                }
                None => Some(format!("{t:?}: a reverberation time in seconds, above 0")),
            },
            SetupField::Count => match t.trim().parse::<usize>() {
                Ok(n @ 1..=WalkState::MAX_POINTS) => {
                    self.n = Some(n);
                    self.refresh(plan);
                    None
                }
                _ => Some(format!(
                    "{t:?}: positions are 1 … {}",
                    WalkState::MAX_POINTS
                )),
            },
            _ => None,
        };
        match refused {
            Some(e) => {
                self.typed = Some(t);
                self.error = Some(e);
                false
            }
            None => true,
        }
    }

    /// The ear height at `at`: its zone's, else the first zone walked.
    fn ear_height(&self, plan: &PlanSpec, at: [f64; 2]) -> f64 {
        plan.zones
            .iter()
            .find(|z| ac2_room::plan::contains(&z.outline, at))
            .or_else(|| self.zones(plan).first().and_then(|z| plan.zones.get(*z)))
            .map_or(SEATED_EAR_M, |z| z.ear_height)
    }

    /// A mouse step on the plan: drag a point, add one, delete one.
    pub fn plan_msg(&mut self, m: PlanMsg, plan: &PlanSpec) {
        let flat = |p: &[f64; 3]| [p[0], p[1]];
        match m {
            PlanMsg::Press { at, reach_m } => {
                match nearest(self.points.iter().map(flat), at, reach_m) {
                    Some(i) => self.drag = Some(i),
                    None if self.points.len() < WalkState::MAX_POINTS => {
                        let at = snap(at);
                        self.points.push([at[0], at[1], self.ear_height(plan, at)]);
                        self.drag = Some(self.points.len() - 1);
                    }
                    None => {
                        self.error = Some(format!(
                            "a walk has at most {} positions",
                            WalkState::MAX_POINTS
                        ));
                    }
                }
            }
            PlanMsg::Drag { at } => {
                if let Some(i) = self.drag {
                    let at = snap(at);
                    let h = self.ear_height(plan, at);
                    if let Some(p) = self.points.get_mut(i) {
                        *p = [at[0], at[1], h];
                    }
                }
            }
            PlanMsg::Release => self.drag = None,
            PlanMsg::Remove { at, reach_m } => {
                if let Some(i) = nearest(self.points.iter().map(flat), at, reach_m) {
                    self.points.remove(i);
                }
                self.drag = None;
            }
        }
    }

    /// The points as `walk.new` takes them.
    pub fn point_specs(&self) -> Vec<WalkPointSpec> {
        self.labels()
            .into_iter()
            .zip(&self.points)
            .map(|(label, at)| WalkPointSpec { label, at: *at })
            .collect()
    }
}

/// The plan editor (§3): the outline, each zone and the markers, as tables of typed rows and
/// on the plan view; saved to the library once it validates.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanEditor {
    pub plan: PlanSpec,
    pub layer: Layer,
    pub focus: EditorField,
    pub typed: Option<String>,
    pub row_error: Option<String>,
    pub drag: Option<usize>,
    /// The setup window it opened from, which it returns to.
    pub back: Box<WalkSetup>,
}

impl PlanEditor {
    /// An empty plan named `name`.
    pub fn new(name: String, back: WalkSetup) -> Self {
        Self {
            plan: PlanSpec {
                name,
                outline: Vec::new(),
                ceiling: None,
                zones: Vec::new(),
                markers: Vec::new(),
                surfaces: Vec::new(),
            },
            layer: Layer::Outline,
            focus: EditorField::Row(0),
            typed: None,
            row_error: None,
            drag: None,
            back: Box::new(back),
        }
    }

    pub fn fields(&self) -> Vec<EditorField> {
        editor_fields(&self.plan, self.layer)
    }

    /// What `ac2_room::plan::validate` says, in words.
    pub fn verdict(&self) -> Result<(), String> {
        ac2_room::plan::validate(&self.plan).map_err(|e| e.to_string())
    }

    /// The vertices (or marker positions) of the layer edited.
    pub fn layer_points(&self) -> Vec<[f64; 2]> {
        match self.layer {
            Layer::Outline => self.plan.outline.clone(),
            Layer::Zone(z) => self
                .plan
                .zones
                .get(z)
                .map(|z| z.outline.clone())
                .unwrap_or_default(),
            Layer::Markers => self
                .plan
                .markers
                .iter()
                .map(|m| [m.at[0], m.at[1]])
                .collect(),
        }
    }

    fn polygon_mut(&mut self) -> Option<&mut Vec<[f64; 2]>> {
        match self.layer {
            Layer::Outline => Some(&mut self.plan.outline),
            Layer::Zone(z) => self.plan.zones.get_mut(z).map(|z| &mut z.outline),
            Layer::Markers => None,
        }
    }

    /// Keeps the focus on a row that exists.
    fn settle_focus(&mut self) {
        let f = self.fields();
        if !f.contains(&self.focus) {
            self.focus = f
                .iter()
                .rev()
                .find(|x| matches!(x, EditorField::Row(_)))
                .copied()
                .unwrap_or(EditorField::Layer);
        }
    }

    /// ↑/↓, the typed text taken first.
    pub fn step_row(&mut self, d: isize) {
        if !self.commit() {
            return;
        }
        let f = self.fields();
        let i = f.iter().position(|x| *x == self.focus).unwrap_or(0) as isize;
        self.focus = f[(i + d).clamp(0, f.len() as isize - 1) as usize];
    }

    /// ←/→: on the editing row, the outline, each zone, the markers; on a marker row, its
    /// kind.
    pub fn change(&mut self, d: isize) {
        if !self.commit() {
            return;
        }
        match self.focus {
            EditorField::Layer => {
                let mut layers = vec![Layer::Outline];
                layers.extend((0..self.plan.zones.len()).map(Layer::Zone));
                layers.push(Layer::Markers);
                let i = layers.iter().position(|l| *l == self.layer).unwrap_or(0) as isize;
                self.layer = layers[(i + d).rem_euclid(layers.len() as isize) as usize];
                self.drag = None;
            }
            EditorField::Row(k) if self.layer == Layer::Markers => {
                if let Some(m) = self.plan.markers.get(k) {
                    let kinds = [MarkerKind::Speaker, MarkerKind::Foh, MarkerKind::Point];
                    let i = kinds.iter().position(|x| *x == m.kind).unwrap_or(0) as isize;
                    let kind = kinds[(i + d).rem_euclid(3) as usize];
                    let name = marker_name(&self.plan.markers, kind, k);
                    let m = &mut self.plan.markers[k];
                    m.kind = kind;
                    m.name = name;
                }
            }
            _ => {}
        }
    }

    /// Insert: on the editing row a new zone (edited next); elsewhere a row after the
    /// focused one.
    pub fn insert(&mut self) {
        if !self.commit() {
            return;
        }
        if self.focus == EditorField::Layer {
            let n = self.plan.zones.len() + 1;
            self.plan.zones.push(ZoneSpec {
                name: format!("Zone {n}"),
                outline: Vec::new(),
                ear_height: SEATED_EAR_M,
                weight: None,
                seats: None,
            });
            self.layer = Layer::Zone(n - 1);
            return;
        }
        let after = match self.focus {
            EditorField::Row(k) => k + 1,
            _ => 0,
        };
        match self.layer {
            Layer::Markers => {
                let at = self
                    .plan
                    .markers
                    .get(after.wrapping_sub(1))
                    .map_or([0.0, 0.0, SPEAKER_HEIGHT_M], |m| m.at);
                self.add_marker(after, [at[0], at[1]]);
            }
            _ => {
                let pts = self.layer_points();
                let at = pts
                    .get(after.wrapping_sub(1))
                    .copied()
                    .unwrap_or([0.0, 0.0]);
                if let Some(p) = self.polygon_mut() {
                    p.insert(after.min(p.len()), at);
                }
            }
        }
        self.focus = EditorField::Row(after);
    }

    fn add_marker(&mut self, at_index: usize, at: [f64; 2]) {
        // The first marker is the FOH the positions read from; later ones are speakers.
        let kind = if self.plan.markers.iter().any(|m| m.kind == MarkerKind::Foh) {
            MarkerKind::Speaker
        } else {
            MarkerKind::Foh
        };
        let name = marker_name(&self.plan.markers, kind, usize::MAX);
        let (z, heading) = match kind {
            MarkerKind::Foh => (SEATED_EAR_M, Some(90.0)),
            _ => (SPEAKER_HEIGHT_M, Some(270.0)),
        };
        let i = at_index.min(self.plan.markers.len());
        self.plan.markers.insert(
            i,
            Marker {
                name,
                kind,
                at: [at[0], at[1], z],
                heading_deg: heading,
            },
        );
    }

    /// Delete: the focused row (a vertex, a marker; on the editing row of a zone, the zone).
    pub fn delete(&mut self) {
        self.typed = None;
        self.row_error = None;
        match (self.focus, self.layer) {
            (EditorField::Layer, Layer::Zone(z)) => {
                self.plan.zones.remove(z);
                self.layer = Layer::Outline;
            }
            (EditorField::Row(k), Layer::Markers) if k < self.plan.markers.len() => {
                self.plan.markers.remove(k);
            }
            (EditorField::Row(k), _) => {
                if let Some(p) = self.polygon_mut()
                    && k < p.len()
                {
                    p.remove(k);
                }
            }
            _ => {}
        }
        self.settle_focus();
    }

    pub fn type_text(&mut self, t: &str) {
        if self.focus.typed() {
            self.typed.get_or_insert_with(String::new).push_str(t);
            self.row_error = None;
        }
    }

    pub fn backspace(&mut self) {
        if let Some(t) = &mut self.typed {
            t.pop();
        }
    }

    /// Takes the typed text into its row; `false` (and why in `row_error`) when it does not
    /// read.
    pub fn commit(&mut self) -> bool {
        let Some(t) = self.typed.take() else {
            return true;
        };
        let refused = self.take(&t).err();
        match refused {
            Some(e) => {
                self.typed = Some(t);
                self.row_error = Some(e);
                false
            }
            None => {
                self.row_error = None;
                true
            }
        }
    }

    fn take(&mut self, t: &str) -> Result<(), String> {
        let words = t.trim();
        match self.focus {
            EditorField::Name => {
                if words.is_empty() {
                    return Err("a plan needs a name".into());
                }
                self.plan.name = words.to_owned();
            }
            EditorField::Ceiling => {
                self.plan.ceiling = match words {
                    "" | "none" | "-" => None,
                    _ => Some(
                        number(words, "m")
                            .filter(|v| *v > 0.0)
                            .ok_or_else(|| format!("{t:?}: a height in metres, or none"))?,
                    ),
                };
            }
            EditorField::Layer => {}
            EditorField::ZoneName => {
                if words.is_empty() {
                    return Err("a zone needs a name".into());
                }
                if let Layer::Zone(z) = self.layer
                    && let Some(z) = self.plan.zones.get_mut(z)
                {
                    z.name = words.to_owned();
                }
            }
            EditorField::EarHeight => {
                let h = number(words, "m")
                    .filter(|v| *v >= 0.0)
                    .ok_or_else(|| format!("{t:?}: a height in metres"))?;
                if let Layer::Zone(z) = self.layer
                    && let Some(z) = self.plan.zones.get_mut(z)
                {
                    z.ear_height = h;
                }
            }
            EditorField::Row(k) => {
                let v = numbers(words).unwrap_or_default();
                match self.layer {
                    Layer::Markers => {
                        let m = self
                            .plan
                            .markers
                            .get_mut(k)
                            .ok_or_else(|| "no such marker".to_owned())?;
                        match v.as_slice() {
                            [x, y] => m.at = [*x, *y, m.at[2]],
                            [x, y, h] => {
                                m.at = [*x, *y, m.at[2]];
                                m.heading_deg = Some(h.rem_euclid(360.0));
                            }
                            _ => {
                                return Err(format!(
                                    "{t:?}: a marker is x y (metres) and a heading (degrees)"
                                ));
                            }
                        }
                    }
                    _ => {
                        let [x, y] = v.as_slice() else {
                            return Err(format!("{t:?}: a vertex is x y, in metres"));
                        };
                        let (x, y) = (*x, *y);
                        let p = self
                            .polygon_mut()
                            .ok_or_else(|| "no such zone".to_owned())?;
                        match p.get_mut(k) {
                            Some(slot) => *slot = [x, y],
                            None => p.push([x, y]),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// A mouse step on the plan: drag a vertex or marker, add one at the end, delete one.
    pub fn plan_msg(&mut self, m: PlanMsg) {
        let _ = self.commit();
        match m {
            PlanMsg::Press { at, reach_m } => {
                match nearest(self.layer_points().into_iter(), at, reach_m) {
                    Some(i) => self.drag = Some(i),
                    None => {
                        let at = snap(at);
                        match self.layer {
                            Layer::Markers => {
                                let n = self.plan.markers.len();
                                self.add_marker(n, at);
                            }
                            _ => {
                                if let Some(p) = self.polygon_mut() {
                                    p.push(at);
                                }
                            }
                        }
                        let n = self.layer_points().len();
                        self.drag = n.checked_sub(1);
                        if let Some(i) = self.drag {
                            self.focus = EditorField::Row(i);
                        }
                    }
                }
            }
            PlanMsg::Drag { at } => {
                let Some(i) = self.drag else {
                    return;
                };
                let at = snap(at);
                match self.layer {
                    Layer::Markers => {
                        if let Some(m) = self.plan.markers.get_mut(i) {
                            m.at = [at[0], at[1], m.at[2]];
                        }
                    }
                    _ => {
                        if let Some(v) = self.polygon_mut().and_then(|p| p.get_mut(i)) {
                            *v = at;
                        }
                    }
                }
            }
            PlanMsg::Release => self.drag = None,
            PlanMsg::Remove { at, reach_m } => {
                if let Some(i) = nearest(self.layer_points().into_iter(), at, reach_m) {
                    self.focus = EditorField::Row(i);
                    self.delete();
                }
                self.drag = None;
            }
        }
    }
}

/// `FOH`, `Speaker 2`: the kind's word, numbered past every other marker of the kind
/// (`skip`: the marker being renamed).
fn marker_name(markers: &[Marker], kind: MarkerKind, skip: usize) -> String {
    let word = match kind {
        MarkerKind::Speaker => "Speaker",
        MarkerKind::Foh => "FOH",
        MarkerKind::Point => "Point",
    };
    let taken: Vec<&str> = markers
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != skip)
        .map(|(_, m)| m.name.as_str())
        .collect();
    if kind == MarkerKind::Foh && !taken.contains(&"FOH") {
        return word.to_owned();
    }
    (1..)
        .map(|i| format!("{word} {i}"))
        .find(|n| !taken.contains(&n.as_str()))
        .unwrap_or_else(|| word.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hall() -> PlanSpec {
        PlanSpec {
            name: "Hall".into(),
            outline: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 15.0], [0.0, 15.0]],
            ceiling: Some(8.0),
            zones: vec![ZoneSpec {
                name: "Floor".into(),
                outline: vec![[2.0, 2.0], [18.0, 2.0], [18.0, 11.0], [2.0, 11.0]],
                ear_height: 1.2,
                weight: None,
                seats: Some(400),
            }],
            markers: vec![Marker {
                name: "FOH".into(),
                kind: MarkerKind::Foh,
                at: [10.0, 3.0, 1.2],
                heading_deg: Some(90.0),
            }],
            surfaces: vec![],
        }
    }

    #[test]
    fn a_setup_suggests_then_takes_edits() {
        let plan = hall();
        let mut s = WalkSetup::new(
            "Walk 1".into(),
            std::slice::from_ref(&plan),
            &[(MeasId(1), 1)],
        );
        assert_eq!(s.points.len(), 8);
        s.plan_msg(
            PlanMsg::Press {
                at: [10.03, 6.04],
                reach_m: 0.01,
            },
            &plan,
        );
        assert_eq!(s.points.len(), 9);
        assert_eq!(s.points[8], [10.0, 6.0, 1.2]);
        s.plan_msg(PlanMsg::Drag { at: [12.0, 7.0] }, &plan);
        s.plan_msg(PlanMsg::Release, &plan);
        assert_eq!(s.points[8], [12.0, 7.0, 1.2]);
        s.plan_msg(
            PlanMsg::Remove {
                at: [12.0, 7.0],
                reach_m: 0.5,
            },
            &plan,
        );
        assert_eq!(s.points.len(), 8);
        // A typed count makes a new suggestion; a bad one is refused with the reason.
        s.focus = SetupField::Count;
        s.type_text("0");
        assert!(!s.commit(Some(&plan)));
        assert!(s.error.as_deref().expect("why").contains("positions are 1"));
        s.typed = Some("5".into());
        assert!(s.commit(Some(&plan)));
        assert_eq!(s.points.len(), 5);
    }

    #[test]
    fn the_editor_builds_a_valid_plan() {
        let back = WalkSetup::new("Walk 1".into(), &[], &[]);
        let mut e = PlanEditor::new("Room".into(), back);
        assert!(e.verdict().is_err());
        for (i, v) in ["0 0", "10 0", "10 8", "0 8"].iter().enumerate() {
            e.focus = EditorField::Row(i);
            e.type_text(v);
            assert!(e.commit(), "{v}");
        }
        assert_eq!(e.plan.outline.len(), 4);
        e.focus = EditorField::Row(1);
        e.type_text("ten 0");
        assert!(!e.commit());
        assert!(e.row_error.as_deref().expect("why").contains("x y"));
        e.typed = None;
        // A zone from the editing row, its vertices clicked on the plan.
        e.focus = EditorField::Layer;
        e.insert();
        assert_eq!(e.layer, Layer::Zone(0));
        for at in [[1.0, 1.0], [9.0, 1.0], [9.0, 6.0], [1.0, 6.0]] {
            e.plan_msg(PlanMsg::Press { at, reach_m: 0.2 });
            e.plan_msg(PlanMsg::Release);
        }
        assert_eq!(e.plan.zones[0].outline.len(), 4);
        e.focus = EditorField::Layer;
        e.change(1);
        assert_eq!(e.layer, Layer::Markers);
        e.plan_msg(PlanMsg::Press {
            at: [5.0, 2.0],
            reach_m: 0.2,
        });
        assert_eq!(e.plan.markers[0].kind, MarkerKind::Foh);
        assert_eq!(e.verdict(), Ok(()));
    }
}
