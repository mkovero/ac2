use super::*;

#[test]
fn three_panes_share_the_x_axis() {
    let c = cols(100);
    let s = transfer_scene(
        &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
        &DisplayCache::default(),
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    );
    let kinds: Vec<_> = s.panes.iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        [
            TfPaneKind::Magnitude,
            TfPaneKind::Phase,
            TfPaneKind::Coherence
        ]
    );
    // Heights 3:2:1, no overlap.
    let h: Vec<f32> = s.panes.iter().map(|p| p.plot.h).collect();
    assert!((h[0] / h[2] - 3.0).abs() < 1e-4 && (h[1] / h[2] - 2.0).abs() < 1e-4);
    assert!(s.panes[1].plot.y >= s.panes[0].plot.bottom());
    assert_eq!(
        s.x_axis.labels(),
        [
            "20", "50", "100", "200", "500", "1k", "2k", "5k", "10k", "20k"
        ]
    );
    assert_eq!(
        s.panes[1].y_axis.labels(),
        ["−180", "−90", "0", "90", "180"]
    );
    assert_eq!(s.panes[2].title, "Coherence γ²");
    // One polyline per pane for the one trace; x labels only under the bottom pane.
    assert_eq!(s.scene.layers[1].polylines.len(), 3);
    let x_labels = s.scene.layers[0]
        .labels
        .iter()
        .filter(|l| l.text == "1k")
        .count();
    assert_eq!(x_labels, 1);
    assert!(s.banners.is_empty());
}

#[test]
fn invalid_columns_split_the_line() {
    let mut c = cols(100);
    c.validity[40] = ValidityMask::THINNED;
    c.validity[41] = ValidityMask::THINNED;
    c.mag[70] = f32::NAN;
    let s = transfer_scene(
        &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
        &DisplayCache::default(),
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    );
    let mag = &s.scene.layers[1].polylines[0];
    let segs = segments(&mag.points);
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[0].len(), 40);
    assert_eq!(segs[1].len(), 28);
    assert_eq!(segs[2].len(), 29);
    assert_eq!(mag.alpha.len(), mag.points.len());
    // Phase and coherence panes break at the same columns: a column without a valid
    // magnitude is invalid as a whole.
    assert_eq!(segments(&s.scene.layers[1].polylines[1].points).len(), 3);
    let coh = &s.scene.layers[1].polylines[2];
    assert_eq!(segments(&coh.points).len(), 3);
}

#[test]
fn wrapped_phase_breaks_at_the_wrap() {
    let mut c = cols(200);
    for (i, f) in c.freqs.iter().enumerate() {
        c.phase[i] = crate::trace::wrap_deg(-360.0 * f * 0.000_25) as f32;
    }
    let s = transfer_scene(
        &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
        &DisplayCache::default(),
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    );
    let phase = &s.scene.layers[1].polylines[1];
    let segs = segments(&phase.points);
    // 0.25 ms up to 17 kHz wraps four times; no segment jumps across the pane.
    assert!(segs.len() >= 4, "{}", segs.len());
    let plot = s.panes[1].plot;
    for seg in &segs {
        for w in seg.windows(2) {
            assert!((w[1][1] - w[0][1]).abs() < plot.h / 2.0);
        }
    }
}

#[test]
fn coherence_alpha_reaches_the_polyline() {
    let mut c = cols(50);
    c.coh[10] = 0.0;
    let s = transfer_scene(
        &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
        &DisplayCache::default(),
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    );
    let mag = &s.scene.layers[1].polylines[0];
    assert_eq!(mag.alpha[10], 0.15);
    assert_eq!(mag.alpha[11], 1.0);
    // The coherence trace itself is never faded.
    assert!(s.scene.layers[1].polylines[2].alpha.is_empty());
}

#[test]
fn smoothing_captions() {
    let s = |fraction, mode| Some(Smoothing { fraction, mode });
    assert_eq!(smoothing_caption(None), "smoothing off");
    assert_eq!(
        smoothing_caption(s(SmoothingFraction::Third, SmoothingMode::MagnitudePhase)),
        "smoothing 1/3 oct"
    );
    assert_eq!(
        smoothing_caption(s(
            SmoothingFraction::FortyEighth,
            SmoothingMode::MagnitudePhase
        )),
        "smoothing 1/48 oct"
    );
    assert_eq!(
        smoothing_caption(s(SmoothingFraction::Twelfth, SmoothingMode::Magnitude)),
        "smoothing 1/12 oct mag only"
    );
}

/// The selected stored trace is marked: a bar before its legend swatch, a thicker
/// swatch, and a line twice as wide in every pane; the others stay as they are.
#[test]
fn the_selected_trace_is_marked_in_legend_and_plot() {
    let a = cols(97);
    let ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
    let mut ts = trace(&a, TraceKey::Stored(TraceId(7)), 0.010);
    ts.selected = true;
    let theme = Theme::dark();
    let s = transfer_scene(
        &[ta, ts],
        &DisplayCache::default(),
        &Status::default(),
        &ViewState::default(),
        &theme,
        SIZE,
    );
    let sel: Vec<bool> = s.legend.iter().map(|e| e.selected).collect();
    assert_eq!(sel, [false, true]);
    assert_eq!(s.legend[1].text, "t7 · Δt 0.00 ms");
    let widths: Vec<f32> = s.scene.layers[1]
        .polylines
        .iter()
        .map(|p| p.stroke.width)
        .collect();
    // Magnitude, phase and coherence: the live line then the selected one, each pane.
    assert!(widths.len() >= 6, "{widths:?}");
    for pair in widths.chunks(2) {
        assert_eq!(
            pair,
            [theme.trace_width, theme.trace_width * SELECTED_WIDTH]
        );
    }
    let row = s.scene.layers[2]
        .labels
        .iter()
        .find(|l| l.text == "t7 · Δt 0.00 ms")
        .expect("legend row");
    // The two swatches (12 wide), the selected one thicker, and the bar on its row.
    let rects: Vec<Rect> = s.scene.layers[2].rects.iter().map(|r| r.rect).collect();
    let swatches: Vec<&Rect> = rects.iter().filter(|r| r.w == 12.0).collect();
    assert_eq!(swatches.len(), 2, "{rects:?}");
    assert_eq!(swatches[0].h, 3.0);
    assert_eq!(swatches[1].h, 3.0 * SELECTED_WIDTH);
    let bar = rects
        .iter()
        .find(|r| r.w == 2.0 && r.h == 10.0)
        .expect("bar");
    assert_eq!(bar.y + bar.h / 2.0, row.pos[1], "on the selected row");
    assert!(bar.right() < swatches[1].x);
    // All of it on the plate.
    let plate = s.legend_box.as_ref().expect("legend").rect;
    assert!(rects.contains(&plate));
    for r in [*swatches[0], *swatches[1], *bar] {
        assert!(
            r.x >= plate.x && r.right() <= plate.right(),
            "{r:?} in {plate:?}"
        );
    }
}

#[test]
fn legend_cursor_delay_and_stale() {
    let a = cols(97);
    let b = cols(97);
    let mut ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
    ta.name = "Main L".into();
    ta.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::MagnitudePhase,
    });
    let mut tb = trace(&b, TraceKey::Live(MeasId(2)), 0.0115);
    tb.name = "Delay tower".into();
    tb.polarity = Polarity::Inverted;
    tb.offset_db = 3.0;
    tb.freshness = Some(Freshness::from_age(3.24));
    let mut ti = trace(&b, TraceKey::Stored(TraceId(7)), 0.0);
    ti.name = "imported".into();
    ti.time_base = TimeBase::Independent;
    ti.freshness = None;
    ti.nudge = Seconds(0.00025);
    ti.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::TwentyFourth,
        mode: SmoothingMode::Magnitude,
    });
    let view = ViewState {
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let status = Status {
        frame_age_s: Some(3.24),
        ..Status::default()
    };
    let s = transfer_scene(
        &[ta, tb, ti],
        &DisplayCache::default(),
        &status,
        &view,
        &Theme::dark(),
        SIZE,
    );
    let texts: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Main L · ref · 1/6 oct",
            "Delay tower · Δt +1.50 ms · inv · +3.0 dB · STALE 3.2 s",
            "imported · indep. · +0.25 ms from arrival · 1/24 oct mag only"
        ]
    );
    assert_eq!(
        s.delay.as_deref(),
        Some("ref Main L 10.00 ms · 3.43 m @ 20 °C")
    );
    let cur = s.cursor.as_ref().expect("cursor");
    assert_eq!(cur.freq, "1.00 kHz");
    assert_eq!(cur.rows[0].magnitude, "0.0 dB");
    assert_eq!(cur.rows[1].magnitude, "+3.0 dB");
    // Inverted and 1.5 ms later: 180° − 540° ≡ 0° at exactly 1 kHz.
    assert_eq!(cur.rows[1].phase, "0°");
    // Stale trace dims.
    let stroke = &s.scene.layers[1].polylines[1].stroke;
    assert!((stroke.color.a - Theme::dark().stale_alpha).abs() < 1e-6);
    assert_eq!(s.banners[0].text, "STALE · 3.2 s");
    // Cursor line in every pane, with the curves: under the legend's plate.
    let cursor_lines = s.scene.layers[1]
        .polylines
        .iter()
        .filter(|p| p.stroke == Theme::dark().cursor)
        .count();
    assert_eq!(cursor_lines, 3);
    assert!(s.scene.layers[2].polylines.is_empty());
    // The drawn strings include the legend and the cursor values.
    let labels: Vec<&str> = s.scene.layers[2]
        .labels
        .iter()
        .map(|l| l.text.as_str())
        .collect();
    assert!(labels.contains(&"Main L · ref · 1/6 oct"));
    assert!(labels.contains(&"1.00 kHz"));
    assert!(labels.contains(&"+3.0 dB  0°  1.00"));
}

/// Full draws grid, tick labels and cursor; no grid keeps the labels and the cursor;
/// traces only drops all three. The curves are the same in every step.
#[test]
fn plot_chrome_steps_strip_grid_labels_and_cursor() {
    use crate::view::PlotChrome;
    let a = cols(97);
    let draw = |chrome: PlotChrome| {
        let view = ViewState {
            cursor_hz: Some(1000.0),
            chrome,
            ..ViewState::default()
        };
        transfer_scene(
            &[trace(&a, TraceKey::Live(MeasId(1)), 0.0)],
            &DisplayCache::default(),
            &Status::default(),
            &view,
            &Theme::dark(),
            SIZE,
        )
    };
    let grid_lines = |s: &TfScene| -> usize {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.grids)
            .map(|g| g.lines.len())
            .sum()
    };
    let tick_labels = |s: &TfScene| -> usize {
        let ticks: Vec<&str> = s
            .x_axis
            .ticks
            .iter()
            .filter_map(|t| t.label.as_deref())
            .collect();
        s.scene.layers[0]
            .labels
            .iter()
            .filter(|l| ticks.contains(&l.text.as_str()))
            .count()
    };
    let cursor_lines = |s: &TfScene| -> usize {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.polylines)
            .filter(|p| p.stroke == Theme::dark().cursor)
            .count()
    };
    let curves = |s: &TfScene| -> Vec<Vec<[f32; 2]>> {
        s.scene.layers[1]
            .polylines
            .iter()
            .filter(|p| p.stroke != Theme::dark().cursor)
            .map(|p| p.points.clone())
            .collect()
    };

    let full = draw(PlotChrome::Full);
    assert!(grid_lines(&full) > 0);
    assert!(tick_labels(&full) > 0);
    assert!(full.cursor.is_some());
    assert_eq!(cursor_lines(&full), 3);

    let no_grid = draw(PlotChrome::NoGrid);
    assert_eq!(grid_lines(&no_grid), 0);
    assert_eq!(tick_labels(&no_grid), tick_labels(&full));
    assert!(no_grid.cursor.is_some());
    assert_eq!(cursor_lines(&no_grid), 3);

    let bare = draw(PlotChrome::Bare);
    assert_eq!(grid_lines(&bare), 0);
    assert_eq!(tick_labels(&bare), 0);
    assert!(bare.cursor.is_none() && bare.readout_box.is_none());
    assert_eq!(cursor_lines(&bare), 0);

    assert!(!curves(&full).is_empty());
    assert_eq!(curves(&no_grid), curves(&full));
    assert_eq!(curves(&bare), curves(&full));
}

/// A stopped measurement's curve is its final result: tagged, drawn at full strength.
#[test]
fn stopped_trace_is_tagged_not_dimmed() {
    let c = cols(100);
    let mut t = trace(&c, TraceKey::Live(MeasId(1)), 0.0);
    t.name = "Main L".into();
    t.freshness = Some(Freshness::Stopped { age_s: 23.0 });
    let s = transfer_scene(
        &[t],
        &DisplayCache::default(),
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    );
    assert_eq!(s.legend[0].text, "Main L · ref · stopped");
    assert!(!s.legend[0].stale);
    let stroke = &s.scene.layers[1].polylines[0].stroke;
    assert_eq!(stroke.color.a, 1.0);
}

#[test]
fn group_delay_pane() {
    let mut c = cols(100);
    for (i, f) in c.freqs.iter().enumerate() {
        c.phase[i] = crate::trace::wrap_deg(-360.0 * f * 0.0002) as f32;
    }
    let view = ViewState {
        tf: crate::view::TfView {
            phase: PhaseView::GroupDelay {
                range_ms: Range::new(-2.0, 2.0),
            },
            show_coherence: false,
            ..Default::default()
        },
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let s = transfer_scene(
        &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
        &DisplayCache::default(),
        &Status::default(),
        &view,
        &Theme::dark(),
        SIZE,
    );
    assert_eq!(s.panes.len(), 2);
    assert_eq!(s.panes[1].title, "Group delay ms · 1/12 oct");
    assert_eq!(s.cursor.as_ref().expect("cursor").rows[0].phase, "0.20 ms");
    // Every phase-pane point sits at the 0.2 ms line.
    let y = s.panes[1].y_axis.mapping.to_px(0.2);
    let line = &s.scene.layers[1].polylines[1];
    assert!(line.points.iter().all(|p| (p[1] - y).abs() < 0.01));
}
