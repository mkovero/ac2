//! The dock ladder: beside, above, fewer slots, none; a moved plate floats (docs/design/
//! programme-meter.md §6.2 *Where it sits*).

use super::super::loudness_plate;
use super::super::*;
use super::bars::{Rig, bar_rects, scene_of};
use crate::primitives::Rect;
use crate::view::PlotChrome;

const FONT: f32 = 11.0;

fn plate_of(l: &ProgrammeLayout) -> Docked {
    l.plate.expect("a docked plate")
}

/// The fit scale beside the bars, else stage 4's ladder from 1×: above, then fewer slots,
/// then none.
#[test]
fn the_dock_fits_the_plate_to_its_room() {
    let (w1, h1) = loudness_plate::size(FONT, 1.0, 0);
    let (w3, h3) = loudness_plate::size(FONT, 3.0, 0);
    let need = 300.0;
    // Beside at 3×, the cap, in a pane with more room.
    let wide = Rect::new(0.0, 0.0, need + w3 + 200.0, h3 + 200.0);
    let l = dock(wide, false, need, FONT);
    let d = plate_of(&l);
    assert_eq!((d.scale, d.dropped), (FIT_SCALE_MAX, 0));
    assert_eq!(d.rect, Rect::new(wide.right() - w3, 0.0, w3, h3));
    assert_eq!(l.bars.right(), d.rect.x, "the bars give the plate its room");
    // Between: the largest 0.05 step that fits beside.
    let mid = Rect::new(0.0, 0.0, need + 1.5 * w1 + 3.0, 600.0);
    let d = plate_of(&dock(mid, false, need, FONT));
    assert!(d.scale > 1.0 && d.scale < 3.0, "{}", d.scale);
    let steps = d.scale / FIT_SCALE_STEP;
    assert!(
        (steps - steps.round()).abs() < 1e-4,
        "floored to a step: {}",
        d.scale
    );
    assert!(d.rect.w <= mid.w - need);
    let next = loudness_plate::size(FONT, d.scale + FIT_SCALE_STEP, 0);
    assert!(next.0 > mid.w - need, "the next step would not fit");
    // A pair's strip comes off the bars' height first: the plate fits above it.
    let l = dock(mid, true, need, FONT);
    let d = plate_of(&l);
    assert!(d.rect.bottom() <= mid.h - CHIP_H);
    assert_eq!(
        l.correlation,
        Some(Rect::new(0.0, mid.h - CHIP_H, l.bars.w, CHIP_H))
    );
    // Above at 1× when there is no room beside but height to spare.
    let tall = Rect::new(0.0, 0.0, w1 + 1.0, h1 + BAR_MIN_H + 1.0);
    let l = dock(tall, false, need, FONT);
    let d = plate_of(&l);
    assert_eq!((d.scale, d.dropped), (1.0, 0));
    assert_eq!(d.rect.y, 0.0);
    assert_eq!(l.bars.y, h1, "the bars below the reserve");
    assert_eq!(l.bars.h, tall.h - h1);
    // Fewer slots at 1×, in the drop order.
    let mut last = 0;
    for dropped in 1..=loudness_plate::MAX_DROPPED {
        let (w, h) = loudness_plate::size(FONT, 1.0, dropped);
        let r = Rect::new(0.0, 0.0, need + w1 + 1.0, h + 0.5);
        let d = plate_of(&dock(r, false, need, FONT));
        assert_eq!(d.dropped, dropped);
        assert_eq!(d.rect.h, h);
        assert!(w <= w1);
        last = dropped;
    }
    assert_eq!(last, 4);
    // None: no plate, the whole rect for the bars.
    let (_, h4) = loudness_plate::size(FONT, 1.0, 4);
    let tiny = Rect::new(0.0, 0.0, 300.0, h4 - 1.0);
    let l = dock(tiny, true, need, FONT);
    assert_eq!(l.plate, None);
    assert_eq!(l, layout(tiny, true));
}

fn pane_layout(w: f32, h: f32) -> ProgrammeLayout {
    let theme = crate::theme::Theme::dark();
    Rig::new().pane(&[2, 3], Ballistic::ShortTerm).layout(
        Rect::new(10.0, 10.0, w - 20.0, h - 20.0),
        theme.font_size,
        theme.small_font_size,
        PlotChrome::Full,
    )
}

/// A pair's Meters pane: 3× at 1700 × 900; between 1× and 3× in half a 1920 px window;
/// above at 1× in a third of it.
#[test]
fn the_plate_size_follows_the_pane() {
    let big = plate_of(&pane_layout(1700.0, 900.0));
    assert_eq!(big.scale, 3.0);
    assert!(big.rect.w > 800.0 && big.rect.h > 300.0, "{:?}", big.rect);
    let half = pane_layout(840.0, 900.0);
    let d = plate_of(&half);
    assert!(d.scale > 1.0 && d.scale < 3.0, "{}", d.scale);
    assert_eq!(d.rect.right(), 830.0, "beside, at the right edge");
    let third = pane_layout(620.0, 900.0);
    let d = plate_of(&third);
    assert_eq!((d.scale, d.dropped), (1.0, 0));
    assert!(third.bars.y >= d.rect.bottom(), "above the bars");
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
    assert!(docked_plate.docked);
    assert_eq!(
        docked_bars.last().map(|b| b.right() < docked_plate.rect.x),
        Some(true)
    );
    p.plate.move_to(0.0, 0.0);
    let floating = scene_of(&p, PlotChrome::Full, 900.0, 400.0);
    let plate = floating.plate.expect("floating");
    assert!(!plate.docked);
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
        let l = p.layout(area, 12.0, FONT, PlotChrome::Full);
        assert_eq!(l.plate, None);
        assert_eq!(l.bars.right(), area.right(), "{shown:?}");
    }
}
