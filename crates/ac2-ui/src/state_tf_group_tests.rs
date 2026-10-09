//! Reducer and scene tests of the transfer pane's group: it draws the measurement it shows,
//! that measurement's math channels and stored traces, and nothing of another measurement or
//! of Imported.

use super::*;
use ac2_proto::model::{MathConfig, MathDomain, MathExpr, MathOp, Operand};
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

const SIZE: Viewport = Viewport {
    width: 900.0,
    height: 500.0,
};

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    }
}

/// A stored transfer trace under `owner`.
fn owned(id: u32, owner: TraceOwner) -> TraceMeta {
    let mut t = stored(id, None, 2);
    t.edit.owner = owner;
    t
}

fn under(meas: u32) -> TraceOwner {
    TraceOwner::Meas { meas: MeasId(meas) }
}

/// Main L (1) and Delay tower (3) are transfer measurements; "L ÷ tower" (6) is a math
/// channel filed under Main L. t10, t13 are Main L's captures, t11 Delay tower's, t12 an
/// import.
fn two_groups() -> State {
    let mut s = four();
    s.measurements.push(meas(
        6,
        "L ÷ tower",
        MeasKind::Math {
            config: MathConfig::of(
                under(1),
                MathDomain::Transfer,
                MathExpr::Binary {
                    a: Operand::Meas { meas: MeasId(1) },
                    op: MathOp::Divide,
                    b: Operand::Meas { meas: MeasId(3) },
                },
            ),
        },
    ));
    s.traces = vec![
        owned(10, under(1)),
        owned(11, under(3)),
        owned(12, TraceOwner::Imported),
        owned(13, under(1)),
    ];
    s
}

fn trace_data(meta: &TraceMeta) -> ConnEvent {
    ConnEvent::Trace(
        Arc::new(TraceData {
            meta: meta.clone(),
            mag_db: vec![0.0; 4],
            phase_deg: None,
            coherence: None,
            sweep: None,
        }),
        Arc::new(GridDef::Log {
            ppo: 1,
            k_min: 0,
            k_max: 3,
        }),
    )
}

/// Live transfer frames of measurements `ids`.
pub(super) fn tf_frames(t: &mut T, ids: &[u32]) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::FrameData;
    let mut latest = Latest::default();
    for id in ids {
        let mut f = ac2_proto::samples::tf_frame();
        if let FrameData::Tf(tf) = &mut f.data {
            tf.meas = MeasId(*id);
            tf.meta.math = None;
        }
        let f = TopicFrame {
            topic: f.data.topic(),
            frame: Arc::new(f),
            received: Instant::now(),
            since_new: std::time::Duration::ZERO,
            age: Some(0.0),
            stale: false,
        };
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    let mut grids = std::collections::BTreeMap::new();
    let g = ac2_proto::samples::log_grid();
    grids.insert(g.id(), Arc::new(g));
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
}

/// The state mirrored, every trace's data arrived, live frames of 1, 3 and 6.
fn loaded(s: State) -> T {
    let mut t = T::new();
    let traces = s.traces.clone();
    t.conn(mirror(s));
    for m in &traces {
        t.conn(trace_data(m));
    }
    tf_frames(&mut t, &[1, 3, 6]);
    t
}

fn legend(t: &T) -> Vec<String> {
    let mut v: Vec<String> = crate::scenes::transfer(
        &t.st,
        t.pane(PaneKind::Transfer),
        &Theme::dark(),
        SIZE,
        now(),
    )
    .legend
    .iter()
    .map(|e| e.name.clone())
    .collect();
    v.sort();
    v
}

/// The pane draws its measurement's group only: Main L's live curve, its math channel and
/// its two captures; neither Delay tower's capture nor the import. Choosing Delay tower
/// switches the whole set; selecting a capture of Main L brings its group back.
#[test]
fn the_transfer_pane_draws_its_measurements_group_only() {
    let mut t = loaded(two_groups());
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert_eq!(legend(&t), ["L ÷ tower", "Main L", "t10", "t13"]);
    assert_eq!(t.st.transfer_group(), Some(MeasId(1)));
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(3))),
        &t.keys,
    );
    assert_eq!(legend(&t), ["Delay tower", "t11"]);
    // A trace selected brings its measurement's group with it.
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    assert_eq!(t.st.transfer_group(), Some(MeasId(1)));
    assert_eq!(legend(&t), ["L ÷ tower", "Main L", "t10", "t13"]);
    // The math channel shown on the pane draws its owner's group.
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(6))),
        &t.keys,
    );
    assert_eq!(t.st.transfer_group(), Some(MeasId(1)));
    assert_eq!(legend(&t), ["L ÷ tower", "Main L", "t10", "t13"]);
    // A hidden measurement leaves its live curve out; its group stays.
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("A");
    assert_eq!(legend(&t), ["L ÷ tower", "t10", "t13"]);
}

/// An import shows on the transfer pane once moved under the pane's measurement.
#[test]
fn a_moved_import_joins_its_measurements_pane() {
    let mut t = loaded(two_groups());
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    assert_eq!(legend(&t), ["Delay tower", "t11"]);
    let mut s = two_groups();
    s.traces[2].edit.owner = under(3);
    let moved = s.traces[2].clone();
    t.conn(mirror(s));
    t.conn(trace_data(&moved));
    assert_eq!(legend(&t), ["Delay tower", "t11", "t12"]);
}

/// M averages the shown stored traces of the pane's group, never another group's or an
/// import.
#[test]
fn average_takes_the_panes_group_only() {
    let mut t = loaded(two_groups());
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    let r = t.key("M");
    let [
        Request::Call {
            cmd: Command::TraceAverage { traces, .. },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    assert_eq!(traces, &[TraceId(10), TraceId(13)]);
    // Delay tower's group holds one capture: nothing to average.
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    assert!(t.key("M").is_empty());
}

/// With nothing of its group stored but shown traces under Imported, the pane says where
/// they are and how to bring them; in the title strip over the live curve.
#[test]
fn the_pane_points_at_imported_traces() {
    let mut s = daemon_state();
    s.traces = vec![owned(12, TraceOwner::Imported)];
    let mut t = loaded(s);
    let hint = t.st.empty_hint(&t.keys).expect("hint");
    assert_eq!(
        hint.text,
        "1 trace under Imported is not on this pane — select it, then Move to measurement… (Shift+F2)"
    );
    assert_eq!(hint.place, HintPlace::Title);
    assert_eq!(legend(&t), ["Main L"]);
    // Showing it with its slot key says why it does not appear.
    let mut hidden = owned(12, TraceOwner::Imported);
    hidden.edit.visible = false;
    hidden.edit.slot = Some(2);
    let mut s = daemon_state();
    s.traces = vec![hidden];
    t.conn(mirror(s));
    let r = t.key("2");
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call { what, .. }] if what.ends_with("under Imported: compare it (C) or Move to measurement… to see it")
        ),
        "{r:?}"
    );
    // Moved under Main L: drawn, and the hint is gone.
    let mut s = daemon_state();
    s.traces = vec![owned(12, under(1))];
    t.conn(mirror(s));
    assert_eq!(t.st.empty_hint(&t.keys), None);
    assert_eq!(legend(&t), ["Main L", "t12"]);
}

/// A sweep measurement's runs show on the transfer pane when the sweep is its measurement
/// (chosen with the pane's chip, or a run selected); the live measurement's group otherwise.
/// S there does not start the sweep: it runs from the sweep pane.
#[test]
fn a_sweeps_runs_show_when_the_sweep_is_the_panes() {
    let mut s = daemon_state();
    s.measurements.push(meas(
        5,
        "Sweep 1",
        MeasKind::Sweep {
            config: SweepConfig {
                reference_input: 0,
                measurement_input: 1,
                outputs: vec![0],
                level: Dbfs(-50.0),
                sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
                repeats: 1,
                gate: None,
                tail: Some(Seconds(1.0)),
                lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
            },
        },
    ));
    let run = sweep_meta(20);
    s.traces = vec![run.clone()];
    let mut t = T::new();
    t.conn(mirror(s));
    // A sweep pane beside the grid: the sweep selected goes there.
    t.go(PaneKind::Distortion);
    t.key("Alt+1");
    let (data, grid) = sweep_data(20);
    t.conn(ConnEvent::Trace(data, grid));
    tf_frames(&mut t, &[1]);
    // Selecting the sweep in the list leaves the transfer pane on the live measurement.
    t.st.update(Msg::SelectMeas(MeasId(5)), &t.keys);
    assert_eq!(
        t.st.kind_meas(PaneKind::Transfer).map(|m| m.id),
        Some(MeasId(1))
    );
    assert_eq!(legend(&t), ["Main L"]);
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(5))),
        &t.keys,
    );
    assert_eq!(legend(&t), [run.edit.name.as_str()]);
    // The IR pane keeps a transfer measurement.
    assert_eq!(
        crate::scenes::focus_tf(&t.st).map(|m| m.id),
        Some(MeasId(1))
    );
    assert!(t.key("S").is_empty());
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(1))),
        &t.keys,
    );
    assert_eq!(legend(&t), ["Main L"]);
    t.st.update(Msg::SelectTrace(TraceId(20)), &t.keys);
    assert_eq!(legend(&t), [run.edit.name.as_str()]);
}

/// The legend's `name · tags` lines, sorted.
fn legend_text(t: &T) -> Vec<String> {
    let mut v: Vec<String> = crate::scenes::transfer(
        &t.st,
        t.pane(PaneKind::Transfer),
        &Theme::dark(),
        SIZE,
        now(),
    )
    .legend
    .iter()
    .map(|e| {
        let mut s = e.name.clone();
        for tag in &e.tags {
            s.push_str(" · ");
            s.push_str(tag);
        }
        s
    })
    .collect();
    v.sort();
    v
}

fn tree_detail(t: &T, key: ac2_scene::meas_list::TreeKey) -> String {
    t.st.tree_rows()
        .into_iter()
        .find(|r| r.key == key)
        .map(|r| r.details[0].clone())
        .expect("row")
}

/// C compares what the tree has selected: another measurement's live curve and an
/// imported trace are drawn on the transfer pane besides its group, tagged `cmp` in the
/// legend and the tree; C again stops; average still takes the group only; Clear compare
/// drops every one.
#[test]
fn c_compares_the_selection_on_the_transfer_pane() {
    use ac2_scene::meas_list::TreeKey;
    let mut t = loaded(two_groups());
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    t.key("C");
    assert_eq!(
        t.last_toast(),
        "Delay tower compared on the transfer pane (cmp)"
    );
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    t.key("C");
    assert_eq!(t.st.transfer_group(), Some(MeasId(1)));
    assert_eq!(
        legend(&t),
        ["Delay tower", "L ÷ tower", "Main L", "t10", "t12", "t13"]
    );
    let tagged: Vec<String> = legend_text(&t)
        .into_iter()
        .filter(|s| s.contains("cmp"))
        .collect();
    assert_eq!(tagged.len(), 2, "{tagged:?}");
    assert!(tagged[0].starts_with("Delay tower · cmp"), "{tagged:?}");
    assert!(tagged[1].starts_with("t12 · cmp"), "{tagged:?}");
    assert!(tree_detail(&t, TreeKey::Live(MeasId(3))).ends_with(" · cmp"));
    assert!(tree_detail(&t, TreeKey::Trace(TraceId(12))).ends_with(" · cmp"));
    // Compare is an overlay: M averages the group's own traces.
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    let r = t.key("M");
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call { cmd: Command::TraceAverage { traces, .. }, .. }]
                if traces == &[TraceId(10), TraceId(13)]
        ),
        "{r:?}"
    );
    // A compared curve of the pane's own group is drawn once, without the tag.
    t.key("C");
    assert!(
        legend_text(&t)
            .iter()
            .all(|s| !s.starts_with("Main L · cmp")),
        "{:?}",
        legend_text(&t)
    );
    t.key("C");
    // C again un-compares.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    t.key("C");
    assert_eq!(t.last_toast(), "t12: compare off");
    assert_eq!(
        legend(&t),
        ["Delay tower", "L ÷ tower", "Main L", "t10", "t13"]
    );
    // Clear compare from the palette drops the rest.
    t.key("Ctrl+K");
    t.text("clear compare");
    t.key("Enter");
    assert_eq!(legend(&t), ["L ÷ tower", "Main L", "t10", "t13"]);
    assert!(t.st.compared_meas.is_empty() && t.st.compared_traces.is_empty());
}

/// A compared trace deleted (here or by another client) drops out of compare; the compared
/// measurements are kept by name in `ui.toml`, as hidden ones are.
#[test]
fn compare_forgets_deleted_traces_and_keeps_names() {
    let mut t = loaded(two_groups());
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    t.key("C");
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    t.key("C");
    let mut s = two_groups();
    s.traces.retain(|x| x.id != TraceId(12));
    t.conn(mirror(s));
    assert!(t.st.compared_traces.is_empty());
    assert_eq!(
        t.st.layout_prefs().compared,
        ["Delay tower".to_owned()].into()
    );
}

/// Plain C is compare now: the comparison cursor is on the palette only.
#[test]
fn the_cursor_is_on_the_palette_not_c() {
    let mut t = loaded(two_groups());
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("C");
    assert_eq!(t.st.view.cursor_hz, None);
    t.key("Ctrl+K");
    t.text("comparison cursor");
    t.key("Enter");
    assert!(t.st.view.cursor_hz.is_some());
}
