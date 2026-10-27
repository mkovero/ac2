//! Reducer tests of the programme meter dialog: opened by the New measurement chooser's
//! Programme meter row, reopened by *Edit measurement…* on a focused Programme pane; what it
//! offers, the pane its Enter opens or changes, and the rig's settings it sends.

use super::*;
use crate::forms::{FieldId, Form, FormKind};

/// Inputs 1–4 captured, 3 + 4 the pair *Main*; the rig reads Type I, on rev 7.
pub(super) fn rig_with_pair() -> State {
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.config.input_channels = vec![0, 1, 2, 3];
    }
    s.inputs = [(2, PairSide::Left), (3, PairSide::Right)]
        .map(|(channel, side)| InputSetup {
            channel,
            kind: InputKind::Line,
            pair: Some(side),
        })
        .to_vec();
    s.pairs = vec![Pair {
        left: 2,
        right: 3,
        name: Some("Main".into()),
    }];
    s.rig.rev = Rev(7);
    s.rig.levels.ballistic = Ballistic::Type1;
    s
}

pub(super) fn meter_t() -> T {
    let mut t = T::new();
    t.conn(mirror(rig_with_pair()));
    t.put(PaneKind::Transfer);
    t
}

fn dialog(t: &T) -> &Form {
    match &t.st.overlay {
        Overlay::Form(f) if matches!(f.kind, FormKind::Programme | FormKind::ProgrammeEdit) => f,
        other => panic!("no programme meter dialog: {other:?}"),
    }
}

/// Each field as `label: value`.
fn rows(t: &T) -> Vec<String> {
    dialog(t)
        .fields
        .iter()
        .map(|x| format!("{}: {}", x.label, x.display()))
        .collect()
}

/// Focus field `id` with ↑/↓, as the operator would.
fn to_field(t: &mut T, id: FieldId) {
    let at = dialog(t)
        .fields
        .iter()
        .position(|x| x.id == id)
        .expect("field");
    while dialog(t).focus != at {
        t.key(if dialog(t).focus < at {
            "ArrowDown"
        } else {
            "ArrowUp"
        });
    }
}

fn open_from_chooser(t: &mut T) {
    t.key("Ctrl+M");
    t.st.apply(Msg::Choose(Some(7)), &t.keys);
}

fn calls(r: &[Request]) -> Vec<(&Command, &str)> {
    r.iter()
        .filter_map(|r| match r {
            Request::Call { cmd, what } => Some((cmd, what.as_str())),
            _ => None,
        })
        .collect()
}

#[test]
fn the_chooser_row_opens_the_form_with_the_rigs_settings() {
    let mut t = meter_t();
    open_from_chooser(&mut t);
    let f = dialog(&t);
    assert_eq!(f.kind.title(), "New programme meter");
    assert_eq!(f.heading(FieldId::Metered), Some("What to meter"));
    assert_eq!(
        f.heading(FieldId::Ballistic),
        Some("How it reads · Whose: the rig — all clients")
    );
    assert_eq!(
        rows(&t),
        [
            "Meter: Main · line · pair (in 3, 4)",
            "Programme meters read: Type I",
            "Loudness target: −23 LUFS",
            "Correlation window: 1 s",
            "An over is above: 0.0 dBTP",
            "Correlation gate: −60 dBFS",
            "Ember decay: 0.4 s: a goniometer point fades out that long after it lands",
            "Ember glow: ×1: a goniometer point lands that many times as bright",
        ]
    );
    assert_eq!(
        f.heading(FieldId::EmberDecay),
        Some("Display · Whose: this app")
    );
}

/// Enter on a pair: its meters open beside the focused one, which keeps what it shows, with
/// its goniometer pane on their left; the meters take the focus. The rig's settings left
/// alone send nothing.
#[test]
fn a_pair_opens_in_a_pane_beside_the_focused_one() {
    let mut t = meter_t();
    let tf = t.st.layout.focus;
    let before = t.visible().len();
    open_from_chooser(&mut t);
    let r = t.key("Enter");
    assert!(calls(&r).is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.layout.kind(tf), PaneKind::Transfer);
    assert_eq!(t.visible().len(), before + 2);
    let p = t.st.layout.focus;
    assert_ne!(p, tf);
    assert_eq!(t.st.layout.kind(p), PaneKind::Programme);
    let pair = Some(Metered::Pair { left: 2, right: 3 });
    assert_eq!(t.st.layout.view(p).and_then(|v| v.meters), pair);
    let programme: Vec<_> =
        t.st.visible_panes()
            .into_iter()
            .filter(|id| t.st.layout.kind(*id) == PaneKind::Programme)
            .collect();
    assert_eq!(programme.len(), 2);
    let views: Vec<_> = programme
        .iter()
        .map(|id| t.st.layout.view(*id).expect("view"))
        .map(|v| (v.meters, v.modes.programme, v.meas))
        .collect();
    use ac2_scene::view::ProgrammeMode as M;
    assert_eq!(
        views,
        [(pair, M::Goniometer, None), (pair, M::Meters, None)]
    );
    assert_eq!(programme[1], p);
}

/// Inputs by name as bars, a mic among them: the pane meters them left to right.
#[test]
fn several_inputs_open_side_by_side() {
    let mut t = meter_t();
    let tf = t.st.layout.focus;
    open_from_chooser(&mut t);
    t.key("ArrowRight");
    to_field(&mut t, FieldId::Bar(2));
    t.key("ArrowRight");
    assert_eq!(
        rows(&t)[..5],
        [
            "Meter: inputs side by side",
            "Input 1: 3 · Input 3",
            "Input 2: 4 · Input 4",
            "Input 3: 1 · Input 1",
            "Input 4: none",
        ]
    );
    t.key("Enter");
    let p = t.st.layout.focus;
    assert_ne!(p, tf);
    assert_eq!(
        t.st.layout.view(p).and_then(|v| v.meters),
        Metered::inputs_of(&[2, 3, 0])
    );
}

/// A rig setting changed in the dialog goes out on Enter as the Rig monitor page's
/// `levels.set`: that value alone, on the rig's rev.
#[test]
fn a_changed_rig_setting_is_sent_as_levels_set() {
    let mut t = meter_t();
    open_from_chooser(&mut t);
    to_field(&mut t, FieldId::Ballistic);
    let r = t.key("ArrowRight");
    assert!(calls(&r).is_empty(), "nothing is sent before Enter");
    let r = t.key("Enter");
    assert_eq!(
        calls(&r),
        [(
            &Command::LevelsSet {
                base: Rev(7),
                ballistic: Some(Ballistic::Type2),
                correlation_window: None,
                over_threshold: None,
                correlation_gate: None,
                loudness_target: None,
            },
            "programme meters read Type II"
        )]
    );
    assert_eq!(t.st.layout.kind(t.st.layout.focus), PaneKind::Programme);
}

/// *Edit measurement…* on a focused Programme pane: the form filled in with what it meters;
/// Enter changes that pane in place.
#[test]
fn edit_on_a_programme_pane_changes_it_in_place() {
    let mut t = meter_t();
    open_from_chooser(&mut t);
    t.key("Enter");
    let p = t.st.layout.focus;
    let panes = t.visible().len();
    t.st.apply(Msg::Command(CommandId::EditMeas), &t.keys);
    let f = dialog(&t);
    assert_eq!(f.kind, FormKind::ProgrammeEdit);
    assert_eq!(f.kind.verb(), "Apply");
    assert_eq!(rows(&t)[0], "Meter: Main · line · pair (in 3, 4)");
    t.key("ArrowRight");
    to_field(&mut t, FieldId::Bar(1));
    // ← past inputs 2 and 1 (input 3 is in row 1): none.
    for _ in 0..3 {
        t.key("ArrowLeft");
    }
    assert_eq!(rows(&t)[1..3], ["Input 1: 3 · Input 3", "Input 2: none"]);
    t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.layout.focus, p);
    assert_eq!(t.visible().len(), panes, "no pane added");
    assert_eq!(
        t.st.layout.view(p).and_then(|v| v.meters),
        Metered::inputs_of(&[2])
    );
}

/// The form's *Loudness target* row steps 1 LU and goes out on Enter as the target alone.
#[test]
fn the_loudness_target_is_sent_as_levels_set() {
    use ac2_proto::units::Lufs;
    let mut t = meter_t();
    open_from_chooser(&mut t);
    to_field(&mut t, FieldId::LoudnessTarget);
    assert!(
        rows(&t)
            .iter()
            .any(|r| r == "Loudness target: \u{2212}23 LUFS"),
        "{:?}",
        rows(&t)
    );
    t.key("ArrowRight");
    let r = t.key("Enter");
    assert_eq!(
        calls(&r),
        [(
            &Command::LevelsSet {
                base: Rev(7),
                ballistic: None,
                correlation_window: None,
                over_threshold: None,
                correlation_gate: None,
                loudness_target: Some(Lufs(-22.0)),
            },
            "loudness target \u{2212}22 LUFS"
        )]
    );
}

/// Esc closes the form and does nothing else: no pane, no rig setting sent, whatever was
/// changed in it.
#[test]
fn esc_only_closes() {
    let mut t = meter_t();
    let tf = t.st.layout.focus;
    let before = t.visible();
    open_from_chooser(&mut t);
    to_field(&mut t, FieldId::Ballistic);
    t.key("ArrowRight");
    to_field(&mut t, FieldId::LoudnessTarget);
    t.key("ArrowLeft");
    let r = t.key("Escape");
    assert!(calls(&r).is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.visible(), before);
    assert_eq!(t.st.layout.focus, tf);
}

/// No inputs captured: the chooser's row says why, and so does Edit on a Programme pane.
#[test]
fn without_inputs_it_says_why() {
    let mut t = meter_t();
    open_from_chooser(&mut t);
    t.key("Enter");
    let mut s = rig_with_pair();
    if let Some(o) = &mut s.session.open {
        o.config.input_channels.clear();
    }
    t.conn(mirror(s));
    t.st.apply(Msg::Command(CommandId::EditMeas), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.last_toast(), "no inputs in the session to meter");
}
