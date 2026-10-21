use super::*;

fn overlay_view() -> ViewState {
    ViewState {
        tf: crate::view::TfView {
            coherence_placement: CoherencePlacement::OverlayOnMagnitude,
            ..Default::default()
        },
        ..ViewState::default()
    }
}

/// Two traces with a legend, delay line, cursor readout and every banner up.
fn busy(view: &ViewState, status: &Status) -> TfScene {
    let a = cols(97);
    let mut ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
    ta.name = "Main L".into();
    ta.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::MagnitudePhase,
    });
    let mut tb = trace(&a, TraceKey::Live(MeasId(2)), 0.0115);
    tb.name = "Delay tower".into();
    tb.offset_db = 3.0;
    let view = ViewState {
        cursor_hz: Some(1000.0),
        ..*view
    };
    transfer_scene(
        &[ta, tb],
        &DisplayCache::default(),
        status,
        &view,
        &Theme::dark(),
        SIZE,
    )
}

#[test]
fn banners_live_in_a_strip_above_the_panes() {
    use crate::banner::{BANNER_GAP, BANNER_HEIGHT, BANNER_PAD, MAX_BANNERS};
    use crate::canvas::tests::assert_banners_clear;
    for view in [ViewState::default(), overlay_view()] {
        let calm = busy(&view, &Status::default());
        assert!(calm.banners.is_empty());
        assert_eq!(calm.strip.h, 0.0);
        assert_eq!(calm.panes[0].plot.y, MARGINS.top);
        assert!(calm.scene.layers[3].rects.is_empty());

        let s = busy(&view, &crate::banner::tests::everything());
        assert_eq!(s.banners.len(), MAX_BANNERS);
        assert_eq!(s.banners[2].text, "+7 more");
        let strip_h = 2.0 * BANNER_PAD + 3.0 * BANNER_HEIGHT + 2.0 * BANNER_GAP;
        assert_eq!(s.strip, Rect::new(0.0, 0.0, SIZE.width, strip_h));
        // Panes start below the strip and shrink by its height, keeping their ratios.
        assert_eq!(s.panes[0].plot.y, strip_h + MARGINS.top);
        let total = |s: &TfScene| s.panes.iter().map(|p| p.plot.h).sum::<f32>();
        assert!((total(&calm) - total(&s) - strip_h).abs() < 1e-3);
        for (a, b) in calm.panes.iter().zip(&s.panes) {
            assert!((b.plot.h / a.plot.h - total(&s) / total(&calm)).abs() < 1e-4);
        }
        // Nothing drawn below the strip is covered: panes, the overlay band, legend,
        // delay line, cursor readout, axis labels and titles.
        assert!(!s.legend.is_empty() && s.cursor.is_some() && s.delay.is_some());
        let mut areas: Vec<Rect> = s.panes.iter().map(|p| p.plot).collect();
        areas.extend(s.coherence_overlay.as_ref().map(|o| o.band));
        assert_banners_clear(&s.scene, &s.banners, &areas);
        for r in &s.banners {
            assert!(r.rect.bottom() <= s.strip.bottom());
        }
    }
}

#[test]
fn coherence_overlay_layout() {
    let c = cols(100);
    let t = [trace(&c, TraceKey::Live(MeasId(1)), 0.0)];
    let pane = transfer_scene(
        &t,
        &DisplayCache::default(),
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    );
    assert!(pane.coherence_overlay.is_none());
    let s = transfer_scene(
        &t,
        &DisplayCache::default(),
        &Status::default(),
        &overlay_view(),
        &Theme::dark(),
        SIZE,
    );
    let kinds: Vec<_> = s.panes.iter().map(|p| p.kind).collect();
    assert_eq!(kinds, [TfPaneKind::Magnitude, TfPaneKind::Phase]);
    let (mag, phase) = (s.panes[0].plot, s.panes[1].plot);
    // Magnitude : phase = 3 : 2, filling the height the coherence pane had.
    assert!((mag.h / phase.h - 1.5).abs() < 1e-4);
    assert!((phase.bottom() - pane.panes[2].plot.bottom()).abs() < 1e-3);
    // Narrower plots: the right margin holds the overlay axis.
    assert_eq!(mag.right(), SIZE.width - OVERLAY_MARGIN_RIGHT);
    assert_eq!(s.x_axis.mapping.to_px(20_000.0), mag.right());
    // γ² 0…1 fills the top 30 % of the magnitude pane, inset from its top border so
    // γ² = 1 does not sit on the border line.
    let o = s.coherence_overlay.as_ref().expect("overlay");
    let top = mag.y + OVERLAY_INSET;
    let bottom = mag.y + mag.h * 0.3;
    assert_eq!(o.band, Rect::new(mag.x, top, mag.w, bottom - top));
    assert_eq!(o.axis.mapping.to_px(1.0), top);
    assert!((o.axis.mapping.to_px(0.0) - bottom).abs() < 1e-3);
    assert_eq!(o.axis.title, "γ²");
    assert_eq!(o.axis.labels(), ["0.0", "0.5", "1.0"]);
    // Axes as before: same ranges and the same steps although both panes grew; the
    // overlay labels sit in the right margin, outside the plot.
    for i in 0..2 {
        assert_eq!(
            s.panes[i].y_axis.mapping.range,
            pane.panes[i].y_axis.mapping.range
        );
        assert_eq!(s.panes[i].y_axis.labels(), pane.panes[i].y_axis.labels());
    }
    assert_eq!(
        s.panes[1].y_axis.labels(),
        ["−180", "−90", "0", "90", "180"]
    );
    assert_eq!(s.panes[0].title, "Magnitude dB");
    let right: Vec<&str> = s.scene.layers[0]
        .labels
        .iter()
        .filter(|l| l.pos[0] > mag.right())
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(right, ["0.0", "0.5", "1.0", "γ²"]);
    // Polylines: magnitude, overlaid coherence, phase.
    let lines = &s.scene.layers[1].polylines;
    assert_eq!(lines.len(), 3);
    let coh = &lines[1];
    assert!(coh.points.iter().all(|p| (p[1] - top).abs() < 1e-3));
    assert_eq!(coh.clip, Some(mag));
    assert!(coh.alpha.is_empty());
    let a = Theme::dark().trace_color(0).a * OVERLAY_ALPHA;
    assert!((coh.stroke.color.a - a).abs() < 1e-6);
}

#[test]
fn overlay_keeps_alpha_and_blanking() {
    let mut c = cols(60);
    for i in 10..14 {
        c.coh[i] = 0.2;
    }
    c.coh[30] = 0.7;
    let blank = |placement| ViewState {
        tf: crate::view::TfView {
            coherence: crate::view::CoherenceStyle {
                blank_below: Some(0.5),
                ..Default::default()
            },
            coherence_placement: placement,
            ..Default::default()
        },
        ..ViewState::default()
    };
    let t = [trace(&c, TraceKey::Live(MeasId(1)), 0.0)];
    let build = |v: &ViewState| {
        transfer_scene(
            &t,
            &DisplayCache::default(),
            &Status::default(),
            v,
            &Theme::dark(),
            SIZE,
        )
        .scene
        .layers[1]
            .polylines
            .clone()
    };
    let pane = build(&blank(CoherencePlacement::Pane));
    let over = build(&blank(CoherencePlacement::OverlayOnMagnitude));
    // Magnitude: same gap and the same per-point opacity in both layouts.
    assert_eq!(segments(&pane[0].points).len(), 2);
    assert_eq!(pane[0].alpha, over[0].alpha);
    assert!(pane[0].alpha.iter().any(|a| *a < 1.0));
    // Coherence is never blanked or faded, in its pane or overlaid.
    for coh in [&pane[2], &over[1]] {
        assert_eq!(segments(&coh.points).len(), 1);
        assert_eq!(coh.points.len(), 60);
        assert!(coh.alpha.is_empty());
    }
    // Phase blanks like magnitude.
    assert_eq!(segments(&over[2].points).len(), 2);
}

#[test]
fn overlay_without_magnitude_keeps_the_pane() {
    let c = cols(20);
    let mut view = overlay_view();
    view.tf.show_magnitude = false;
    let s = transfer_scene(
        &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
        &DisplayCache::default(),
        &Status::default(),
        &view,
        &Theme::dark(),
        SIZE,
    );
    let kinds: Vec<_> = s.panes.iter().map(|p| p.kind).collect();
    assert_eq!(kinds, [TfPaneKind::Phase, TfPaneKind::Coherence]);
    assert!(s.coherence_overlay.is_none());
    // Hidden coherence is hidden in both placements.
    let mut view = overlay_view();
    view.tf.show_coherence = false;
    let s = transfer_scene(
        &[trace(&c, TraceKey::Live(MeasId(1)), 0.0)],
        &DisplayCache::default(),
        &Status::default(),
        &view,
        &Theme::dark(),
        SIZE,
    );
    assert!(s.coherence_overlay.is_none());
    assert_eq!(s.scene.layers[1].polylines.len(), 2);
}

/// The magnitude alone: one plot over the pane's full height, the magnitude curve and
/// its level axis, no phase or coherence.
#[test]
fn magnitude_alone_fills_the_pane() {
    let c = cols(20);
    let t = [trace(&c, TraceKey::Live(MeasId(1)), 0.0)];
    let build = |v: &ViewState| {
        transfer_scene(
            &t,
            &DisplayCache::default(),
            &Status::default(),
            v,
            &Theme::dark(),
            SIZE,
        )
    };
    let mut view = ViewState::default();
    let all = build(&view);
    (view.tf.show_phase, view.tf.show_coherence) = (false, false);
    let s = build(&view);
    let kinds: Vec<_> = s.panes.iter().map(|p| p.kind).collect();
    assert_eq!(kinds, [TfPaneKind::Magnitude]);
    assert!(s.coherence_overlay.is_none());
    let titles: Vec<_> = s.panes.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, [all.panes[0].title.as_str()]);
    // From the stack's top to the stack's bottom: the whole height.
    let (one, stacked) = (s.panes[0].plot, &all.panes);
    let bottom = stacked[stacked.len() - 1].plot;
    assert_eq!(one.y, stacked[0].plot.y);
    assert!((one.y + one.h - (bottom.y + bottom.h)).abs() < 1e-3);
    // The magnitude curve, drawn as in the stack.
    let curves = &s.scene.layers[1].polylines;
    assert_eq!(curves.len(), 1);
    assert_eq!(
        curves[0].points.len(),
        all.scene.layers[1].polylines[0].points.len()
    );
}

#[test]
fn overlay_text_sits_below_the_band() {
    use crate::canvas::tests::{intersects, label_box};
    let s = busy(&overlay_view(), &Status::default());
    let o = s.coherence_overlay.as_ref().expect("overlay");
    // Title, legend, delay line and cursor values all start below the band.
    let texts: Vec<&str> = s
        .legend
        .iter()
        .map(|e| e.text.as_str())
        .chain(s.delay.as_deref())
        .chain(["Magnitude dB", "1.00 kHz"])
        .collect();
    let labels: Vec<&crate::primitives::Label> = s.scene.layers[..3]
        .iter()
        .flat_map(|l| &l.labels)
        .filter(|l| texts.contains(&l.text.as_str()) || l.text.contains("dB  "))
        .collect();
    assert!(labels.len() >= texts.len() + 2, "{labels:?}");
    for l in labels {
        assert!(
            !intersects(label_box(l), o.band),
            "{:?} over the coherence band",
            l.text
        );
    }
    // Pane mode keeps the block at the top of the pane.
    let p = busy(&ViewState::default(), &Status::default());
    let title = |s: &TfScene| {
        s.scene.layers[0]
            .labels
            .iter()
            .find(|l| l.text == "Magnitude dB")
            .map(|l| l.pos[1])
    };
    assert_eq!(title(&p), Some(p.panes[0].plot.y + 4.0));
    assert_eq!(title(&s), Some(o.band.bottom() + 4.0));
}

#[test]
fn narrow_panes_move_cursor_values_below_the_legend() {
    use crate::canvas::tests::{intersects, label_box};
    let a = cols(97);
    let mut ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
    ta.name = "Main left hang".into();
    let mut tb = trace(&a, TraceKey::Live(MeasId(2)), 0.0115);
    tb.name = "Delay tower stage right".into();
    let view = ViewState {
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let build = |w: f32| {
        let size = Viewport {
            width: w,
            height: 600.0,
        };
        transfer_scene(
            &[ta.clone(), tb.clone()],
            &DisplayCache::default(),
            &Status::default(),
            &view,
            &Theme::dark(),
            size,
        )
    };
    let row_y = |s: &TfScene, needle: &str| {
        s.scene.layers[2]
            .labels
            .iter()
            .find(|l| l.text.starts_with(needle))
            .map(|l| l.pos[1])
            .expect(needle)
    };
    // Wide: values share the legend rows, on a plate of their own at the right.
    let wide = build(1400.0);
    assert_eq!(row_y(&wide, "0.0 dB"), row_y(&wide, "Main left hang"));
    let (legend, values) = (
        wide.legend_box.as_ref().expect("legend").rect,
        wide.readout_box.expect("values").rect,
    );
    assert!(values.x > legend.right());
    // Narrow: the plates would meet, so the values go below the legend (its rows and
    // delay line); nothing overlaps.
    let narrow = build(360.0);
    let legend = narrow.legend_box.as_ref().expect("legend").rect;
    let values = narrow.readout_box.expect("values").rect;
    assert!(values.y >= legend.bottom(), "{values:?} below {legend:?}");
    assert!(row_y(&narrow, "0.0 dB") > row_y(&narrow, "ref Main left hang"));
    let labels: Vec<&crate::primitives::Label> = narrow.scene.layers[2].labels.iter().collect();
    for (i, a) in labels.iter().enumerate() {
        for b in &labels[i + 1..] {
            assert!(
                !intersects(label_box(a), label_box(b)),
                "{:?} overlaps {:?}",
                a.text,
                b.text
            );
        }
    }
}

/// Eighteen curves: the legend stops at 70 % of the magnitude pane and scrolls, its rows on
/// a translucent plate in the plot's colour, the rest counted; the cursor values follow
/// the shown rows. The pointer on it makes the plate opaque.
#[test]
fn many_curves_scroll_on_a_plate() {
    let a = cols(97);
    let ts: Vec<TfTrace<'_>> = (0..18)
        .map(|i| {
            let mut t = trace(&a, TraceKey::Stored(TraceId(i)), 0.010);
            t.name = format!("xc-xone-tf-cap {i}");
            t
        })
        .collect();
    let theme = Theme::light();
    let mut view = ViewState {
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let build = |view: &ViewState| {
        transfer_scene(
            &ts,
            &DisplayCache::default(),
            &Status::default(),
            view,
            &theme,
            SIZE,
        )
    };
    let s = build(&view);
    let b = s.legend_box.clone().expect("legend");
    let mag = s.panes[0].plot;
    assert!(b.rect.h <= 0.7 * mag.h, "{:?} in {mag:?}", b.rect);
    assert!(b.shown > 3 && b.shown < 18, "{}", b.shown);
    assert_eq!(b.more, Some(format!("{} below", 18 - b.shown)));
    let overlay = &s.scene.layers[2];
    let plate = overlay
        .rects
        .iter()
        .find(|r| r.rect == b.rect)
        .expect("plate");
    assert_eq!(plate.color, theme.plot_background.with_alpha(0.88));
    let on_plate = |y: f32| y > b.rect.y && y < b.rect.bottom();
    for (text, y) in &b.rows {
        let l = overlay
            .labels
            .iter()
            .find(|l| &l.text == text)
            .expect("row");
        assert!(on_plate(l.pos[1]) && l.pos[1] == *y);
        assert_eq!(l.color, theme.text);
    }
    // The values of the shown rows only, and the frequency.
    let values = s.readout_box.expect("values").rect;
    let n = overlay
        .labels
        .iter()
        .filter(|l| l.pos[1] > values.y && l.pos[1] < values.bottom() && l.pos[0] > values.x)
        .count();
    assert_eq!(n, b.shown + 1);
    // Scrolled to the end, the pointer on it.
    view.tf.legend.first = b.scrolled(100);
    view.plate_hover = Some(crate::plate::PlateHover {
        kind: crate::plate::PlateKind::Legend,
        grip: false,
    });
    let s = build(&view);
    let b = s.legend_box.clone().expect("legend");
    assert_eq!(b.first + b.shown, 18);
    assert_eq!(
        b.rows.last().map(|r| r.0.as_str()),
        Some("xc-xone-tf-cap 17 · Δt 0.00 ms")
    );
    assert_eq!(b.more, Some(format!("{} above", 18 - b.shown)));
    // A click on a shown row names that row's curve, scrolled as drawn; the note of
    // what is scrolled out names none.
    assert_eq!(s.legend_hits.len(), b.shown);
    let (_, last_y) = *b.rows.last().expect("row");
    let x = b.rect.x + b.rect.w / 2.0;
    assert_eq!(
        crate::legend::hit_at(&s.legend_hits, x, last_y),
        Some(s.legend[17].key)
    );
    assert_eq!(
        crate::legend::hit_at(&s.legend_hits, x, b.rows[0].1),
        Some(s.legend[b.first].key)
    );
    assert_eq!(
        crate::legend::hit_at(&s.legend_hits, x, b.rect.y + 1.0),
        None
    );
    let plate = s.scene.layers[2]
        .rects
        .iter()
        .find(|r| r.rect == b.rect)
        .expect("plate");
    assert_eq!(plate.color, theme.plot_background);
}

/// Snapped to the bottom-right corner, the legend sits there and the values move to the
/// left; hidden, no plate and each value names its trace.
#[test]
fn legend_corners_and_hidden() {
    let a = cols(97);
    let ta = trace(&a, TraceKey::Live(MeasId(1)), 0.010);
    let tb = trace(&a, TraceKey::Live(MeasId(2)), 0.0115);
    let mut view = ViewState {
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    view.plates.legend.at = Some(crate::legend::LegendCorner::BottomRight.position());
    let build = |view: &ViewState| {
        transfer_scene(
            &[ta.clone(), tb.clone()],
            &DisplayCache::default(),
            &Status::default(),
            view,
            &Theme::dark(),
            SIZE,
        )
    };
    let s = build(&view);
    let mag = s.panes[0].plot;
    let b = s.legend_box.as_ref().expect("legend");
    assert_eq!(b.rect.right(), mag.right() - crate::legend::INSET);
    assert_eq!(b.rect.bottom(), mag.bottom() - crate::legend::INSET);
    let values = s.readout_box.expect("values").rect;
    assert_eq!(values.x, mag.x + crate::legend::INSET);
    view.tf.legend.hidden = true;
    let s = build(&view);
    assert_eq!(s.legend_box, None);
    let labels: Vec<&str> = s.scene.layers[2]
        .labels
        .iter()
        .map(|l| l.text.as_str())
        .collect();
    assert!(
        labels.iter().any(|l| l.starts_with("m2  0.0 dB  ")),
        "{labels:?}"
    );
    assert!(!labels.iter().any(|l| l.starts_with("m1 · ")), "{labels:?}");
}

/// The marker: one curve's unresolved ranges, shaded in every pane with dashed edges,
/// named in the scene and on the cursor's frequency line; off by the view switch.
#[test]
fn unresolved_ranges_are_shaded_and_named() {
    use ac2_proto::model::{FreqRange, Resolution, Unresolved};
    use ac2_proto::units::Hz;
    let c = cols(100);
    let un = Unresolved {
        resolution: Resolution::NinetySixth,
        ranges: [(50.0, 135.0), (1625.0, 3250.0)]
            .iter()
            .map(|&(lo, hi)| FreqRange {
                lo: Hz(lo),
                hi: Hz(hi),
            })
            .collect(),
    };
    let mut t = trace(&c, TraceKey::Live(MeasId(1)), 0.0);
    t.unresolved = Some(crate::unresolved::Source {
        unresolved: &un,
        fill: crate::unresolved::Fill::Gaps,
    });
    let theme = Theme::dark();
    let view = ViewState {
        cursor_hz: Some(2000.0),
        ..ViewState::default()
    };
    let s = transfer_scene(
        std::slice::from_ref(&t),
        &DisplayCache::default(),
        &Status::default(),
        &view,
        &theme,
        SIZE,
    );
    let texts: Vec<&str> = s.unresolved.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "below 140 Hz: coarser than 1/96 oct (gaps)",
            "1.6 kHz–3.3 kHz: coarser than 1/96 oct (gaps)",
        ]
    );
    let cur = s.cursor.as_ref().expect("cursor");
    assert!(
        cur.freq.ends_with(" · coarser than 1/96 oct (gaps)"),
        "{}",
        cur.freq
    );
    let shades = s.scene.layers[0]
        .rects
        .iter()
        .filter(|r| r.color == theme.unresolved_shade)
        .count();
    assert_eq!(shades, 2 * s.panes.len());
    // The lowest range has only its upper edge; the other both.
    let edges = s.scene.layers[0]
        .polylines
        .iter()
        .filter(|p| p.stroke == theme.unresolved_edge)
        .count();
    assert_eq!(edges, 3 * s.panes.len());
    assert!(theme.unresolved_edge.dash.is_some());

    // At 1 kHz the cursor line says nothing more; off, nothing is marked.
    let at_1k = transfer_scene(
        std::slice::from_ref(&t),
        &DisplayCache::default(),
        &Status::default(),
        &ViewState {
            cursor_hz: Some(1000.0),
            ..ViewState::default()
        },
        &theme,
        SIZE,
    );
    assert!(!at_1k.cursor.expect("cursor").freq.contains("coarser"));
    let off = transfer_scene(
        std::slice::from_ref(&t),
        &DisplayCache::default(),
        &Status::default(),
        &ViewState {
            unresolved: false,
            ..view
        },
        &theme,
        SIZE,
    );
    assert!(off.unresolved.is_empty());
    assert!(!off.cursor.expect("cursor").freq.contains("coarser"));
    assert!(
        !off.scene.layers[0]
            .rects
            .iter()
            .any(|r| r.color == theme.unresolved_shade)
    );
}
