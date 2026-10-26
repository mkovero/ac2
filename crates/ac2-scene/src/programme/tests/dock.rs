//! The dock ladder: beside, above, fewer slots, none; a moved plate floats (docs/design/
//! programme-meter.md §6.2 *Where it sits*).

use super::super::loudness_plate;
use super::super::*;
use super::bars::{Rig, bar_rects, scene_of};
use crate::primitives::Rect;
use crate::view::PlotChrome;

const FONT: f32 = 11.0;

fn sw() -> f32 {
    scale_w(ScaleKind::Lufs, FONT)
}

fn plate_of(l: &ProgrammeLayout) -> Docked {
    l.plate.expect("a docked plate")
}

#[test]
fn the_dock_ladder_lands_on_each_step() {
    let (w0, h0) = loudness_plate::size(FONT, 1.0, 0);
    let (w2, h2) = loudness_plate::size(FONT, 2.0, 0);
    // One bar, a pair's goniometer beside: what the rest must keep.
    let need = sw() + BAR_MIN_W + GONIO_MIN_W;
    // 1. Beside at the plate's own scale.
    let wide = Rect::new(0.0, 0.0, need + w2 + 1.0, h2 + 1.0);
    let l = dock(wide, true, 1, sw(), FONT, 2.0);
    let d = plate_of(&l);
    assert_eq!((d.scale, d.dropped), (2.0, 0));
    assert_eq!(d.rect, Rect::new(wide.right() - w2, 0.0, w2, h2));
    assert_eq!(
        l.bars.right(),
        wide.right() - w2,
        "the bars give the plate its room"
    );
    assert!(l.gonio.is_some());
    // 2. Above at that scale when there is no room beside but height to spare.
    let tall = Rect::new(0.0, 0.0, w2 + 1.0, h2 + BAR_MIN_H + 1.0);
    let l = dock(tall, true, 1, sw(), FONT, 2.0);
    let d = plate_of(&l);
    assert_eq!((d.scale, d.dropped), (2.0, 0));
    assert_eq!(d.rect.y, 0.0);
    assert_eq!(l.bars.y, h2, "the bars below the reserve");
    assert_eq!(l.bars.h, tall.h - h2);
    // Then at 1×, beside: the stored scale is the operator's and comes back on a larger pane.
    let mid = Rect::new(0.0, 0.0, need + w0 + 1.0, h0 + 1.0);
    let d = plate_of(&dock(mid, true, 1, sw(), FONT, 2.0));
    assert_eq!((d.scale, d.dropped), (1.0, 0));
    // 3. Fewer slots at 1×, in the drop order.
    let mut last = 0;
    for dropped in 1..=loudness_plate::MAX_DROPPED {
        let (w, h) = loudness_plate::size(FONT, 1.0, dropped);
        let r = Rect::new(0.0, 0.0, need + w0 + 1.0, h + 0.5);
        let d = plate_of(&dock(r, true, 1, sw(), FONT, 1.0));
        assert_eq!(d.dropped, dropped);
        assert_eq!(d.rect.h, h);
        assert!(w <= w0);
        last = dropped;
    }
    assert_eq!(last, 4);
    // 4. None: no plate, the whole rect for the rest.
    let (_, h4) = loudness_plate::size(FONT, 1.0, 4);
    let tiny = Rect::new(0.0, 0.0, 300.0, h4 - 1.0);
    let l = dock(tiny, true, 1, sw(), FONT, 1.0);
    assert_eq!(l.plate, None);
    assert_eq!(l, layout(tiny, true, 1, sw()));
}

/// A plate the operator moved floats over the pane: the dock's room goes back to the bars,
/// and a double-click (the place reset) docks it again.
#[test]
fn a_moved_plate_floats_and_the_bars_reclaim_its_room() {
    let rig = Rig::new();
    let mut p = rig.pane(&[2, 3], Ballistic::ShortTerm);
    let docked = scene_of(&p, PlotChrome::Full, 900.0, 400.0);
    let docked_bars = bar_rects(&docked.scene);
    let docked_plate = docked.plate.expect("docked");
    assert_eq!(
        docked_bars.last().map(|b| b.right() < docked_plate.rect.x),
        Some(true)
    );
    p.plate.move_to(0.0, 0.0);
    let floating = scene_of(&p, PlotChrome::Full, 900.0, 400.0);
    let plate = floating.plate.expect("floating");
    let bars = bar_rects(&floating.scene);
    assert!(bars.last().expect("a bar").x > docked_bars.last().expect("a bar").x);
    // At its place in the pane's rect, which is also its box's area.
    let area = plate.area;
    assert_eq!(plate.rect.x, area.x + crate::legend::INSET);
    assert_eq!(plate.rect.y, area.y + crate::legend::INSET);
    assert_eq!(docked_plate.area, area, "one room, docked or not");
    assert_eq!(plate.position_for(plate.rect.x, plate.rect.y), [0.0, 0.0]);
    // Drawn over the bars: no label of theirs sits in the plate's layer or above it, so
    // none shows through its fill.
    let layers = &floating.scene.layers;
    let plate_layer = layers
        .iter()
        .position(|l| l.rects.iter().any(|r| r.rect == plate.rect))
        .expect("the plate's fill");
    assert!(
        layers[plate_layer..]
            .iter()
            .flat_map(|l| &l.labels)
            .all(|l| l.clip.is_some()),
        "only the plate's own (clipped) labels at or above its fill"
    );
    // A floating plate too large for the pane shrinks, then drops slots.
    p.plate.set_scale(3.0);
    let small = scene_of(&p, PlotChrome::Full, 900.0, 260.0)
        .plate
        .expect("floating");
    assert!(small.rect.h <= 260.0 - 20.0);
    // Back where the pane places it: docked again.
    p.plate = crate::plate::PlatePlace::default();
    assert_eq!(scene_of(&p, PlotChrome::Full, 900.0, 400.0), docked);
}

/// A pane with several sources, or none, docks nothing and reserves nothing.
#[test]
fn no_plate_no_reserve() {
    let rig = Rig::new();
    for shown in [&[2u16, 0][..], &[1][..], &[1, 7][..]] {
        let p = rig.pane(shown, Ballistic::ShortTerm);
        assert_eq!(p.loudness, None, "{shown:?}");
        let area = Rect::new(10.0, 10.0, 880.0, 380.0);
        let l = p.layout(area, FONT);
        assert_eq!(l.plate, None);
        assert_eq!(l.bars.right(), area.right(), "{shown:?}");
    }
}
