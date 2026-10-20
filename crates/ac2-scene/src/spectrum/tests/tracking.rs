use super::*;
use crate::canvas::tests::{intersects, label_box};
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
    // The list sits under the cursor readout, never over it.
    let labels = &s.scene.layers[2].labels;
    let list = labels
        .iter()
        .find(|l| l.text.starts_with("1  1.00 kHz"))
        .expect("list drawn");
    let cursor = labels
        .iter()
        .find(|l| l.text.starts_with("1.00 kHz\n"))
        .expect("cursor readout drawn");
    assert!(!intersects(label_box(list), label_box(cursor)));
    assert!(list.pos[1] > cursor.pos[1]);
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
