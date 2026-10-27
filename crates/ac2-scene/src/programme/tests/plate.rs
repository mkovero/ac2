//! The loudness plate: its cells and states, its static geometry, its fill (docs/design/
//! programme-meter.md §6.2 *The loudness plate*, *Static*, *Drawing*).

use super::super::loudness::LoudnessReadout;
use super::super::loudness_plate::{self, Field, Slot};
use super::super::*;
use super::bars::{Rig, scene_of};
use crate::plate::{PlateHover, PlateKind};
use crate::primitives::{Label, Scene};
use crate::theme::Theme;
use crate::view::PlotChrome;

/// The plate's layer: the last, above the bars' labels.
const PLATE_LAYER: usize = 3;

fn settled() -> LoudnessReadout {
    LoudnessReadout {
        title: "Main".into(),
        momentary: -23.06,
        short_term: -22.84,
        integrated: Some(-23.0),
        range: Some(6.21),
        duration_s: 75.0,
        max_true_peak: -1.2,
        max_momentary: -18.0,
        max_short_term: -20.1,
        since: "since 20:14 · R resets".into(),
    }
}

fn texts_of(r: &LoudnessReadout) -> Vec<(Field, String)> {
    let (_, cells) = loudness_plate::cells([0.0, 0.0], 11.0, 1.0, 0);
    cells
        .iter()
        .map(|c| (c.field, loudness_plate::text(r, c.field)))
        .collect()
}

fn text(r: &LoudnessReadout, f: Field) -> String {
    loudness_plate::text(r, f)
}

#[test]
fn plate_cells_read_one_decimal_on_the_absolute_scale() {
    let r = settled();
    use Field as F;
    for (f, want) in [
        (F::Title, "Main"),
        (F::IntCaption, "Int"),
        (F::Int, "−23.0"),
        (F::IntUnit, "LUFS"),
        (F::IntStatus, ""),
        (F::MomCaption, "Mom"),
        (F::Mom, "−23.1"),
        (F::ShortCaption, "Short"),
        (F::Short, "−22.8"),
        (F::MaxMomCaption, "max Mom"),
        (F::MaxMom, "−18.0"),
        (F::MaxShortCaption, "max Short"),
        (F::MaxShort, "−20.1"),
        (F::LraCaption, "LRA"),
        (F::Lra, "6.2"),
        (F::LraUnit, "LU"),
        (F::LraStatus, ""),
        (F::TpCaption, "TP max"),
        (F::Tp, "−1.2"),
        (F::TpUnit, "dBTP"),
        (F::Foot, "since 20:14 · R resets"),
    ] {
        assert_eq!(text(&r, f), want, "{f:?}");
    }
    // Every field is in a cell, once, top down in the slots' order.
    let fields: Vec<Field> = texts_of(&r).into_iter().map(|(f, _)| f).collect();
    assert_eq!(fields.len(), 26);
    assert_eq!(fields.first(), Some(&F::Title));
    assert_eq!(fields.last(), Some(&F::Foot));
}

#[test]
fn plate_states_waiting_settling_and_reset() {
    use Field as F;
    let mut r = settled();
    r.duration_s = 10.0;
    assert_eq!(text(&r, F::LraStatus), "settling");
    assert_eq!(text(&r, F::Lra), "6.2");
    // Just reset: Int — waiting, the maxima —, LRA — waiting, TP max —; Mom and Short carry
    // on; the since line the new time.
    let reset = LoudnessReadout {
        integrated: None,
        range: None,
        duration_s: 0.0,
        max_true_peak: f64::NEG_INFINITY,
        max_momentary: f64::NEG_INFINITY,
        max_short_term: f64::NEG_INFINITY,
        since: "since 20:30 · R resets".into(),
        ..settled()
    };
    for (f, want) in [
        (F::Int, "—"),
        (F::IntStatus, "waiting for programme"),
        (F::MaxMom, "—"),
        (F::MaxShort, "—"),
        (F::Lra, "—"),
        (F::LraStatus, "waiting"),
        (F::Tp, "—"),
        (F::Mom, "−23.1"),
        (F::Short, "−22.8"),
        (F::Foot, "since 20:30 · R resets"),
    ] {
        assert_eq!(text(&reset, f), want, "{f:?}");
    }
    // A source the frame does not carry yet: — everywhere, a blank foot.
    let empty = LoudnessReadout::empty("Main".into());
    for f in [
        F::Int,
        F::Mom,
        F::Short,
        F::MaxMom,
        F::MaxShort,
        F::Lra,
        F::Tp,
    ] {
        assert_eq!(text(&empty, f), "—", "{f:?}");
    }
    assert_eq!(text(&empty, F::Foot), "");
}

/// The plate's labels: the clipped ones inside its rect (the bars' are not clipped).
fn plate_labels(s: &Scene, rect: crate::primitives::Rect) -> Vec<Label> {
    s.layers[PLATE_LAYER]
        .labels
        .iter()
        .filter(|l| {
            l.clip.is_some()
                && (rect.x..=rect.right()).contains(&l.pos[0])
                && (rect.y..=rect.bottom()).contains(&l.pos[1])
        })
        .cloned()
        .collect()
}

/// Nothing moves with a value: the plate's rect and every label's anchor are the same for
/// any reading, waiting or settled.
#[test]
fn plate_slots_are_static_across_values() {
    let rig = Rig::new();
    let base = rig.pane(&[2, 3], Ballistic::ShortTerm);
    let draw = |r: LoudnessReadout| {
        let mut p = base.clone();
        p.loudness = Some(r);
        let s = scene_of(&p, PlotChrome::Full, 900.0, 400.0);
        let plate = s.plate.expect("a docked plate");
        let anchors: Vec<([u32; 2], f32)> = plate_labels(&s.scene, plate.rect)
            .iter()
            .map(|l| (l.pos.map(f32::to_bits), l.size))
            .collect();
        (plate.rect, anchors)
    };
    let (rect, all) = draw(LoudnessReadout {
        range: Some(1.0),
        duration_s: 1.0,
        integrated: None,
        ..settled()
    });
    for v in [f64::NEG_INFINITY, -150.7, -70.0, -5.0, 3.2] {
        for (integrated, range, duration_s) in [
            (Some(v), Some(v), 75.0),
            (None, None, 0.0),
            (Some(v), Some(v), 10.0),
        ] {
            let r = LoudnessReadout {
                momentary: v,
                short_term: v,
                integrated,
                range,
                duration_s,
                max_true_peak: v,
                max_momentary: v,
                max_short_term: v,
                ..settled()
            };
            let (got_rect, got) = draw(r);
            assert_eq!(got_rect, rect, "{v}");
            for a in &got {
                assert!(all.contains(a), "{a:?} moved at {v}");
            }
        }
    }
    // A long pair name is cut, never widening the plate.
    let mut long = settled();
    long.title = "A very long pair name indeed, longer than any plate".into();
    assert_eq!(draw(long).0, rect);
}

/// Values and the bars' readings are drawn with tabular digits; captions are not.
#[test]
fn values_are_tabular() {
    let rig = Rig::new();
    let p = rig.pane(&[2, 3], Ballistic::ShortTerm);
    let s = scene_of(&p, PlotChrome::Full, 900.0, 400.0);
    let find = |t: &str| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == t)
            .unwrap_or_else(|| panic!("{t}"))
    };
    for t in ["−23.0", "−22.8", "−1.2", "−22.8 LUFS", "−21.0 LUFS"] {
        assert!(find(t).tabular, "{t}");
    }
    for t in ["Int", "LUFS", "Main", "max", "TP max"] {
        assert!(!find(t).tabular, "{t}");
    }
}

/// The plate's fill is the plates' own: the theme's plate background, opaque on hover,
/// with the grip.
#[test]
fn plate_fill_honours_plate_alpha_and_hover() {
    let rig = Rig::new();
    let mut p = rig.pane(&[2, 3], Ballistic::ShortTerm);
    for alpha in [crate::plate::SEE_THROUGH_ALPHA, 1.0] {
        let mut theme = Theme::dark();
        theme.plate_alpha = alpha;
        for hovered in [false, true] {
            p.plate_hover = hovered.then_some(PlateHover {
                kind: PlateKind::Loudness,
                grip: false,
            });
            let s = programme_scene(
                &p,
                crate::view::Style {
                    chrome: PlotChrome::Full,
                    theme: &theme,
                },
                crate::primitives::Viewport {
                    width: 900.0,
                    height: 400.0,
                },
                None,
            );
            let plate = s.plate.expect("a plate");
            let fill = s.scene.layers[PLATE_LAYER]
                .rects
                .iter()
                .find(|r| r.rect == plate.rect)
                .expect("the plate's fill");
            let want = if hovered { 1.0 } else { alpha };
            assert_eq!(fill.color, theme.plot_background.with_alpha(want));
            assert_eq!(plate.kind, PlateKind::Loudness);
            assert_eq!(s.plates(), vec![plate]);
            // The grip's three dots inside the corner.
            let dots = s.scene.layers[PLATE_LAYER]
                .rects
                .iter()
                .filter(|r| r.rect.w == 2.0 && r.rect.h == 2.0 && plate.grip.x <= r.rect.x)
                .count();
            assert_eq!(dots, 3);
        }
    }
}

/// At *traces only* the plate goes, wherever it sits, and cannot be grabbed; a docked
/// one's room goes to the bars, whose right edge is then the pane's.
#[test]
fn t_hides_the_plate_and_gives_its_room_to_the_bars() {
    let rig = Rig::new();
    let mut p = rig.pane(&[2, 3], Ballistic::ShortTerm);
    let full = scene_of(&p, PlotChrome::Full, 900.0, 400.0);
    assert!(full.plate.is_some());
    assert!(
        scene_of(&p, PlotChrome::NoGrid, 900.0, 400.0)
            .plate
            .is_some()
    );
    let bare = scene_of(&p, PlotChrome::Bare, 900.0, 400.0);
    assert_eq!(bare.plate, None);
    assert!(bare.plates().is_empty());
    assert!(!super::texts(&bare.scene).iter().any(|t| t == "Int"));
    assert_eq!(
        super::bars::bar_rects(&bare.scene),
        super::bars::bar_rects(&scene_of(&p_floating(&p), PlotChrome::Bare, 900.0, 400.0).scene),
        "laid out as with no docked plate"
    );
    p.plate.move_to(0.0, 0.0);
    assert_eq!(scene_of(&p, PlotChrome::Bare, 900.0, 400.0).plate, None);
    assert_eq!(
        PlotChrome::Full.programme_name(),
        "grid, target label, scale and loudness plate"
    );
    assert_eq!(
        PlotChrome::NoGrid.programme_name(),
        "no grid or target label"
    );
    assert_eq!(
        PlotChrome::Bare.programme_name(),
        "no grid, target label, scale or loudness plate"
    );
}

fn p_floating(p: &ProgrammePane) -> ProgrammePane {
    let mut q = p.clone();
    q.plate.move_to(0.5, 0.5);
    q
}

#[test]
fn plate_kind_loudness() {
    assert_eq!(PlateKind::ALL.len(), 5);
    assert_eq!(PlateKind::ALL[4], PlateKind::Loudness);
    assert_eq!(PlateKind::Loudness.name(), "loudness readout");
    let mut plates = crate::plate::Plates::default();
    plates.get_mut(PlateKind::Loudness).set_scale(2.0);
    assert_eq!(plates.loudness.scale, 2.0);
    assert!(!plates.is_auto());
    assert!(
        crate::plate::hint(PlateHover {
            kind: PlateKind::Loudness,
            grip: false
        })
        .starts_with("drag: move the loudness readout")
    );
}

/// Slots' sizes and the drop order: foot, maxima, range and peak, live; title, hero and
/// hero status never drop.
#[test]
fn slots_and_drop_order() {
    assert_eq!(Slot::Hero.size(), 2.5);
    assert_eq!(Slot::Live.size(), 1.5);
    assert_eq!(Slot::Foot.size(), 0.8);
    let shown = |d| -> Vec<Slot> { Slot::ALL.into_iter().filter(|s| s.shown(d)).collect() };
    assert_eq!(shown(0), Slot::ALL);
    assert!(!shown(1).contains(&Slot::Foot));
    assert!(!shown(2).contains(&Slot::Maxima));
    assert!(!shown(3).contains(&Slot::RangePeak));
    assert_eq!(shown(4), [Slot::Title, Slot::Hero, Slot::HeroStatus]);
    // Each step leaves the plate no taller, and the size is the cells' own.
    let mut last = f32::INFINITY;
    for d in 0..=loudness_plate::MAX_DROPPED {
        let (w, h) = loudness_plate::size(11.0, 1.0, d);
        let (r, _) = loudness_plate::cells([5.0, 7.0], 11.0, 1.0, d);
        assert_eq!((r.w, r.h), (w, h));
        assert!(h < last);
        last = h;
    }
    // Row heights are 1.3 × each slot's size, the pads the plates' own.
    let f = 11.0;
    let rows: f32 = Slot::ALL.iter().map(|s| 1.3 * s.size() * f).sum();
    let (_, h) = loudness_plate::size(f, 1.0, 0);
    assert!(
        (h - rows - 2.0 * crate::legend::PAD_Y).abs() < 1e-3,
        "{h} {rows}"
    );
    // Twice the scale, twice the size.
    let (w1, h1) = loudness_plate::size(f, 1.0, 0);
    let (w2, h2) = loudness_plate::size(f, 2.0, 0);
    assert!((w2 - 2.0 * w1).abs() < 1e-3 && (h2 - 2.0 * h1).abs() < 1e-3);
}

/// A value cell is the template's width: the widest value fits right-aligned without
/// touching its caption.
#[test]
fn value_cells_hold_the_template() {
    let (_, cells) = loudness_plate::cells([0.0, 0.0], 11.0, 3.0, 0);
    let cell = |f: Field| {
        cells
            .iter()
            .find(|c| c.field == f)
            .copied()
            .expect("a cell")
    };
    let int = cell(Field::Int);
    let caption = cell(Field::IntCaption);
    let caption_right = caption.pos[0] + crate::canvas::text_width("Int", caption.size);
    let value_left = int.pos[0] - crate::canvas::text_width("−188.8", int.size);
    assert!(value_left > caption_right, "{value_left} {caption_right}");
    assert!(cell(Field::IntUnit).pos[0] > int.pos[0]);
}
