use ac2_proto::model::{
    CaptureContext, EqMark, Marker, MarkerKind, MicCurveRef, MicState, PlanSpec, PointState,
    WalkKind, WalkPoint, WalkState, ZoneSpec,
};
use ac2_proto::units::{EqSetId, Hz, MeasId, TraceId, WalkId, WallNs};
use ac2_room::pattern::{Broken, PresetKind, Relaxed};

use super::setup::*;
use super::*;
use crate::primitives::Viewport;
use crate::theme::Theme;
use crate::view::{PlotChrome, Style};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// A 20 × 30 m hall, stage at y = 30, FOH at (10, 10) facing the stage (+y).
fn hall() -> PlanSpec {
    PlanSpec {
        name: "Hall".into(),
        outline: rect(0.0, 0.0, 20.0, 30.0),
        ceiling: Some(10.0),
        zones: vec![ZoneSpec {
            name: "Stalls".into(),
            outline: rect(0.0, 0.0, 20.0, 24.0),
            ear_height: 1.2,
            weight: None,
            seats: None,
        }],
        markers: vec![Marker {
            name: "FOH".into(),
            kind: MarkerKind::Foh,
            at: [10.0, 10.0, 0.0],
            heading_deg: Some(90.0),
        }],
        surfaces: vec![],
    }
}

fn walk(states: &[PointState], current: Option<u16>) -> WalkState {
    WalkState {
        id: WalkId(1),
        name: "Mains".into(),
        plan: "Hall".into(),
        kind: WalkKind::LiveTransfer,
        meas: vec![MeasId(1)],
        points: states
            .iter()
            .enumerate()
            .map(|(i, s)| WalkPoint {
                label: format!("P{}", i + 1),
                // P3 at 2.1 m left (−x facing +y) and 4.0 m back (−y) of FOH.
                at: if i == 2 {
                    [7.9, 6.0, 1.2]
                } else {
                    [2.0 + 2.0 * i as f64, 18.0, 1.2]
                },
                step: i as u16,
                mic: MeasId(1),
                state: s.clone(),
            })
            .collect(),
        step: current,
        average: None,
    }
}

fn pending(n: usize) -> Vec<PointState> {
    vec![PointState::Pending; n]
}

/// 2026-10-29 12:00 UTC, the daemon's clock.
const NOON: u64 = 1_793_275_200_000_000_000;
const DAY: u64 = 86_400_000_000_000;

fn chain(mic: Option<(&str, Option<&str>)>, eq: &[(u32, u32)]) -> CaptureContext {
    CaptureContext {
        at: WallNs(0),
        mic: mic.map(|(name, hash)| MicState {
            name: name.into(),
            curve: hash.map(|h| MicCurveRef {
                label: "0°".into(),
                file_name: "mic.txt".into(),
                content_hash: h.into(),
                points: 100,
                f_lo: Hz(20.0),
                f_hi: Hz(20_000.0),
                imported_at: WallNs(0),
                stated_sensitivity: None,
            }),
        }),
        eq_in: eq
            .iter()
            .map(|&(set, rev)| EqMark {
                set: EqSetId(set),
                rev,
            })
            .collect(),
    }
}

fn captured_at(t: u32, at: u64, c: CaptureContext) -> PointState {
    PointState::Captured {
        trace: TraceId(t),
        context: CaptureContext {
            at: WallNs(at),
            ..c
        },
    }
}

fn captured(t: u32) -> PointState {
    captured_at(t, NOON, chain(Some(("Vilppu", Some("aa"))), &[]))
}

fn utc(_: WallNs) -> i32 {
    0
}

#[test]
fn the_step_reads_from_foh_facing_the_stage() {
    let w = walk(&pending(8), Some(2));
    assert_eq!(
        step_text(&w, Some(&hall()), &[]),
        "Move mic to P3 · 2.1 m left, 4.0 m back from FOH"
    );
    // Without a FOH or speaker heading the readout says it is plan x/y.
    let mut bare = hall();
    bare.markers.clear();
    assert_eq!(
        step_text(&w, Some(&bare), &[]),
        "Move mic to P3 · x 7.9 m, y 6.0 m in plan (no FOH or speaker heading)"
    );
    let done = walk(&[captured(1), PointState::Skipped], None);
    assert_eq!(
        step_text(&done, Some(&hall()), &[]),
        "every point is captured or skipped"
    );
}

#[test]
fn ready_states_read_as_the_note_says() {
    assert_eq!(Ready::of(Some(0.6), false).text(), "settling");
    assert_eq!(Ready::of(Some(0.6), false).bar(), Some(0.6));
    // One block short of full is not ready.
    assert_eq!(Ready::of(Some(0.999), false).text(), "settling");
    assert_eq!(Ready::of(Some(1.0), false).text(), "ready");
    assert_eq!(Ready::of(Some(1.0), false).bar(), Some(1.0));
    assert_eq!(Ready::of(Some(0.2), true).bar(), None);
    assert_eq!(Ready::of(None, false).bar(), None);
    // A pick waits for the operator, full or not.
    assert_eq!(Ready::of(Some(1.0), true).text(), "arrival: pick one");
    assert_eq!(Ready::of(Some(0.2), true), Ready::Pick);
    assert_eq!(Ready::of(None, false).text(), "waiting for the measurement");
}

#[test]
fn the_average_line_counts_positions_and_unsettled_captures() {
    let mut states = pending(8);
    states[0] = captured(1);
    let w = walk(&states, Some(1));
    assert_eq!(average_text(&w, None, 0), "NO AVERAGE · 1 OF 8 POSITIONS");
    // A skipped point is no position.
    states[1] = PointState::Skipped;
    assert_eq!(
        average_text(&walk(&states, Some(2)), None, 0),
        "NO AVERAGE · 1 OF 7 POSITIONS"
    );
    let mut all = vec![captured(1); 8];
    all[7] = captured(8);
    let mut w = walk(&all, None);
    w.average = Some(MeasId(9));
    assert_eq!(
        average_text(&w, Some("Mains · average"), 2),
        "Mains · average · 8 positions · 2 unsettled"
    );
    assert_eq!(positions_text(8, 0), "8 positions");
    assert_eq!(positions_text(1, 0), "1 position");
    assert_eq!(walk_row(&w), "Mains · 8 of 8 captured · Hall");
}

#[test]
fn dots_mark_the_current_point_and_the_next_one_faint() {
    let mut states = pending(4);
    states[0] = captured(1);
    states[2] = PointState::Skipped;
    let w = walk(&states, Some(1));
    let d = walk_dots(&w);
    let looks: Vec<(DotLook, bool, bool)> = d.iter().map(|d| (d.look, d.current, d.next)).collect();
    assert_eq!(
        looks,
        [
            (DotLook::Captured, false, false),
            (DotLook::Pending, true, false),
            (DotLook::Skipped, false, false),
            (DotLook::Pending, false, true),
        ]
    );
    // The next step wraps round past the end.
    let w = walk(&pending(3), Some(2));
    assert_eq!(next_step(&w), Some(0));
}

/// Two mics, Vilppu (1) and MM1 (2), over five points: step 0 at P1 and P2, step 1 at P3
/// (MM1) and P4 (Vilppu), step 2 at P5 (MM1).
fn two_mics(states: &[PointState], step: Option<u16>) -> WalkState {
    let mut w = walk(states, step);
    w.meas = vec![MeasId(1), MeasId(2)];
    let plan = [(0, 1), (0, 2), (1, 2), (1, 1), (2, 2)];
    for (p, &(s, m)) in w.points.iter_mut().zip(&plan) {
        p.step = s;
        p.mic = MeasId(m);
    }
    w
}

fn names() -> Vec<String> {
    vec!["Vilppu".into(), "MM1".into()]
}

#[test]
fn with_two_mics_the_step_names_each_mic_and_its_point() {
    let w = two_mics(&pending(5), Some(1));
    assert_eq!(
        step_text(&w, Some(&hall()), &names()),
        "Move Vilppu to P4, MM1 to P3"
    );
    let t = target_lines(&w, Some(&hall()), &names());
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].0, 0);
    assert!(t[0].1.starts_with("Vilppu → P4 · "), "{t:?}");
    assert_eq!(
        t[1],
        (1, "MM1 → P3 · 2.1 m left, 4.0 m back from FOH".to_owned())
    );
    // One mic left to place at the step (the other's point captured): it alone, with its
    // readout, and no target lines.
    let mut states = pending(5);
    states[3] = captured(1);
    let w = two_mics(&states, Some(1));
    assert_eq!(
        step_text(&w, Some(&hall()), &names()),
        "Move MM1 to P3 · 2.1 m left, 4.0 m back from FOH"
    );
    assert!(target_lines(&w, Some(&hall()), &names()).is_empty());
}

#[test]
fn with_two_mics_the_plan_rings_each_mic_and_the_next_step() {
    let mut states = pending(5);
    states[0] = captured(1);
    states[1] = captured(2);
    let w = two_mics(&states, Some(1));
    let d = walk_dots(&w);
    let rings: Vec<(bool, bool, usize)> = d.iter().map(|d| (d.current, d.next, d.mic)).collect();
    assert_eq!(
        rings,
        [
            (false, false, 0),
            (false, false, 1),
            (true, false, 1),
            (true, false, 0),
            (false, true, 1),
        ]
    );
}

#[test]
fn the_slowest_mic_is_the_one_read() {
    assert_eq!(
        Ready::slowest(&[Ready::Ready, Ready::Settling(0.4), Ready::Settling(0.7)]),
        Some(1)
    );
    assert_eq!(
        Ready::slowest(&[Ready::Settling(0.1), Ready::Pick]),
        Some(1)
    );
    assert_eq!(Ready::slowest(&[Ready::Ready, Ready::Waiting]), Some(1));
    assert_eq!(Ready::slowest(&[Ready::Ready, Ready::Ready]), Some(0));
    assert_eq!(Ready::slowest(&[]), None);
    let w = two_mics(&pending(5), Some(0));
    let mics = names();
    let p = WalkPane {
        walk: &w,
        plan: None,
        ready: Ready::Settling(0.3),
        mics: &mics,
        slowest: Some(1),
        average: None,
        unsettled: 0,
        bar_drawn: None,
        conditions: String::new(),
    };
    assert_eq!(p.lines().ready, "settling · MM1");
    let ready = WalkPane {
        ready: Ready::Ready,
        ..p.clone()
    };
    assert_eq!(ready.lines().ready, "ready");
}

#[test]
fn a_mic_change_is_a_change_only_on_one_measurement() {
    // P1 by Vilppu's measurement, P2 by MM1's: two mics by design, nothing differs.
    let mut states = pending(5);
    states[0] = captured_at(1, NOON, chain(Some(("Vilppu", None)), &[]));
    states[1] = captured_at(2, NOON, chain(Some(("MM1", None)), &[]));
    let w = two_mics(&states, Some(1));
    assert_eq!(conditions_text(&w, WallNs(NOON), utc), "");
    // P4 by Vilppu's measurement with another mic on its input: that is a change.
    states[3] = captured_at(3, NOON, chain(Some(("Behringer", None)), &[]));
    let w = two_mics(&states, Some(2));
    assert_eq!(
        conditions_text(&w, WallNs(NOON), utc),
        "captures differ · mic changed"
    );
}

#[test]
fn the_walk_pane_draws_its_lines_and_the_plan() {
    let theme = Theme::dark();
    let plan = hall();
    let mut states = pending(8);
    states[0] = captured(1);
    let w = walk(&states, Some(2));
    let p = WalkPane {
        walk: &w,
        plan: Some(&plan),
        ready: Ready::Settling(0.6),
        mics: &[],
        slowest: None,
        average: None,
        unsettled: 0,
        bar_drawn: Some(0.5),
        conditions: "P1 yesterday, P2 today · EQ changed".into(),
    };
    let s = walk_scene(
        &p,
        Style {
            chrome: PlotChrome::Full,
            theme: &theme,
        },
        Viewport {
            width: 900.0,
            height: 600.0,
        },
    );
    let texts: Vec<&str> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect();
    for want in [
        "Move mic to P3 · 2.1 m left, 4.0 m back from FOH",
        "settling",
        "NO AVERAGE · 1 OF 8 POSITIONS",
        "P1 yesterday, P2 today · EQ changed",
        "Stalls",
        "FOH",
        "P1",
        "P8",
    ] {
        assert!(texts.contains(&want), "{want} not in {texts:?}");
    }
    assert!(
        texts.iter().any(|t| t.ends_with(" m")),
        "a scale bar: {texts:?}"
    );
    // The readiness bar: its track, and its fill at the drawn (gliding) value, not the
    // frame's.
    let bars: Vec<&crate::primitives::FillRect> = s.scene.layers[1].rects.iter().collect();
    let track = s.scene.layers[0]
        .rects
        .iter()
        .find(|r| r.color == theme.plot_background && r.rect.w == BAR_W)
        .expect("a bar track");
    let fill = bars
        .iter()
        .find(|r| r.rect.y == track.rect.y)
        .expect("a bar fill");
    assert!((fill.rect.w - 0.5 * BAR_W).abs() < 1e-3, "{fill:?}");
    // The floor and the zone are filled polygons.
    let polys: usize = s.scene.layers.iter().map(|l| l.polygons.len()).sum();
    assert!(polys >= 2);
    // The map puts plan points inside the plan area and reads them back.
    let map = s.map.expect("a plan");
    let px = map.to_px([10.0, 10.0]);
    let back = map.to_m(px);
    assert!((back[0] - 10.0).abs() < 1e-6 && (back[1] - 10.0).abs() < 1e-6);
    // Plan y goes up the screen.
    assert!(map.to_px([10.0, 20.0])[1] < px[1]);

    let none = WalkPane { plan: None, ..p };
    let s = walk_scene(
        &none,
        Style {
            chrome: PlotChrome::Full,
            theme: &theme,
        },
        Viewport {
            width: 900.0,
            height: 600.0,
        },
    );
    assert!(s.map.is_none());
}

#[test]
fn scale_bar_and_snap() {
    assert_eq!(scale_bar(20.0, 120.0), (5.0, "5 m".to_owned()));
    assert_eq!(scale_bar(200.0, 120.0), (0.5, "0.5 m".to_owned()));
    assert_eq!(snap([1.234, -0.04]), [1.2, 0.0]);
    assert_eq!(snap([2.25, 7.96]), [2.3, 8.0]);
}

fn input<'a>(
    plan: Option<&'a PlanSpec>,
    labels: &'a [String],
    breaks: &'a [Vec<Broken>],
    relaxed: &'a [Relaxed],
) -> SetupInput<'a> {
    SetupInput {
        name: "Walk 1",
        plan,
        zone: None,
        preset: PresetKind::Tuning,
        reverb_s: 1.0,
        n: 8,
        default_n: Some(8),
        seed: 3,
        mics: &["Main L"],
        add_mic: false,
        focus: SetupField::Pattern,
        typed: None,
        relaxed,
        labels,
        breaks,
        error: None,
    }
}

#[test]
fn the_setup_names_its_rows_relaxed_rules_and_broken_points() {
    let plan = hall();
    let labels: Vec<String> = (1..=3).map(|i| format!("P{i}")).collect();
    let breaks = vec![
        vec![],
        vec![Broken::Spacing {
            other: 0,
            got: 0.5,
            wanted: 2.0,
        }],
        vec![],
    ];
    let relaxed = vec![Relaxed::Spacing {
        wanted: 2.0,
        got: 1.6,
        n: 8,
    }];
    let v = setup_view(&input(Some(&plan), &labels, &breaks, &relaxed));
    let rows: Vec<(String, String, bool)> = v
        .rows
        .iter()
        .map(|r| (r.label.clone(), r.value.clone(), r.focused))
        .collect();
    assert_eq!(
        rows,
        [
            ("name".into(), "Walk 1".into(), false),
            ("plan".into(), "Hall · 1 zone".into(), false),
            ("zones".into(), "all (Stalls)".into(), false),
            ("preset".into(), "Tuning".into(), false),
            ("positions".into(), "8".into(), false),
            ("pattern".into(), "3".into(), true),
            ("mic".into(), "Main L".into(), false),
        ]
    );
    assert_eq!(
        v.notes,
        [
            "relaxed: spacing 1.6 m: 8 positions do not fit at 2.0 m",
            "P2: 0.5 m from P1 (spacing 2.0 m)",
        ]
    );
    assert!(v.hint.contains("Enter starts the walk"));

    // ISO 3382-1 adds its RT row; a zone without seats says the count is by hand.
    let mut iso = input(Some(&plan), &labels, &breaks, &relaxed);
    iso.preset = PresetKind::Iso3382;
    iso.reverb_s = 1.25;
    iso.default_n = None;
    iso.focus = SetupField::Count;
    iso.typed = Some("1");
    let v = setup_view(&iso);
    let labels_of: Vec<&str> = v.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels_of,
        [
            "name",
            "plan",
            "zones",
            "preset",
            "RT",
            "positions",
            "pattern",
            "mic"
        ]
    );
    assert_eq!(v.rows[3].value, "ISO 3382-1");
    assert_eq!(v.rows[4].value, "1.3 s");
    assert_eq!(v.rows[5].value, "1▏");

    // Another transfer on its own input to add: a row to add it; two chosen, each a row.
    let mut more = input(Some(&plan), &labels, &breaks, &relaxed);
    more.add_mic = true;
    let v = setup_view(&more);
    let last = v.rows.last().expect("a setup row");
    assert_eq!(
        (last.label.as_str(), last.value.as_str()),
        ("mic 2", "none · →: another mic")
    );
    more.mics = &["Main L", "Vilppu"];
    more.add_mic = false;
    let v = setup_view(&more);
    let tail: Vec<(&str, &str)> = v.rows[v.rows.len() - 2..]
        .iter()
        .map(|r| (r.label.as_str(), r.value.as_str()))
        .collect();
    assert_eq!(tail, [("mic", "Main L"), ("mic 2", "Vilppu")]);

    // On New plan…, Enter opens the editor.
    let mut new = input(None, &labels, &[], &[]);
    new.focus = SetupField::Plan;
    let v = setup_view(&new);
    assert_eq!(v.rows[1].value, NEW_PLAN);
    assert!(v.hint.contains("Enter opens the plan editor"));
}

#[test]
fn the_editor_lists_vertices_and_says_why_a_plan_is_refused() {
    let mut plan = hall();
    let v = editor_view(&EditorInput {
        plan: &plan,
        layer: Layer::Outline,
        focus: EditorField::Row(1),
        typed: None,
        verdict: Ok(()),
        row_error: None,
    });
    let rows: Vec<(&str, &str)> = v
        .rows
        .iter()
        .map(|r| (r.label.as_str(), r.value.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("name", "Hall"),
            ("ceiling", "10.0 m"),
            ("editing", "outline · 4 vertices"),
            ("1", "0.0 0.0"),
            ("2", "20.0 0.0"),
            ("3", "20.0 30.0"),
            ("4", "0.0 30.0"),
        ]
    );
    assert!(v.rows[4].focused);
    assert_eq!(v.verdict, "valid · Enter saves it to the plan library");

    plan.ceiling = None;
    let v = editor_view(&EditorInput {
        plan: &plan,
        layer: Layer::Markers,
        focus: EditorField::Row(0),
        typed: Some("10 9"),
        verdict: Err("edge 1 crosses edge 3".into()),
        row_error: None,
    });
    assert_eq!(v.rows[1].value, "none (outdoors)");
    assert_eq!(v.rows[2].value, "markers · 1");
    assert_eq!(v.rows[3].label, "FOH");
    assert_eq!(v.rows[3].value, "10 9▏");
    assert_eq!(v.verdict, "refused: edge 1 crosses edge 3");
    assert!(v.refused);
    let v = editor_view(&EditorInput {
        plan: &plan,
        layer: Layer::Markers,
        focus: EditorField::Layer,
        typed: None,
        verdict: Ok(()),
        row_error: None,
    });
    assert_eq!(v.rows[3].value, "FOH · 10.0 10.0 · heading 90°");
    let v = editor_view(&EditorInput {
        plan: &plan,
        layer: Layer::Zone(0),
        focus: EditorField::Layer,
        typed: None,
        verdict: Ok(()),
        row_error: None,
    });
    assert_eq!(v.rows[2].value, "zone Stalls · 4 vertices");
    assert_eq!(v.rows[3].value, "Stalls");
    assert_eq!(v.rows[4].value, "1.2 m");
}

#[test]
fn the_plan_view_of_an_empty_plan_has_a_scale() {
    let theme = Theme::dark();
    let empty = PlanSpec {
        name: String::new(),
        outline: vec![],
        ceiling: None,
        zones: vec![],
        markers: vec![],
        surfaces: vec![],
    };
    let pic = PlanPicture {
        plan: &empty,
        dots: vec![],
        editing: Some(vec![]),
    };
    assert_eq!(pic.bounds(), None);
    let (_, map) = plan_scene(
        &pic,
        Viewport {
            width: 400.0,
            height: 400.0,
        },
        &theme,
    );
    let o = map.to_px([0.0, 0.0]);
    let far = map.to_px([EMPTY_PLAN_M, EMPTY_PLAN_M]);
    assert!(o[0] < far[0] && o[1] > far[1]);
}

#[test]
fn captures_of_one_day_and_chain_say_nothing() {
    let mut states = pending(8);
    states[0] = captured(1);
    states[1] = captured(2);
    let w = walk(&states, Some(2));
    assert_eq!(conditions_text(&w, WallNs(NOON), utc), "");
    // Nothing captured: nothing to compare.
    assert_eq!(
        conditions_text(&walk(&pending(3), Some(0)), WallNs(NOON), utc),
        ""
    );
}

#[test]
fn a_resumed_walk_groups_its_points_by_day() {
    let mic = || chain(Some(("Vilppu", Some("aa"))), &[(1, 4)]);
    let mut states: Vec<PointState> = (0..8).map(|i| captured_at(i, NOON - DAY, mic())).collect();
    for (i, s) in states.iter_mut().enumerate().skip(4) {
        *s = captured_at(
            i as u32,
            NOON,
            chain(Some(("Vilppu", Some("aa"))), &[(1, 5)]),
        );
    }
    let w = walk(&states, None);
    assert_eq!(
        conditions_text(&w, WallNs(NOON), utc),
        "P1–P4 yesterday, P5–P8 today · EQ changed"
    );
    // Older days read as dates, oldest first; a skipped point carries the run on, and a
    // day's runs split by another day's are listed together.
    let mut states = vec![
        captured_at(1, NOON - 3 * DAY, mic()),
        PointState::Skipped,
        captured_at(3, NOON - 3 * DAY, mic()),
        captured_at(4, NOON, mic()),
        captured_at(5, NOON - 3 * DAY, mic()),
        PointState::Pending,
    ];
    assert_eq!(
        conditions_text(&walk(&states, Some(5)), WallNs(NOON), utc),
        "P1–P3 + P5 26 Oct, P4 today"
    );
    // Another year names it.
    states[0] = captured_at(1, NOON - 365 * DAY, mic());
    assert!(
        conditions_text(&walk(&states, Some(5)), WallNs(NOON), utc).starts_with("P1 29 Oct 2025, "),
    );
}

#[test]
fn the_day_is_the_local_one() {
    // 23:30 UTC and 00:30 UTC the next day: one day three hours east of UTC, two at UTC.
    let late = NOON + 11 * 3_600_000_000_000 + 1_800_000_000_000;
    let early = late + 3_600_000_000_000;
    let m = || chain(None, &[]);
    let w = walk(
        &[captured_at(1, late, m()), captured_at(2, early, m())],
        None,
    );
    assert_eq!(conditions_text(&w, WallNs(early), |_| 3 * 3600), "");
    assert_eq!(
        conditions_text(&w, WallNs(early), utc),
        "P1 yesterday, P2 today"
    );
}

#[test]
fn each_recorded_change_is_named_once() {
    let w = |a: CaptureContext, b: CaptureContext, c: CaptureContext| {
        walk(
            &[
                captured_at(1, NOON, a),
                captured_at(2, NOON, b),
                captured_at(3, NOON, c),
            ],
            None,
        )
    };
    let v = |h: Option<&'static str>| Some(("Vilppu", h));
    // Same day: the line says what it is about.
    assert_eq!(
        conditions_text(
            &w(
                chain(v(Some("aa")), &[]),
                chain(v(Some("bb")), &[]),
                chain(v(None), &[])
            ),
            WallNs(NOON),
            utc
        ),
        "captures differ · mic curve changed"
    );
    assert_eq!(
        conditions_text(
            &w(
                chain(v(Some("aa")), &[(1, 1)]),
                chain(Some(("Behringer", Some("aa"))), &[(1, 1)]),
                chain(None, &[(1, 2)]),
            ),
            WallNs(NOON),
            utc
        ),
        "captures differ · mic changed · EQ changed"
    );
    // The order of the marks is no change; a re-import of the same curve file is none.
    assert_eq!(
        conditions_text(
            &w(
                chain(v(Some("aa")), &[(1, 1), (2, 3)]),
                chain(v(Some("aa")), &[(2, 3), (1, 1)]),
                chain(v(Some("aa")), &[(1, 1), (2, 3)]),
            ),
            WallNs(NOON),
            utc
        ),
        ""
    );
}
