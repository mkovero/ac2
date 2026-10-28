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

    /// The measurements a walk can capture: the session's live transfers.
    pub fn walk_mics(&self) -> Vec<MeasId> {
        self.measurements()
            .into_iter()
            .filter(|m| matches!(m.config.kind, MeasKind::Transfer { .. }))
            .map(|m| m.id)
            .collect()
    }

    /// Whether the walk's measurement may be captured now: its newest transfer frame's fill
    /// and its delay finder's pick.
    pub fn walk_ready(&self, w: &WalkState) -> Ready {
        let Some(m) = w.meas.first().and_then(|m| self.meas(*m)) else {
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
        let first = w
            .current
            .and_then(|i| w.points.get(usize::from(i)))
            .map_or(String::new(), |p| format!(": move the mic to {}", p.label));
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
                if !matches!(s.focus, SetupField::Mic | SetupField::Name) {
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
        let refused = match (s.mic, s.points.is_empty()) {
            (None, _) => Some("no transfer measurement to capture with: make one first"),
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
            meas: s.mic.into_iter().collect(),
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

    /// X: capture here, go on to the next pending point.
    pub(super) fn walk_capture(&mut self, out: &mut Vec<Request>) {
        if let Ok(w) = self.focused_walk()
            && w.current.is_none()
        {
            let redo = self.keys.reach(CommandId::WalkRedo);
            self.warn(format!(
                "{}: every point is captured or skipped ({redo} redoes one)",
                w.name
            ));
            return;
        }
        self.walk_call(out, |w| {
            let at = w
                .current
                .and_then(|i| w.points.get(usize::from(i)))
                .map_or(String::new(), |p| format!(" at {}", p.label));
            (
                Command::WalkCapture { walk: w.id },
                format!("{} captured{at}", w.name),
            )
        });
    }

    /// PageDown / PageUp: the next or previous point in walking order, wrapping round.
    pub(super) fn walk_step(&mut self, d: isize, out: &mut Vec<Request>) {
        self.walk_call(out, |w| {
            let n = w.points.len().max(1) as isize;
            let from = w.current.map_or(if d > 0 { -1 } else { 0 }, |c| c as isize);
            let to = (from + d).rem_euclid(n) as usize;
            let label = w.points.get(to).map_or("", |p| p.label.as_str());
            (
                Command::WalkGoto {
                    walk: w.id,
                    point: to as u16,
                },
                format!("{}: move the mic to {label}", w.name),
            )
        });
    }

    /// Shift+R: redo the point the mic is at when it is captured, else the last one captured
    /// before it; its captures leave the average.
    pub(super) fn walk_redo(&mut self, out: &mut Vec<Request>) {
        let target = self.focused_walk().map(|w| {
            let n = w.points.len();
            let cur = w.current.map(usize::from);
            let captured = |i: usize| matches!(w.points[i].state, PointState::Captured { .. });
            match cur {
                Some(c) if captured(c) => Some(c),
                _ => {
                    let start = cur.unwrap_or(0) + n;
                    (1..=n).map(|k| (start - k) % n).find(|&i| captured(i))
                }
            }
        });
        match target {
            Ok(Some(point)) => self.walk_call(out, |w| {
                (
                    Command::WalkRedo {
                        walk: w.id,
                        point: point as u16,
                    },
                    format!(
                        "{}: {} pending again, its capture out of the average",
                        w.name, w.points[point].label
                    ),
                )
            }),
            Ok(None) => self.warn("no point is captured yet: nothing to redo"),
            Err(e) => self.warn(e),
        }
    }

    /// K: leave the current point out and go on.
    pub(super) fn walk_skip(&mut self, out: &mut Vec<Request>) {
        if let Ok(w) = self.focused_walk()
            && w.current.is_none()
        {
            self.warn(format!("{}: no point to skip", w.name));
            return;
        }
        self.walk_call(out, |w| {
            let at = w
                .current
                .and_then(|i| w.points.get(usize::from(i)))
                .map_or(String::new(), |p| p.label.clone());
            (
                Command::WalkSkip { walk: w.id },
                format!("{}: {at} skipped", w.name),
            )
        });
    }
}
