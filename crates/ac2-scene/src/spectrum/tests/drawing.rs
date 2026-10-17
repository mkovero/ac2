use super::*;

#[test]
fn bars_cover_band_edges() {
    let g = third_octaves();
    let (f, e) = (column_frequencies(&g), column_edges(&g));
    let level: Vec<f32> = vec![-40.0; f.len()];
    let t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    let s = spectrum_scene(
        &[t],
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    );
    assert_eq!(s.unit, "dBFS (band)");
    assert_eq!(s.caption, "1/3 oct · A-weighted");
    assert_eq!(s.y_axis.title, "dBFS (band)");
    // 20 Hz … 20 kHz shows 31 bands (the 20 Hz band is partly visible).
    let bars = &s.scene.layers[1].rects;
    assert_eq!(bars.len(), 31);
    // The 1 kHz bar spans its band edges and reaches −40 dB.
    let xm = s.x_axis.mapping;
    let k = f.iter().position(|x| *x == 1000.0).expect("1k");
    let bar = bars
        .iter()
        .find(|b| b.rect.x <= xm.to_px(1000.0) && b.rect.right() >= xm.to_px(1000.0))
        .expect("bar");
    assert!((bar.rect.x - (xm.to_px(e[k].0) + 0.5)).abs() < 1e-3);
    assert!((bar.rect.y - s.y_axis.mapping.to_px(-40.0)).abs() < 1e-3);
    assert!((bar.rect.bottom() - s.plot.bottom()).abs() < 1e-3);
}

#[test]
fn line_style_gaps_and_cursor() {
    let g = third_octaves();
    let (f, e) = (column_frequencies(&g), column_edges(&g));
    let mut level: Vec<f32> = vec![-40.0; f.len()];
    level[15] = f32::NAN;
    let mut validity = vec![ValidityMask::NONE; f.len()];
    validity[20] = ValidityMask::INSUFFICIENT_RESOLUTION;
    let mut t = rta_trace(&f, &e, &level, LevelScale::DbSpl);
    t.validity = Some(&validity);
    let view = ViewState {
        spectrum: SpectrumView {
            style: SpectrumStyle::Line,
            level: Range::new(-60.0, 0.0),
            peak_hold: false,
            ..SpectrumView::default()
        },
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
    let line = &s.scene.layers[1].polylines[0];
    assert_eq!(crate::canvas::tests::segments(&line.points).len(), 3);
    let cur = s.cursor.expect("cursor");
    assert_eq!(cur.freq, "1.00 kHz");
    assert_eq!(
        cur.rows,
        vec![("RTA".to_string(), "−40.0 dB SPL".to_string())]
    );
}

/// A gliding curve is drawn where its glide has got to, while the cursor reads the
/// newest frame itself.
#[test]
fn a_gliding_curve_is_drawn_between_frames_and_read_at_the_newest() {
    let g = third_octaves();
    let (f, e) = (column_frequencies(&g), column_edges(&g));
    let level = vec![-40.0f32; f.len()];
    let drawn = vec![-50.0f32; f.len()];
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.drawn = Some(&drawn);
    let view = ViewState {
        spectrum: SpectrumView {
            style: SpectrumStyle::Line,
            level: Range::new(-60.0, 0.0),
            peak_hold: false,
            ..SpectrumView::default()
        },
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
    let ym = s.y_axis.mapping;
    let line = &s.scene.layers[1].polylines[0];
    let y = line.points.iter().find(|p| p[1].is_finite()).map(|p| p[1]);
    assert_eq!(y, Some(ym.to_px(-50.0)));
    let cur = s.cursor.expect("cursor");
    assert_eq!(
        cur.rows,
        vec![("RTA".to_string(), "−40.0 dBFS".to_string())]
    );
}

/// A trace spread by a display offset is drawn that much higher, its peak hold too, and
/// the plot and the cursor name the offset; an unshifted trace adds no note.
#[test]
fn offset_traces_move_and_say_so() {
    let g = third_octaves();
    let (f, e) = (column_frequencies(&g), column_edges(&g));
    let level = vec![-40.0f32; f.len()];
    let peak = vec![-30.0f32; f.len()];
    let a = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    let mut b = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    b.name = "Main L S1".into();
    b.offset_db = 3.0;
    b.peak = Some(&peak);
    let view = ViewState {
        spectrum: SpectrumView {
            style: SpectrumStyle::Line,
            level: Range::new(-60.0, 0.0),
            peak_hold: true,
            ..SpectrumView::default()
        },
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let s = spectrum_scene(&[a, b], &Status::default(), &view, &Theme::dark(), SIZE);
    let texts: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, ["RTA", "Main L S1 · offset +3.0 dB"]);
    let ym = s.y_axis.mapping;
    let lines = &s.scene.layers[1].polylines;
    let y_of = |l: &Polyline| l.points.iter().find(|p| p[1].is_finite()).map(|p| p[1]);
    assert_eq!(y_of(&lines[0]), Some(ym.to_px(-40.0)));
    assert_eq!(y_of(&lines[1]), Some(ym.to_px(-37.0)));
    assert_eq!(
        y_of(&lines[2]),
        Some(ym.to_px(-27.0)),
        "peak hold moves too"
    );
    let cur = s.cursor.expect("cursor");
    assert_eq!(
        cur.rows,
        [
            ("RTA".to_string(), "−40.0 dBFS".to_string()),
            (
                "Main L S1 · offset +3.0 dB".to_string(),
                "−37.0 dBFS".to_string()
            ),
        ]
    );
    assert!(
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .any(|l| l.text == "Main L S1 · offset +3.0 dB")
    );
}

#[test]
fn peak_hold_decays_and_restarts() {
    let mut p = PeakHold::new(10.0);
    p.update(&[-20.0, -30.0], None, 0.0);
    assert_eq!(p.values(), [-20.0, -30.0]);
    p.update(&[-40.0, -25.0], None, 0.5);
    assert_eq!(p.values(), [-25.0, -25.0]);
    // Invalid column only decays.
    p.update(
        &[0.0, -60.0],
        Some(&[ValidityMask::SETTLING, ValidityMask::NONE]),
        0.1,
    );
    assert_eq!(p.values(), [-26.0, -26.0]);
    // New grid restarts.
    p.update(&[-50.0], None, 0.1);
    assert_eq!(p.values(), [-50.0]);
    let mut hold = PeakHold::new(0.0);
    hold.update(&[-10.0], None, 1.0);
    hold.update(&[-90.0], None, 100.0);
    assert_eq!(hold.values(), [-10.0]);
    hold.reset();
    assert!(hold.values().is_empty());
}

#[test]
fn peak_overlay_drawn_when_enabled() {
    let g = third_octaves();
    let (f, e) = (column_frequencies(&g), column_edges(&g));
    let level = vec![-40.0f32; f.len()];
    let peak = vec![-20.0f32; f.len()];
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.peak = Some(&peak);
    let mut view = ViewState::default();
    let s = spectrum_scene(
        &[t.clone()],
        &Status::default(),
        &view,
        &Theme::dark(),
        SIZE,
    );
    assert!(s.scene.layers[1].polylines.is_empty());
    view.spectrum.peak_hold = true;
    let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
    let caps = &s.scene.layers[1].polylines[0];
    assert_eq!(crate::canvas::tests::segments(&caps.points).len(), 31);
    let y = s.y_axis.mapping.to_px(-20.0);
    assert!(
        caps.points
            .iter()
            .filter(|p| p[1].is_finite())
            .all(|p| (p[1] - y).abs() < 1e-3)
    );
}

#[test]
fn banners_sit_above_the_plot() {
    let g = third_octaves();
    let (f, e) = (column_frequencies(&g), column_edges(&g));
    let level = vec![-40.0f32; f.len()];
    let view = ViewState {
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let build = |status: &Status| {
        spectrum_scene(
            &[rta_trace(&f, &e, &level, LevelScale::Dbfs)],
            status,
            &view,
            &Theme::dark(),
            SIZE,
        )
    };
    let calm = build(&Status::default());
    assert_eq!(calm.strip.h, 0.0);
    // The legend row sits between the strip and the plot.
    assert_eq!(calm.legend_rect.y, 4.0);
    assert_eq!(calm.plot.y, calm.legend_rect.bottom() - 4.0 + MARGINS.top);
    let s = build(&crate::banner::tests::everything());
    assert_eq!(s.banners.len(), crate::banner::MAX_BANNERS);
    assert_eq!(s.legend_rect.y, s.strip.bottom() + 4.0);
    assert_eq!(s.plot.y, s.legend_rect.bottom() - 4.0 + MARGINS.top);
    assert_eq!(s.plot.bottom(), calm.plot.bottom());
    assert!(s.cursor.is_some());
    crate::canvas::tests::assert_banners_clear(&s.scene, &s.banners, &[s.plot, s.legend_rect]);
}

/// Several live spectra and captures are told apart by a legend above the plot: name,
/// colour and tags in full while they fit, names alone when not, then the names that fit
/// and `+N more`. It never reaches into the plot or past the pane.
#[test]
fn legend_names_every_curve_and_fits_small_panes() {
    let g = third_octaves();
    let (f, e) = (column_frequencies(&g), column_edges(&g));
    let level = vec![-40.0f32; f.len()];
    let names = ["Main L", "Main R", "Sub array", "Front fill", "Delay tower"];
    let mut traces: Vec<SpectrumTrace<'_>> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
            t.key = TraceKey::Live(MeasId(i as u32 + 1));
            t.name = (*n).to_string();
            t.color = Color::rgb(0.1 * i as f32, 0.5, 0.5);
            t
        })
        .collect();
    traces[1].freshness = Some(Freshness::Stopped { age_s: 3.0 });
    traces[2].offset_db = -6.0;
    traces[3].selected = true;
    let build = |w: f32, h: f32| {
        spectrum_scene(
            &traces,
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            Viewport {
                width: w,
                height: h,
            },
        )
    };
    let texts = |s: &SpectrumScene| s.legend_shown.clone();
    let wide = build(1200.0, 500.0);
    assert_eq!(
        texts(&wide),
        [
            "Main L",
            "Main R · stopped",
            "Sub array · offset \u{2212}6.0 dB",
            "Front fill",
            "Delay tower"
        ]
    );
    assert_eq!(wide.legend.len(), 5);
    assert_eq!(wide.legend[1].tags, ["stopped"]);
    let selected: Vec<bool> = wide.legend.iter().map(|e| e.selected).collect();
    assert_eq!(selected, [false, false, false, true, false]);
    assert_eq!(wide.legend_rect.h, LEGEND_ROW);
    // A click on an entry's swatch or text names its curve.
    let hit = |s: &SpectrumScene, text: &str| {
        let l = s
            .scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == text)
            .expect("entry");
        let b = crate::canvas::tests::label_box(l);
        (
            crate::legend::hit_at(&s.legend_hits, b.x + b.w / 2.0, b.y + b.h / 2.0),
            crate::legend::hit_at(&s.legend_hits, b.x - SWATCH_GAP - 2.0, l.pos[1]),
        )
    };
    let front = Some(TraceKey::Live(MeasId(4)));
    assert_eq!(hit(&wide, "Front fill"), (front, front));
    assert_eq!(wide.legend_hits.len(), 5);
    // Narrow: rows wrap, then the tags go, then `+N more`.
    let narrow = build(420.0, 500.0);
    assert!(
        narrow.legend_rect.h > LEGEND_ROW,
        "{:?}",
        narrow.legend_rect
    );
    let short = build(300.0, 120.0);
    assert_eq!(texts(&short).first().map(String::as_str), Some("Main L"));
    assert!(
        texts(&short).last().is_some_and(|t| t.ends_with(" more")),
        "{:?}",
        texts(&short)
    );
    assert!(!texts(&short).iter().any(|t| t.contains('·')));
    // `+N more` is no curve's.
    assert_eq!(short.legend_hits.len(), texts(&short).len() - 1);
    let more = texts(&short).last().cloned().expect("more");
    assert_eq!(hit(&short, &more).0, None);
    let tiny = build(160.0, 120.0);
    assert!(tiny.legend_rect.h <= LEGEND_ROW);
    for (w, h) in [
        (1200.0, 500.0),
        (420.0, 500.0),
        (300.0, 200.0),
        (220.0, 160.0),
        (160.0, 120.0),
    ] {
        let s = build(w, h);
        let boxes: Vec<Rect> = s
            .scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .filter(|l| s.legend_shown.contains(&l.text))
            .map(crate::canvas::tests::label_box)
            .collect();
        assert_eq!(boxes.len(), s.legend_shown.len(), "{w}x{h}");
        for b in &boxes {
            assert!(
                !crate::canvas::tests::intersects(*b, s.plot),
                "{w}x{h}: {b:?} in the plot"
            );
            assert!(b.right() <= w + 0.5, "{w}x{h}: {b:?} past the pane");
        }
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                assert!(
                    !crate::canvas::tests::intersects(*a, *b),
                    "{w}x{h}: {a:?} {b:?}"
                );
            }
        }
    }
}

#[test]
fn line_thinning_keeps_peaks() {
    // 32768 linear bins up to 24 kHz on a ~740 px log axis: thousands of bins per
    // pixel at the top. Two single-bin tones and one sub-pixel invalid bin.
    let n = 32_768;
    let f: Vec<f64> = (0..n).map(|i| i as f64 * 24_000.0 / n as f64).collect();
    let e: Vec<(f64, f64)> = f.iter().map(|x| (*x, *x)).collect();
    let mut level = vec![-90.0f32; n];
    let (k1, k2) = (13_653, 25_000); // 10.0 kHz, 18.3 kHz
    level[k1] = -12.0;
    level[k2] = -20.0;
    let mut validity = vec![ValidityMask::NONE; n];
    validity[k1 + 1] = ValidityMask::SETTLING;
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.quantity = Quantity::Tone;
    t.validity = Some(&validity);
    t.peak = Some(&level);
    let view = ViewState {
        spectrum: SpectrumView {
            style: SpectrumStyle::Line,
            level: Range::new(-100.0, 0.0),
            peak_hold: true,
            ..SpectrumView::default()
        },
        ..ViewState::default()
    };
    let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
    let (xm, ym) = (s.x_axis.mapping, s.y_axis.mapping);
    for line in &s.scene.layers[1].polylines {
        // At most one point per pixel column, plus the off-plot neighbours.
        assert!(
            line.points.len() <= s.plot.w as usize + 3,
            "{}",
            line.points.len()
        );
        assert_eq!(crate::canvas::tests::segments(&line.points).len(), 1);
        for (k, v) in [(k1, -12.0), (k2, -20.0)] {
            let want = [xm.to_px(f[k]), ym.to_px(v)];
            assert!(line.points.contains(&want), "peak at {} Hz missing", f[k]);
        }
        // Everything else stays on the floor.
        let floor = ym.to_px(-90.0);
        let high = line.points.iter().filter(|p| p[1] < floor - 1e-3).count();
        assert_eq!(high, 2);
    }
}

#[test]
fn tone_traces_ignore_bar_style() {
    // 4096-point FFT at 48 kHz: a 1 kHz tone in one bin, ~0.5 px wide on this axis.
    let n = 2049;
    let f: Vec<f64> = (0..n).map(|i| i as f64 * 48_000.0 / 4096.0).collect();
    let e: Vec<(f64, f64)> = f.iter().map(|x| (x - 5.86, x + 5.86)).collect();
    let k = 85; // 996 Hz
    let mut level = vec![-80.0f32; n];
    level[k] = -30.0;
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.quantity = Quantity::Tone;
    let bars = ViewState::default();
    assert_eq!(bars.spectrum.style, SpectrumStyle::Bars);
    let s = spectrum_scene(&[t], &Status::default(), &bars, &Theme::dark(), SIZE);
    assert!(s.scene.layers[1].rects.is_empty());
    let line = &s.scene.layers[1].polylines[0];
    let want = [s.x_axis.mapping.to_px(f[k]), s.y_axis.mapping.to_px(-30.0)];
    assert!(line.points.contains(&want));
}

#[test]
fn max_per_pixel_groups_by_pixel() {
    let xs = [0.1, 0.5, 0.9, 1.2, 2.0, 2.5, 3.7];
    let ys = [5.0, 3.0, 4.0, f32::NAN, f32::NAN, 7.0, 1.0];
    let (x, y) = max_per_pixel(&xs, &ys);
    assert_eq!(x, [0.5, 1.2, 2.5, 3.7]);
    assert_eq!(y[0], 3.0);
    assert!(y[1].is_nan());
    assert_eq!(&y[2..], [7.0, 1.0]);
    // Sparse input passes through unchanged.
    let (x, y) = max_per_pixel(&[1.0, 5.0], &[2.0, 3.0]);
    assert_eq!((x, y), (vec![1.0, 5.0], vec![2.0, 3.0]));
}

/// A live multi-resolution spectrum: its caption and unit say so, a dashed line marks
/// each window change with the window's length, the columns below what its longest
/// window resolves are shaded, and the cursor names the windows behind its column.
#[test]
fn multires_spectrum_marks_where_its_windows_change() {
    let bands = crate::stitch::tests::bands_96k();
    let grid = GridDef::Log {
        ppo: 48,
        k_min: -7 * 48,
        k_max: 263,
    };
    let freqs = column_frequencies(&grid);
    let edges = column_edges(&grid);
    let spec = SpecFrame {
        meas: MeasId(3),
        meta: ac2_proto::frame::SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            level: SpecLevel::Tone,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            smoothing: None,
            math: None,
            bands: Some(Box::new(bands)),
        },
        level: freqs.iter().map(|_| -60.0).collect(),
    };
    let t = SpectrumTrace::spectrum(
        &spec,
        WallNs(0),
        None,
        &freqs,
        &edges,
        "Main L",
        Color::WHITE,
        Freshness::from_age(0.0),
    );
    assert_eq!(t.caption, "Hann window · multi-res 1/48 oct");
    let view = ViewState {
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let s = spectrum_scene(
        std::slice::from_ref(&t),
        &Status::default(),
        &view,
        &Theme::dark(),
        SIZE,
    );
    let windows: Vec<&str> = s.stitches.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(
        windows,
        ["683 ms", "341 ms", "171 ms", "85 ms", "43 ms", "21 ms"]
    );
    assert_eq!(s.unresolved.len(), 1);
    assert!(s.unit.starts_with("dBFS (tone, multi-res)"), "{}", s.unit);
    assert!(s.unit_help.as_deref().is_some_and(|h| h.contains("21 ms")));
    let cur = s.cursor.expect("cursor");
    assert!(cur.freq.ends_with(" · 171 + 85 ms windows"), "{}", cur.freq);
    let labels: Vec<&str> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect();
    for w in &windows {
        assert!(labels.contains(w), "{w} not drawn");
    }

    // T past the grid takes the window changes and the shading away with it.
    for chrome in [
        crate::view::PlotChrome::NoGrid,
        crate::view::PlotChrome::Bare,
    ] {
        let bare = ViewState { chrome, ..view };
        let s = spectrum_scene(
            std::slice::from_ref(&t),
            &Status::default(),
            &bare,
            &Theme::dark(),
            SIZE,
        );
        assert!(s.stitches.is_empty(), "{chrome:?}");
        assert!(s.unresolved.is_empty(), "{chrome:?}");
    }

    // Below the longest window's reach the cursor says the column repeats its bin.
    let low = ViewState {
        cursor_hz: Some(50.0),
        ..ViewState::default()
    };
    let s = spectrum_scene(
        std::slice::from_ref(&t),
        &Status::default(),
        &low,
        &Theme::dark(),
        SIZE,
    );
    let cur = s.cursor.expect("cursor");
    assert!(cur.freq.contains("683 ms window"), "{}", cur.freq);

    // The marker's toggle hides both.
    let off = ViewState {
        unresolved: false,
        ..ViewState::default()
    };
    let s = spectrum_scene(
        std::slice::from_ref(&t),
        &Status::default(),
        &off,
        &Theme::dark(),
        SIZE,
    );
    assert!(s.stitches.is_empty() && s.unresolved.is_empty());
}

/// The cursor stays at the frequency it was put at, as on the spectrograph beside the
/// spectrum, and reads the bin whose band holds it: 59.9 Hz on 5.86 Hz bins is bin 10
/// (55.7–61.5 Hz), not that bin's centre, nor the nearest centre's neighbour.
#[test]
fn cursor_stays_where_it_was_put_and_reads_the_bin_holding_it() {
    let bin = 48_000.0 / 8192.0;
    let f: Vec<f64> = (1..40).map(|k| f64::from(k) * bin).collect();
    let e: Vec<(f64, f64)> = f.iter().map(|c| (c - bin / 2.0, c + bin / 2.0)).collect();
    let level: Vec<f32> = (1..40).map(|k| -(k as f32)).collect();
    let t = rta_trace(&f, &e, &level, LevelScale::DbSpl);
    let view = ViewState {
        spectrum: SpectrumView {
            level: Range::new(-60.0, 0.0),
            ..SpectrumView::default()
        },
        cursor_hz: Some(59.9),
        ..ViewState::default()
    };
    let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
    let cur = s.cursor.expect("cursor");
    assert_eq!(cur.freq_hz, 59.9);
    assert_eq!(cur.freq, "59.9 Hz");
    assert_eq!(
        cur.rows,
        vec![("RTA".to_string(), "−10.0 dB SPL".to_string())]
    );
}
