//! The 3D spectrograph's lake mirror: a spectrum compared into the pane's measurement (C)
//! hangs below the floor; without one the view draws its measurement alone.

use super::follow::{Clock, sp};
use super::*;
use crate::state::{Pick, With};
use ac2_proto::model::layout::EntryRef;

/// What the pane's 3D view draws: whether its skyline carries a mirror, and the legend.
fn drawn_3d(t: &T, p: PaneId, sky: &mut crate::scenes::PaneSkylines) -> (bool, Vec<String>) {
    let s = crate::scenes::spectrograph_3d(
        &t.st,
        p,
        sky,
        (ac2_scene::primitives::SkylineTier::Full, false),
        (0.0, 0.0),
        &Theme::dark(),
        Viewport::new(800.0, 500.0),
        now(),
    );
    let mirrored = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.skylines)
        .next()
        .expect("a skyline")
        .mirror
        .is_some();
    (mirrored, s.legend_shown)
}

#[test]
fn a_compared_spectrum_hangs_below_the_floor_until_the_compare_is_cleared() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(4, "Sub 2", spectrum()));
    t.conn(mirror(s));
    let p = sp(&t);
    t.drop_meas(p, MeasId(2));
    if let Some(v) = t.st.layout.view_mut(p) {
        v.modes.spectrum = ac2_scene::view::SpectrumMode::Spectrograph3d;
    }
    // The mirrored channel the louder: the level axis must rise for it.
    let both = [(2, -30.0), (4, 0.0)];
    let mut c = Clock::new();
    c.run(&mut t, 2.0, &both);
    let mut sky = crate::scenes::PaneSkylines::default();
    assert_eq!(t.st.lake_mirror(p).map(|m| m.id), None, "nothing compared");
    assert_eq!(drawn_3d(&t, p, &mut sky), (false, vec!["Sub".to_owned()]));
    assert!(sky.mirror.is_none());

    let pick = Pick::here(EntryRef::Group { meas: MeasId(4) });
    t.st.layout.view_mut(p).expect("the pane").with = With::of(&[pick]);
    c.run(&mut t, 5.0, &both);
    assert_eq!(t.st.lake_mirror(p).map(|m| m.id), Some(MeasId(4)));
    assert_eq!(
        drawn_3d(&t, p, &mut sky),
        (
            true,
            vec!["Sub".to_owned(), "Sub 2 · cmp · mirrored".to_owned()]
        )
    );
    assert_eq!(sky.mirror.as_ref().map(|m| m.0), Some(MeasId(4)));
    // The level axis frames both halves: the mirrored one's levels are in its sample, so
    // its peaks do not stand past the ceiling.
    let r = t.st.level_range(p).expect("level axis");
    assert!(r.hi >= 0.0, "{r:?}");

    t.st.layout.view_mut(p).expect("the pane").with = With::of(&[]);
    c.run(&mut t, 0.5, &both);
    assert_eq!(drawn_3d(&t, p, &mut sky), (false, vec!["Sub".to_owned()]));
    assert!(sky.mirror.is_none());
}
