//! The app's ember intensity mirrors the daemon's: the app links no DSP, so `ac2-scene`
//! keeps its own copy of the landing curve, and this holds the two equal on every point a
//! goniometer frame can carry.

#[test]
fn landing_intensity_matches_the_daemons() {
    for x in i8::MIN..=i8::MAX {
        for y in i8::MIN..=i8::MAX {
            let app = ac2_scene::gonio::landing_intensity([x, y]);
            let daemon = ac2_core::stereo::landing_intensity([x, y]);
            assert!(
                (app - daemon).abs() <= 1e-6,
                "({x}, {y}): app {app}, daemon {daemon}"
            );
        }
    }
}
