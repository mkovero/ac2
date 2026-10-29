//! The spatial walk in the reducer: the *Spatial walk…* window, the setup window and the plan
//! editor's keys and mouse, `walk.new` and the walk pane it opens, and the walk pane's keys
//! (capture, next and previous point, redo, skip).

use super::*;
use crate::walk_setup::{PlanEditor, PlanMsg, WalkSetup};
use ac2_proto::model::{Operand, PlanSpec, PointState, WalkKind, WalkState};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::WalkId;
use ac2_scene::walk::Ready;
use ac2_scene::walk::setup::SetupField;

/// `Walk 3`, `Room 2`: the first numbered name not in `taken`.
fn numbered(word: &str, taken: &[&str]) -> String {
    (1..)
        .map(|i| format!("{word} {i}"))
        .find(|n| !taken.contains(&n.as_str()))
        .unwrap_or_else(|| word.to_owned())
}

impl AppState {
    /// The session's walks.
    pub fn walks(&self) -> &[WalkState] {
        self.daemon().map_or(&[], |s| s.walks.as_slice())
    }

    pub fn walk(&self, id: WalkId) -> Option<&WalkState> {
        self.walks().iter().find(|w| w.id == id)
    }

    /// The plan a walk walks: the session's copy.
    pub fn walk_plan(&self, w: &WalkState) -> Option<&PlanSpec> {
        self.daemon()?.walk_plans.iter().find(|p| p.name == w.plan)
    }

    /// The plans a new walk may walk: the session's copies, then the library's others.
    pub fn walk_plan_choices(&self) -> Vec<PlanSpec> {
        let Some(s) = self.daemon() else {
            return Vec::new();
        };
        let mut v: Vec<PlanSpec> = s.walk_plans.clone();
        for p in &s.plans {
            if !v.iter().any(|x| x.name == p.name) {
                v.push(p.clone());
            }
        }
        v
    }

    /// The measurements a walk can capture: the session's live transfers, each with the
    /// input it measures (a walk takes one mic per input).
    pub fn walk_mics(&self) -> Vec<(MeasId, u16)> {
        self.measurements()
            .into_iter()
            .filter_map(|m| match &m.config.kind {
                MeasKind::Transfer { config } => Some((m.id, config.measurement_input)),
                _ => None,
            })
            .collect()
    }

    /// The walk's mics by name, in its mic order: the mic on each measurement's input, else
    /// the measurement's name (as the daemon names the captures).
    pub fn walk_mic_names(&self, w: &WalkState) -> Vec<String> {
        w.meas
            .iter()
            .map(|&id| {
                let Some(m) = self.meas(id) else {
                    return format!("measurement {id}");
                };
                let input = match &m.config.kind {
                    MeasKind::Transfer { config } => Some(config.measurement_input),
                    _ => None,
                };
                input
                    .and_then(|i| self.input_setup(i).mic().map(str::to_owned))
                    .unwrap_or_else(|| m.config.name.clone())
            })
            .collect()
    }

    /// Whether the step may be captured now: the slowest of the mics standing at it, and
    /// which of the walk's mics that is.
    pub fn walk_ready(&self, w: &WalkState) -> (Ready, Option<usize>) {
        let standing: Vec<MeasId> = w.standing().into_iter().map(|i| w.points[i].mic).collect();
        let ready: Vec<Ready> = standing.iter().map(|&m| self.mic_ready(m)).collect();
        match Ready::slowest(&ready) {
            Some(k) => (ready[k], Some(w.mic_index(standing[k]))),
            None => (Ready::Waiting, None),
        }
    }

    /// Whether walk measurement `meas` may be captured now: its newest transfer frame's fill
    /// and its delay finder's pick.
    fn mic_ready(&self, meas: MeasId) -> Ready {
        let Some(m) = self.meas(meas) else {
            return Ready::Waiting;
        };
        let filled = m
            .running
            .then(|| {
                self.rig
                    .data
                    .as_ref()?
                    .latest
                    .get(&Topic::Data {
                        meas: m.id,
                        stream: Stream::Tf,
                    })
                    .and_then(|f| match &f.frame.data {
                        ac2_proto::FrameData::Tf(t) => Some(t.meta.filled),
                        _ => None,
                    })
            })
            .flatten();
        Ready::of(filled, m.delay.as_ref().is_some_and(|d| d.awaiting_pick))
    }

    /// How many of `traces` a walk captured before ready.
    pub fn unsettled_traces(&self, traces: &[TraceId]) -> usize {
        self.trace_list()
            .iter()
            .filter(|t| t.captured_unsettled && traces.contains(&t.id))
            .count()
    }

    /// How many of `operands` are stored traces a walk captured before ready.
    pub fn unsettled_operands(&self, operands: &[Operand]) -> usize {
        let traces: Vec<TraceId> = operands
            .iter()
            .filter_map(|o| match o {
                Operand::Trace { trace } => Some(*trace),
                _ => None,
            })
            .collect();
        self.unsettled_traces(&traces)
    }

    /// The walk the focused pane shows, or why none.
    fn focused_walk(&self) -> Result<&WalkState, String> {
        let v = self.layout.focused();
        let open = self.keys.reach(CommandId::Walks);
        if v.kind != PaneKind::Walk {
            return Err(format!(
                "focus a Spatial walk pane first ({open} opens one)"
            ));
        }
        v.walk
            .and_then(|id| self.walk(id))
            .ok_or_else(|| format!("this pane has no walk: {open} opens one"))
    }

    // ----- the Spatial walk window -------------------------------------------------------

    /// Shift+W: *New walk…*, then every walk of the session.
    pub(super) fn open_walks(&mut self) {
        if matches!(self.overlay, Overlay::Choose(ref c) if matches!(c.purpose, ChoicePurpose::Walks { .. }))
        {
            self.overlay = Overlay::None;
            return;
        }
        let walks = self.walks();
        let mut choices = vec![ac2_scene::meas_list::Choice {
            label: ac2_scene::walk::NEW_WALK.to_owned(),
            blocked: None,
        }];
        choices.extend(walks.iter().map(|w| ac2_scene::meas_list::Choice {
            label: ac2_scene::walk::walk_row(w),
            blocked: None,
        }));
        let ids = walks.iter().map(|w| w.id).collect();
        let lines = if walks.is_empty() {
            vec!["no walk in this session yet".to_owned()]
        } else {
            Vec::new()
        };
        self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
            purpose: ChoicePurpose::Walks { walks: ids },
            label: ac2_scene::walk::WINDOW_TITLE.to_owned(),
            title: ac2_scene::walk::WINDOW_TITLE.to_owned(),
            lines,
            choices,
            owners: Vec::new(),
            index: 0,
            hint: "↑/↓ pick · Enter: a walk opens in its pane, New walk… sets one up · Esc closes"
                .to_owned(),
        }));
    }

    pub(super) fn walks_pick(&mut self, walks: &[WalkId], i: usize) {
        match i.checked_sub(1).and_then(|k| walks.get(k)) {
            Some(w) => self.open_walk_pane(*w),
            None => self.open_walk_setup(),
        }
    }

    /// The setup window of a new walk.
    pub(super) fn open_walk_setup(&mut self) {
        let taken: Vec<&str> = self.walks().iter().map(|w| w.name.as_str()).collect();
        let name = numbered("Walk", &taken);
        let s = WalkSetup::new(name, &self.walk_plan_choices(), &self.walk_mics());
        self.overlay = Overlay::WalkSetup(Box::new(s));
    }

    /// The walk's pane: the one on screen showing it, else one of its own split off the
    /// focused pane (never taking that one over). It gets the focus, so its keys work.
    pub(super) fn open_walk_pane(&mut self, walk: WalkId) {
        if let Some(p) = self
            .layout
            .panes()
            .into_iter()
            .find(|p| self.layout.view(*p).is_some_and(|v| v.walk == Some(walk)))
        {
            self.layout.set_focus(p);
            return;
        }
        let Some(new) = self.split_for_new() else {
            let close = self.keys.reach(CommandId::ClosePane);
            self.warn(format!(
                "no room to split a pane for the walk: {close} closes one to make room"
            ));
            return;
        };
        self.set_pane_kind(new, PaneKind::Walk);
        if let Some(v) = self.layout.view_mut(new) {
            v.meas = None;
            v.walk = Some(walk);
        }
        self.layout.set_focus(new);
    }

    /// `walk.new` answered: the setup closes and the walk opens in its pane.
    pub(super) fn walk_created(&mut self, w: WalkState) {
        if matches!(self.overlay, Overlay::WalkSetup(_)) {
            self.overlay = Overlay::None;
        }
        self.open_walk_pane(w.id);
        let names = self.walk_mic_names(&w);
        let first = match w.standing().as_slice() {
            [] => String::new(),
            at => format!(": move {}", ac2_scene::walk::moves_text(&w, at, &names)),
        };
        self.toast(format!("{} started{first}", w.name));
    }

    // ----- the setup window --------------------------------------------------------------

    fn setup(&mut self) -> Option<&mut WalkSetup> {
        match &mut self.overlay {
            Overlay::WalkSetup(s) => Some(s),
            _ => None,
        }
    }

    pub(super) fn walk_setup_key(&mut self, chord: Chord, out: &mut Vec<Request>) {
        use eframe::egui::Key;
        let plans = self.walk_plan_choices();
        let names: Vec<String> = plans.iter().map(|p| p.name.clone()).collect();
        let mics = self.walk_mics();
        let Some(s) = self.setup() else {
            return;
        };
        let plan = s.plan_in(&plans).cloned();
        match chord.key {
            Key::ArrowUp => s.step_row(-1, plan.as_ref()),
            Key::ArrowDown => s.step_row(1, plan.as_ref()),
            Key::ArrowLeft | Key::ArrowRight => {
                let d = if chord.key == Key::ArrowLeft { -1 } else { 1 };
                if !s.commit(plan.as_ref()) {
                    return;
                }
                s.change(d, &names, &mics);
                let plan = s.plan_in(&plans).cloned();
                s.clamp_zone(plan.as_ref());
                if !matches!(s.focus, SetupField::Mic(_) | SetupField::Name) {
                    s.refresh(plan.as_ref());
                }
            }
            Key::Enter => self.walk_setup_enter(&plans, out),
            // Letters and digits are typed text; nothing else acts.
            _ => {}
        }
    }

    fn walk_setup_enter(&mut self, plans: &[PlanSpec], out: &mut Vec<Request>) {
        let library: Vec<String> = self
            .daemon()
            .map(|s| s.plans.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        let Some(s) = self.setup() else {
            return;
        };
        let plan = s.plan_in(plans).cloned();
        if !s.commit(plan.as_ref()) || s.sent {
            return;
        }
        let Some(plan) = plan else {
            // *New plan…*: the editor, which comes back here with the plan saved.
            let taken: Vec<&str> = library.iter().map(String::as_str).collect();
            let back = s.clone();
            self.overlay =
                Overlay::PlanEditor(Box::new(PlanEditor::new(numbered("Room", &taken), back)));
            return;
        };
        let refused = match (s.mics.is_empty(), s.points.is_empty()) {
            (true, _) => Some("no transfer measurement to capture with: make one first"),
            (_, true) => Some("no positions: add some on the plan"),
            _ => None,
        };
        if let Some(e) = refused {
            s.error = Some(e.to_owned());
            return;
        }
        s.sent = true;
        let cmd = Command::WalkNew {
            name: s.name.clone(),
            plan: plan.name.clone(),
            kind: WalkKind::LiveTransfer,
            meas: s.mics.clone(),
            points: s.point_specs(),
        };
        out.push(Request::NewWalk {
            cmd,
            name: s.name.clone(),
        });
    }

    /// The mouse on the plan view of the setup window or the editor.
    pub(super) fn plan_msg(&mut self, m: PlanMsg) {
        let plans = self.walk_plan_choices();
        match &mut self.overlay {
            Overlay::WalkSetup(s) => {
                if let Some(plan) = s.plan_in(&plans).cloned() {
                    s.plan_msg(m, &plan);
                }
            }
            Overlay::PlanEditor(e) => e.plan_msg(m),
            _ => {}
        }
    }

    // ----- the plan editor ---------------------------------------------------------------

    pub(super) fn plan_editor_key(&mut self, chord: Chord, out: &mut Vec<Request>) {
        use eframe::egui::Key;
        let Overlay::PlanEditor(e) = &mut self.overlay else {
            return;
        };
        match chord.key {
            Key::ArrowUp => e.step_row(-1),
            Key::ArrowDown => e.step_row(1),
            Key::ArrowLeft => e.change(-1),
            Key::ArrowRight => e.change(1),
            Key::Insert => e.insert(),
            Key::Delete => e.delete(),
            Key::Enter => {
                if !e.commit() || e.verdict().is_err() {
                    return;
                }
                let plan = e.plan.clone();
                let mut back = e.back.clone();
                back.plan = Some(plan.name.clone());
                back.saved = Some(plan.clone());
                back.zone = None;
                back.focus = SetupField::Zones;
                back.error = None;
                back.refresh(Some(&plan));
                self.overlay = Overlay::WalkSetup(back);
                self.call(
                    out,
                    Command::PlanSave { plan: plan.clone() },
                    format!("plan {} saved", plan.name),
                );
            }
            _ => {}
        }
    }

    // ----- the walk pane's keys ----------------------------------------------------------

    fn walk_call(
        &mut self,
        out: &mut Vec<Request>,
        cmd: impl FnOnce(&WalkState) -> (Command, String),
    ) {
        match self.focused_walk() {
            Ok(w) => {
                let (cmd, what) = cmd(w);
                self.call(out, cmd, what);
            }
            Err(e) => self.warn(e),
        }
    }

    /// X: capture where the mics stand, go on to the next step with a pending point.
    pub(super) fn walk_capture(&mut self, out: &mut Vec<Request>) {
        if let Ok(w) = self.focused_walk()
            && w.standing().is_empty()
        {
            let redo = self.keys.reach(CommandId::WalkRedo);
            let why = if w.step.is_none() {
                "every point is captured or skipped"
            } else {
                "this step is captured"
            };
            self.warn(format!("{}: {why} ({redo} redoes one)", w.name));
            return;
        }
        self.walk_call(out, |w| {
            let labels: Vec<&str> = w
                .standing()
                .into_iter()
                .map(|i| w.points[i].label.as_str())
                .collect();
            (
                Command::WalkCapture { walk: w.id },
                format!("{} captured at {}", w.name, labels.join(", ")),
            )
        });
    }

    /// PageDown / PageUp: the next or previous step, wrapping round.
    pub(super) fn walk_step(&mut self, d: isize, out: &mut Vec<Request>) {
        let names = self
            .focused_walk()
            .map(|w| self.walk_mic_names(w))
            .unwrap_or_default();
        self.walk_call(out, |w| {
            let n = w.steps().max(1) as isize;
            let from = w.step.map_or(if d > 0 { -1 } else { 0 }, |c| c as isize);
            let to = (from + d).rem_euclid(n) as u16;
            let at = w.step_points(to);
            (
                Command::WalkGoto {
                    walk: w.id,
                    step: to,
                },
                format!(
                    "{}: move {}",
                    w.name,
                    ac2_scene::walk::moves_text(w, &at, &names)
                ),
            )
        });
    }

    /// Shift+R: redo at the step the mics are at when it holds a capture, else at the last
    /// step before it that does; with several captures there, a chooser picks which.
    pub(super) fn walk_redo(&mut self, out: &mut Vec<Request>) {
        let target = self.focused_walk().map(|w| {
            let captured = |s: u16| -> Vec<usize> {
                w.step_points(s)
                    .into_iter()
                    .filter(|&i| matches!(w.points[i].state, PointState::Captured { .. }))
                    .collect()
            };
            // Back from the step the mics are at (from the last when the walk is done).
            let n = w.steps().max(1);
            let start = w.step.unwrap_or(n - 1);
            (0..n)
                .map(|k| (start + n - k) % n)
                .map(captured)
                .find(|c| !c.is_empty())
                .unwrap_or_default()
        });
        match target {
            Ok(points) if points.is_empty() => {
                self.warn("no point is captured yet: nothing to redo");
            }
            Ok(points) => self.walk_points(points, true, out),
            Err(e) => self.warn(e),
        }
    }

    /// K: leave out the point a mic stands at; with several mics standing, a chooser picks
    /// which (or the whole step).
    pub(super) fn walk_skip(&mut self, out: &mut Vec<Request>) {
        match self.focused_walk().map(WalkState::standing) {
            Ok(points) if points.is_empty() => {
                let name = self
                    .focused_walk()
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
                self.warn(format!("{name}: no point to skip"));
            }
            Ok(points) => self.walk_points(points, false, out),
            Err(e) => self.warn(e),
        }
    }

    /// M: the mics standing at the step move round by one (two trade places).
    pub(super) fn walk_swap(&mut self, out: &mut Vec<Request>) {
        let names = self
            .focused_walk()
            .map(|w| self.walk_mic_names(w))
            .unwrap_or_default();
        match self.focused_walk().map(WalkState::standing) {
            Ok(points) if points.len() < 2 => {
                self.warn("one mic stands at this step: no other to trade places with");
            }
            Ok(points) => self.walk_call(out, |w| {
                // As the daemon will place them: each mic one point on.
                let mut after = w.clone();
                let n = points.len();
                for (k, &i) in points.iter().enumerate() {
                    after.points[i].mic = w.points[points[(k + n - 1) % n]].mic;
                }
                (
                    Command::WalkSwap {
                        walk: w.id,
                        points: points.iter().map(|&i| i as u16).collect(),
                    },
                    format!(
                        "{}: move {}",
                        w.name,
                        ac2_scene::walk::moves_text(&after, &points, &names)
                    ),
                )
            }),
            Err(e) => self.warn(e),
        }
    }

    /// Redo (`redo`) or skip `points` of the focused walk: one at once, several by a chooser
    /// of each by point and mic, the whole step first.
    fn walk_points(&mut self, points: Vec<usize>, redo: bool, out: &mut Vec<Request>) {
        let Ok(w) = self.focused_walk() else {
            return;
        };
        let walk = w.id;
        if let [one] = points.as_slice() {
            self.walk_point_sets(walk, &[*one as u16], redo, out);
            return;
        }
        let names = self.walk_mic_names(w);
        let row = |i: usize| {
            let p = &w.points[i];
            let name = names.get(w.mic_index(p.mic)).map_or("mic", String::as_str);
            format!("{} · {name}", p.label)
        };
        let labels: Vec<&str> = points.iter().map(|&i| w.points[i].label.as_str()).collect();
        let mut sets: Vec<Vec<u16>> = vec![points.iter().map(|&i| i as u16).collect()];
        let mut choices = vec![ac2_scene::meas_list::Choice {
            label: format!("the whole step: {}", labels.join(", ")),
            blocked: None,
        }];
        for &i in &points {
            sets.push(vec![i as u16]);
            choices.push(ac2_scene::meas_list::Choice {
                label: row(i),
                blocked: None,
            });
        }
        let title = if redo {
            "Redo which point?"
        } else {
            "Skip which point?"
        };
        self.overlay = Overlay::Choose(Box::new(ChoicePrompt {
            purpose: ChoicePurpose::WalkPoints { walk, redo, sets },
            label: title.to_owned(),
            title: title.to_owned(),
            lines: Vec::new(),
            choices,
            owners: Vec::new(),
            index: 0,
            hint: "↑/↓ pick · Enter: that point · Esc closes".to_owned(),
        }));
    }

    /// A chooser's answer, or a single point: redo or skip each of `points`.
    pub(super) fn walk_point_sets(
        &mut self,
        walk: WalkId,
        points: &[u16],
        redo: bool,
        out: &mut Vec<Request>,
    ) {
        let Some(w) = self.walk(walk) else {
            return;
        };
        let name = w.name.clone();
        let labels: Vec<String> = points
            .iter()
            .filter_map(|&i| w.points.get(usize::from(i)))
            .map(|p| p.label.clone())
            .collect();
        for (&point, label) in points.iter().zip(&labels) {
            let (cmd, what) = if redo {
                (
                    Command::WalkRedo { walk, point },
                    format!("{name}: {label} pending again, its capture out of the average"),
                )
            } else {
                (
                    Command::WalkSkip { walk, point },
                    format!("{name}: {label} skipped"),
                )
            };
            self.call(out, cmd, what);
        }
    }
}
