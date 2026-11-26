//! Spike spacing: the same room on the loosest and the tightest grid of the full tier.

use super::*;

#[test]
fn skyline_spacing_goldens() {
    for (step, name) in [
        (
            spectrograph_3d::SpikeSpacing::Loosest,
            "skyline_spacing_loosest_pink_room",
        ),
        (
            spectrograph_3d::SpikeSpacing::Tightest,
            "skyline_spacing_tightest_pink_room",
        ),
    ] {
        let (cols, rows) = spectrograph_3d::grid(SkylineTier::Full, step);
        let (cols, rows) = (cols as u32, rows as u32 + 1);
        let mut s = Sky::new(cols, rows, pink_room(cols, rows));
        s.tier = SkylineTier::Full;
        s.phase = 0.4;
        s.glow = true;
        golden_of(name, &s);
    }
}
