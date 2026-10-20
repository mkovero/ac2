use super::*;
use crate::cursor_jump::{Landmark, Step};

/// Third-octave bands with a 9 dB bump at 1 kHz and a 9 dB dip at 4 kHz on −40 dB.
fn levels(f: &[f64]) -> Vec<f32> {
    f.iter()
        .map(|x| {
            let b = (x / 1000.0).log2() * 3.0;
            let d = (x / 4000.0).log2() * 3.0;
            (-40.0 + 9.0 * (-0.5 * b * b).exp() - 9.0 * (-0.5 * d * d).exp()) as f32
        })
        .collect()
}

#[test]
fn jumps_and_the_mark_read_the_drawn_levels() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let t = rta_trace(&f, &e, &level, LevelScale::DbSpl);
    let shown = Range::new(20.0, 20_000.0);
    let mut view = ViewState::default();
    let peak = jump(
        std::slice::from_ref(&t),
        &view,
        shown,
        Landmark::Peak,
        Step::Up,
    );
    assert_eq!(peak, Some(1000.0));
    let notch = jump(
        std::slice::from_ref(&t),
        &view,
        shown,
        Landmark::Notch,
        Step::Down,
    );
    assert_eq!(notch.map(|f| f.round()), Some(3981.0));
    // A 12 dB prominence passes both by.
    view.jump_prominence_db = 12.0;
    assert_eq!(
        jump(
            std::slice::from_ref(&t),
            &view,
            shown,
            Landmark::Peak,
            Step::Up
        ),
        None
    );
    view.jump_prominence_db = 3.0;
    view.cursor_mark_hz = Some(1000.0);
    view.cursor_hz = Some(2000.0);
    let s = spectrum_scene(&[t], &Status::default(), &view, &Theme::dark(), SIZE);
    let m = s.mark.expect("mark");
    // The 1 kHz band is the bump's top, −31 dB; the 2 kHz band sits a third of the way
    // between the bump and the dip, whose tails cancel there: −40 dB.
    assert_eq!(
        m.lines,
        [
            "mark 1.00 kHz",
            "Δ +1.00 kHz · −9.0 dB",
            "comb Δf 1.00 kHz · 1.00 ms · 0.34 m @ 20 °C",
        ]
    );
    // Under the cursor's values, in one label.
    let label = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .find(|l| l.text.starts_with("2.00 kHz"))
        .expect("cursor label");
    assert!(
        label
            .text
            .ends_with("comb Δf 1.00 kHz · 1.00 ms · 0.34 m @ 20 °C"),
        "{}",
        label.text
    );
}
