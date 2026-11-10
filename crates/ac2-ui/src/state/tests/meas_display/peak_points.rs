//! Peak points: a legend click picks a curve in its pane, whose peaks that pane shows
//! (`docs/design/legend-peak-points.md`). App-local: no request, no undo entry; dropped
//! whenever the curve stops being a member of the pane.

use super::*;
use ac2_scene::trace::TraceKey;

const SUB: TraceKey = TraceKey::Live(MeasId(2));
const SUB2: TraceKey = TraceKey::Live(MeasId(9));

/// Columns at −80 dB but one at −20 dB: one peak.
fn bump(at: usize) -> Vec<f32> {
    let mut l = vec![-80.0; 5];
    l[at] = -20.0;
    l
}

/// Frames of Sub and Sub 2, each with a peak, numbered `seq`.
fn peaked(t: &mut T, seq: u64) {
    let inc = DaemonIncarnation(1);
    spectra_of(t, &[(2, bump(1)), (9, bump(3))], seq, inc);
}

/// Sub's spectrum pane with Sub 2 compared, frames with peaks in, its level range fitted
/// to them.
fn picked_pane() -> (T, PaneId) {
    let mut t = on_sub(two_spectra(), &[9]);
    peaked(&mut t, 10);
    let p = t.pane(PaneKind::Spectrum);
    t.st.layout.set_focus(p);
    t.key_answered("Shift+Home");
    (t, p)
}

fn pick(t: &mut T, p: PaneId, key: TraceKey) -> Vec<Request> {
    t.st.apply(Msg::LegendPick(p, key), &t.keys)
}

fn scene(t: &T, p: PaneId) -> ac2_scene::spectrum::SpectrumScene {
    crate::scenes::spectrum(&t.st, p, &Theme::dark(), SIZE, now())
}

fn labels(t: &T, p: PaneId) -> Vec<String> {
    scene(t, p)
        .peak_labels
        .into_iter()
        .map(|l| l.text)
        .collect()
}

/// A click picks and selects, shows the curve's points with no request and no undo entry;
/// the same row again clears the points and keeps the selection.
#[test]
fn a_pick_shows_points_and_again_clears_them() {
    let (mut t, p) = picked_pane();
    assert!(labels(&t, p).is_empty(), "nothing picked");
    let undo = t.st.history.undo_len();
    assert!(pick(&mut t, p, SUB2).is_empty());
    assert_eq!(t.st.rig.selected, Some(MeasId(9)));
    assert_eq!(t.st.peak_points_of(p), Some(SUB2));
    assert_eq!(t.st.history.undo_len(), undo, "no undo entry");
    let s = scene(&t, p);
    assert_eq!(s.picked_cue, Some(SUB2));
    assert_eq!(s.peak_list, None, "P is off: points, no list");
    let ls: Vec<&str> = s.peak_labels.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(ls.len(), 1, "{ls:?}");
    assert!(ls[0].contains("−20.0 dBFS"), "{ls:?}");
    assert!(pick(&mut t, p, SUB2).is_empty());
    assert_eq!(t.st.peak_points_of(p), None);
    assert_eq!(t.st.rig.selected, Some(MeasId(9)), "the selection stays");
    let s = scene(&t, p);
    assert!(s.peak_labels.is_empty());
    assert_eq!(s.picked_cue, None);
}

/// Two panes on one measurement pick apart: clearing one leaves the other.
#[test]
fn two_panes_pick_independently() {
    let (mut t, p) = picked_pane();
    t.st.layout.set_focus(p);
    let q = t.st.split_focused_pane().expect("room for a split");
    pick(&mut t, p, SUB2);
    pick(&mut t, q, SUB);
    assert_eq!(
        (t.st.peak_points_of(p), t.st.peak_points_of(q)),
        (Some(SUB2), Some(SUB))
    );
    pick(&mut t, p, SUB2);
    assert_eq!(
        (t.st.peak_points_of(p), t.st.peak_points_of(q)),
        (None, Some(SUB))
    );
}

/// With P off a curve has no tracker; a pick starts one from the newest frame kept (a
/// stopped curve sends none again), and it lives on through later frames while picked.
#[test]
fn a_pick_seeds_its_tracker_and_keeps_it() {
    let (mut t, p) = picked_pane();
    assert!(!t.st.rig.peak_tracks.contains_key(&MeasId(9)), "P is off");
    pick(&mut t, p, SUB2);
    let freqs = |t: &T| -> Vec<f64> {
        t.st.rig
            .peak_tracks
            .get(&MeasId(9))
            .map_or(vec![], |e| e.1.peaks().iter().map(|p| p.freq_hz).collect())
    };
    assert_eq!(freqs(&t).len(), 1, "seeded from the newest frame");
    peaked(&mut t, 11);
    assert_eq!(freqs(&t).len(), 1, "kept by the pick");
    pick(&mut t, p, SUB2);
    peaked(&mut t, 12);
    assert!(
        !t.st.rig.peak_tracks.contains_key(&MeasId(9)),
        "gone with it"
    );
}

/// P on the pane's own measurement gates the list and ranks; they go to the pick, named
/// when it is not the pane's own curve.
#[test]
fn the_own_mode_gates_the_list_and_it_names_a_compared_pick() {
    let (mut t, p) = picked_pane();
    t.st.layout.set_focus(p);
    t.key_answered("P");
    t.key_answered("P");
    peaked(&mut t, 11);
    pick(&mut t, p, SUB2);
    let s = scene(&t, p);
    assert!(s.peak_labels.is_empty(), "the list names them");
    let list = s.peak_list.expect("listed");
    assert!(list.starts_with("Sub 2\n1  "), "{list}");
    pick(&mut t, p, SUB);
    let list = scene(&t, p).peak_list.expect("listed");
    assert!(list.starts_with("1  "), "the own curve unnamed: {list}");
}

/// What ends the curve's membership drops the pick, and it never comes back by itself:
/// C unticked, A hidden, another row dropped on the pane.
#[test]
fn a_curve_leaving_the_pane_drops_the_pick() {
    // C unticks Sub 2.
    let (mut t, p) = picked_pane();
    pick(&mut t, p, SUB2);
    t.st.layout.set_focus(p);
    t.key("C");
    t.key("Down");
    t.key("Enter");
    t.key("Escape");
    assert_eq!(t.st.peak_points_of(p), None, "unticked");

    // A hides it (Sub 2 is selected by the pick).
    let (mut t, p) = picked_pane();
    pick(&mut t, p, SUB2);
    t.st.layout.set_focus(p);
    t.key_answered("A");
    assert!(t.st.rig.hidden_meas.contains(&MeasId(9)));
    assert_eq!(t.st.peak_points_of(p), None, "hidden");
    t.key_answered("A");
    assert_eq!(t.st.peak_points_of(p), None, "not back by itself");

    // Another row dropped: Sub is no longer the pane's.
    let (mut t, p) = picked_pane();
    pick(&mut t, p, SUB);
    t.drop_meas(p, MeasId(9));
    assert_eq!(t.st.peak_points_of(p), None, "dropped over");
    t.key("Ctrl+Z");
    assert_eq!(t.st.peak_points_of(p), None, "the undo brings no pick back");
}

/// An undo that leaves the curve on the pane keeps the pick; the pane closed drops it.
#[test]
fn undo_keeps_a_member_and_a_close_drops_it() {
    let (mut t, p) = picked_pane();
    t.st.layout.set_focus(p);
    pick(&mut t, p, SUB2);
    t.key("G");
    t.key("Ctrl+Z");
    assert_eq!(t.st.peak_points_of(p), Some(SUB2), "still a member");
    t.st.layout.set_focus(p);
    t.key("Q");
    assert!(t.st.layout.view(p).is_none(), "closed");
    assert!(t.st.peak_points.is_empty());
}

/// Another client's state event that takes the curve off the pane (its kind no longer
/// drawn there) drops the pick; one that deletes the measurement too.
#[test]
fn a_state_event_that_ends_membership_drops_it() {
    let (mut t, p) = picked_pane();
    pick(&mut t, p, SUB2);
    t.remirror(|s| s.measurements[2].config.kind = transfer());
    assert_eq!(t.st.peak_points_of(p), None, "no longer drawn there");

    let (mut t, p) = picked_pane();
    pick(&mut t, p, SUB2);
    t.remirror(|s| s.measurements.retain(|m| m.id != MeasId(9)));
    assert_eq!(t.st.peak_points_of(p), None, "deleted");
}

/// A reconnect to the same daemon keeps the pick while its curve has no data; a daemon
/// that numbers afresh drops it, as an id may name another curve.
#[test]
fn a_reconnect_keeps_it_and_a_reborn_daemon_drops_it() {
    let (mut t, p) = picked_pane();
    pick(&mut t, p, SUB2);
    let held = t.st.daemon().expect("mirrored").clone();
    let restart = |t: &mut T, incarnation: u64| {
        t.conn(ConnEvent::Failed {
            target: "tcp://pupu".into(),
            error: "gone".into(),
            retry_in: std::time::Duration::from_secs(1),
        });
        t.conn(ConnEvent::Connected {
            target: "tcp://pupu".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c1".into()),
            rig_id: t.st.rig.id,
            name: "test rig".into(),
        });
        t.conn(mirror_of(held.clone(), incarnation, Some("c1")));
    };
    restart(&mut t, 1);
    assert_eq!(t.st.peak_points_of(p), Some(SUB2), "the same daemon");
    restart(&mut t, 2);
    assert_eq!(t.st.peak_points_of(p), None, "another incarnation");
}

/// Prefs put a layout in place by stored ids, which may name other content: every pick
/// goes. A layout installed takes fresh ids, so the gone pane's pick goes with it.
#[test]
fn set_prefs_and_install_drop_picks() {
    let (mut t, p) = picked_pane();
    pick(&mut t, p, SUB2);
    let prefs = t.st.prefs.clone();
    t.st.set_prefs(prefs);
    assert!(t.st.peak_points.is_empty(), "set_prefs");

    let (mut t, p) = picked_pane();
    pick(&mut t, p, SUB2);
    let id = t.st.layout.fresh_id();
    let mut v = *t.st.layout.view(p).expect("the pane");
    v.with = With::default();
    t.st.layout
        .install(PaneNode::Leaf(id), [(id, v)].into_iter().collect(), id);
    peaked(&mut t, 11);
    assert!(t.st.peak_points.is_empty(), "install");
}

/// An id the rig does not know is ignored.
#[test]
fn an_unknown_curve_is_not_picked() {
    let (mut t, p) = picked_pane();
    pick(&mut t, p, TraceKey::Live(MeasId(77)));
    pick(&mut t, p, TraceKey::Stored(TraceId(77)));
    assert!(t.st.peak_points.is_empty());
}

/// On a transfer pane the pick chooses min / max's curve: while min / max is off a click
/// selects and leaves the pick alone, and the cue shows only while it is on.
#[test]
fn a_transfer_pick_needs_min_max() {
    let mut t = T::new();
    t.pick_panes();
    let p = t.pane(PaneKind::Transfer);
    let id =
        t.st.layout
            .view(p)
            .and_then(|v| v.meas)
            .expect("a measurement");
    let key = TraceKey::Live(id);
    assert!(!t.st.view.tf.extremes);
    pick(&mut t, p, key);
    assert_eq!(t.st.rig.selected, Some(id));
    assert_eq!(t.st.peak_points_of(p), None, "min / max is off");
    t.st.view.tf.extremes = true;
    pick(&mut t, p, key);
    assert_eq!(t.st.peak_points_of(p), Some(key));
}

/// A stored trace's peaks are found once and kept; new data for it drops them, to be
/// found again from the new data.
#[test]
fn new_trace_data_drops_its_cached_peaks() {
    let (mut t, _) = picked_pane();
    let (data, grid) = sweep_data(40);
    t.st.rig.trace_peaks.insert(TraceId(40), vec![]);
    t.st.rig.keep_trace(data, grid);
    assert!(!t.st.rig.trace_peaks.contains_key(&TraceId(40)));
}
