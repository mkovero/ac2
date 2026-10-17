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
            ir: None,
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

/// The measurement whose group the transfer pane draws: the one it shows, or the owner of
/// the math channel it shows.
fn transfer_group(t: &T) -> Option<MeasId> {
    t.st.group_of_shown(t.st.kind_meas(PaneKind::Transfer))
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
    assert_eq!(transfer_group(&t), Some(MeasId(1)));
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(3))),
        &t.keys,
    );
    assert_eq!(legend(&t), ["Delay tower", "t11"]);
    // A trace selected brings its measurement's group with it.
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    assert_eq!(transfer_group(&t), Some(MeasId(1)));
    assert_eq!(legend(&t), ["L ÷ tower", "Main L", "t10", "t13"]);
    // The math channel shown on the pane draws its owner's group.
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(6))),
        &t.keys,
    );
    assert_eq!(transfer_group(&t), Some(MeasId(1)));
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
    let hint =
        t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys)
            .expect("hint");
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
            [Request::Call { what, .. }] if what.ends_with("under Imported: Move to measurement… to see it")
        ),
        "{r:?}"
    );
    // Moved under Main L: drawn, and the hint is gone.
    let mut s = daemon_state();
    s.traces = vec![owned(12, under(1))];
    t.conn(mirror(s));
    assert_eq!(t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys), None);
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
                resolution: ac2_proto::model::Resolution::FortyEighth,
            },
        },
    ));
    let run = sweep_meta(20);
    s.traces = vec![run.clone()];
    let mut t = T::new();
    t.conn(mirror(s));
    // A sweep pane beside the grid, focused: the sweep selected in the list goes there.
    t.go(PaneKind::Distortion);
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
    // The IR keys keep a transfer measurement: the IR view's.
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

/// The labels of the compare chooser open now.
fn chooser(t: &T) -> (String, Vec<String>) {
    match &t.st.overlay {
        Overlay::Choose(c) => (
            c.title.clone(),
            // Whether a target is on screen depends on what the selection put there.
            c.choices
                .iter()
                .map(|c| c.label.replace(" · on screen", ""))
                .collect(),
        ),
        o => panic!("no chooser: {o:?}"),
    }
}

/// C on what the tree has selected asks which measurement's panes draw it, and nothing is
/// drawn until one is ticked: then another measurement's live curve and capture are drawn
/// on that measurement's transfer pane besides its group, tagged `cmp` in the legend and
/// the tree; an imported trace is not compared (it is no measurement's until moved under
/// one); the pane's own group is not offered; Enter on a ticked row takes it out; average
/// still takes the group only; Clear compare drops every one.
#[test]
fn c_compares_the_selection_on_the_transfer_pane() {
    use ac2_scene::meas_list::TreeKey;
    let mut t = loaded(two_groups());
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(1))),
        &t.keys,
    );
    let before = legend(&t);
    t.st.update(Msg::SelectMeas(MeasId(3)), &t.keys);
    assert!(t.key("C").is_empty(), "nothing is compared before a target");
    let (title, rows) = chooser(&t);
    assert_eq!(title, "Compare Delay tower with…");
    assert_eq!(
        rows.first().map(String::as_str),
        Some("  Main L"),
        "{rows:?}"
    );
    assert!(rows.iter().all(|r| !r.contains("Delay tower")), "{rows:?}");
    // The row chosen put Delay tower on the pane; Main L back there draws as before.
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(1))),
        &t.keys,
    );
    assert_eq!(legend(&t), before);
    let r = t.key("Enter");
    assert_eq!(
        calls(&r).first().map(|(c, _)| (*c).clone()),
        Some(Command::MeasCompare {
            meas: MeasId(1),
            compared: vec![Compared::Meas { meas: MeasId(3) }],
        })
    );
    t.answer(&r);
    assert_eq!(t.last_toast(), "Delay tower compared into Main L (cmp)");
    assert_eq!(chooser(&t).1[0], "\u{2713} Main L");
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key("C").is_empty());
    assert!(
        t.last_toast().starts_with("t12 is under Imported"),
        "{}",
        t.last_toast()
    );
    assert_eq!(t.st.overlay, Overlay::None);
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("C");
    let r = t.key("Enter");
    t.answer(&r);
    t.key("Escape");
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(1))),
        &t.keys,
    );
    assert_eq!(transfer_group(&t), Some(MeasId(1)));
    let tagged: Vec<String> = legend_text(&t)
        .into_iter()
        .filter(|s| s.contains("cmp"))
        .collect();
    assert_eq!(tagged.len(), 2, "{tagged:?}");
    assert!(tagged[0].starts_with("Delay tower · cmp"), "{tagged:?}");
    assert!(tagged[1].starts_with("t11 · cmp"), "{tagged:?}");
    assert!(tree_detail(&t, TreeKey::Live(MeasId(3))).ends_with(" · cmp"));
    assert!(tree_detail(&t, TreeKey::Trace(TraceId(11))).ends_with(" · cmp"));
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
    // The pane's own group is not offered: C on Main L lists Delay tower only.
    t.key("C");
    let (_, rows) = chooser(&t);
    assert!(rows.iter().all(|r| !r.contains("Main L")), "{rows:?}");
    t.key("Escape");
    // Enter on a ticked row takes it out.
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("C");
    assert_eq!(chooser(&t).1[0], "\u{2713} Main L");
    let r = t.key("Enter");
    t.answer(&r);
    assert_eq!(t.last_toast(), "t11: no longer compared into Main L");
    t.key("Escape");
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(1))),
        &t.keys,
    );
    assert!(
        legend_text(&t).iter().all(|s| !s.starts_with("t11")),
        "{:?}",
        legend_text(&t)
    );
    // Clear compare from the palette drops the rest.
    t.key("Ctrl+K");
    t.text("clear compare");
    let r = t.key("Enter");
    t.answer(&r);
    assert_eq!(legend(&t), ["L ÷ tower", "Main L", "t10", "t13"]);
    assert_eq!(t.st.meas(MeasId(1)).map(|m| m.compared.len()), Some(0));
}

/// A compare list is the daemon's, with the measurement: an app connecting draws it as
/// listed, and an entry whose trace is gone draws nothing (the daemon prunes it).
#[test]
fn compare_lists_come_from_the_daemon() {
    let mut s = two_groups();
    s.measurements
        .iter_mut()
        .find(|m| m.id == MeasId(1))
        .expect("Main L")
        .compared = vec![
        Compared::Trace { trace: TraceId(11) },
        Compared::Meas { meas: MeasId(3) },
    ];
    let mut t = loaded(s.clone());
    t.st.update(
        Msg::PanePick(t.pane(PaneKind::Transfer), PaneMenuRow::Meas(MeasId(1))),
        &t.keys,
    );
    let tagged: Vec<String> = legend_text(&t)
        .into_iter()
        .filter(|s| s.contains("cmp"))
        .collect();
    assert_eq!(tagged.len(), 2, "{tagged:?}");
    s.traces.retain(|x| x.id != TraceId(11));
    t.conn(mirror(s));
    let tagged: Vec<String> = legend_text(&t)
        .into_iter()
        .filter(|s| s.contains("cmp"))
        .collect();
    assert_eq!(tagged.len(), 1, "{tagged:?}");
    assert!(tagged[0].starts_with("Delay tower · cmp"), "{tagged:?}");
}

/// Plain C is compare now: the comparison cursor is on the palette only.
#[test]
fn the_cursor_is_on_the_palette_not_c() {
    let mut t = loaded(two_groups());
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("C");
    assert_eq!(t.st.view.cursor_hz, None);
    t.key("Escape");
    t.key("Ctrl+K");
    t.text("comparison cursor");
    t.key("Enter");
    assert!(t.st.view.cursor_hz.is_some());
}

/// A stopped transfer measurement keeps its last curve, tagged `stopped` as a stopped
/// spectrum is, in each of the magnitude, phase and coherence views (so in the transfer
/// stack); hiding it is A's. Started again, the tag goes.
#[test]
fn a_stopped_transfer_measurement_keeps_its_curve_tagged() {
    let mut s = four();
    s.traces = vec![owned(10, under(1))];
    s.traces[0].edit.slot = Some(1);
    let mut t = loaded(s.clone());
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    let pane = t.pane(PaneKind::Transfer);
    let main = |t: &T| {
        crate::scenes::transfer(&t.st, pane, &Theme::dark(), SIZE, now())
            .legend
            .into_iter()
            .find(|e| e.name == "Main L")
            .map(|e| e.tags.contains(&"stopped".to_owned()))
    };
    assert_eq!(legend(&t), ["Main L", "t10"]);
    assert_eq!(main(&t), Some(false));

    s.measurements[1].running = false;
    t.conn(mirror(s.clone()));
    for _ in 0..3 {
        assert!(t.st.layout.view(pane).is_some_and(|v| !v.shows_ir()));
        assert_eq!(legend(&t), ["Main L", "t10"]);
        assert_eq!(main(&t), Some(true));
        let caption = t.st.pane_caption(pane).unwrap_or_default();
        assert!(!caption.contains("stopped"), "{caption}");
        t.st.layout.set_focus(pane);
        t.key("G");
    }
    // Hidden only when the operator says so.
    for _ in 0..3 {
        t.key("Shift+G");
    }
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    t.key("A");
    assert_eq!(legend(&t), ["t10"]);
    t.key("A");

    s.measurements[1].running = true;
    t.conn(mirror(s));
    assert_eq!(main(&t), Some(false));
}

/// A click on a transfer legend row selects that row's curve, wherever it comes from —
/// a capture of the group, its math channel, a compared measurement, another measurement's
/// compared capture —
/// the list marks it (a live curve: its measurement's row), the pane keeps its group, and A hides exactly that curve.
#[test]
fn a_legend_row_click_selects_its_curve_for_a() {
    use ac2_scene::meas_list::{Mark, TreeKey};
    use ac2_scene::trace::TraceKey;
    let mut t = loaded(two_groups());
    t.remirror(|s| {
        if let Some(m) = s.measurements.iter_mut().find(|m| m.id == MeasId(1)) {
            m.compared = vec![
                Compared::Meas { meas: MeasId(3) },
                Compared::Trace { trace: TraceId(11) },
            ];
        }
    });
    t.st.update(Msg::SelectMeas(MeasId(1)), &t.keys);
    let pane = t.pane(PaneKind::Transfer);
    // The click as the pane makes it: the scene's row under the pointer.
    let click = |t: &mut T, name: &str| {
        let s = crate::scenes::transfer(&t.st, pane, &Theme::dark(), SIZE, now());
        let i = s.legend.iter().position(|e| e.name == name).expect(name);
        let r = s
            .legend_hits
            .iter()
            .find(|h| h.key == s.legend[i].key)
            .expect("row shown")
            .rect;
        let key =
            ac2_scene::legend::hit_at(&s.legend_hits, r.x + 4.0, r.y + r.h / 2.0).expect("hit");
        t.st.update(Msg::LegendPick(pane, key), &t.keys);
        key
    };
    let active = |t: &T| {
        t.st.tree_rows()
            .into_iter()
            .filter(|r| r.mark == Mark::Active)
            .map(|r| r.key)
            .collect::<Vec<_>>()
    };
    for (name, key, row) in [
        (
            "t13",
            TraceKey::Stored(TraceId(13)),
            TreeKey::Trace(TraceId(13)),
        ),
        (
            "t11",
            TraceKey::Stored(TraceId(11)),
            TreeKey::Trace(TraceId(11)),
        ),
        (
            "L ÷ tower",
            TraceKey::Live(MeasId(6)),
            TreeKey::Math(MeasId(6)),
        ),
        (
            "Delay tower",
            TraceKey::Live(MeasId(3)),
            TreeKey::Live(MeasId(3)),
        ),
        (
            "Main L",
            TraceKey::Live(MeasId(1)),
            TreeKey::Live(MeasId(1)),
        ),
    ] {
        assert_eq!(click(&mut t, name), key);
        match key {
            TraceKey::Stored(id) => assert_eq!(t.st.selected_trace, Some(id)),
            TraceKey::Live(id) => {
                assert_eq!((t.st.selected, t.st.selected_trace), (Some(id), None));
            }
        }
        assert!(active(&t).contains(&row), "{name}: {:?}", active(&t));
        assert_eq!(transfer_group(&t), Some(MeasId(1)), "{name}");
        assert_eq!(t.st.layout.focus, pane);
    }
    // A on a compared measurement picked in the legend hides it there.
    click(&mut t, "Delay tower");
    t.key("A");
    assert!(
        t.last_toast().starts_with("Delay tower hidden"),
        "{}",
        t.last_toast()
    );
    assert!(!legend(&t).contains(&"Delay tower".to_owned()));
    t.key("A");
    assert!(legend(&t).contains(&"Delay tower".to_owned()));
    // A on a compared capture picked in the legend asks the daemon to hide that trace.
    click(&mut t, "t11");
    let r = t.key("A");
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call { cmd: Command::TraceUpdate { trace: TraceId(11), edit }, .. }]
                if !edit.visible
        ),
        "{r:?}"
    );
}
