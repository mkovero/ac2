//! The sweep view's share of what every frequency view has: the cursor's jumps and mark,
//! the plates' places, and levels read as differences.

use super::*;

/// A response with a notch at `notch_hz` and a peak at `peak_hz`, each 12 dB out of −6 dB.
fn bumpy(notch_hz: f64, peak_hz: f64) -> TraceData {
    let mut d = data();
    let f = grid().centres();
    let at = |hz| nearest_column(&f, hz).unwrap();
    d.mag_db[at(notch_hz)] = -18.0;
    d.mag_db[at(peak_hz)] = 6.0;
    d
}

#[test]
fn the_cursor_jumps_along_the_fundamental() {
    use crate::cursor_jump::{Landmark, Step};
    let d = bumpy(200.0, 1000.0);
    let f = grid().centres();
    let t = SweepView {
        data: &d,
        freqs: &f,
        color: Color::from_rgba8([1, 2, 3, 255]),
        selected: false,
    };
    let mut view = ViewState {
        cursor_hz: Some(100.0),
        ..ViewState::default()
    };
    let shown = view.freq.range();
    let col = |hz: f64| f[nearest_column(&f, hz).unwrap()];
    assert_eq!(
        jump(&t, &view, shown, Landmark::Peak, Step::Up),
        Some(col(1000.0))
    );
    assert_eq!(
        jump(&t, &view, shown, Landmark::Notch, Step::Up),
        Some(col(200.0))
    );
    assert_eq!(jump(&t, &view, shown, Landmark::Notch, Step::Down), None);
    // A harmonic's level is never a landmark: H2 ends at 2 kHz, the fundamental does not.
    view.cursor_hz = Some(1500.0);
    assert_eq!(jump(&t, &view, shown, Landmark::Notch, Step::Up), None);
}

#[test]
fn the_mark_reads_the_fundamental_on_its_own_plate() {
    let d = bumpy(200.0, 1000.0);
    let f = grid().centres();
    let t = SweepView {
        data: &d,
        freqs: &f,
        color: Color::from_rgba8([1, 2, 3, 255]),
        selected: false,
    };
    let mut view = ViewState {
        cursor_hz: Some(1000.0),
        cursor_mark_hz: Some(200.0),
        ..ViewState::default()
    };
    let scene = |view: &ViewState, compared: &[SweepView<'_>]| {
        distortion_scene(
            Some(t),
            compared,
            MISSING,
            &status(),
            view,
            &Theme::dark(),
            SIZE,
        )
    };
    let sc = scene(&view, &[]);
    let col = |hz: f64| f[nearest_column(&f, hz).unwrap()];
    let m = sc.mark.as_ref().expect("mark");
    assert_eq!(m.freq_hz, col(200.0));
    let delta = format!(
        "Δ {} · +24.0 dB",
        crate::readout::freq_delta(col(1000.0) - col(200.0))
    );
    assert_eq!(
        m.lines,
        [format!("mark {}", format::freq_readout(col(200.0))), delta]
    );
    // On the mark's plate in a bottom corner of the fundamental's plot, as the transfer
    // view puts it in its magnitude plot; its dashed line runs through both plots.
    let b = sc.mark_box.expect("mark plate");
    assert_eq!(b.kind, PlateKind::Mark);
    assert_eq!(b.area, sc.fundamental);
    assert!((b.rect.bottom() - (sc.fundamental.bottom() - crate::legend::INSET)).abs() < 0.01);
    assert_eq!(sc.plates().len(), 3);
    let x = sc.x_axis.mapping.to_px(m.freq_hz);
    let dashed = sc.scene.layers[2]
        .polylines
        .iter()
        .filter(|p| p.stroke.dash.is_some() && p.points.iter().all(|q| (q[0] - x).abs() < 0.01))
        .count();
    assert_eq!(dashed, 2);
    // Beside a compared run it says whose curve it reads.
    let named = scene(&view, &[t]);
    assert_eq!(
        named.mark.unwrap().lines[0],
        format!("mark 1083 sweep {}", format::freq_readout(col(200.0)))
    );
    // Traces only (T) hides the cursor, its values and the mark; the legend stays.
    view.chrome = crate::view::PlotChrome::Bare;
    let bare = scene(&view, &[]);
    assert!(bare.cursor.is_none() && bare.readout_box.is_none() && bare.mark_box.is_none());
    assert!(bare.legend_box.is_some());
    // Hidden as on every pane, and placed where the pane's plates say.
    view.chrome = crate::view::PlotChrome::Full;
    view.legend.hidden = true;
    assert!(scene(&view, &[]).legend_box.is_none());
    view.legend.hidden = false;
    view.plates.legend.move_to(1.0, 1.0);
    view.plates.readout.move_to(0.0, 1.0);
    let moved = scene(&view, &[]);
    let (l, r) = (
        moved.legend_box.unwrap().rect,
        moved.readout_box.unwrap().rect,
    );
    assert!((l.right() - (moved.plot.right() - crate::legend::INSET)).abs() < 0.01);
    assert!((r.x - (moved.plot.x + crate::legend::INSET)).abs() < 0.01);
    assert!((r.bottom() - (moved.plot.bottom() - crate::legend::INSET)).abs() < 0.01);
}

#[test]
fn levels_re_the_fundamental_read_signed() {
    assert_eq!(db_text(-40.12), "−40.1 dB");
    assert_eq!(db_text(1.5), "+1.5 dB");
    assert_eq!(db_text(0.0), "0.0 dB");
    assert_eq!(db_text(f64::NAN), "—");
    assert_eq!(
        reading_text(Reading::Level(2.0), DistortionUnit::Db),
        "+2.0 dB"
    );
    assert_eq!(
        reading_text(Reading::BelowFloor(-70.0), DistortionUnit::Db),
        "< −70.0 dB"
    );
    // Percent has no sign: it is a ratio, never below zero.
    assert_eq!(
        reading_text(Reading::Level(2.0), DistortionUnit::Percent),
        "126 %"
    );
}

/// Every text a scene draws, whichever layer it is on.
fn drawn(s: &crate::primitives::Scene) -> Vec<String> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.clone()))
        .collect()
}

/// Traces only (T) leaves each of the sweep's plot views its curves, its legend and its
/// caption, as the transfer and spectrum views keep theirs: no axis text, cursor values,
/// mark, note about the readings or harmonic marks over the curves.
#[test]
fn traces_only_leaves_every_view_its_curves_legend_and_caption() {
    use crate::view::{IrMode, PlotChrome};
    let d = with_reference();
    let f = grid().centres();
    let t = SweepView {
        data: &d,
        freqs: &f,
        color: Color::from_rgba8([1, 2, 3, 255]),
        selected: false,
    };
    let mut view = ViewState {
        cursor_hz: Some(500.0),
        cursor_mark_hz: Some(200.0),
        ..ViewState::default()
    };
    view.distortion.ir.cursor_ms = Some(0.2);
    let response = |view: &ViewState| {
        distortion_scene(Some(t), &[], MISSING, &status(), view, &Theme::dark(), SIZE)
    };
    let ir = |view: &ViewState| {
        sweep_ir_scene(
            &d,
            Color::from_rgba8([255, 0, 0, 255]),
            &status(),
            view,
            &Theme::dark(),
            SIZE,
        )
        .expect("ir")
    };

    // Response and distortion: the note about the readings stays while they do.
    for chrome in [PlotChrome::Full, PlotChrome::NoGrid] {
        view.chrome = chrome;
        let sc = response(&view);
        assert_eq!(sc.near_reference.len(), 1, "{chrome:?}");
        assert!(
            drawn(&sc.scene).contains(&sc.near_reference[0]),
            "{chrome:?}"
        );
    }
    view.chrome = PlotChrome::Bare;
    let bare = response(&view);
    assert!(bare.cursor.is_none() && bare.readout_box.is_none() && bare.mark_box.is_none());
    assert!(bare.near_reference.is_empty());
    assert!(!bare.legend_shown.is_empty() && !bare.caption.is_empty());
    for text in drawn(&bare.scene) {
        assert!(
            bare.legend_shown.contains(&text) || text == bare.caption,
            "{text:?} over the traces"
        );
    }

    // The impulse response in each of its forms: the harmonics' marks go with the marks.
    for mode in [IrMode::Linear, IrMode::Log, IrMode::Etc] {
        view.ir.mode = mode;
        view.chrome = PlotChrome::NoGrid;
        let kept = drawn(&ir(&view).scene);
        assert!(kept.iter().any(|t| t == "H2"), "{mode:?}: {kept:?}");
        view.chrome = PlotChrome::Bare;
        let bare = ir(&view);
        assert!(
            bare.cursor.is_none() && bare.readout_box.is_none(),
            "{mode:?}"
        );
        assert_eq!(
            drawn(&bare.scene),
            std::slice::from_ref(&bare.origin),
            "{mode:?}"
        );
        assert!(
            bare.scene.layers[2].polylines.is_empty(),
            "{mode:?}: marks over the traces"
        );
    }
}
