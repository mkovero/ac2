//! T past full on the Leq views: the state words and limit texts go, every colour, value
//! and other word stays.

use super::*;
use crate::view::PlotChrome;

/// The texts T past full leaves out: the three state words, what to stay under (in any
/// of its shortenings) and the limit.
fn status_words<'a>(texts: &[&'a str]) -> Vec<&'a str> {
    texts
        .iter()
        .copied()
        .filter(|t| {
            matches!(*t, "OK" | "NEAR" | "OVER")
                || t.contains("stay ≤")
                || t.starts_with("≤ ")
                || t.starts_with("limit ")
        })
        .collect()
}

fn with(v: &LeqView<'_>, style: LeqStyle, chrome: PlotChrome) -> LeqScene {
    let v = LeqView {
        layout: LeqLayout { style, ..v.layout },
        chrome,
        ..v.clone()
    };
    leq_scene(&v, &Status::default(), &Theme::dark(), size(1920.0, 1080.0))
}

#[test]
fn full_chrome_writes_the_words_and_limits() {
    assert_eq!(PlotChrome::default(), PlotChrome::Full);
    let (c, f) = many(8, false, LevelScale::DbSpl);
    let v = columns_view(&c, &f, None);
    for style in [LeqStyle::Columns, LeqStyle::Tiles] {
        let s = with(&v, style, PlotChrome::Full);
        let all = texts(&s.scene);
        let words = status_words(&all);
        for want in ["OVER", "NEAR", "OK"] {
            assert!(words.contains(&want), "{style:?}: {want} in {all:?}");
        }
        assert!(words.iter().any(|t| t.starts_with("limit ")), "{all:?}");
        assert!(
            words
                .iter()
                .any(|t| t.contains("stay ≤") || t.starts_with("≤ ")),
            "{all:?}"
        );
    }
}

#[test]
fn past_full_drops_the_words_and_keeps_the_colours() {
    let (c, f) = many(8, false, LevelScale::DbSpl);
    let v = columns_view(&c, &f, None);
    for (style, chrome) in [LeqStyle::Columns, LeqStyle::Tiles]
        .into_iter()
        .flat_map(|s| [(s, PlotChrome::NoGrid), (s, PlotChrome::Bare)])
    {
        let words = with(&v, style, PlotChrome::Full);
        let colours = with(&v, style, chrome);
        let all = texts(&colours.scene);
        assert_eq!(
            status_words(&all),
            Vec::<&str>::new(),
            "{style:?} {chrome:?}: {all:?}"
        );
        // The values, the names and every word that is not one of them stay: on course
        // and cooling down say what no colour does.
        // Traces only also takes the columns' scale numbers ([`t_steps_the_column_scale`]).
        let before = texts(&words.scene);
        let scale =
            |t: &str| style == LeqStyle::Columns && chrome == PlotChrome::Bare && is_scale(t);
        for t in &before {
            if status_words(&[t]).is_empty() && !scale(t) {
                assert!(all.contains(t), "{style:?} {chrome:?}: {t} kept in {all:?}");
            }
        }
        assert!(
            all.iter()
                .any(|t| t.starts_with("ON COURSE") || t.starts_with("over in")),
            "{style:?} {chrome:?}: {all:?}"
        );
        assert!(
            all.iter().any(|t| t.starts_with("cooling")),
            "{style:?} {chrome:?}: {all:?}"
        );
        for t in leq_tiles(&c, &f) {
            assert!(all.contains(&t.value.as_str()), "{}", t.value);
        }
        match style {
            // The same columns in the same colours, each limit line still drawn.
            LeqStyle::Columns => {
                let (a, b) = (cols(&words), cols(&colours));
                assert_eq!(a.columns.len(), b.columns.len());
                for (x, y) in a.columns.iter().zip(&b.columns) {
                    assert_eq!(x.window, y.window);
                    assert_eq!(x.background, y.background);
                    assert_eq!(x.bar_color, y.bar_color);
                    assert_eq!(x.track_color, y.track_color);
                    assert_eq!(x.bar_db.to_bits(), y.bar_db.to_bits());
                    assert_eq!(x.limit_y.is_some(), y.limit_y.is_some(), "{}", x.name);
                    assert_eq!(x.name, y.name);
                }
                assert!(b.columns.iter().all(|k| k.limit_y.is_some()));
            }
            // The same tiles in the same places and colours.
            LeqStyle::Tiles => {
                assert_eq!(words.tiles, colours.tiles);
                assert_eq!(words.scene.layers[0].rects, colours.scene.layers[0].rects);
            }
        }
    }
}

/// A number of the columns' dB scale: whole tens.
fn is_scale(t: &str) -> bool {
    t.parse::<i64>().is_ok_and(|v| v % 10 == 0)
}

/// T on the Leq columns, as on a graph: no grid drops the 10 dB lines across the tracks,
/// traces only the scale's numbers too; the scale keeps its room, so the columns, their
/// bars, colours, values and limit lines stay where they were.
#[test]
fn t_steps_the_column_scale() {
    let (c, f) = many(8, false, LevelScale::DbSpl);
    let v = columns_view(&c, &f, None);
    let full = with(&v, LeqStyle::Columns, PlotChrome::Full);
    let no_grid = with(&v, LeqStyle::Columns, PlotChrome::NoGrid);
    let bare = with(&v, LeqStyle::Columns, PlotChrome::Bare);
    assert!(cols(&full).gutter.is_some());
    let numbers =
        |s: &LeqScene| -> usize { texts(&s.scene).iter().filter(|t| is_scale(t)).count() };
    assert!(numbers(&full) > 0);
    assert_eq!(numbers(&no_grid), numbers(&full));
    assert_eq!(numbers(&bare), 0);
    // The grid: lines across each column's track, on the base layer.
    let lines = |s: &LeqScene| s.scene.layers[0].polylines.len();
    assert!(lines(&full) > 0);
    assert_eq!((lines(&no_grid), lines(&bare)), (0, 0));
    // Past full the limit row goes and the tracks grow, words and lines alike;
    // traces only then moves nothing.
    assert_eq!(cols(&full).gutter, cols(&no_grid).gutter);
    for l in 0..full.scene.layers.len() {
        assert_eq!(no_grid.scene.layers[l].rects, bare.scene.layers[l].rects);
        assert_eq!(
            no_grid.scene.layers[l].polylines,
            bare.scene.layers[l].polylines
        );
    }
    let (a, b) = (cols(&no_grid), cols(&bare));
    assert_eq!((a.gutter, a.range, a.large), (b.gutter, b.range, b.large));
    assert_eq!(a.columns.len(), b.columns.len());
    for (x, y) in a.columns.iter().zip(&b.columns) {
        assert_eq!(
            (x.rect, x.track, x.bar, x.limit_y, &x.value, &x.name),
            (y.rect, y.track, y.bar, y.limit_y, &y.value, &y.name)
        );
        assert_eq!(x.bar_db.to_bits(), y.bar_db.to_bits());
    }
}

/// Without a limit nothing is judged: the tiles say the same either way; uncalibrated, the
/// word that says so stays.
#[test]
fn past_full_keeps_what_no_colour_says() {
    let c = cfg();
    let f = frame(LevelScale::DbSpl);
    let v = columns_view(&c, &f, None);
    let t = leq_tiles(&c, &f);
    assert_eq!(t[0].state, TileState::NoLimit);
    assert_eq!(t[0].state_word(PlotChrome::NoGrid), None);
    assert_eq!(t[1].state_word(PlotChrome::NoGrid), None);
    assert_eq!(t[1].state_word(PlotChrome::Full), Some("NEAR"));
    let (c, f) = many(4, false, LevelScale::Dbfs);
    let t = leq_tiles(&c, &f);
    assert!(t.iter().all(|t| t.state == TileState::NotCalibrated));
    assert!(
        t.iter()
            .all(|t| t.state_word(PlotChrome::NoGrid) == Some("not calibrated"))
    );
    let v2 = columns_view(&c, &f, None);
    for style in [LeqStyle::Columns, LeqStyle::Tiles] {
        let s = with(&v2, style, PlotChrome::NoGrid);
        let all = texts(&s.scene);
        assert!(
            all.iter().any(|t| *t == "not calibrated" || *t == "uncal."),
            "{style:?}: {all:?}"
        );
        assert_eq!(status_words(&all), Vec::<&str>::new(), "{style:?}: {all:?}");
    }
    // The meter's caption and the windows' values are untouched.
    let s = with(&v, LeqStyle::Tiles, PlotChrome::NoGrid);
    let all = texts(&s.scene);
    for want in ["97.8", "1 min", "so far · 12:30 / 30:00"] {
        assert!(all.contains(&want), "{want} in {all:?}");
    }
}
