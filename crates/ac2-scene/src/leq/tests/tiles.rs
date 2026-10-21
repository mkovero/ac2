use super::*;

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
    assert_eq!(t[1].limit.as_deref(), Some("limit 100.0 dB SPL"));
    assert_eq!(
        t[1].headroom.as_deref(),
        Some("next 1 min: stay ≤ 101.5 dB SPL")
    );
    assert_eq!(t[1].filling, None);
    // Filling: the value is the Leq so far, elapsed of length; with more than the horizon
    // to fill, the headroom holds until the window is full; the bar is the Leq it ends at if
    // the rest is silent.
    assert_eq!(t[2].state, TileState::Ok);
    assert_eq!(t[2].filling.as_deref(), Some("so far · 12:30 / 30:00"));
    assert_eq!(
        t[2].headroom.as_deref(),
        Some("until full: stay ≤ 100.0 dB SPL")
    );
    assert!(t[2].filling() && t[2].allowed_until_full && !t[2].on_course);
    assert_eq!(t[2].course, None);
    assert!((t[2].bar_db() - 93.13).abs() < 1e-4, "{}", t[2].bar_db());
    assert_eq!(t[1].bar_db(), t[1].leq_db);
    // Over, not recoverable within the horizon, with gaps; no value measured shows a dash.
    assert_eq!(t[3].value, "—");
    assert_eq!(t[3].state_text.as_deref(), Some("OVER"));
    // No headroom then: how long it cools down, at the limit.
    assert_eq!(t[3].headroom, None);
    assert_eq!(t[3].recover.as_deref(), Some("cooling down in 7 min 30 s"));
    let mut f = frame(LevelScale::DbSpl);
    f.recover[3] = f32::NAN;
    let t = leq_tiles(&cfg(), &f);
    assert_eq!(t[3].recover.as_deref(), Some("cooling down"));
    assert_eq!(t[3].incomplete.as_deref(), Some("offline for 2 min"));
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
        layout: LeqLayout {
            style: LeqStyle::Tiles,
            history: true,
        },
        run: run_of(&c, &f),
        stage: false,
        chrome: crate::view::PlotChrome::Full,
        strip: Default::default(),
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
        // Each tile names its own weighting with its value.
        "dB SPL (A)",
        "97.8",
        "OVER",
        "NEAR",
        "limit 99.0 dB SPL",
        // What to do, without what it holds for: the level is what is acted on.
        "stay ≤ 100.0 dB SPL",
        "so far · 12:30 / 30:00",
        "now",
        "2:14:05 since 19:02 · total 97.8 · offline 12 s",
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
    // What to do reads larger than the value, which every tile of a grid shows at one size
    // whether or not it has an instruction.
    let size_of = |s: &LeqScene, t: &str| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == t)
            .map(|l| l.size)
            .expect(t)
    };
    let v97 = size_of(&s, "97.8");
    assert!(v97 > 12.0, "{v97}");
    assert!(size_of(&s, "stay ≤ 100.0 dB SPL") > v97);
    assert!((size_of(&s, "96.9") - v97).abs() < 1e-3);
    // Smaller than the window's name, at one place in every tile, instruction or not.
    assert!(v97 < size_of(&s, "1 min"));
    let at = |t: &str| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == t)
            .map(|l| l.pos[1])
            .expect(t)
    };
    assert!(((at("97.8") - s.tiles[0].y) - (at("98.3") - s.tiles[1].y)).abs() < 1e-3);
    // Maximised to a whole screen they grow.
    let big = leq_scene(&v, &Status::default(), &th, size(2400.0, 1400.0));
    assert!(size_of(&big, "97.8") > v97);
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
        peaks: Default::default(),
    };
    let mut h = LeqHistory::default();
    let mk = |leq: f32, over: bool| LeqFrame {
        leq: vec![leq],
        elapsed: vec![10.0],
        measured: vec![10.0],
        allowed: vec![f32::NAN],
        recover: vec![f32::NAN],
        least: vec![leq],
        over_in: vec![f32::NAN],
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
    let s = history_strip(&c, &h, true, plot, &Theme::dark(), StripSpan::default());
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
    let s = history_strip(&c, &h, false, plot, &Theme::dark(), StripSpan::default());
    assert_eq!(s.lines[0].limit_y, None);
    // A change of unit (a calibration) starts the history over.
    let mut f = mk(-20.0, false);
    f.meta.scale = LevelScale::Dbfs;
    h.push(&c, &f, 60.0);
    assert_eq!(h.points(&c.windows[0]).expect("series").len(), 1);
}

#[test]
fn decimation_keeps_ends_breaks_and_each_columns_extremes() {
    // Sparse: at most two points per pixel column, nothing goes.
    let sparse: Vec<[f32; 2]> = (0..50).map(|i| [i as f32 * 0.6, (i % 7) as f32]).collect();
    assert_eq!(decimate(&sparse), sparse);
    // Dense: 40 points per column over 100 columns, a break in the middle.
    let y = |i: usize| ((i as f32) * 0.37).sin() * 50.0 + if i == 1234 { 80.0 } else { 0.0 };
    let mut dense: Vec<[f32; 2]> = (0..4000).map(|i| [i as f32 / 40.0, y(i)]).collect();
    dense[2000] = [f32::NAN, f32::NAN];
    let d = decimate(&dense);
    assert!(d.len() <= 2 * 100 + 5, "{} points", d.len());
    assert_eq!(d.iter().filter(|p| p[0].is_nan()).count(), 1);
    for run in [&dense[..2000], &dense[2001..]] {
        assert!(d.contains(&run[0]) && d.contains(&run[run.len() - 1]));
    }
    for col in 0..100 {
        let ys = dense
            .iter()
            .filter(|p| p[0].is_finite() && p[0].floor() == col as f32)
            .map(|p| p[1]);
        let (lo, hi) = ys.fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)));
        let kept: Vec<f32> = d
            .iter()
            .filter(|p| p[0].is_finite() && p[0].floor() == col as f32)
            .map(|p| p[1])
            .collect();
        assert!(kept.contains(&lo) && kept.contains(&hi), "column {col}");
    }
    // Time order is kept: x never goes back within a run.
    assert!(
        d.windows(2)
            .all(|w| !(w[0][0].is_finite() && w[1][0].is_finite()) || w[1][0] >= w[0][0])
    );
}

#[test]
fn hours_of_history_draw_a_few_points_per_column_and_are_laid_out_once_per_frame() {
    let c = LeqConfig {
        windows: vec![w(60.0, Some(95.0))],
        horizon: Seconds(60.0),
        peaks: Default::default(),
    };
    let mk = |leq: f32| LeqFrame {
        leq: vec![leq],
        elapsed: vec![3600.0],
        measured: vec![3600.0],
        allowed: vec![f32::NAN],
        recover: vec![f32::NAN],
        least: vec![leq],
        over_in: vec![f32::NAN],
        flags: vec![if leq > 95.0 {
            judged(LeqFlags::OVER)
        } else {
            judged(LeqFlags::NONE)
        }],
        ..frame(LevelScale::DbSpl)
    };
    let level = |k: u32| 90.0 + 4.0 * (f64::from(k) * 0.05).sin() as f32;
    let mut h = LeqHistory::default();
    // Four hours, one point a second; a single loud second an hour before the end.
    let n = 4 * 3600;
    for k in 0..n {
        let v = if k == n - 3600 { 104.0 } else { level(k) };
        h.push(&c, &mk(v), f64::from(k));
    }
    let plot = Rect::new(40.0, 10.0, 600.0, 160.0);
    let th = Theme::dark();
    let s = history_strip(&c, &h, true, plot, &th, StripSpan::default());
    let l = &s.lines[0];
    assert!(l.points.len() <= 2 * 600 + 4, "{} points", l.points.len());
    // The peak is drawn, at its height and time.
    let peak = [s.x.mapping.to_px(-3600.0 / 60.0), s.y.mapping.to_px(104.0)];
    assert!(
        l.points
            .iter()
            .any(|p| (p[0] - peak[0]).abs() < 0.5 && (p[1] - peak[1]).abs() < 1e-3)
    );
    // Every over run still starts at its first over second and ends back under.
    assert!(
        l.over
            .iter()
            .any(|r| r.iter().any(|p| (p[1] - peak[1]).abs() < 1e-3))
    );
    for r in &l.over {
        assert!(r.len() >= 2);
    }
    // The same inputs reuse the strip; a new frame lays it out again.
    assert_eq!(
        history_strip(&c, &h, true, plot, &th, StripSpan::default()),
        s
    );
    h.push(&c, &mk(99.0), f64::from(n));
    let s2 = history_strip(&c, &h, true, plot, &th, StripSpan::default());
    assert_ne!(s2, s);
    assert_eq!(
        s2,
        lay_out_strip(&c, &h, true, plot, &th, StripSpan::default())
    );
    let wider = Rect::new(40.0, 10.0, 700.0, 160.0);
    assert_eq!(
        history_strip(&c, &h, true, wider, &th, StripSpan::default()),
        lay_out_strip(&c, &h, true, wider, &th, StripSpan::default())
    );
}
