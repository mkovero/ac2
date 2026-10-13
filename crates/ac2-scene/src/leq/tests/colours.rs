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
        let before = texts(&words.scene);
        for t in &before {
            if status_words(&[t]).is_empty() {
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
