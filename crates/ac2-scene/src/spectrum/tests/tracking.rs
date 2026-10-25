use super::*;
use crate::canvas::tests::{intersects, label_box};
use crate::plate::PlateKind;
use crate::spectrum::peaks::{Peak, PeakInput, PeakTracker, find_peaks};

/// An RTA with peaks at 100 Hz, 1 kHz and 4 kHz, tracked as the UI does.
fn tracked_rta(f: &[f64], level: &[f32]) -> Vec<Peak> {
    let mut t = PeakTracker::default();
    t.update(&find_peaks(&PeakInput {
        freqs: f,
        level,
        validity: None,
        unresolved: None,
        narrow: false,
    }));
    t.peaks().to_vec()
}

fn levels(f: &[f64]) -> Vec<f32> {
    f.iter()
        .map(|hz| match *hz {
            x if (x - 1000.0).abs() < 1.0 => -32.0,
            x if (x - 100.0).abs() < 0.1 => -40.0,
            x if (x - 3981.07).abs() < 1.0 => -50.0,
            _ => -70.0,
        })
        .collect()
}

fn view(chrome: PlotChrome) -> ViewState {
    ViewState {
        spectrum: SpectrumView {
            level: Range::new(-100.0, 0.0),
            ..SpectrumView::default()
        },
        cursor_hz: Some(1000.0),
        chrome,
        ..ViewState::default()
    }
}

#[test]
fn tracked_peaks_are_listed_under_the_cursor_and_marked() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let peaks = tracked_rta(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let s = spectrum_scene(
        &[t],
        &Status::default(),
        &view(PlotChrome::Full),
        &Theme::dark(),
        SIZE,
    );
    assert_eq!(
        s.peak_list.as_deref(),
        Some(
            "1  1.00 kHz  −32.0 dBFS (band)\n2  100 Hz  −40.0 dBFS (band)\n3  3.98 kHz  −50.0 dBFS (band)"
        )
    );
    assert_eq!(s.peaks.len(), 1);
    let rows = &s.peaks[0].rows;
    assert_eq!(s.peaks[0].name, "RTA");
    // Each mark is centred on its point on the curve.
    let (xm, ym) = (s.x_axis.mapping, s.y_axis.mapping);
    for r in rows {
        let m = r.mark.expect("on the plot");
        assert!((m.x + m.w / 2.0 - xm.to_px(r.freq_hz)).abs() < 1e-3);
        assert!((m.y + m.h / 2.0 - ym.to_px(r.level)).abs() < 1e-3);
    }
    // The list is a plate of its own under the cursor readout, never over it, a row per
    // line.
    let labels = &s.scene.layers[2].labels;
    let list = labels
        .iter()
        .find(|l| l.text == "1  1.00 kHz  −32.0 dBFS (band)")
        .expect("list drawn");
    let cursor = s.readout_box.expect("cursor readout drawn").rect;
    let plate = s.peaks_box.expect("the peak list's plate");
    assert_eq!(plate.kind, PlateKind::Peaks);
    assert!(!intersects(label_box(list), cursor));
    assert!(plate.rect.y > cursor.bottom());
    assert!(intersects(label_box(list), plate.rect));
    assert_eq!(plate.rect.right(), s.plot.right() - crate::legend::INSET);
    assert_eq!(
        s.plates().iter().map(|b| b.kind).collect::<Vec<_>>(),
        [PlateKind::Readout, PlateKind::Peaks]
    );
    // Rank numbers beside the marks.
    for k in ["1", "2", "3"] {
        assert!(labels.iter().any(|l| l.text == k), "rank {k}");
    }
}

#[test]
fn plot_chrome_off_leaves_only_the_marks() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let peaks = tracked_rta(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let s = spectrum_scene(
        &[t],
        &Status::default(),
        &view(PlotChrome::Bare),
        &Theme::dark(),
        SIZE,
    );
    assert_eq!(s.peak_list, None);
    assert_eq!(s.peaks_box, None);
    assert!(s.plates().is_empty());
    assert!(s.cursor.is_none());
    assert_eq!(s.peaks[0].rows.len(), 3);
    let overlay = &s.scene.layers[2];
    // Three closed boxes, no rank numbers and no list.
    let boxes = overlay
        .polylines
        .iter()
        .filter(|p| p.points.len() == 5)
        .count();
    assert_eq!(boxes, 3);
    assert!(
        overlay
            .labels
            .iter()
            .all(|l| !l.text.contains("kHz") && l.text != "1"),
        "{:?}",
        overlay.labels
    );
}

#[test]
fn an_untracked_curve_lists_nothing() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    let v = view(PlotChrome::Full);
    let s = spectrum_scene(&[t], &Status::default(), &v, &Theme::dark(), SIZE);
    assert!(s.peaks.is_empty());
    assert_eq!(s.peak_list, None);
}

#[test]
fn an_offset_curve_lists_its_displayed_levels_under_its_name() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let peaks = tracked_rta(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    t.offset_db = 6.0;
    let mut v = view(PlotChrome::Full);
    v.cursor_hz = None;
    let s = spectrum_scene(&[t], &Status::default(), &v, &Theme::dark(), SIZE);
    let list = s.peak_list.expect("list");
    assert!(
        list.starts_with("RTA · offset +6.0 dB\n1  1.00 kHz  −26.0 dBFS (band)"),
        "{list}"
    );
}

/// The cursor values dragged to the plot's bottom-left at twice the text size: their plate
/// sits there, and the peak list takes the top of the plot's right side they left.
#[test]
fn dragged_cursor_values_leave_the_list_the_top() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let peaks = tracked_rta(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let mut v = view(PlotChrome::Full);
    v.plates.readout.at = Some([0.0, 1.0]);
    v.plates.readout.scale = 2.0;
    let s = spectrum_scene(&[t], &Status::default(), &v, &Theme::dark(), SIZE);
    let b = s.readout_box.expect("cursor values");
    assert_eq!(b.area, s.plot);
    assert_eq!(b.scale, 2.0);
    assert_eq!(b.rect.x, s.plot.x + crate::legend::INSET);
    assert_eq!(b.rect.bottom(), s.plot.bottom() - crate::legend::INSET);
    let labels = &s.scene.layers[2].labels;
    let freq = labels
        .iter()
        .find(|l| l.text == "1.00 kHz")
        .expect("frequency");
    assert_eq!(freq.size, 2.0 * Theme::dark().small_font_size);
    let peaks = s.peaks_box.expect("the peak list's plate");
    let m = crate::plate::Metrics::new(&Theme::dark(), 1.0);
    assert_eq!(peaks.rect.y + m.pad_y, s.plot.y + 22.0);
    assert_eq!(s.plates(), vec![b, peaks]);
}

/// The peak list dragged to the bottom-left at twice the text size: its plate sits there
/// and its text is larger, while the boxes stay on the curve where they were.
#[test]
fn a_dragged_peak_list_moves_and_scales_but_the_marks_stay() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let peaks = tracked_rta(&f, &level);
    let scene = |v: &ViewState| {
        let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        t.tracked = Some(&peaks);
        spectrum_scene(&[t], &Status::default(), v, &Theme::dark(), SIZE)
    };
    let auto = scene(&view(PlotChrome::Full));
    let mut v = view(PlotChrome::Full);
    v.plates.peaks.move_to(0.0, 1.0);
    v.plates.peaks.set_scale(2.0);
    let s = scene(&v);
    let b = s.peaks_box.expect("the peak list's plate");
    assert_eq!(b.scale, 2.0);
    assert_eq!(b.rect.x, s.plot.x + crate::legend::INSET);
    assert_eq!(b.rect.bottom(), s.plot.bottom() - crate::legend::INSET);
    let row = s.scene.layers[2]
        .labels
        .iter()
        .find(|l| l.text == "1  1.00 kHz  −32.0 dBFS (band)")
        .expect("list drawn");
    assert_eq!(row.size, 2.0 * Theme::dark().small_font_size);
    let marks = |s: &SpectrumScene| -> Vec<_> { s.peaks[0].rows.iter().map(|r| r.mark).collect() };
    assert_eq!(marks(&s), marks(&auto));
    // The cursor values stay where the scene puts them.
    assert_eq!(s.readout_box, auto.readout_box);
}

/// Placed by the scene, the list keeps clear of cursor values dragged onto its place: it
/// takes the bottom-right corner.
#[test]
fn the_list_leaves_its_place_to_cursor_values_dropped_there() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let peaks = tracked_rta(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let mut v = view(PlotChrome::Full);
    v.plates.readout.move_to(1.0, 0.0);
    let s = spectrum_scene(&[t], &Status::default(), &v, &Theme::dark(), SIZE);
    let r = s.readout_box.expect("cursor values").rect;
    let p = s.peaks_box.expect("the peak list's plate").rect;
    assert!(!intersects(p, r), "{p:?} {r:?}");
    assert_eq!(p.bottom(), s.plot.bottom() - crate::legend::INSET);
    assert_eq!(p.right(), s.plot.right() - crate::legend::INSET);
}

/// A plate's fill off hover is the theme's plate opacity: see-through by default, the
/// plot's background at full opacity when the plates are solid.
#[test]
fn the_peak_plate_is_filled_by_the_plate_background() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let peaks = tracked_rta(&f, &level);
    let fill = |bg: crate::plate::PlateBackground| {
        let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        t.tracked = Some(&peaks);
        let mut theme = Theme::dark();
        theme.plate_alpha = bg.alpha();
        let s = spectrum_scene(
            &[t],
            &Status::default(),
            &view(PlotChrome::Full),
            &theme,
            SIZE,
        );
        let rect = s.peaks_box.expect("plate").rect;
        s.scene.layers[2]
            .rects
            .iter()
            .find(|r| r.rect == rect)
            .expect("the plate's fill")
            .color
    };
    let bg = Theme::dark().plot_background;
    assert_eq!(
        fill(crate::plate::PlateBackground::SeeThrough),
        bg.with_alpha(crate::plate::SEE_THROUGH_ALPHA)
    );
    assert_eq!(
        fill(crate::plate::PlateBackground::Solid),
        bg.with_alpha(1.0)
    );
}
