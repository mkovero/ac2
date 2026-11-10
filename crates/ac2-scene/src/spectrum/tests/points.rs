//! Peak points: a picked curve's peaks labelled on the plot, and its legend row's cue.

use super::*;
use crate::spectrum::peaks::{Peak, PeakInput, PeakTracker, find_peaks};
use crate::view::PeakPoints;

/// Wide enough that labels half a decade apart never touch.
const WIDE: Viewport = Viewport {
    width: 1600.0,
    height: 400.0,
};

fn peaks_of(f: &[f64], level: &[f32]) -> Vec<Peak> {
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

/// Five peaks half a decade apart on a −70 dB floor.
fn five(f: &[f64]) -> Vec<f32> {
    let at = |hz: f64, c: f64| (hz / c - 1.0).abs() < 0.01;
    f.iter()
        .map(|hz| match *hz {
            x if at(x, 31.62) => -45.0,
            x if at(x, 100.0) => -40.0,
            x if at(x, 316.2) => -38.0,
            x if at(x, 1000.0) => -32.0,
            x if at(x, 3162.0) => -50.0,
            _ => -70.0,
        })
        .collect()
}

fn view(chrome: PlotChrome, list: bool) -> ViewState {
    ViewState {
        spectrum: SpectrumView {
            level: Range::new(-100.0, 0.0),
            ..SpectrumView::default()
        },
        chrome,
        peak_points: Some(PeakPoints {
            key: TraceKey::Live(MeasId(1)),
            list,
            named: false,
        }),
        ..ViewState::default()
    }
}

/// Peak boxes on the plot (the legend's cue is a box above it).
fn boxes(s: &SpectrumScene) -> usize {
    s.scene.layers[2]
        .polylines
        .iter()
        .filter(|p| p.points.len() == 5 && p.stroke.width == 1.5 && p.points[0][1] >= s.plot.y)
        .count()
}

fn texts(s: &SpectrumScene) -> Vec<&str> {
    s.peak_labels.iter().map(|l| l.text.as_str()).collect()
}

#[test]
fn a_picked_curve_shows_five_labelled_points() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = five(&f);
    let peaks = peaks_of(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let s = spectrum_scene(
        &[t],
        &Status::default(),
        &view(PlotChrome::Full, false),
        &Theme::dark(),
        WIDE,
    );
    assert_eq!(
        texts(&s),
        [
            "1.00 kHz  −32.0 dBFS",
            "316 Hz  −38.0 dBFS",
            "100 Hz  −40.0 dBFS",
            "31.6 Hz  −45.0 dBFS",
            "3.16 kHz  −50.0 dBFS",
        ]
    );
    // Boxes, no ranks, no list.
    assert_eq!(boxes(&s), 5);
    assert_eq!(s.peak_list, None);
    let labels = &s.scene.layers[2].labels;
    assert!(labels.iter().all(|l| l.text != "1"), "{labels:?}");
    // Each label above its box, inside the plot.
    for (l, r) in s.peak_labels.iter().zip(&s.peaks[0].rows) {
        let m = r.mark.expect("on the plot");
        assert!(l.rect.bottom() <= m.y, "{l:?} {m:?}");
        assert!(l.rect.x >= s.plot.x && l.rect.right() <= s.plot.right());
    }
}

#[test]
fn with_the_own_mode_tracking_the_pick_is_ranked_and_listed() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = five(&f);
    let peaks = peaks_of(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let mut v = view(PlotChrome::Full, true);
    let s = spectrum_scene(
        std::slice::from_ref(&t),
        &Status::default(),
        &v,
        &Theme::dark(),
        WIDE,
    );
    assert!(s.peak_labels.is_empty());
    let list = s.peak_list.expect("listed");
    assert!(list.starts_with("1  1.00 kHz  −32.0 dBFS (band)"), "{list}");
    assert!(s.scene.layers[2].labels.iter().any(|l| l.text == "1"));
    // Not the pane's own curve: the list says whose it is.
    v.peak_points = v.peak_points.map(|p| PeakPoints { named: true, ..p });
    let s = spectrum_scene(&[t], &Status::default(), &v, &Theme::dark(), WIDE);
    let list = s.peak_list.expect("listed");
    assert!(list.starts_with("RTA\n1  1.00 kHz"), "{list}");
}

#[test]
fn no_grid_keeps_the_points_and_traces_only_hides_them() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = five(&f);
    let peaks = peaks_of(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let scene = |c| {
        spectrum_scene(
            std::slice::from_ref(&t),
            &Status::default(),
            &view(c, false),
            &Theme::dark(),
            WIDE,
        )
    };
    let s = scene(PlotChrome::NoGrid);
    assert_eq!((boxes(&s), s.peak_labels.len()), (5, 5));
    let s = scene(PlotChrome::Bare);
    assert_eq!((boxes(&s), s.peak_labels.len()), (0, 0));
    assert_eq!(s.peak_list, None);
    assert!(
        s.scene.layers[2]
            .labels
            .iter()
            .all(|l| !l.text.contains("dBFS") && l.text != "1")
    );
}

#[test]
fn an_overlapping_label_is_dropped_and_its_box_stays() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    // Two peaks a fifth of a decade apart at nearly one level: their labels would touch.
    let level: Vec<f32> = f
        .iter()
        .map(|hz| match *hz {
            x if (x / 1000.0 - 1.0).abs() < 0.01 => -32.0,
            x if (x / 1585.0 - 1.0).abs() < 0.01 => -33.0,
            _ => -70.0,
        })
        .collect();
    let peaks = peaks_of(&f, &level);
    assert_eq!(peaks.len(), 2);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let s = spectrum_scene(
        &[t],
        &Status::default(),
        &view(PlotChrome::Full, false),
        &Theme::dark(),
        SIZE,
    );
    assert_eq!(boxes(&s), 2);
    // The louder peak keeps its label.
    assert_eq!(texts(&s), ["1.00 kHz  −32.0 dBFS"]);
}

#[test]
fn a_label_the_top_would_cut_goes_below_its_box() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level: Vec<f32> = f
        .iter()
        .map(|hz| {
            if (hz / 1000.0 - 1.0).abs() < 0.01 {
                -1.0
            } else {
                -70.0
            }
        })
        .collect();
    let peaks = peaks_of(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let s = spectrum_scene(
        &[t],
        &Status::default(),
        &view(PlotChrome::Full, false),
        &Theme::dark(),
        SIZE,
    );
    let m = s.peaks[0].rows[0].mark.expect("on the plot");
    let l = &s.peak_labels[0];
    assert!(l.rect.y >= m.bottom(), "{l:?} {m:?}");
    assert!(l.rect.y >= s.plot.y);
}

#[test]
fn the_quantity_note_only_on_a_pane_mixing_quantities() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = five(&f);
    let peaks = peaks_of(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    let mut tone = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    tone.key = TraceKey::Live(MeasId(2));
    tone.quantity = Quantity::Tone;
    let s = spectrum_scene(
        &[t, tone],
        &Status::default(),
        &view(PlotChrome::Full, false),
        &Theme::dark(),
        WIDE,
    );
    assert_eq!(texts(&s)[0], "1.00 kHz  −32.0 dBFS (band)");
}

#[test]
fn a_peak_off_the_level_range_has_no_point_and_the_offset_counts() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = five(&f);
    let peaks = peaks_of(&f, &level);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.tracked = Some(&peaks);
    t.offset_db = 6.0;
    let mut v = view(PlotChrome::Full, false);
    // −32 + 6 = −26 is the one peak above −30.
    v.spectrum.level = Range::new(-30.0, 0.0);
    let s = spectrum_scene(&[t], &Status::default(), &v, &Theme::dark(), WIDE);
    assert_eq!(boxes(&s), 1);
    assert_eq!(texts(&s), ["1.00 kHz  −26.0 dBFS"]);
}

#[test]
fn the_picked_legend_entry_has_the_cue_and_stays_out_of_more() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = five(&f);
    let names: Vec<String> = (1..=12)
        .map(|i| format!("Measurement number {i}"))
        .collect();
    let traces: Vec<SpectrumTrace<'_>> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
            t.key = TraceKey::Live(MeasId(i as u32 + 1));
            t.name = n.clone();
            t
        })
        .collect();
    let mut v = view(PlotChrome::Full, false);
    let last = TraceKey::Live(MeasId(12));
    let narrow = Viewport {
        width: 400.0,
        height: 300.0,
    };
    // Unpicked, the last entry is folded into `+N more`.
    v.peak_points = None;
    let s = spectrum_scene(&traces, &Status::default(), &v, &Theme::dark(), narrow);
    assert!(s.legend_shown.iter().any(|t| t.starts_with('+')));
    assert!(s.legend_hits.iter().all(|h| h.key != last));
    assert_eq!(s.picked_cue, None);
    // Picked, it comes first and carries the cue.
    v.peak_points = Some(PeakPoints {
        key: last,
        list: false,
        named: true,
    });
    let s = spectrum_scene(&traces, &Status::default(), &v, &Theme::dark(), narrow);
    assert_eq!(s.legend_hits.first().map(|h| h.key), Some(last));
    assert_eq!(s.legend_shown[0], "Measurement number 12");
    assert_eq!(s.picked_cue, Some(last));
}
