//! Spike spacing: the grid at each step, and a pick through it.

use super::points::{chrome, scene_posed};
use super::*;
use crate::spectrograph_3d::pick::spike;
use crate::spectrograph_3d::scene::Projector;

#[test]
fn each_step_scales_columns_and_rows_alike_and_normal_is_the_present_grid() {
    assert_eq!(SpikeSpacing::default(), SpikeSpacing::Normal);
    for (tier, base) in [
        (SkylineTier::Full, (192, 96)),
        (SkylineTier::Lite, (128, 48)),
        (SkylineTier::Bare, (128, 48)),
    ] {
        assert_eq!(grid(tier, SpikeSpacing::Normal), base, "{tier:?}");
        let n = |s| {
            let (c, r) = grid(tier, s);
            c * r
        };
        let normal = n(SpikeSpacing::Normal) as f64;
        for (s, ratio) in [
            (SpikeSpacing::Tightest, 4.0),
            (SpikeSpacing::Tight, 2.0),
            (SpikeSpacing::Loose, 0.5),
            (SpikeSpacing::Loosest, 0.25),
        ] {
            let (c, r) = grid(tier, s);
            // Both axes alike: the cell keeps its shape.
            let (kc, kr) = (c as f64 / base.0 as f64, r as f64 / base.1 as f64);
            assert!((kc - kr).abs() < 0.02, "{tier:?} {s:?}: {c}×{r}");
            assert!(
                (n(s) as f64 / normal - ratio).abs() < 0.02 * ratio,
                "{tier:?} {s:?}"
            );
            // The tightest is the bound: four times the tier's count, never more.
            assert!(n(s) <= 4 * n(SpikeSpacing::Normal));
        }
    }
    // Held at the ends.
    assert_eq!(SpikeSpacing::Tightest.step(-1), SpikeSpacing::Tightest);
    assert_eq!(SpikeSpacing::Loosest.step(1), SpikeSpacing::Loosest);
    assert_eq!(SpikeSpacing::Normal.step(1), SpikeSpacing::Loose);
    assert_eq!(SpikeSpacing::Normal.step(-1), SpikeSpacing::Tight);
}

#[test]
fn a_skyline_rebuilds_on_a_new_step_and_a_pick_reads_its_cell() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    for slot in 0..h.slots() as u64 {
        let l: Vec<f32> = f.edges.iter().map(|_| -30.0).collect();
        f.push_with(&mut h, slot, &l, None);
    }
    let mut s = Skyline::default();
    s.update(&h, full_range(), SkylineTier::Full, SpikeSpacing::Normal);
    let normal = (s.cols(), s.rows(), s.bucket_slots());
    for step in [SpikeSpacing::Tightest, SpikeSpacing::Loosest] {
        s.update(&h, full_range(), SkylineTier::Full, step);
        assert!(same(&s, &rebuilt_at(&h, step)), "{step:?}: a rebuild");
        assert_ne!((s.cols(), s.rows(), s.bucket_slots()), normal, "{step:?}");
        // A click on a front-row tip picks that cell at this step too.
        let b = (h.clone(), s.clone());
        let pose = Pose::from(SpectrumCamera::DEFAULT);
        let sc = scene_posed(
            &b,
            None,
            &chrome(PlotChrome::Full),
            1.0,
            (pose, 0.0, SkylineTier::Full),
        );
        let fr = sc.pick.expect("a picture");
        let col = cell_at(&s, 1000.0, 0.0).expect("in the span").0;
        let v = s.row_back(0).expect("a row")[col];
        let (_, top) = spike(&fr, s.cols(), s.rows(), col, 0, v).expect("a spike");
        let at = Projector::new(fr.view_proj, fr.plot)
            .point(top)
            .expect("before the eye");
        let p = pick(
            &fr,
            &s,
            Pointer3d {
                pos: at,
                px_per_pt: 1.0,
            },
        )
        .expect("a pick");
        assert_eq!(p.cell, (col, 0), "{step:?}");
    }
    s.update(&h, full_range(), SkylineTier::Full, SpikeSpacing::Normal);
    assert_eq!((s.cols(), s.rows(), s.bucket_slots()), normal);
}

fn rebuilt_at(h: &SpectrographHistory, step: SpikeSpacing) -> Skyline {
    let mut s = Skyline::default();
    s.update(h, full_range(), SkylineTier::Full, step);
    s
}
