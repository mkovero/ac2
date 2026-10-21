use super::*;

// ---------------------------------------------------------------------------------------
// Columns

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
        ..c2.windows[0].clone()
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

/// A filling window on course: amber, `ON COURSE` with the time until it spends its budget,
/// the value the Leq so far, the bar at the Leq it ends at if the rest is silent — under the
/// limit line until going over is certain, while a full window's bar is its Leq.
#[test]
fn on_course_while_filling() {
    let (c, f) = many(6, false, LevelScale::DbSpl);
    let t = leq_tiles(&c, &f);
    // The sixth window listed is the 1 min one, 15 s in at 102.4 dB, over in 30 s.
    let oc = &t[5];
    assert_eq!(oc.name, "LAeq 1 min");
    assert_eq!(oc.state, TileState::Near);
    assert!(oc.on_course && oc.filling());
    assert_eq!(oc.state_text.as_deref(), Some("ON COURSE"));
    assert_eq!(oc.course.as_deref(), Some("on course — over in 30 s"));
    assert_eq!(oc.value, "102.4");
    assert_eq!(oc.filling.as_deref(), Some("so far · 0:15 / 1:00"));
    // 15 s of 60 at 102.4 dB: 6.0 dB under it.
    assert!(
        (oc.bar_db() - (102.4 - 6.0206)).abs() < 1e-3,
        "{}",
        oc.bar_db()
    );
    // A near window that is not on course says NEAR, and no course.
    assert_eq!(t[1].state_text.as_deref(), Some("NEAR"));
    assert_eq!(t[1].course, None);
    assert_eq!(time_to(29.9), "29 s");
    assert_eq!(time_to(119.0), "119 s");
    assert_eq!(time_to(754.0), "12 min");
    assert_eq!(time_to(3900.0), "1 h 05 min");

    let th = Theme::dark();
    let s = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &th,
        size(1920.0, 1080.0),
    );
    let all = texts(&s.scene);
    // Six columns: the shorter wording of the state.
    for want in ["over in 30 s", "so far · 0:15 / 1:00", "102.4"] {
        assert!(all.contains(&want), "{want} in {all:?}");
    }
    let k = cols(&s);
    let col = k.columns.iter().find(|x| x.window == 5).expect("col");
    assert_eq!(col.bar_color, th.banner_warning.background);
    assert!(col.filling);
    assert!((col.bar_db - oc.bar_db()).abs() < 1e-9);
    let bar = col.bar.expect("a bar");
    let limit_y = col.limit_y.expect("a limit");
    assert!(
        bar.y > limit_y,
        "the bar under the limit line while not certain"
    );
    // Over (full): the bar is the Leq, above the line.
    let over = k.columns.iter().find(|x| x.window == 0).expect("col");
    assert_eq!(over.bar_db, 101.3);
    assert!(over.bar.expect("a bar").y < over.limit_y.expect("a limit"));
    // Tiles say it too.
    let v = LeqView {
        layout: LeqLayout {
            style: LeqStyle::Tiles,
            history: false,
        },
        ..columns_view(&c, &f, None)
    };
    let s = leq_scene(&v, &Status::default(), &th, size(1920.0, 1080.0));
    let all = texts(&s.scene);
    assert!(all.contains(&"ON COURSE"), "{all:?}");
    assert!(all.contains(&"on course — over in 30 s"), "{all:?}");
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
    for gone in ["next 1 min", "until full"] {
        assert!(!all.contains(&gone), "{gone} in {all:?}");
    }
    for want in [
        "FOH SPL · LAeq, dB SPL",
        "M30 · cal 3 h ago",
        "101.3",
        "OVER",
        "NEAR",
        "OK",
        "limit 99.0 dB SPL",
        // What to do: the level to stay under, alone (not what it holds for).
        "stay ≤ 101.5 dB SPL",
        // How long an over window cools down when it cannot recover within the horizon,
        // in the longest wording that fits.
        "cooling down in 7 min 30 s",
        // The 5 min window, 40 % elapsed, and the 1 min one just started: their values are
        // the Leq so far.
        "so far · 2:00 / 5:00",
        "so far · 0:00 / 1:00",
        "—",
        // One weighting: the caption names it once, the columns only their lengths.
        "15 min",
        // …and each column still says its unit and weighting under its value.
        "dB SPL (A)",
    ] {
        assert!(all.contains(&want), "{want} in {all:?}");
    }
    // The state and the instruction are the large text; the value is smaller, under the
    // window's name too, and grows with the column.
    let size_of = |s: &LeqScene, t: &str| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == t)
            .map(|l| l.size)
            .expect(t)
    };
    let k = cols(&s);
    let v = size_of(&s, "101.3");
    assert!(v > 15.0 && v < 30.0, "{v}");
    assert!(v < size_of(&s, "15 min"), "{v}");
    assert!((size_of(&s, "OVER") - k.large).abs() < 1e-3);
    // The instruction may shrink a little to keep its wording.
    let stay = size_of(&s, "stay ≤ 101.5 dB SPL");
    assert!(stay <= k.large && stay > v, "{stay}");
    assert!(v <= k.large * VALUE_RATIO + 1e-3, "{v} vs {}", k.large);
    let small = leq_scene(
        &columns_view(&c, &f, None),
        &Status::default(),
        &Theme::dark(),
        size(800.0, 450.0),
    );
    assert!(size_of(&small, "101.3") < v);
}

/// Each value holds still low in its track whatever the level — the same box at any level —
/// inside its column, under the window's name in size, never across the limit line, in a
/// colour that reads on what is behind it: inked for the fill (never red on red), the bar's
/// colour or plain text on the track, an ink for both across the fill's edge.
#[test]
fn values_hold_still_low_on_their_bars() {
    for th in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
        for scale in [LevelScale::DbSpl, LevelScale::Dbfs] {
            for n in 1..=8 {
                let (c, f) = many(n, false, scale);
                for (w, h) in [
                    (480.0, 320.0),
                    (960.0, 540.0),
                    (1920.0, 1080.0),
                    (3840.0, 2160.0),
                ] {
                    let at = format!("{:?} {scale:?} {n} at {w}x{h}", th.name);
                    let scene_at = |shift: f32| {
                        let mut g = f.clone();
                        for l in g.leq.iter_mut() {
                            *l += shift;
                        }
                        for l in g.least.iter_mut() {
                            *l += shift;
                        }
                        leq_scene(
                            &columns_view(&c, &g, None),
                            &Status::default(),
                            &th,
                            size(w, h),
                        )
                    };
                    let base = scene_at(0.0);
                    let k = cols(&base);
                    // Levels from the bottom of the scale to over the top: the box stays.
                    for shift in [-30.0, -12.0, -3.0, 4.0, 15.0] {
                        let other = scene_at(shift);
                        for (a, b) in k.columns.iter().zip(&cols(&other).columns) {
                            // The same place and size; only the width follows the digits.
                            let (ra, rb) = (a.value.rect, b.value.rect);
                            assert_eq!((ra.y, ra.h), (rb.y, rb.h), "{at} {shift:+}: {}", a.name);
                            let cx = |r: Rect| r.x + r.w / 2.0;
                            assert!((cx(ra) - cx(rb)).abs() < 1e-3, "{at} {shift:+}: {}", a.name);
                            assert_eq!(a.value.size, b.value.size, "{at} {shift:+}: {}", a.name);
                        }
                        check_values(&other, &at);
                    }
                    check_values(&base, &at);
                }
            }
        }
    }
}

fn check_values(s: &LeqScene, at: &str) {
    let k = cols(s);
    let name_size = |x: &LeqColumn| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == x.name && l.pos[1] > x.track.bottom())
            .map(|l| l.size)
    };
    for x in &k.columns {
        let v = x.value;
        let (r, t) = (v.rect, x.track);
        assert!(
            r.y >= t.y - 0.01 && r.bottom() <= t.bottom() + 0.01,
            "{at}: {} value {r:?} outside its track {t:?}",
            x.name
        );
        assert!(
            r.x >= x.rect.x - 0.01 && r.right() <= x.rect.right() + 0.01,
            "{at}: {} value {r:?} wider than its column",
            x.name
        );
        // Low: in the bottom part of a roomy track.
        if t.h > 6.0 * r.h {
            assert!(
                r.y > t.y + t.h * 0.5,
                "{at}: {} value {r:?} not low",
                x.name
            );
        }
        if let Some(y) = x.limit_y {
            assert!(
                r.bottom() < y - 1.0 || r.y > y + 1.0,
                "{at}: {} value {r:?} across the limit at {y}",
                x.name
            );
        }
        if let Some(ns) = name_size(x) {
            assert!(
                v.size < ns,
                "{at}: {} value {} not under its name {ns}",
                x.name,
                v.size
            );
        }
        assert!(v.size <= k.large * VALUE_RATIO + 1e-3, "{at}");
        let fill_top = x.bar.map_or(f32::INFINITY, |b| b.y);
        let ratio = |bg| crate::theme::contrast_ratio(v.color, bg);
        match v.behind {
            Behind::Fill => {
                assert!(fill_top <= r.y + 0.01, "{at}: {}", x.name);
                assert_ne!(v.color, x.bar_color, "{at}: {}", x.name);
                assert!(ratio(x.bar_color) >= 4.5, "{at}: {} on its fill", x.name);
            }
            Behind::Track => {
                assert!(fill_top >= r.bottom() - 0.01, "{at}: {}", x.name);
                if x.bar.is_some() {
                    assert!(ratio(x.track_color) >= 3.0, "{at}: {} on the track", x.name);
                }
            }
            Behind::Both => {
                assert!(fill_top > r.y && fill_top < r.bottom(), "{at}: {}", x.name);
                assert!(
                    ratio(x.bar_color) >= 3.0 && ratio(x.track_color) >= 3.0,
                    "{at}: {} across the fill's edge",
                    x.name
                );
            }
        }
    }
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
                    for history in [false, true] {
                        let v = LeqView {
                            layout: LeqLayout {
                                style: LeqStyle::Columns,
                                history,
                            },
                            ..columns_view(&c, &f, None)
                        };
                        let s = leq_scene(&v, &Status::default(), &th, size(w, h));
                        let k = cols(&s);
                        assert_eq!(k.columns.len(), n);
                        let at = format!(
                            "{n} windows ({mixed}, {scale:?}, history {history}) at {w}×{h}"
                        );
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
                        assert_caption_fits(&s, w, history, &at);
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
