//! Peak points on a pane pinned to a background rig: the pick is that rig's, and its
//! frames keep the picked curve's tracker though the rig is not the one shown.

use super::*;
use ac2_scene::trace::TraceKey;

/// Pupu's spectrum picked in a pane pinned to pupu: pupu's folding tracks it (P is off),
/// the shown rig's same id does not; the pane pinned back to the shown rig drops it.
#[test]
fn a_background_rigs_pick_tracks_its_curve() {
    let (mut t, _) = two_lists();
    let p = t.pane(PaneKind::Spectrum);
    drop_pupu(&mut t, p);
    let key = TraceKey::Live(MeasId(2));
    t.st.update(Msg::LegendPick(p, key), &t.keys);
    assert_eq!(t.st.peak_points_of(p), Some(key));
    to(&mut t, PUPU, spectrum_at(5, -40.0));
    assert!(
        pupu(&t).peak_tracks.contains_key(&MeasId(2)),
        "pupu tracks it"
    );
    t.conn(spectrum_at(5, -40.0));
    assert!(
        !t.st.rig.peak_tracks.contains_key(&MeasId(2)),
        "the shown rig's id 2 is not picked"
    );
    // The shown rig's row dropped there pins the pane back: the pick goes.
    t.drop_meas(p, MeasId(2));
    assert_eq!(t.st.peak_points_of(p), None);
}
