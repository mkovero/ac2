//! Which analysis stages of a transfer at `bins` the link receives: from the shown range and
//! the transfer pane's plot width, nothing under a peak hold, peak tracking or smoothing,
//! for a coarser resolution or for a curve the pane does not draw.

use super::*;
use ac2_client::{Latest, TopicFrame};
use ac2_proto::FrameData;
use ac2_proto::model::{BinRange, PeakMode, TransferResolution};
use ac2_proto::topic::Topic;
use ac2_proto::units::Hz;
use ac2_scene::view::FreqRange;

/// Main L (1) at `bins`, a transfer pane showing it.
fn at_bins() -> T {
    let mut t = T::new();
    let mut s = daemon_state();
    for m in &mut s.measurements {
        if let MeasKind::Transfer { config } = &mut m.config.kind {
            config.resolution = TransferResolution::Bins;
        }
    }
    t.conn(mirror(s));
    t.go(PaneKind::Transfer);
    frames(&mut t, true, None);
    t
}

/// Main L's `tf` frame, its layout stated (`bins`), unsmoothed; with `stage` held when
/// given, stamped as the `tf`.
fn frames(t: &mut T, bins: bool, stage: Option<u8>) {
    let mut latest = Latest::default();
    let mut f = ac2_proto::samples::tf_frame();
    let (delay, stamp) = (Seconds(0.0125), f.stamp);
    if let FrameData::Tf(tf) = &mut f.data {
        tf.meta.math = None;
        tf.meta.smoothing = None;
        tf.meta.bins = bins.then(|| Box::new(ac2_proto::samples::bin_layout()));
        tf.meta.unresolved = Some(
            [
                (1.0, 135.3, 0.977),
                (203.125, 405.8, 2.93),
                (812.5, 1623.0, 11.72),
            ]
            .into_iter()
            .map(|(lo, hi, b)| BinRange {
                lo: Hz(lo),
                hi: Hz(hi),
                bin_hz: Hz(b),
            })
            .collect(),
        );
    }
    let mut all = vec![f];
    if let Some(b) = stage {
        let mut s = ac2_proto::samples::bins_frame();
        s.stamp = stamp;
        if let FrameData::Bins(bf) = &mut s.data {
            let l = ac2_proto::samples::bin_layout().stages[usize::from(b)];
            let n = l.bins as usize;
            bf.meta.stage = b;
            bf.meta.first_bin = l.first_bin;
            bf.meta.bin_hz = l.bin_hz;
            bf.meta.delay = delay;
            bf.mag = vec![-3.0; n];
            bf.phase = vec![10.0; n];
            bf.coh = vec![0.95; n];
        }
        all.push(s);
    }
    for f in all {
        let f = TopicFrame {
            topic: f.data.topic(),
            frame: Arc::new(f),
            received: Instant::now(),
            since_new: std::time::Duration::ZERO,
            age: Some(0.0),
            stale: false,
        };
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    let mut grids = std::collections::BTreeMap::new();
    let g = ac2_proto::samples::log_grid();
    grids.insert(g.id(), Arc::new(g));
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
}

fn width(t: &mut T, px: f32) {
    let pane = t.pane(PaneKind::Transfer);
    t.st.apply(Msg::PlotWidth { pane, px }, &t.keys);
}

fn view(t: &mut T, lo: f64, hi: f64) {
    t.st.view.freq = FreqRange { lo, hi };
    t.st.nav = crate::anim::FreqNav::new(t.st.view.freq);
}

/// The stages wanted, as (measurement, stage).
fn stages(t: &T) -> Vec<(u32, u8)> {
    let mut v: Vec<(u32, u8)> =
        t.st.wanted_topics()
            .into_iter()
            .filter_map(|tp| match tp {
                Topic::Bins { meas, stage } => Some((meas.0, stage)),
                _ => None,
            })
            .collect();
    v.sort_unstable();
    v
}

#[test]
fn the_default_view_wants_what_the_threshold_table_says() {
    let mut t = at_bins();
    // Not drawn yet: no width, no stage.
    assert_eq!(stages(&t), []);
    width(&mut t, 800.0);
    assert_eq!(stages(&t), []);
    // 1900 px: stage 2 at 1.32 px, stage 1 at 0.99 (received ahead of drawing).
    width(&mut t, 1900.0);
    assert_eq!(stages(&t), [(1, 1), (1, 2)]);
    // The link is asked for them.
    let r = t.st.sync_link();
    assert!(
        r.iter().any(|(_, r)| matches!(r, Request::Topics(x)
            if x.contains(&Topic::Bins { meas: MeasId(1), stage: 2 }))),
        "{r:?}"
    );
    assert!(
        t.st.sync_link().is_empty(),
        "the same view asks nothing more"
    );
}

#[test]
fn zoom_and_pan_move_the_stages() {
    let mut t = at_bins();
    view(&mut t, 10_000.0, 20_000.0);
    width(&mut t, 1900.0);
    assert_eq!(stages(&t), [(1, 0)]);
    // 800 px: 0.68 px apart at 20 kHz, under the 0.8 px that receives.
    width(&mut t, 800.0);
    assert_eq!(stages(&t), []);
    // An octave down they are twice as far apart.
    t.st.apply(Msg::Pan { octaves: -1.0 }, &t.keys);
    assert_eq!(stages(&t), [(1, 0)]);
    // Below 1 kHz stage 0 is never wanted, however wide the plot.
    for px in [800.0, 1900.0, 8000.0] {
        view(&mut t, 300.0, 900.0);
        width(&mut t, px);
        assert!(stages(&t).iter().all(|(_, s)| *s != 0), "{px}");
    }
    view(&mut t, 300.0, 900.0);
    width(&mut t, 1900.0);
    assert_eq!(stages(&t), [(1, 1)]);
}

#[test]
fn the_same_view_always_wants_the_same_stages() {
    let mut t = at_bins();
    // Stage 1 at the default view crosses 0.8 px near 1528 px.
    for px in [1527.0, 1528.0, 1529.0, 1530.0] {
        width(&mut t, px);
        let first = stages(&t);
        assert_eq!(stages(&t), first, "{px}");
    }
    width(&mut t, 1520.0);
    assert_eq!(stages(&t), [(1, 2)]);
    width(&mut t, 1540.0);
    assert_eq!(stages(&t), [(1, 1), (1, 2)]);
}

#[test]
fn what_keeps_the_grid_wants_no_stage() {
    let mut t = at_bins();
    view(&mut t, 10_000.0, 20_000.0);
    width(&mut t, 1900.0);
    assert_eq!(stages(&t), [(1, 0)]);
    // A peak hold, then peak tracking.
    for peak in [PeakMode::Hold, PeakMode::Track] {
        t.remirror(|s| {
            for m in &mut s.measurements {
                if let MeasKind::Transfer { config } = &mut m.config.kind {
                    config.peak = peak;
                }
            }
        });
        assert_eq!(stages(&t), [], "{peak:?}");
    }
    t.remirror(|s| {
        for m in &mut s.measurements {
            if let MeasKind::Transfer { config } = &mut m.config.kind {
                config.peak = PeakMode::Off;
            }
        }
    });
    assert_eq!(stages(&t), [(1, 0)]);
    // Its curve hidden: the pane draws only stored traces.
    t.st.apply(Msg::ToggleMeasShown(MeasId(1)), &t.keys);
    assert_eq!(stages(&t), []);
    t.st.apply(Msg::ToggleMeasShown(MeasId(1)), &t.keys);
    assert_eq!(stages(&t), [(1, 0)]);
    // Smoothed: its columns are, its bins are not.
    let mut smoothed = at_bins();
    view(&mut smoothed, 10_000.0, 20_000.0);
    width(&mut smoothed, 1900.0);
    let mut latest = smoothed.st.rig.data.as_ref().expect("data").latest.clone();
    for f in latest.frames.values_mut() {
        let mut frame = (*f.frame).clone();
        if let FrameData::Tf(tf) = &mut frame.data {
            tf.meta.smoothing = Some(Smoothing {
                fraction: SmoothingFraction::Sixth,
                mode: SmoothingMode::Magnitude,
            });
        }
        f.frame = Arc::new(frame);
    }
    let grids = smoothed.st.rig.data.as_ref().expect("data").grids.clone();
    smoothed.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
    assert_eq!(stages(&smoothed), []);
    // At 1/96 no layout is stated, and nothing is wanted.
    let mut grid = at_bins();
    frames(&mut grid, false, None);
    view(&mut grid, 10_000.0, 20_000.0);
    width(&mut grid, 1900.0);
    assert_eq!(stages(&grid), []);
}

#[test]
fn the_pane_draws_a_held_stage() {
    let mut t = at_bins();
    frames(&mut t, true, Some(0));
    view(&mut t, 10_000.0, 20_000.0);
    let pane = t.pane(PaneKind::Transfer);
    let size = ac2_scene::primitives::Viewport::new(2000.0, 600.0);
    let now = crate::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let s = crate::scenes::transfer(&t.st, pane, &ac2_scene::theme::Theme::dark(), size, now);
    let drawn = s.bins.as_ref().expect("stage 0 drawn");
    assert_eq!(drawn.spans[0].len(), 1);
    assert_eq!(drawn.spans[0][0].stage, 0);
    // The scene's plot width is what the view reports.
    assert!(s.panes[0].plot.w > 1800.0);
}
