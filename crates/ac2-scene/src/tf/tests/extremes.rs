use super::*;
use crate::view::PlotChrome;

/// A direct sound plus a copy 0.9 as loud 1 ms later, on 1/48 octave from 20 Hz to 20 kHz:
/// notches at odd multiples of 500 Hz, peaks at multiples of 1 kHz.
pub(super) fn comb_cols() -> Cols {
    let freqs: Vec<f64> = (0..)
        .map(|i| 20.0 * 2f64.powf(f64::from(i) / 48.0))
        .take_while(|f| *f <= 20_000.0)
        .collect();
    let mut c = cols(freqs.len());
    c.mag = freqs
        .iter()
        .map(|f| {
            let w = 2.0 * std::f64::consts::PI * f * 1e-3;
            (10.0 * (1.0 + 0.81 + 1.8 * w.cos()).log10()) as f32
        })
        .collect();
    c.freqs = freqs;
    c
}

fn view(chrome: PlotChrome) -> ViewState {
    ViewState {
        chrome,
        ..ViewState::default()
    }
}

/// The curve of a measurement whose peak mode tracks.
fn tracking<'a>(c: &'a Cols, key: TraceKey) -> TfTrace<'a> {
    TfTrace {
        tracks: true,
        ..trace(c, key, 0.0)
    }
}

fn scene(c: &Cols, v: &ViewState, cache: &DisplayCache) -> TfScene {
    let t = tracking(c, TraceKey::Live(MeasId(1)));
    transfer_scene(&[t], cache, &Status::default(), v, &Theme::dark(), SIZE)
}

#[test]
fn the_list_names_peaks_dips_and_the_comb() {
    let c = comb_cols();
    let s = scene(&c, &view(PlotChrome::Full), &DisplayCache::default());
    let list = s.extremes_list.expect("list");
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines[0], "max", "{list}");
    assert_eq!(lines[6], "min", "{list}");
    assert_eq!(lines.len(), 13, "{list}");
    // 999 Hz: the notches read on the 1/48-octave grid; the delay is 1.00 ms.
    assert_eq!(
        lines[12], "comb Δf 999 Hz · 1.00 ms · 0.34 m @ 20 °C",
        "{list}"
    );
    // Rows: rank, frequency, dB.
    assert!(
        lines[1].starts_with("1  ") && lines[1].ends_with(" +5.6 dB"),
        "{list}"
    );
    let e = s.extremes.expect("extremes");
    for r in &e.min {
        let k = (r.freq_hz / 1000.0 - 0.5).round();
        assert!(
            (r.freq_hz - (k + 0.5) * 1000.0).abs() < 0.01 * r.freq_hz,
            "{r:?}"
        );
    }
    // Every mark is on the magnitude view, a triangle each.
    let mag = s.panes[0].plot;
    assert!(e.max.iter().chain(&e.min).all(|r| {
        r.mark
            .is_some_and(|m| m.y >= mag.y - 5.0 && m.bottom() <= mag.bottom() + 5.0)
    }));
    let triangles = s.scene.layers[2]
        .polylines
        .iter()
        .filter(|p| p.points.len() == 4)
        .count();
    assert_eq!(triangles, 10);
    // The plate is in the magnitude view, clear of the legend.
    let list_label = s.scene.layers[2]
        .labels
        .iter()
        .find(|l| l.text == "max")
        .expect("drawn");
    let inside = |r: Rect, p: [f32; 2]| {
        p[0] >= r.x && p[0] <= r.right() && p[1] >= r.y && p[1] <= r.bottom()
    };
    assert!(inside(mag, list_label.pos));
    if let Some(b) = &s.legend_box {
        assert!(!inside(b.rect, list_label.pos));
    }
}

#[test]
fn traces_only_hides_the_marks() {
    let c = comb_cols();
    let s = scene(&c, &view(PlotChrome::Bare), &DisplayCache::default());
    assert_eq!(s.extremes_list, None);
    let e = s.extremes.expect("still tracked");
    assert_eq!((e.max.len(), e.min.len()), (5, 5));
    assert!(e.max.iter().chain(&e.min).all(|r| r.mark.is_none()));
    let overlay = &s.scene.layers[2];
    assert_eq!(
        overlay
            .polylines
            .iter()
            .filter(|p| p.points.len() == 4)
            .count(),
        0
    );
    assert!(
        overlay
            .labels
            .iter()
            .all(|l| l.text != "max" && l.text != "1")
    );
}

#[test]
fn blanked_columns_hold_no_dip_and_off_tracks_nothing() {
    let mut c = comb_cols();
    // Low coherence around the 1.5 kHz notch, blanked below 0.5.
    for (i, f) in c.freqs.iter().enumerate() {
        if (1300.0..1700.0).contains(f) {
            c.coh[i] = 0.1;
        }
    }
    let mut v = view(PlotChrome::Full);
    v.tf.coherence.blank_below = Some(0.5);
    let cache = DisplayCache::default();
    let s = scene(&c, &v, &cache);
    let e = s.extremes.expect("extremes");
    assert!(
        e.min.iter().all(|r| !(1300.0..1700.0).contains(&r.freq_hz)),
        "{:?}",
        e.min
    );
    // A measurement whose mode does not track: nothing marked, nothing listed.
    let t = trace(&c, TraceKey::Live(MeasId(1)), 0.0);
    let s = transfer_scene(&[t], &cache, &Status::default(), &v, &Theme::dark(), SIZE);
    assert!(s.extremes.is_none() && s.extremes_list.is_none() && s.extremes_box.is_none());
}

/// Tracking follows the selected curve when the pane shows it, else the pane's own.
#[test]
fn tracking_follows_the_selected_curve_else_the_own() {
    let c = comb_cols();
    let (own, other) = (TraceKey::Live(MeasId(1)), TraceKey::Live(MeasId(2)));
    let mut traces = [tracking(&c, own), tracking(&c, other)];
    let cache = DisplayCache::default();
    let v = view(PlotChrome::Full);
    let at = |traces: &[TfTrace<'_>]| {
        transfer_scene(traces, &cache, &Status::default(), &v, &Theme::dark(), SIZE)
            .extremes
            .map(|e| e.key)
    };
    assert_eq!(at(&traces), Some(own));
    traces[1].selected = true;
    assert_eq!(at(&traces), Some(other));
}

/// The list is on the peak list's plate, the one a spectrum lists its peaks on: in a bottom
/// corner of the magnitude view until the operator drops it somewhere, sized as set.
#[test]
fn the_list_is_on_the_movable_peaks_plate() {
    use crate::plate::PlateKind;
    let c = comb_cols();
    let mut v = view(PlotChrome::Full);
    let s = scene(&c, &v, &DisplayCache::default());
    let b = s.extremes_box.expect("plate");
    assert_eq!(b.kind, PlateKind::Peaks);
    assert!(s.plates().contains(&b), "the pointer finds it");
    let mag = s.panes[0].plot;
    assert!(
        (b.rect.bottom() - (mag.bottom() - INSET)).abs() < 0.01,
        "{:?} in {mag:?}",
        b.rect
    );
    v.plates.peaks.move_to(0.0, 0.0);
    let moved = scene(&c, &v, &DisplayCache::default())
        .extremes_box
        .expect("plate");
    assert!(
        moved.rect.x < b.rect.x && moved.rect.y < b.rect.y,
        "{:?}, from {:?}",
        moved.rect,
        b.rect
    );
    // Traces only (T) hides the list with its plate.
    let bare = scene(&c, &view(PlotChrome::Bare), &DisplayCache::default());
    assert_eq!(bare.extremes_box, None);
}

/// A held magnitude is drawn with the curve's offset, leaving out the columns no frame
/// has shown yet; a hold kept on another grid is not drawn.
#[test]
fn the_hold_draws_with_the_curves_offset() {
    let c = comb_cols();
    let n = c.freqs.len();
    let mut hold = vec![3.0_f32; n];
    hold[0] = f32::NAN;
    let lines = |hold: Option<&[f32]>| {
        let t = TfTrace {
            hold,
            offset_db: 2.0,
            ..trace(&c, TraceKey::Live(MeasId(1)), 0.0)
        };
        let s = transfer_scene(
            &[t],
            &DisplayCache::default(),
            &Status::default(),
            &view(PlotChrome::Full),
            &Theme::dark(),
            SIZE,
        );
        (
            s.traces[0].hold_db.clone(),
            s.scene
                .layers
                .iter()
                .map(|l| l.polylines.len())
                .sum::<usize>(),
        )
    };
    let (none, plain) = lines(None);
    assert_eq!(none, None);
    let (held, with_hold) = lines(Some(&hold));
    let held = held.expect("hold");
    assert!(held[0].is_nan());
    assert!(held[1..].iter().all(|v| *v == 5.0), "{:?}", &held[..4]);
    assert_eq!(with_hold, plain + 1, "one line more, on the magnitude");
    let (other_grid, lines_drawn) = lines(Some(&hold[..n - 1]));
    assert_eq!((other_grid, lines_drawn), (None, plain));
}

/// Columns the plot leaves out for their coherence add nothing to a hold.
#[test]
fn a_hold_takes_only_the_shown_magnitude() {
    let got = crate::trace::shown_magnitude(&[1.0, 2.0, 3.0], &[0.9, 0.2, f32::NAN], Some(0.5));
    assert_eq!(got[0], 1.0);
    assert!(got[1].is_nan(), "γ² 0.2 is blanked below 0.5");
    assert_eq!(got[2], 3.0, "no coherence known: drawn, so held");
    assert_eq!(
        crate::trace::shown_magnitude(&[1.0, 2.0], &[0.9, 0.2], None),
        vec![1.0, 2.0]
    );
}
