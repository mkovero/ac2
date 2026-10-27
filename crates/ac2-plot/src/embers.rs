//! The goniometer's style: the blackbody ramp an ember cools along and the point cloud a
//! ring becomes. The truth (where each point is, how old, how bright it landed) is
//! `ac2_scene::gonio`'s; this file only chooses colours.

use ac2_scene::gonio::{AshStage, Cooling, EmberRing, ash_stage, brightness, place};

use crate::scene::{Blend, Color, Point, Points, Rect};

/// Disc diameter of an ember, logical pixels: small enough that a tone stays a line, soft
/// enough that neighbours fuse into a glow.
pub const EMBER_SIZE_PX: f32 = 2.5;

/// The colour of a stage at full brightness: a cooling body from white-yellow through
/// orange and deep red to ash grey. The ash keeps a little lightness so the oldest points
/// smoulder rather than vanish before the ring retires them.
fn stage_color(stage: AshStage) -> Color {
    match stage {
        AshStage::White => Color::rgb(1.0, 0.97, 0.85),
        AshStage::Yellow => Color::rgb(1.0, 0.82, 0.40),
        AshStage::Orange => Color::rgb(1.0, 0.50, 0.12),
        AshStage::Red => Color::rgb(0.80, 0.16, 0.06),
        AshStage::Ash => Color::rgb(0.28, 0.26, 0.25),
    }
}

/// An ember's colour: the stage's hue with `brightness` (0 … 1) as its opacity, so under
/// additive blending a fresh loud point adds a full white grain and an old quiet one a trace
/// of grey. The ash stage is lifted to a floor of 6 % so it stays visible until retired.
pub fn ember_color(stage: AshStage, brightness: f32) -> Color {
    let floor = if stage == AshStage::Ash { 0.06 } else { 0.0 };
    stage_color(stage).with_alpha(brightness.clamp(0.0, 1.0).max(floor))
}

/// The ring as an additive point cloud in the goniometer's `square`
/// ([`ac2_scene::gonio::square`]) at display time `now`, cooling as `cooling` (decay and
/// glow); oldest points first, so the freshest land on top.
pub fn ember_points(ring: &EmberRing, now: f64, cooling: Cooling, square: Rect) -> Points {
    let points = ring
        .age(now)
        .map(|e| Point {
            pos: place(square, e.x, e.y),
            color: ember_color(
                ash_stage(e.age_s, cooling),
                brightness(e.age_s, cooling, e.intensity),
            ),
        })
        .collect();
    Points {
        points,
        size_px: EMBER_SIZE_PX,
        blend: Blend::Additive,
        clip: Some(square),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_cools_from_white_to_grey() {
        let lum = |c: Color| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
        let stages = [
            AshStage::White,
            AshStage::Yellow,
            AshStage::Orange,
            AshStage::Red,
            AshStage::Ash,
        ];
        for w in stages.windows(2) {
            assert!(lum(stage_color(w[0])) > lum(stage_color(w[1])), "{w:?}");
        }
        assert_eq!(ember_color(AshStage::White, 1.0).a, 1.0);
        assert_eq!(ember_color(AshStage::Orange, 0.3).a, 0.3);
        assert_eq!(ember_color(AshStage::Red, 0.0).a, 0.0);
        assert!(ember_color(AshStage::Ash, 0.0).a > 0.0, "ash smoulders");
    }

    #[test]
    fn points_follow_the_ring() {
        let mut ring = EmberRing::new(8);
        ring.push_frame(0.0, &[[127, 0, 127]]);
        ring.push_frame(1.0, &[[0, 127, 64]]);
        let sq = Rect::new(0.0, 0.0, 100.0, 100.0);
        let pts = ember_points(&ring, 1.0, Cooling::new(0.4, 1.0), sq);
        assert_eq!(pts.blend, Blend::Additive);
        assert_eq!(pts.clip, Some(sq));
        assert_eq!(pts.points.len(), 2);
        assert_eq!(pts.points[0].pos, [100.0, 50.0]);
        assert!(pts.points[0].color.a < 0.1, "a second old at τ = 0.4 s");
        assert_eq!(pts.points[1].pos, [50.0, 0.0]);
        assert!((pts.points[1].color.a - 64.0 / 127.0).abs() < 1e-6);
    }
}
