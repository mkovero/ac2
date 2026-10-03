use super::*;
use ac2_proto::frame::{LeqFlags, LeqMeta};
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
    };
    let th = Theme::dark();
    let s = leq_scene(&v, &Status::default(), &th, size(1200.0, 700.0));
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
        "FOH SPL",
        "M30 · cal 3 h ago",
        "LAeq 30 min",
        "97.8",
        "OVER",
        "NEAR",
        "limit 99.0 dB · next 1 min ≤ 104.2 dB",
        "12:30 / 30:00",
        "now",
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
