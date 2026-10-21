use super::*;
use crate::cursor_jump::{Landmark, Step};

fn scene(c: &Cols, v: &ViewState) -> TfScene {
    let t = trace(c, TraceKey::Live(MeasId(1)), 0.0);
    transfer_scene(
        &[t],
        &DisplayCache::default(),
        &Status::default(),
        v,
        &Theme::dark(),
        SIZE,
    )
}

/// The cursor on one notch of the comb and its mark on the one below: the readout names
/// their spacing as the comb's, the delay and the path it stands for.
#[test]
fn the_mark_reads_the_comb_between_two_notches() {
    let c = super::extremes::comb_cols();
    let t = [trace(&c, TraceKey::Live(MeasId(1)), 0.0)];
    let cache = DisplayCache::default();
    let shown = Range::new(200.0, 4000.0);
    let mut v = ViewState::default();
    let first = jump(&t, &cache, &v, shown, Landmark::Notch, Step::Up).expect("notch");
    v.cursor_hz = Some(first);
    let second = jump(&t, &cache, &v, shown, Landmark::Notch, Step::Up).expect("notch");
    assert!((first - 500.0).abs() < 10.0 && (second - 1500.0).abs() < 25.0);
    v.cursor_mark_hz = Some(first);
    v.cursor_hz = Some(second);
    let s = scene(&c, &v);
    let m = s.mark.expect("mark");
    assert_eq!(m.freq_hz, first);
    // The 1/48-octave column nearest the notch.
    assert_eq!(m.lines[0], "mark 501 Hz", "{m:?}");
    assert!(m.lines[1].starts_with("Δ +1.0"), "{m:?}");
    assert_eq!(
        m.lines[2], "comb Δf 1.00 kHz · 1.00 ms · 0.34 m @ 20 °C",
        "{m:?}"
    );
    // The plate is drawn, and its text is on the scene.
    assert!(s.mark_box.is_some());
    let texts: Vec<&str> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .map(|l| l.text.as_str())
        .collect();
    assert!(
        texts.contains(&"comb Δf 1.00 kHz · 1.00 ms · 0.34 m @ 20 °C"),
        "{texts:?}"
    );
    // The peak between the notches is half the comb's spacing from either, and a column
    // that is neither has none: no comb.
    let peak = jump(&t, &cache, &v, shown, Landmark::Peak, Step::Down).expect("peak");
    assert!((peak - 1000.0).abs() < 25.0, "{peak}");
    for hz in [peak, 1200.0] {
        v.cursor_hz = Some(hz);
        let m = scene(&c, &v).mark.expect("mark");
        assert_eq!(m.lines.len(), 2, "{hz}: {m:?}");
    }
    // Up from the peak between the two notches is the upper notch, down the lower.
    v.cursor_hz = Some(1000.0);
    let up = jump(&t, &cache, &v, shown, Landmark::Notch, Step::Up);
    let down = jump(&t, &cache, &v, shown, Landmark::Notch, Step::Down);
    assert_eq!((down, up), (Some(first), Some(second)));
}

#[test]
fn no_mark_readout_without_the_cursor_or_with_it_hidden() {
    let c = super::extremes::comb_cols();
    let mut v = ViewState {
        cursor_mark_hz: Some(500.0),
        ..ViewState::default()
    };
    assert!(scene(&c, &v).mark.is_none());
    v.cursor_hz = Some(1500.0);
    assert!(scene(&c, &v).mark.is_some());
    v.chrome = crate::view::PlotChrome::Bare;
    let s = scene(&c, &v);
    assert!(s.mark.is_none() && s.mark_box.is_none());
}

/// Room ripple, not a comb: bumps at 1.15, 1.45 and 1.85 kHz, 300 then 400 Hz apart. The
/// mark on the first and the cursor on the second read no delay: no third peak repeats
/// their spacing; drawn at 1/6 octave, the smoothing is too wide for it to say anything.
#[test]
fn ripple_peaks_read_no_comb() {
    let mut c = super::extremes::comb_cols();
    c.mag = c
        .freqs
        .iter()
        .map(|f| {
            [1150.0, 1450.0, 1850.0]
                .iter()
                .map(|p| 6.0 * (-0.5 * ((f - p) / 40.0f64).powi(2)).exp())
                .sum::<f64>() as f32
        })
        .collect();
    let mut v = ViewState {
        cursor_mark_hz: Some(1150.0),
        cursor_hz: Some(1450.0),
        ..ViewState::default()
    };
    let m = scene(&c, &v).mark.expect("mark");
    assert_eq!(m.lines[2], "comb: uneven, no third peak", "{m:?}");
    let mut t = trace(&c, TraceKey::Live(MeasId(1)), 0.0);
    t.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::MagnitudePhase,
    });
    let s = transfer_scene(
        &[t],
        &DisplayCache::default(),
        &Status::default(),
        &v,
        &Theme::dark(),
        SIZE,
    );
    let m = s.mark.expect("mark");
    // 2 · 1.45 kHz · (2^(1/12) − 2^(−1/12)), at the 1/48-octave column nearest 1.45 kHz
    // (1.46 kHz).
    assert_eq!(
        m.lines[2], "comb: smoothing 1/6 oct hides combs under 337 Hz here",
        "{m:?}"
    );
    // The comb itself stands, read between the 1/48-octave columns nearest two notches.
    v.cursor_mark_hz = Some(1500.0);
    v.cursor_hz = Some(2500.0);
    let m = scene(&super::extremes::comb_cols(), &v).mark.expect("mark");
    assert_eq!(
        m.lines[2], "comb Δf 987 Hz · 1.01 ms · 0.35 m @ 20 °C",
        "{m:?}"
    );
}
