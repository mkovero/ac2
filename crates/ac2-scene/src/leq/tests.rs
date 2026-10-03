use super::*;
use ac2_proto::frame::{LeqFlags, LeqMeta, LeqRun};
use ac2_proto::model::CalStatus;
use ac2_proto::units::{Db, DbSpl, MeasId, Seconds, WallNs};

fn w(minutes: f64, limit: Option<f64>) -> LeqWindow {
    LeqWindow {
        duration: Seconds(minutes * 60.0),
        weighting: Weighting::A,
        limit: limit.map(DbSpl),
        warn_margin: Db(3.0),
    }
}

fn cfg() -> LeqConfig {
    LeqConfig {
        windows: vec![
            w(1.0, None),
            w(15.0, Some(100.0)),
            w(30.0, Some(99.0)),
            w(60.0, Some(93.0)),
        ],
        horizon: Seconds(60.0),
    }
}

fn judged(extra: LeqFlags) -> LeqFlags {
    LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(extra)
}

/// 17:02 UTC on 3 October 2026: 19:02 at UTC+2.
const START_S: u64 = 1_791_046_920;
const UTC_PLUS_2: i32 = 7200;

/// A run of 2:14:05 from 19:02 local, 12 s of it not measured.
fn run() -> LeqRun {
    LeqRun {
        started_at: WallNs(START_S * 1_000_000_000),
        until: WallNs((START_S + 8045) * 1_000_000_000 + 400_000_000),
        measured: Seconds(8033.0),
        gaps: Seconds(12.4),
        trimmed: false,
        laeq: 97.84,
        lceq: 110.21,
        lzeq: 112.0,
    }
}

fn run_of(c: &LeqConfig, f: &LeqFrame) -> Option<LeqRunText> {
    f.meta.run.map(|r| run_text(&r, c, |_| UTC_PLUS_2))
}

fn frame(scale: LevelScale) -> LeqFrame {
    LeqFrame {
        meas: MeasId(4),
        meta: LeqMeta {
            scale,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(1),
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            logged: 3600,
            run: Some(run()),
        },
        leq: vec![97.84, 98.26, 96.94, f32::NAN],
        elapsed: vec![60.0, 900.0, 750.0, 3600.0],
        measured: vec![60.0, 900.0, 750.0, 3480.0],
        allowed: vec![f32::NAN, 101.56, 104.2, f32::NAN],
        recover: vec![f32::NAN, f32::NAN, f32::NAN, 450.0],
        flags: vec![
            LeqFlags::NONE,
            judged(LeqFlags::NEAR),
            judged(LeqFlags::NONE),
            judged(LeqFlags::OVER)
                .with(LeqFlags::CANNOT_RECOVER)
                .with(LeqFlags::INCOMPLETE),
        ],
    }
}

#[test]
fn names_and_lengths() {
    assert_eq!(window_name(&w(30.0, None)), "LAeq 30 min");
    assert_eq!(window_name(&w(60.0, None)), "LAeq 60 min");
    let c = LeqWindow {
        weighting: Weighting::C,
        duration: Seconds(10.0),
        ..w(1.0, None)
    };
    assert_eq!(window_name(&c), "LCeq 10 s");
    assert_eq!(length(90.0), "1 min 30 s");
    assert_eq!(length(4.0 * 3600.0), "4 h");
    assert_eq!(length(7200.0), "120 min");
    assert_eq!(clock(750.0), "12:30");
    assert_eq!(clock(3723.0), "1:02:03");
    assert_eq!(clock(f64::NAN), "—");
}

#[test]
fn tile_strings() {
    let t = leq_tiles(&cfg(), &frame(LevelScale::DbSpl));
    assert_eq!(t.len(), 4);
    // No limit: the value only.
    assert_eq!(t[0].name, "LAeq 1 min");
    assert_eq!(t[0].value, "97.8");
    assert_eq!(t[0].unit, "dB SPL");
    assert_eq!(t[0].state, TileState::NoLimit);
    assert_eq!(t[0].state_text, None);
    assert_eq!((t[0].limit.as_ref(), t[0].headroom.as_ref()), (None, None));
    // Near: limit and the floored headroom.
    assert_eq!(t[1].state, TileState::Near);
    assert_eq!(t[1].state_text.as_deref(), Some("NEAR"));
    assert_eq!(t[1].limit.as_deref(), Some("limit 100.0 dB"));
    assert_eq!(t[1].headroom.as_deref(), Some("next 1 min ≤ 101.5 dB"));
    assert_eq!(t[1].filling, None);
    // Filling: elapsed of length.
    assert_eq!(t[2].state, TileState::Ok);
    assert_eq!(t[2].filling.as_deref(), Some("12:30 / 30:00"));
    assert_eq!(t[2].headroom.as_deref(), Some("next 1 min ≤ 104.2 dB"));
    // Over, not recoverable within the horizon, with gaps; no value measured shows a dash.
    assert_eq!(t[3].value, "—");
    assert_eq!(t[3].state_text.as_deref(), Some("OVER"));
    assert_eq!(
        t[3].headroom.as_deref(),
        Some("over — can't recover within 1 min")
    );
    assert_eq!(
        t[3].recover.as_deref(),
        Some("at the limit: back under in 7 min 30 s")
    );
    assert_eq!(
        t[3].incomplete.as_deref(),
        Some("gaps: 58:00 of 1:00:00 measured")
    );
}

#[test]
fn uncalibrated_tiles_judge_nothing() {
    let mut f = frame(LevelScale::Dbfs);
    f.flags = vec![
        LeqFlags::NONE,
        LeqFlags::LIMIT,
        LeqFlags::LIMIT,
        LeqFlags::LIMIT,
    ];
    let t = leq_tiles(&cfg(), &f);
    assert_eq!(t[1].unit, "dBFS");
    assert_eq!(t[1].state, TileState::NotCalibrated);
    assert_eq!(t[1].state_text.as_deref(), Some("not calibrated"));
    assert_eq!(t[1].headroom, None);
    assert_eq!(t[3].recover, None);
    // A frame of another configuration gives no tiles.
    let mut short = cfg();
    short.windows.pop();
    assert!(leq_tiles(&short, &f).is_empty());
}

#[test]
fn colours_follow_the_state() {
    for th in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
        assert_eq!(
            tile_colors(TileState::Over, &th),
            (th.banner_fault.background, th.banner_fault.text)
        );
        assert_eq!(
            tile_colors(TileState::Near, &th),
            (th.banner_warning.background, th.banner_warning.text)
        );
        for s in [TileState::Ok, TileState::NoLimit, TileState::NotCalibrated] {
            assert_eq!(tile_colors(s, &th), (th.plot_background, th.text));
        }
    }
}

fn size(w: f32, h: f32) -> Viewport {
    Viewport {
        width: w,
        height: h,
    }
}

fn texts(s: &Scene) -> Vec<&str> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect()
}

#[test]
fn scene_lays_tiles_out_and_colours_them() {
    let c = cfg();
    let f = frame(LevelScale::DbSpl);
    let mut h = LeqHistory::default();
    for k in 0..30 {
        h.push(&c, &f, 1000.0 + f64::from(k));
    }
    let v = LeqView {
        meter: "FOH SPL".into(),
        cal: "M30 · cal 3 h ago".into(),
        cfg: &c,
        tiles: leq_tiles(&c, &f),
        history: Some(&h),
        stale: None,
        scale: LevelScale::DbSpl,
        horizon: "1 min".into(),
        layout: LeqLayout {
            style: LeqStyle::Tiles,
            history: true,
        },
        run: run_of(&c, &f),
    };
    let th = Theme::dark();
    let s = leq_scene(&v, &Status::default(), &th, size(1200.0, 700.0));
    assert!(s.columns.is_none());
    assert_eq!(s.tiles.len(), 4);
    let hist = s.history.as_ref().expect("room for the history");
    // Tiles do not overlap and stay above the strip.
    for (i, a) in s.tiles.iter().enumerate() {
        assert!(a.bottom() <= hist.plot.y, "{a:?} above {:?}", hist.plot);
        for b in &s.tiles[i + 1..] {
            assert!(!crate::canvas::tests::intersects(*a, *b));
        }
    }
    // Four tiles above a strip on 1200 × 700: two by two, wider than tall.
    assert_eq!(s.tiles[0].y, s.tiles[1].y);
    assert_eq!(s.tiles[2].y, s.tiles[3].y);
    assert!(s.tiles[2].y > s.tiles[0].y);
    assert!(s.tiles.iter().all(|r| r.w > r.h));
    let all = texts(&s.scene);
    for want in [
        "FOH SPL · LAeq, dB SPL",
        "M30 · cal 3 h ago",
        "30 min",
        "97.8",
        "OVER",
        "NEAR",
        "limit 99.0 dB · next 1 min ≤ 104.2 dB",
        "12:30 / 30:00",
        "now",
        "running 2:14:05 since 19:02 · LAeq total 97.8 · gaps 0:12",
    ] {
        assert!(all.contains(&want), "{want} in {all:?}");
    }
    // The over tile is red, the near tile amber, the others plain.
    let fill = |r: &Rect| {
        s.scene.layers[0]
            .rects
            .iter()
            .find(|x| x.rect == *r)
            .map(|x| x.color)
    };
    assert_eq!(fill(&s.tiles[3]), Some(th.banner_fault.background));
    assert_eq!(fill(&s.tiles[1]), Some(th.banner_warning.background));
    assert_eq!(fill(&s.tiles[0]), Some(th.plot_background));
    // Big numbers: the value is the largest text of its tile.
    let v97 = s.scene.layers[2]
        .labels
        .iter()
        .find(|l| l.text == "97.8")
        .expect("value");
    assert!(v97.size > 40.0, "{}", v97.size);
    // Maximised to a whole screen the numbers grow.
    let big = leq_scene(&v, &Status::default(), &th, size(2400.0, 1400.0));
    let b97 = big.scene.layers[2]
        .labels
        .iter()
        .find(|l| l.text == "97.8")
        .expect("value");
    assert!(b97.size > v97.size * 1.5);
    // A short pane is all tiles.
    let short = leq_scene(&v, &Status::default(), &th, size(900.0, 260.0));
    assert!(short.history.is_none());
    assert!(short.tiles.iter().all(|r| r.bottom() <= 260.0));
    // History off: all tiles however tall.
    let off = LeqView {
        layout: LeqLayout {
            style: LeqStyle::Tiles,
            history: false,
        },
        ..v.clone()
    };
    assert!(
        leq_scene(&off, &Status::default(), &th, size(1200.0, 700.0))
            .history
            .is_none()
    );
    let banners = leq_scene(
        &v,
        &crate::banner::tests::everything(),
        &th,
        size(900.0, 500.0),
    );
    crate::canvas::tests::assert_banners_clear(&banners.scene, &banners.banners, &banners.tiles);
}

#[test]
fn history_marks_over_segments_and_limits() {
    let c = LeqConfig {
        windows: vec![LeqWindow {
            duration: Seconds(10.0),
            ..w(1.0, Some(90.0))
        }],
        horizon: Seconds(2.0),
    };
    let mut h = LeqHistory::default();
    let mk = |leq: f32, over: bool| LeqFrame {
        leq: vec![leq],
        elapsed: vec![10.0],
        measured: vec![10.0],
        allowed: vec![f32::NAN],
        recover: vec![f32::NAN],
        flags: vec![if over {
            judged(LeqFlags::OVER)
        } else {
            judged(LeqFlags::NONE)
        }],
        ..frame(LevelScale::DbSpl)
    };
    // 10 s at 74, 10 s over at 94, 10 s back at 74; then a 20 s hole, then 5 s.
    for k in 0..30 {
        let over = (10..20).contains(&k);
        h.push(&c, &mk(if over { 94.0 } else { 74.0 }, over), f64::from(k));
    }
    for k in 50..55 {
        h.push(&c, &mk(74.0, false), f64::from(k));
    }
    // The same frame twice is one point.
    h.push(&c, &mk(74.0, false), 54.0);
    assert_eq!(h.points(&c.windows[0]).expect("series").len(), 35);
    let plot = Rect::new(50.0, 10.0, 600.0, 200.0);
    let s = history_strip(&c, &h, true, plot, &Theme::dark());
    assert_eq!(history_span(&c), 120.0);
    let l = &s.lines[0];
    assert_eq!(l.name, "LAeq 10 s");
    // One over run, from the first over second to the first one back under.
    assert_eq!(l.over.len(), 1);
    assert_eq!(l.over[0].len(), 11);
    // The hole breaks the line.
    assert_eq!(l.points.iter().filter(|p| p[0].is_nan()).count(), 1);
    // The limit line sits between 74 and 94.
    let y = |v: f64| s.y.mapping.to_px(v);
    let ly = l.limit_y.expect("limit");
    assert!((ly - y(90.0)).abs() < 1e-3);
    assert!(y(94.0) < ly && ly < y(74.0));
    // Time labels count back from the newest frame, in seconds for a short span.
    let labels = s.x.labels();
    assert_eq!(labels.last().copied(), Some("now"));
    assert!(
        labels
            .iter()
            .any(|t| t.ends_with(" s") && t.starts_with('−'))
    );
    // Unjudged: no limit line.
    let s = history_strip(&c, &h, false, plot, &Theme::dark());
    assert_eq!(s.lines[0].limit_y, None);
    // A change of unit (a calibration) starts the history over.
    let mut f = mk(-20.0, false);
    f.meta.scale = LevelScale::Dbfs;
    h.push(&c, &f, 60.0);
    assert_eq!(h.points(&c.windows[0]).expect("series").len(), 1);
}

// ---------------------------------------------------------------------------------------
// Columns

/// `n` windows (1 … 120 min, some C-weighted when `mixed`), listed longest first so the
/// columns must reorder them, each limited at 99 dB; the frame puts them over, near, ok,
/// filling and unmeasured in turn.
fn many(n: usize, mixed: bool, scale: LevelScale) -> (LeqConfig, LeqFrame) {
    let minutes = [120.0, 60.0, 30.0, 15.0, 10.0, 5.0, 2.0, 1.0];
    let windows: Vec<LeqWindow> = minutes[8 - n..]
        .iter()
        .enumerate()
        .map(|(i, m)| LeqWindow {
            weighting: if mixed && i % 3 == 1 {
                Weighting::C
            } else {
                Weighting::A
            },
            ..w(*m, Some(99.0))
        })
        .collect();
    let judge = |extra| match scale {
        LevelScale::DbSpl => judged(extra),
        LevelScale::Dbfs => LeqFlags::LIMIT,
    };
    let mut f = frame(scale);
    f.leq.clear();
    f.elapsed.clear();
    f.measured.clear();
    f.allowed.clear();
    f.recover.clear();
    f.flags.clear();
    for (i, win) in windows.iter().enumerate() {
        let (leq, flags, elapsed) = match i % 5 {
            0 => (
                101.3,
                judge(LeqFlags::OVER.with(LeqFlags::CANNOT_RECOVER)),
                1.0,
            ),
            1 => (97.2, judge(LeqFlags::NEAR), 1.0),
            2 => (88.4, judge(LeqFlags::NONE), 0.4),
            3 => (100.6, judge(LeqFlags::OVER), 1.0),
            _ => (f32::NAN, judge(LeqFlags::NONE), 0.0),
        };
        let leq = match scale {
            LevelScale::DbSpl => leq,
            LevelScale::Dbfs => leq - 120.0,
        };
        f.leq.push(leq);
        f.elapsed.push((win.duration.0 * elapsed) as f32);
        f.measured.push((win.duration.0 * elapsed) as f32);
        f.allowed.push(if i % 5 == 0 { f32::NAN } else { 101.56 });
        f.recover.push(if i % 5 == 0 { 450.0 } else { f32::NAN });
        f.flags.push(flags);
    }
    (
        LeqConfig {
            windows,
            horizon: Seconds(60.0),
        },
        f,
    )
}

fn columns_view<'a>(c: &'a LeqConfig, f: &LeqFrame, h: Option<&'a LeqHistory>) -> LeqView<'a> {
    LeqView {
        meter: "FOH SPL".into(),
        cal: "M30 · cal 3 h ago".into(),
        cfg: c,
        tiles: leq_tiles(c, f),
        history: h,
        stale: None,
        scale: f.meta.scale,
        horizon: "1 min".into(),
        layout: LeqLayout::default(),
        run: run_of(c, f),
    }
}

fn cols(s: &LeqScene) -> &LeqColumns {
    s.columns.as_ref().expect("columns")
}

#[test]
fn columns_are_the_default_and_go_shortest_left() {
    assert_eq!(
        LeqLayout::default(),
        LeqLayout {
            style: LeqStyle::Columns,
            history: false
        }
    );
    let (c, f) = many(5, false, LevelScale::DbSpl);
    let s = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &Theme::dark(),
        size(1200.0, 700.0),
    );
    assert!(s.tiles.is_empty() && s.history.is_none());
    let k = cols(&s);
    // Configured 15, 10, 5, 2, 1 min: drawn 1, 2, 5, 10, 15 min, left to right.
    let order: Vec<usize> = k.columns.iter().map(|x| x.window).collect();
    assert_eq!(order, [4, 3, 2, 1, 0]);
    let names: Vec<&str> = k.columns.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["1 min", "2 min", "5 min", "10 min", "15 min"]);
    assert!(
        k.columns
            .windows(2)
            .all(|p| p[0].rect.right() < p[1].rect.x)
    );
    // Full height: every column spans the area under the caption.
    assert!(k.columns.iter().all(|x| x.rect.h > 600.0));
    // Equal length, C after A.
    let mut c2 = c.clone();
    c2.windows[1] = LeqWindow {
        weighting: Weighting::C,
        ..c2.windows[0]
    };
    let tiles = leq_tiles(&c2, &f);
    let v = LeqView {
        tiles,
        cfg: &c2,
        ..columns_view(&c, &f, None)
    };
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 700.0));
    let order: Vec<usize> = cols(&s).columns.iter().map(|x| x.window).collect();
    assert_eq!(order, [4, 3, 2, 0, 1]);
}

#[test]
fn judged_scale_is_anchored_to_the_limits() {
    let r = column_range(&[99.0], &[120.0, 40.0], LevelScale::DbSpl, None);
    assert_eq!((r.lo, r.hi), (69.0, 105.0));
    // Different limits: one scale covering all of them.
    let r = column_range(&[93.0, 100.0], &[], LevelScale::DbSpl, None);
    assert_eq!((r.lo, r.hi), (63.0, 106.0));
    // Judged limits win over the free range's memory.
    let r = column_range(
        &[99.0],
        &[50.0],
        LevelScale::DbSpl,
        Some(crate::axis::Range::new(20.0, 60.0)),
    );
    assert_eq!((r.lo, r.hi), (69.0, 105.0));

    let (c, f) = many(5, false, LevelScale::DbSpl);
    let s = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &Theme::dark(),
        size(1200.0, 700.0),
    );
    let k = cols(&s);
    assert_eq!((k.range.lo, k.range.hi), (69.0, 105.0));
    // One track geometry for all: bars compare, the limit sits at the same height.
    let t0 = k.columns[0].track;
    for x in &k.columns {
        assert_eq!((x.track.y, x.track.h), (t0.y, t0.h));
        assert_eq!(x.limit_y, k.columns[0].limit_y);
    }
    let y = |v: f64| t0.bottom() - ((v - 69.0) / 36.0) as f32 * t0.h;
    let ly = k.columns[0].limit_y.expect("limit");
    assert!((ly - y(99.0)).abs() < 1e-3);
    // The bar of 101.3 dB reaches above the limit, 88.4 dB stays below; nothing measured
    // is no bar.
    let by = |w: usize| k.columns.iter().find(|x| x.window == w).expect("col");
    let over = by(0).bar.expect("bar");
    assert!((over.y - y(101.3)).abs() < 1e-3 && over.y < ly);
    assert!((over.bottom() - t0.bottom()).abs() < 1e-3);
    assert!(by(2).bar.expect("bar").y > ly);
    assert_eq!(by(4).bar, None);
    // A level off the top fills the track, and no more.
    let mut loud = f.clone();
    loud.leq[1] = 130.0;
    let s = leq_scene(
        &columns_view(&c, &loud, None),
        &Status::default(),
        &Theme::dark(),
        size(1200.0, 700.0),
    );
    let b = cols(&s)
        .columns
        .iter()
        .find(|x| x.window == 1)
        .and_then(|x| x.bar)
        .expect("bar");
    assert!((b.y - t0.y).abs() < 1e-3);
    // The scale is written on the left when there is room.
    let all = texts(&s.scene);
    for t in ["70", "80", "90", "100"] {
        assert!(all.contains(&t), "{t} in {all:?}");
    }
    assert!(cols(&s).gutter.is_some());
}

#[test]
fn free_scale_moves_in_steps_with_hysteresis() {
    let spl = LevelScale::DbSpl;
    let r = column_range(&[], &[93.0, 85.0], spl, None);
    assert_eq!((r.lo, r.hi), (60.0, 100.0));
    // Second to second the scale stays while the loudest window is 80 … 98 dB.
    let mut prev = Some(r);
    for v in [97.9, 80.0, 95.0, 88.0, 98.0] {
        let n = column_range(&[], &[v], spl, prev);
        assert_eq!(n, r, "{v}");
        prev = Some(n);
    }
    // Near the top it steps up by 10 dB, then stays on the way back down to 90 dB…
    let up = column_range(&[], &[98.3], spl, prev);
    assert_eq!((up.lo, up.hi), (70.0, 110.0));
    assert_eq!(column_range(&[], &[91.0], spl, Some(up)), up);
    // …and comes back down only below it.
    let down = column_range(&[], &[89.5], spl, Some(up));
    assert_eq!((down.lo, down.hi), (60.0, 100.0));
    // Nothing measured keeps what there was, or starts from the unit's default.
    assert_eq!(column_range(&[], &[f64::NAN], spl, Some(up)), up);
    let none = column_range(&[], &[], LevelScale::Dbfs, None);
    assert_eq!((none.lo, none.hi), (-40.0, 0.0));
    let fs = column_range(&[], &[-23.4], LevelScale::Dbfs, None);
    assert_eq!((fs.lo, fs.hi), (-50.0, -10.0));

    // The history keeps the memory frame to frame; uncalibrated limits do not anchor it.
    let (c, mut f) = many(3, false, LevelScale::Dbfs);
    let mut h = LeqHistory::default();
    f.leq = vec![-20.0, -22.0, -30.0];
    h.push(&c, &f, 1.0);
    let first = h.range().expect("range");
    assert_eq!((first.lo, first.hi), (-50.0, -10.0));
    f.leq = vec![-13.0, -22.0, -30.0];
    h.push(&c, &f, 2.0);
    assert_eq!(h.range(), Some(first));
    let s = leq_scene(
        &columns_view(&c, &f, Some(&h)),
        &Status::default(),
        &Theme::dark(),
        size(900.0, 500.0),
    );
    assert_eq!(cols(&s).range, first);
    f.leq = vec![-11.0, -22.0, -30.0];
    h.push(&c, &f, 3.0);
    assert_eq!(h.range().map(|r| r.hi), Some(0.0));
}

#[test]
fn column_colours_follow_the_state() {
    for th in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
        let (c, f) = many(5, false, LevelScale::DbSpl);
        let s = leq_scene(
            &columns_view(&c, &f, None),
            &Status::default(),
            &th,
            size(1200.0, 700.0),
        );
        let by = |w: usize| {
            cols(&s)
                .columns
                .iter()
                .find(|x| x.window == w)
                .expect("col")
                .clone()
        };
        // Over: the whole column red, the bar the alarm colour, the text still readable.
        let over = by(0);
        assert_eq!(over.bar_color, th.banner_fault.background);
        assert_ne!(over.background, th.plot_background);
        let ratio = crate::theme::contrast_ratio(th.text, over.background);
        assert!(ratio >= 4.5, "{:?}: text on over {ratio:.2}", th.name);
        assert_eq!(by(3).background, over.background);
        // Near: amber bar, a lighter tint.
        let near = by(1);
        assert_eq!(near.bar_color, th.banner_warning.background);
        assert_ne!(near.background, th.plot_background);
        let ratio = crate::theme::contrast_ratio(th.text, near.background);
        assert!(ratio >= 4.5, "{:?}: text on near {ratio:.2}", th.name);
        // Ok and filling: the plain background, the bar dimmed from the ok colour.
        let filling = by(2);
        assert!(filling.filling);
        assert_eq!(filling.background, th.plot_background);
        assert_ne!(filling.bar_color, th.level_ok);
        assert_ne!(filling.bar_color, th.banner_warning.background);
        assert_ne!(filling.bar_color, th.banner_fault.background);
        assert_eq!(
            column_colors(TileState::Ok, &th),
            (th.plot_background, th.level_ok)
        );
        // Each column's background is drawn.
        for x in &cols(&s).columns {
            assert!(
                s.scene.layers[0]
                    .rects
                    .iter()
                    .any(|r| r.rect == x.rect && r.color == x.background)
            );
        }
    }
    // Not judged (uncalibrated): no colour, no limit marker.
    let th = Theme::dark();
    let (c, f) = many(5, false, LevelScale::Dbfs);
    let s = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &th,
        size(1200.0, 700.0),
    );
    for x in &cols(&s).columns {
        assert_eq!(x.background, th.plot_background);
        assert_eq!(x.limit_y, None);
        assert_ne!(x.bar_color, th.banner_fault.background);
        assert_ne!(x.bar_color, th.level_ok);
    }
    let all = texts(&s.scene);
    assert!(all.contains(&"not calibrated"), "{all:?}");
    assert!(all.contains(&"FOH SPL · LAeq, dBFS"), "{all:?}");
    assert!(!all.contains(&"OVER"), "{all:?}");
}

#[test]
fn column_texts() {
    let (c, f) = many(5, false, LevelScale::DbSpl);
    let s = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &Theme::dark(),
        size(1920.0, 1080.0),
    );
    let all = texts(&s.scene);
    for want in [
        "FOH SPL · LAeq, dB SPL",
        "M30 · cal 3 h ago",
        "101.3",
        "OVER",
        "NEAR",
        "OK",
        "limit 99.0 dB",
        "next 1 min ≤ 101.5 dB",
        // The longest wording that fits.
        "back under in 7 min 30 s",
        // The 5 min window, 40 % elapsed, and the 1 min one just started.
        "2:00 / 5:00",
        "0:00 / 1:00",
        "—",
        // One weighting: the caption names it once, the columns only their lengths.
        "15 min",
    ] {
        assert!(all.contains(&want), "{want} in {all:?}");
    }
    // The value is the biggest text, as big as the column allows, and grows with it.
    let size_of = |s: &LeqScene, t: &str| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == t)
            .map(|l| l.size)
            .expect(t)
    };
    let v = size_of(&s, "101.3");
    assert!(v > 60.0, "{v}");
    assert!(v > size_of(&s, "15 min"));
    let small = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &Theme::dark(),
        size(800.0, 450.0),
    );
    assert!(size_of(&small, "101.3") < v);
}

/// Every label of the scene that is not the caption, with the column it sits in.
fn column_labels(s: &LeqScene) -> Vec<(&crate::primitives::Label, Option<usize>)> {
    let k = cols(s);
    s.scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .filter(|l| l.pos[1] >= s.caption.bottom())
        .map(|l| {
            let b = crate::canvas::tests::label_box(l);
            let at = k
                .columns
                .iter()
                .position(|x| b.x >= x.rect.x - 0.5 && b.right() <= x.rect.right() + 0.5);
            (l, at)
        })
        .collect()
}

#[test]
fn columns_fit_two_to_eight_windows_without_overlap() {
    let th = Theme::dark();
    for scale in [LevelScale::DbSpl, LevelScale::Dbfs] {
        for mixed in [false, true] {
            for n in 2..=8 {
                let (c, f) = many(n, mixed, scale);
                for (w, h) in [
                    (320.0, 240.0),
                    (320.0, 480.0),
                    (480.0, 320.0),
                    (640.0, 400.0),
                    (960.0, 540.0),
                    (1280.0, 720.0),
                    (1920.0, 1080.0),
                ] {
                    let s = leq_scene(
                        &columns_view(&c, &f, None),
                        &Status::default(),
                        &th,
                        size(w, h),
                    );
                    let k = cols(&s);
                    assert_eq!(k.columns.len(), n);
                    let at = format!("{n} windows ({mixed}, {scale:?}) at {w}×{h}");
                    for x in &k.columns {
                        assert!(x.rect.x >= 0.0 && x.rect.right() <= w + 0.01, "{at}");
                        assert!(x.rect.bottom() <= h + 0.01, "{at}");
                        assert!(x.track.h >= 20.0, "{at}: track {:?}", x.track);
                    }
                    let labels = column_labels(&s);
                    let boxes: Vec<Rect> = labels
                        .iter()
                        .map(|(l, _)| crate::canvas::tests::label_box(l))
                        .collect();
                    for (i, (l, col)) in labels.iter().enumerate() {
                        // Inside its column, or the scale in the gutter.
                        if col.is_none() {
                            let g = k.gutter.expect("a label outside the columns is the scale");
                            assert!(boxes[i].right() <= g.right() + 0.5, "{at}: {:?}", l.text);
                        }
                        for (j, b) in boxes.iter().enumerate().skip(i + 1) {
                            assert!(
                                !crate::canvas::tests::intersects(boxes[i], *b),
                                "{at}: {:?} overlaps {:?}",
                                l.text,
                                labels[j].0.text
                            );
                        }
                    }
                    assert_caption_fits(&s, w, &at);
                    // Each column keeps its value and its name.
                    for x in &k.columns {
                        let inside = labels
                            .iter()
                            .filter(|(_, col)| {
                                col.is_some_and(|ci| k.columns[ci].window == x.window)
                            })
                            .count();
                        assert!(inside >= 2, "{at}: {}", x.name);
                    }
                }
            }
        }
    }
}

/// The caption's labels (meter, run, calibration) stay apart, inside the pane and above
/// the windows, and the run is there at least as its clock.
fn assert_caption_fits(s: &LeqScene, w: f32, at: &str) {
    let caption: Vec<(&str, Rect)> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .filter(|l| l.pos[1] >= s.caption.y - 0.5 && l.pos[1] < s.caption.bottom())
        .map(|l| (l.text.as_str(), crate::canvas::tests::label_box(l)))
        .collect();
    assert!(s.run.is_some(), "{at}: no run in the caption");
    assert_eq!(caption.len(), 3, "{at}: {caption:?}");
    for (i, (t, b)) in caption.iter().enumerate() {
        assert!(b.x >= -0.5 && b.right() <= w + 0.5, "{at}: {t:?} {b:?}");
        assert!(
            b.bottom() <= s.caption.bottom() + 0.5,
            "{at}: {t:?} {b:?} below the caption"
        );
        for (u, c) in &caption[i + 1..] {
            assert!(
                !crate::canvas::tests::intersects(*b, *c),
                "{at}: {t:?} overlaps {u:?}"
            );
        }
    }
}

/// The run in the caption at every width, in columns and tiles: the longest wording that
/// fits, shortened to the clock and the total and then to the clock, never overlapping
/// the meter or the calibration; large on a full-screen pane.
#[test]
fn caption_run_fits_every_width() {
    let th = Theme::dark();
    for style in [LeqStyle::Columns, LeqStyle::Tiles] {
        for mixed in [false, true] {
            let (c, f) = many(5, mixed, LevelScale::DbSpl);
            for (w, h) in [
                (320.0, 240.0),
                (320.0, 480.0),
                (480.0, 320.0),
                (640.0, 400.0),
                (960.0, 540.0),
                (1280.0, 720.0),
                (1920.0, 1080.0),
            ] {
                let v = LeqView {
                    layout: LeqLayout {
                        style,
                        history: false,
                    },
                    ..columns_view(&c, &f, None)
                };
                let s = leq_scene(&v, &Status::default(), &th, size(w, h));
                let at = format!("{style:?} ({mixed}) at {w}×{h}");
                assert_caption_fits(&s, w, &at);
                // The windows start under the caption.
                for r in s.tiles.iter().chain(
                    s.columns
                        .iter()
                        .flat_map(|k| k.columns.iter().map(|x| &x.rect)),
                ) {
                    assert!(r.y >= s.caption.bottom() - 0.5, "{at}: {r:?}");
                }
                let run = s.run.clone().unwrap_or_default();
                assert!(
                    run.starts_with("running 2:14:05") || run.starts_with("2:14:05"),
                    "{at}: {run}"
                );
                if w >= 1280.0 {
                    assert!(
                        run.starts_with("running 2:14:05 since 19:02 · LAeq total 97.8"),
                        "{at}: {run}"
                    );
                }
                if w >= 1920.0 {
                    let want = if mixed {
                        "running 2:14:05 since 19:02 · LAeq total 97.8 · LCeq total 110.2 · gaps 0:12"
                    } else {
                        "running 2:14:05 since 19:02 · LAeq total 97.8 · gaps 0:12"
                    };
                    assert_eq!(run, want, "{at}");
                }
            }
        }
    }
    // The stage: a full-screen pane writes the run large, on the meter's row.
    let (c, f) = many(5, false, LevelScale::DbSpl);
    let s = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &th,
        size(1920.0, 1080.0),
    );
    let label = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .find(|l| Some(&l.text) == s.run.as_ref())
        .expect("run label");
    assert!(label.size >= 2.0 * th.font_size, "{}", label.size);
    assert!((label.pos[1] - s.caption.y).abs() < 0.5);
    // At 320 px it has a row of its own, shortened.
    let narrow = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &th,
        size(320.0, 480.0),
    );
    let run = narrow.run.clone().expect("run");
    assert!(
        run.len() < "running 2:14:05 since 19:02 · LAeq total 97.8".len(),
        "{run}"
    );
    // Without a run (nothing logged yet) the caption is the meter and the calibration.
    let none = LeqView {
        run: None,
        ..columns_view(&c, &f, None)
    };
    let s = leq_scene(&none, &Status::default(), &th, size(960.0, 540.0));
    assert_eq!(s.run, None);
}

#[test]
fn run_wordings() {
    let c = cfg();
    let r = run_text(&run(), &c, |_| UTC_PLUS_2);
    assert_eq!(r.clock, "2:14:05");
    assert_eq!(r.since, "19:02");
    assert_eq!(r.gaps.as_deref(), Some("0:12"));
    assert_eq!(
        r.variants(),
        [
            "running 2:14:05 since 19:02 · LAeq total 97.8 · gaps 0:12",
            "2:14:05 since 19:02 · total 97.8 · gaps 0:12",
            "2:14:05 · total 97.8 · gaps 0:12",
            "2:14:05 · total 97.8",
            "2:14:05",
        ]
    );
    // No gaps: none named. A C-weighted window adds LCeq and names LAeq in the short form.
    let mut whole = run();
    whole.gaps = Seconds(0.4);
    let mut cc = c.clone();
    cc.windows[1].weighting = Weighting::C;
    let r = run_text(&whole, &cc, |_| UTC_PLUS_2);
    assert_eq!(
        r.variants(),
        [
            "running 2:14:05 since 19:02 · LAeq total 97.8 · LCeq total 110.2",
            "running 2:14:05 since 19:02 · LAeq total 97.8",
            "2:14:05 since 19:02 · LAeq total 97.8",
            "2:14:05 · LAeq total 97.8",
            "2:14:05",
        ]
    );
    // Started yesterday (local): the date is named. Under an hour: still h:mm:ss.
    let mut early = run();
    early.started_at = WallNs((START_S - 20 * 3600) * 1_000_000_000);
    let r = run_text(&early, &c, |_| UTC_PLUS_2);
    assert_eq!(r.since, "2 Oct 23:02");
    let mut short = run();
    short.until = WallNs((START_S + 75) * 1_000_000_000);
    assert_eq!(run_text(&short, &c, |_| UTC_PLUS_2).clock, "0:01:15");
    // The offset in force at each instant: a start before a DST change.
    let r = run_text(&run(), &c, |t| {
        if t.0 < (START_S + 60) * 1_000_000_000 {
            3600
        } else {
            7200
        }
    });
    assert_eq!(r.since, "18:02");
    // Trimmed at 48 h: said so, the clock is the span kept.
    let mut full = run();
    full.trimmed = true;
    full.until = WallNs((START_S + 48 * 3600 + 30) * 1_000_000_000);
    full.gaps = Seconds(30.0);
    let r = run_text(&full, &c, |_| UTC_PLUS_2);
    let v = r.variants();
    assert_eq!(
        v[0],
        "last 48 h: 48:00:30 since 3 Oct 19:02 · LAeq total 97.8 · gaps 0:30"
    );
    assert_eq!(v[v.len() - 1], "last 48 h");
    // Nothing measured: no value.
    let mut empty = run();
    empty.laeq = f64::NAN;
    assert!(run_text(&empty, &c, |_| 0).line().contains("LAeq total —"));
}

#[test]
fn new_log_confirmation_names_what_ends() {
    let c = cfg();
    let r = run_text(&run(), &c, |_| UTC_PLUS_2);
    let k = new_log_confirm("FOH SPL", &c, Some(&r));
    assert_eq!(k.title, "Start a new SPL log for FOH SPL?");
    assert_eq!(
        k.lines[0],
        "The current log ends: running 2:14:05 since 19:02 · LAeq total 97.8 · gaps 0:12."
    );
    assert!(
        k.lines[1].contains(
            "the 4 Leq windows and their states, the alarms, the run clock and the total"
        )
    );
    assert!(k.lines[2].starts_with("Kept: the windows, limits"));
    assert!(k.lines[3].contains("ac2 spl leq export --previous"));
    let k = new_log_confirm("FOH SPL", &c, None);
    assert_eq!(k.lines[0], "The current log ends (no seconds logged yet).");
}

#[test]
fn narrow_columns_shorten_the_names() {
    let th = Theme::dark();
    let (c, f) = many(8, false, LevelScale::DbSpl);
    let s = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &th,
        size(480.0, 400.0),
    );
    let k = cols(&s);
    assert_eq!(k.columns[0].name, "1m");
    assert_eq!(k.columns[7].name, "120m");
    // The weighting the names leave out goes into the caption.
    assert_eq!(k.weighting.as_deref(), Some("LAeq"));
    assert!(texts(&s.scene).contains(&"FOH SPL · LAeq, dB SPL"));
    let (c, f) = many(8, true, LevelScale::DbSpl);
    let s = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &th,
        size(480.0, 400.0),
    );
    // Mixed weightings keep the letter.
    let k = cols(&s);
    assert!(
        k.columns.iter().any(|x| x.name.starts_with('C')),
        "{:?}",
        k.columns
    );
    assert_eq!(k.weighting, None);
    let wide = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &th,
        size(1920.0, 1080.0),
    );
    assert!(
        cols(&wide)
            .columns
            .iter()
            .all(|x| x.name.starts_with("LAeq") || x.name.starts_with("LCeq"))
    );
}

#[test]
fn columns_keep_banners_and_history_clear() {
    let (c, f) = many(5, false, LevelScale::DbSpl);
    let mut h = LeqHistory::default();
    for k in 0..30 {
        h.push(&c, &f, 1000.0 + f64::from(k));
    }
    let v = LeqView {
        layout: LeqLayout {
            style: LeqStyle::Columns,
            history: true,
        },
        ..columns_view(&c, &f, Some(&h))
    };
    let th = Theme::dark();
    let s = leq_scene(&v, &Status::default(), &th, size(1200.0, 800.0));
    let hist = s.history.as_ref().expect("history on");
    assert!(
        cols(&s)
            .columns
            .iter()
            .all(|x| x.rect.bottom() <= hist.plot.y)
    );
    let b = leq_scene(
        &v,
        &crate::banner::tests::everything(),
        &th,
        size(900.0, 500.0),
    );
    let rects: Vec<Rect> = cols(&b).columns.iter().map(|x| x.rect).collect();
    crate::canvas::tests::assert_banners_clear(&b.scene, &b.banners, &rects);
}
