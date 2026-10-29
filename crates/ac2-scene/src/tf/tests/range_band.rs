//! An average's range across positions: shaded behind the magnitude only, its legend tag,
//! the setting and T (plot chrome) taking the shade away.

use super::*;
use ac2_proto::model::{RangeBand, RangeSpread};

fn scene(c: &Cols, src: Option<crate::range_band::Source<'_>>, v: &ViewState) -> TfScene {
    let mut t = trace(c, TraceKey::Live(MeasId(1)), 0.0);
    t.range = src;
    transfer_scene(
        &[t],
        &DisplayCache::default(),
        &Status::default(),
        v,
        &Theme::dark(),
        SIZE,
    )
}

fn shade(s: &TfScene) -> usize {
    let theme = Theme::dark();
    s.scene
        .layers
        .iter()
        .flat_map(|l| &l.bands)
        .filter(|b| b.color == theme.unresolved_shade)
        .count()
}

#[test]
fn band_legend_and_toggles() {
    let c = cols(48);
    let band = RangeBand {
        lo: vec![-2.0; 48],
        hi: vec![3.0; 48],
        coh: vec![0.9; 48],
    };
    let src = crate::range_band::Source {
        spread: RangeSpread::of(4),
        band: Some(&band),
    };
    let v = ViewState::default();
    let s = scene(&c, Some(src), &v);
    // One band, in the magnitude pane only.
    assert_eq!(shade(&s), 1);
    assert_eq!(
        s.legend[0].tags.last().map(String::as_str),
        Some("range of 4 positions")
    );
    assert!(s.legend[0].text.ends_with(" · range of 4 positions"));

    let few = crate::range_band::Source {
        spread: RangeSpread::of(2),
        band: None,
    };
    let s = scene(&c, Some(few), &v);
    assert_eq!(shade(&s), 0);
    assert_eq!(
        s.legend[0].tags.last().map(String::as_str),
        Some("range: too few positions")
    );

    let off = ViewState {
        range_band: false,
        ..v
    };
    let s = scene(&c, Some(src), &off);
    assert_eq!(shade(&s), 0);
    assert!(!s.legend[0].text.contains("range"));

    // T: the shade goes with the grid; the legend still says what the curve is.
    let bare = ViewState {
        chrome: crate::view::PlotChrome::Bare,
        ..v
    };
    assert_eq!(shade(&scene(&c, Some(src), &bare)), 0);

    // No range: an ordinary curve's legend says nothing about one.
    let s = scene(&c, None, &v);
    assert!(!s.legend[0].text.contains("range"));
}
