//! The Meters view's blocks and floor mark, and the Goniometer view
//! (docs/design/programme-meter.md §6.3).

use ac2_proto::units::Lufs;

use super::super::loudness::NamedPair;
use super::super::*;
use super::bars::{Rig, bar_rects, scene, scene_of};
use super::texts;
use crate::primitives::Rect;
use crate::theme::Theme;
use crate::view::{PlotChrome, ProgrammeMode};

fn names(p: &ProgrammePane) -> Vec<String> {
    p.bars
        .iter()
        .map(|b| match b {
            ProgrammeBar::Peak { input, note } => match note {
                Some(n) => format!("{} ({n})", input.label),
                None => input.label.clone(),
            },
            ProgrammeBar::Source(s) => format!("| {}", s.name),
        })
        .collect()
}

/// The rig with inputs 2 and 8 (both mics) declared a pair, Overheads.
fn mic_rig() -> Rig {
    let mut rig = Rig::new();
    rig.pairs.retain(|p| p.left != 6);
    rig.pairs.push(NamedPair {
        left: 1,
        right: 7,
        name: "Overheads".into(),
    });
    rig
}

const MIC: &str = "true peak shown · no loudness on a mic input";

#[test]
fn the_true_peak_block_then_the_lufs_block() {
    let rig = Rig::new();
    for b in [Ballistic::ShortTerm, Ballistic::Momentary] {
        assert_eq!(names(&rig.pane(&[2, 3], b)), ["In 3", "In 4", "| Main"]);
        assert_eq!(
            names(&rig.pane(&[2, 0, 3], b)),
            ["In 3", "In 1", "In 4", "| Main", "| In 1 mono"]
        );
        // `1, 5` with 2 the pair of 1: the unnamed side has no bar of its own.
        assert_eq!(
            names(&rig.pane(&[2, 0], b)),
            ["In 3", "In 1", "| Main", "| In 1 mono"]
        );
        assert_eq!(
            names(&mic_rig().pane(&[1, 7], b)),
            [format!("In 2 ({MIC})"), format!("In 8 ({MIC})")]
        );
        assert_eq!(names(&rig.pane(&[1], b)), [format!("In 2 ({MIC})")]);
    }
    // Under a peak ballistic nothing changes: one bar per named input.
    assert_eq!(
        names(&rig.pane(&[2, 0, 3], Ballistic::TruePeak)),
        ["In 3", "In 1", "In 4"]
    );
}

/// A mic pair: true-peak bars only, no LUFS column and no plate, the pair's chip.
#[test]
fn a_mic_pair_reads_true_peak_and_correlation() {
    let p = mic_rig().pane(&[1, 7], Ballistic::ShortTerm);
    assert_eq!(p.loudness, None);
    assert!(p.pair.is_some());
    assert_eq!(p.title(), "true peak · Overheads · 1 s");
    let s = scene_of(&p, PlotChrome::Full, 900.0, 500.0);
    assert_eq!(s.plate, None);
    let t = texts(&s.scene);
    assert!(
        !t.iter().any(|x| x == "LUFS" || x.contains("target")),
        "{t:?}"
    );
    assert!(t.iter().any(|x| x == "dBTP"), "{t:?}");
    assert!(t.iter().any(|x| x == "+0.50"), "the chip: {t:?}");
}

/// Each column has its unit at its head under a loudness ballistic: `dBTP` while every
/// true-peak bar is digital, `LUFS`; both go with the numbers at *traces only*.
#[test]
fn the_column_heads_name_their_units() {
    let rig = Rig::new();
    let p = rig.pane(&[2, 3], Ballistic::ShortTerm);
    let heads = |p: &ProgrammePane, chrome| {
        scene(p, chrome, 900.0, 500.0)
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .filter(|l| l.pos[1] == 10.0 && (l.text == "dBTP" || l.text == "LUFS"))
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(heads(&p, PlotChrome::Full), ["dBTP", "LUFS"]);
    assert_eq!(heads(&p, PlotChrome::NoGrid), ["dBTP", "LUFS"]);
    assert!(heads(&p, PlotChrome::Bare).is_empty());
    let mut cal = p.clone();
    for b in &mut cal.bars {
        if let ProgrammeBar::Peak { input, .. } = b {
            input.scale = Scale::Dbu { fs_dbu: 13.0 };
        }
    }
    assert_eq!(
        heads(&cal, PlotChrome::Full),
        ["LUFS"],
        "a calibrated bar's own text says its unit"
    );
    assert!(heads(&rig.pane(&[2, 3], Ballistic::TruePeak), PlotChrome::Full).is_empty());
}

fn floor_pane(reading: f64, target: f64) -> ProgrammePane {
    let mut rig = Rig::new();
    rig.sources[0].short_term = Lufs(reading);
    let mut p = rig.pane(&[2, 3], Ballistic::ShortTerm);
    p.loudness_target = target;
    p
}

fn floor_marks(p: &ProgrammePane) -> (Vec<String>, usize) {
    let s = scene(p, PlotChrome::Full, 900.0, 500.0);
    let words = texts(&s)
        .into_iter()
        .filter(|t| t.starts_with("below"))
        .collect();
    let dim = Theme::dark().text_dim;
    let marks = s.layers[1].bands.iter().filter(|b| b.color == dim).count();
    (words, marks)
}

/// A reading under the floor says so in its own row and with a mark at the bar's foot;
/// every bar keeps its rect whatever the reading.
#[test]
fn a_reading_below_the_floor_is_marked() {
    assert_eq!(
        floor_marks(&floor_pane(-56.5, -23.0)),
        (vec!["below −41".into()], 1)
    );
    assert_eq!(floor_marks(&floor_pane(-40.9, -23.0)), (vec![], 0));
    assert_eq!(
        floor_marks(&floor_pane(f64::NEG_INFINITY, -23.0)),
        (vec![], 0)
    );
    assert_eq!(
        floor_marks(&floor_pane(-56.5, -22.0)),
        (vec!["below −40".into()], 1)
    );
    assert_eq!(floor_text(-23.0), "below −41");
    let rects = |p: &ProgrammePane| bar_rects(&scene(p, PlotChrome::Full, 900.0, 500.0));
    let at = rects(&floor_pane(-23.0, -23.0));
    for (r, t) in [
        (-56.5, -23.0),
        (-40.9, -23.0),
        (f64::NEG_INFINITY, -23.0),
        (-56.5, -22.0),
    ] {
        assert_eq!(rects(&floor_pane(r, t)), at, "{r} {t}");
    }
}

/// At *traces only* no plate box, and the bars reach the pane's right edge.
#[test]
fn traces_only_gives_the_plate_room_to_the_bars() {
    let theme = Theme::dark();
    let p = Rig::new().pane(&[2, 3], Ballistic::ShortTerm);
    let area = Rect::new(10.0, 10.0, 1680.0, 880.0);
    let l = p.layout(
        area,
        theme.font_size,
        theme.small_font_size,
        PlotChrome::Bare,
    );
    assert_eq!(l.plate, None);
    assert_eq!(l.bars.right(), area.right());
    assert_eq!(scene_of(&p, PlotChrome::Bare, 1700.0, 900.0).plate, None);
}

fn gonio(shown: &[u16]) -> ProgrammePane {
    let mut p = Rig::new().pane(shown, Ballistic::ShortTerm);
    p.view = ProgrammeMode::Goniometer;
    p
}

/// The Goniometer view: the largest square centred in the pane less the chip, the chip
/// under it as wide as the square or its text, no plate; below the least square the chip
/// alone; a stored Goniometer on a pane without a pair draws Meters.
#[test]
fn the_goniometer_fills_its_pane() {
    let theme = Theme::dark();
    let (fs, small) = (theme.font_size, theme.small_font_size);
    let p = gonio(&[2, 3]);
    assert_eq!(p.shown_view(), ProgrammeMode::Goniometer);
    for (w, h) in [(900.0, 900.0), (1600.0, 600.0), (400.0, 178.0)] {
        let area = Rect::new(10.0, 10.0, w - 20.0, h - 20.0);
        let l = p.layout(area, fs, small, PlotChrome::Full);
        let sq = l.gonio.expect("a square");
        let side = area.w.min(area.h - CHIP_H);
        assert_eq!(sq.w, side);
        assert_eq!(sq.h, side);
        assert!(
            (sq.x + sq.w / 2.0 - (area.x + area.w / 2.0)).abs() < 1e-3,
            "centred"
        );
        let chip = l.correlation.expect("the chip");
        assert_eq!(chip.y, sq.bottom());
        assert_eq!(chip.w, sq.w.max(chip_min_w(fs, small)));
        assert!((chip.x + chip.w / 2.0 - (sq.x + sq.w / 2.0)).abs() < 1e-3);
        assert_eq!(l.plate, None);
        let s = scene_of(&p, PlotChrome::Full, w, h);
        assert_eq!(s.gonio, Some(sq));
        assert_eq!(s.plate, None);
        assert!(s.plates().is_empty());
        assert!(bar_rects(&s.scene).is_empty(), "no bars");
    }
    // Too small for a figure: the chip alone, centred.
    let area = Rect::new(0.0, 0.0, 400.0, CHIP_H + GONIO_MIN_W - 1.0);
    let l = p.layout(area, fs, small, PlotChrome::Full);
    assert_eq!(l.gonio, None);
    let chip = l.correlation.expect("the chip");
    assert_eq!(chip.w, chip_min_w(fs, small));
    assert!((chip.x + chip.w / 2.0 - 200.0).abs() < 1e-3);
    // The chip stays at every T step; the grid at *full* only, L / R at *no grid*.
    for chrome in [PlotChrome::Full, PlotChrome::NoGrid, PlotChrome::Bare] {
        let t = texts(&scene(&p, chrome, 900.0, 900.0));
        assert!(t.iter().any(|x| x == "+0.50"), "{chrome:?} {t:?}");
        assert_eq!(t.iter().any(|x| x == "L"), chrome != PlotChrome::Bare);
    }
    // Stored on a pane that meters no pair: the Meters view, the value kept.
    let alone = gonio(&[0]);
    assert_eq!(alone.view, ProgrammeMode::Goniometer);
    assert_eq!(alone.shown_view(), ProgrammeMode::Meters);
    let s = scene_of(&alone, PlotChrome::Full, 900.0, 500.0);
    assert_eq!(s.gonio, None);
    assert_eq!(bar_rects(&s.scene).len(), 2);
}

#[test]
fn titles_name_the_view() {
    let rig = Rig::new();
    let p = rig.pane(&[2, 3], Ballistic::ShortTerm);
    assert_eq!(p.title(), "Short loudness · target −23 LUFS · Main · 1 s");
    assert_eq!(gonio(&[2, 3]).title(), "goniometer · Main · 1 s");
    assert_eq!(
        rig.pane(&[0], Ballistic::Momentary).title(),
        "Mom loudness · target −23 LUFS"
    );
    assert_eq!(
        gonio(&[0]).title(),
        "Short loudness · target −23 LUFS",
        "drawn as Meters"
    );
    assert_eq!(
        PlotChrome::Bare.programme_name(),
        "no grid, target label, scale or loudness plate"
    );
}

/// G steps the views on a pane that meters a pair and leaves the stored value elsewhere.
#[test]
fn g_steps_only_with_a_pair() {
    use ProgrammeMode as M;
    assert_eq!(M::Meters.next(true), M::Goniometer);
    assert_eq!(M::Goniometer.next(true), M::Meters);
    assert_eq!(M::Meters.prev(true), M::Goniometer);
    assert_eq!(M::Meters.next(false), M::Meters);
    assert_eq!(M::Goniometer.prev(false), M::Goniometer);
    assert_eq!(M::default(), M::Meters);
}
