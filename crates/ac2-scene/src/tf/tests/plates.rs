use super::*;
use crate::plate::{PlateKind, PlatePlace};

fn scene(c: &Cols, v: &ViewState, size: Viewport) -> TfScene {
    let ts = [
        trace(c, TraceKey::Live(MeasId(1)), 0.010),
        trace(c, TraceKey::Live(MeasId(2)), 0.0115),
    ];
    transfer_scene(
        &ts,
        &DisplayCache::default(),
        &Status::default(),
        v,
        &Theme::dark(),
        size,
    )
}

fn view() -> ViewState {
    ViewState {
        cursor_hz: Some(1000.0),
        cursor_mark_hz: Some(500.0),
        ..ViewState::default()
    }
}

/// The labels drawn on `r`.
fn texts_on(s: &TfScene, r: Rect) -> Vec<String> {
    s.scene.layers[2]
        .labels
        .iter()
        .filter(|l| l.pos[1] > r.y && l.pos[1] < r.bottom() && l.pos[0] >= r.x)
        .filter(|l| l.pos[0] <= r.right())
        .map(|l| l.text.clone())
        .collect()
}

/// Dragged plates sit where they were dropped: the cursor values in the plot's bottom-left
/// (the frequency first, each value naming its curve as it is off the legend's rows), the
/// mark's readout in its top-right, the legend in the middle. Each is listed for the mouse.
#[test]
fn dragged_plates_sit_where_dropped() {
    let c = cols(97);
    let mut v = view();
    v.plates.readout.at = Some([0.0, 1.0]);
    v.plates.mark.at = Some([1.0, 0.0]);
    v.plates.legend.at = Some([0.5, 0.5]);
    let s = scene(&c, &v, SIZE);
    let mag = s.panes[0].plot;
    let values = s.readout_box.expect("values");
    assert_eq!(values.rect.x, values.area.x + crate::legend::INSET);
    assert_eq!(
        values.rect.bottom(),
        values.area.bottom() - crate::legend::INSET
    );
    let on = texts_on(&s, values.rect);
    assert!(on[0].ends_with("kHz"), "{on:?}");
    assert!(on[1].starts_with("m1  "), "{on:?}");
    let mark = s.mark_box.expect("mark");
    assert_eq!(mark.area, mag);
    assert_eq!(mark.rect.right(), mag.right() - crate::legend::INSET);
    assert_eq!(mark.rect.y, mag.y + crate::legend::INSET);
    assert!(texts_on(&s, mark.rect)[0].starts_with("mark "));
    let legend = s.legend_box.as_ref().expect("legend");
    assert_eq!(
        legend.plate.position_for(legend.rect.x, legend.rect.y),
        [0.5, 0.5]
    );
    let kinds: Vec<PlateKind> = s.plates().iter().map(|p| p.kind).collect();
    assert_eq!(kinds, PlateKind::ALL);
}

/// A plate dropped in a corner of a large pane stays wholly inside, in that corner, when
/// the pane shrinks.
#[test]
fn a_dropped_plate_stays_inside_a_smaller_pane() {
    let c = cols(97);
    let mut v = view();
    v.plates.readout.at = Some([1.0, 1.0]);
    v.plates.readout.scale = 2.0;
    for size in [
        SIZE,
        Viewport {
            width: 420.0,
            height: 300.0,
        },
    ] {
        let s = scene(&c, &v, size);
        let b = s.readout_box.expect("values");
        let inset = crate::legend::INSET;
        assert_eq!(b.rect.right(), b.area.right() - inset, "{size:?}");
        assert_eq!(b.rect.bottom(), b.area.bottom() - inset, "{size:?}");
        assert!(b.rect.x >= b.area.x && b.rect.y >= b.area.y, "{size:?}");
    }
}

/// A larger text scale grows each plate and its text; the scale is reported for the grip.
#[test]
fn scaled_text_grows_the_plates() {
    let c = cols(97);
    let one = scene(&c, &view(), SIZE);
    let mut v = view();
    for k in PlateKind::ALL {
        v.plates.get_mut(k).set_scale(2.0);
    }
    let two = scene(&c, &v, SIZE);
    for (a, b) in one.plates().iter().zip(two.plates()) {
        assert_eq!(b.scale, 2.0);
        assert!(b.rect.h > 1.9 * a.rect.h, "{a:?} {b:?}");
        assert!(b.rect.w > 1.5 * a.rect.w, "{a:?} {b:?}");
    }
    let font = |s: &TfScene, r: Rect| {
        s.scene.layers[2]
            .labels
            .iter()
            .find(|l| l.pos[1] > r.y && l.pos[1] < r.bottom() && l.pos[0] >= r.x)
            .map(|l| l.size)
            .expect("text")
    };
    let (a, b) = (one.mark_box.expect("mark"), two.mark_box.expect("mark"));
    assert_eq!(font(&two, b.rect), 2.0 * font(&one, a.rect));
}

/// Back to the scene's place and size: the same plates as never touched.
#[test]
fn reset_returns_to_the_automatic_place() {
    let c = cols(97);
    let auto = scene(&c, &view(), SIZE);
    let mut v = view();
    v.plates.readout = PlatePlace {
        at: Some([0.3, 0.6]),
        scale: 2.5,
    };
    let moved = scene(&c, &v, SIZE);
    assert_ne!(moved.readout_box, auto.readout_box);
    v.plates.readout = PlatePlace::default();
    assert!(v.plates.is_auto());
    let back = scene(&c, &v, SIZE);
    assert_eq!(back.readout_box, auto.readout_box);
    assert_eq!(back.scene.layers[2], auto.scene.layers[2]);
}

/// Plates at the largest text in a pane too small for them shrink to fit wholly inside
/// their plot, never below the theme's size; the stored scale is kept for a larger pane.
#[test]
fn enlarged_plates_shrink_to_a_small_pane() {
    let c = cols(97);
    let mut v = view();
    for p in [
        &mut v.plates.readout,
        &mut v.plates.mark,
        &mut v.plates.legend,
    ] {
        p.set_scale(3.0);
    }
    v.plates.readout.move_to(0.0, 1.0);
    v.plates.legend.move_to(1.0, 0.0);
    let small = Viewport {
        width: 420.0,
        height: 300.0,
    };
    let s = scene(&c, &v, small);
    for b in s.plates() {
        let inside = |r: Rect| {
            r.x >= b.area.x - 0.01
                && r.y >= b.area.y - 0.01
                && r.right() <= b.area.right() + 0.01
                && r.bottom() <= b.area.bottom() + 0.01
        };
        assert!(inside(b.rect), "{:?}: {:?} in {:?}", b.kind, b.rect, b.area);
        assert!(b.scale < 3.0 && b.scale >= 1.0, "{:?}: {}", b.kind, b.scale);
    }
    assert_eq!(v.plates.readout.scale(), 3.0);
    let big = scene(&c, &v, SIZE);
    let grown = big.readout_box.expect("values").scale;
    assert!(grown > s.readout_box.expect("values").scale, "{grown}");
}
