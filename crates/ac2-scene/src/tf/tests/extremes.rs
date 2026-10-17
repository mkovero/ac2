use super::*;

/// A direct sound plus a copy 0.9 as loud 1 ms later, on 1/48 octave from 20 Hz to 20 kHz:
/// notches at odd multiples of 500 Hz, peaks at multiples of 1 kHz.
fn comb_cols() -> Cols {
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
        tf: TfView {
            extremes: true,
            ..TfView::default()
        },
        chrome,
        ..ViewState::default()
    }
}

fn scene(c: &Cols, v: &ViewState, cache: &DisplayCache) -> TfScene {
    let t = trace(c, TraceKey::Live(MeasId(1)), 0.0);
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
    assert_eq!(lines[12], "comb Δf 999 Hz · 1.00 ms", "{list}");
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
fn plot_chrome_off_leaves_the_marks() {
    let c = comb_cols();
    let s = scene(&c, &view(PlotChrome::Bare), &DisplayCache::default());
    assert_eq!(s.extremes_list, None);
    let e = s.extremes.expect("still tracked");
    assert_eq!((e.max.len(), e.min.len()), (5, 5));
    let overlay = &s.scene.layers[2];
    assert_eq!(
        overlay
            .polylines
            .iter()
            .filter(|p| p.points.len() == 4)
            .count(),
        10
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
    v.tf.extremes = false;
    let s = scene(&c, &v, &cache);
    assert!(s.extremes.is_none() && s.extremes_list.is_none());
}
