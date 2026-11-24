//! Which analysis stages of a transfer at `bins` the link receives: from the shown range and
//! the transfer pane's plot width, nothing under a peak hold, peak tracking or smoothing,
//! in a view that draws the grid, for a coarser resolution or for a curve the pane does not
//! draw; each rig's from its own link.

use super::*;
use ac2_client::{Latest, TopicFrame};
use ac2_proto::FrameData;
use ac2_proto::model::{BinRange, PeakMode, TransferResolution};
use ac2_proto::topic::Topic;
use ac2_proto::units::Hz;
use ac2_scene::view::{FreqRange, PhaseView};

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

#[test]
fn unwrapped_phase_or_group_delay_wants_no_stage_in_any_view() {
    let mut t = at_bins();
    view(&mut t, 10_000.0, 20_000.0);
    width(&mut t, 1900.0);
    assert_eq!(stages(&t), [(1, 0)]);
    // The magnitude view alone: the whole pane draws the grid once the phase does.
    for phase in [
        PhaseView::Unwrapped {
            range: ac2_scene::axis::Range::new(-720.0, 180.0),
        },
        PhaseView::GroupDelay {
            range_ms: ac2_scene::axis::Range::new(-2.0, 10.0),
        },
    ] {
        t.st.view.tf.phase = phase;
        assert_eq!(stages(&t), [], "{phase:?}");
    }
}

#[test]
fn a_closed_pane_drops_its_width() {
    let mut t = at_bins();
    let pane = t.pane(PaneKind::Transfer);
    width(&mut t, 1900.0);
    assert!(t.st.plot_px.contains_key(&pane));
    t.st.apply(Msg::FocusPane(pane), &t.keys);
    t.st.apply(Msg::Command(crate::keys::CommandId::ClosePane), &t.keys);
    assert!(t.st.layout.view(pane).is_none());
    assert!(!t.st.plot_px.contains_key(&pane));
}

/// Transfer `tf` frames of measurements `ids`, each stating the layout (as at `bins`).
fn laid_out(ids: &[u32]) -> ConnEvent {
    let mut latest = Latest::default();
    for id in ids {
        let mut f = ac2_proto::samples::tf_frame();
        if let FrameData::Tf(tf) = &mut f.data {
            tf.meas = MeasId(*id);
            tf.meta.math = None;
            tf.meta.smoothing = None;
            tf.meta.bins = Some(Box::new(ac2_proto::samples::bin_layout()));
            tf.meta.unresolved = Some(vec![BinRange {
                lo: Hz(812.5),
                hi: Hz(1623.0),
                bin_hz: Hz(11.72),
            }]);
        }
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
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    }))
}

/// Main L (1) and Delay tower (3) at `bins`, "tower ÷ L" (6), a transfer math filed under
/// Delay tower, each with a layout; the grid's transfer pane (1) leads Main L, the second
/// pane (2) is a transfer pane leading Delay tower; zoomed to 10–20 kHz.
fn three_curves() -> T {
    use ac2_proto::model::{MathConfig, MathDomain, MathExpr, MathOp, Operand};
    let mut s = four();
    for m in &mut s.measurements {
        if let MeasKind::Transfer { config } = &mut m.config.kind {
            config.resolution = TransferResolution::Bins;
        }
    }
    s.measurements.push(meas(
        6,
        "tower ÷ L",
        MeasKind::Math {
            config: MathConfig::of(
                TraceOwner::Meas { meas: MeasId(3) },
                MathDomain::Transfer,
                MathExpr::Binary {
                    a: Operand::Meas { meas: MeasId(3) },
                    op: MathOp::Divide,
                    b: Operand::Meas { meas: MeasId(1) },
                },
            ),
        },
    ));
    let mut t = T::new();
    t.conn(mirror(s));
    t.conn(laid_out(&[1, 3, 6]));
    t.st.set_pane_kind(PaneId(2), PaneKind::Transfer);
    for (p, m) in [(1, 1), (2, 3)] {
        t.st.layout.view_mut(PaneId(p)).expect("pane").meas = Some(MeasId(m));
    }
    view(&mut t, 10_000.0, 20_000.0);
    t
}

#[test]
fn what_each_view_wants() {
    /// A case: its name, what it changes, the stages then wanted.
    type Case = (&'static str, fn(&mut T), &'static [(u32, u8)]);
    let cases: [Case; 9] = [
        ("magnitude", |_| {}, &[(1, 0)]),
        (
            "alignment view",
            |t| {
                t.st.layout
                    .view_mut(PaneId(1))
                    .expect("pane")
                    .modes
                    .transfer = TransferView::Alignment
            },
            &[],
        ),
        (
            "IR view",
            |t| {
                t.st.layout
                    .view_mut(PaneId(1))
                    .expect("pane")
                    .modes
                    .transfer = TransferView::Ir(ac2_scene::view::IrMode::Linear)
            },
            &[],
        ),
        (
            "phase only, wrapped",
            |t| {
                t.st.layout
                    .view_mut(PaneId(1))
                    .expect("pane")
                    .modes
                    .transfer = TransferView::Phase
            },
            &[(1, 0)],
        ),
        (
            "phase only, unwrapped",
            |t| {
                t.st.layout
                    .view_mut(PaneId(1))
                    .expect("pane")
                    .modes
                    .transfer = TransferView::Phase;
                t.st.view.tf.phase = PhaseView::Unwrapped {
                    range: ac2_scene::axis::Range::new(-720.0, 180.0),
                };
            },
            &[],
        ),
        (
            "a math with a layout, in its owner's group",
            |t| t.st.layout.view_mut(PaneId(1)).expect("pane").meas = Some(MeasId(3)),
            &[(3, 0), (6, 0)],
        ),
        (
            "a compared curve",
            |t| {
                let picks = [Pick::here(EntryRef::Live { meas: MeasId(3) })];
                t.st.layout.view_mut(PaneId(1)).expect("pane").with = With::of(&picks);
            },
            &[(1, 0), (3, 0)],
        ),
        (
            "two panes, each its own width: the union",
            |t| {
                t.st.apply(
                    Msg::PlotWidth {
                        pane: PaneId(2),
                        px: 1200.0,
                    },
                    &t.keys,
                );
            },
            &[(1, 0), (3, 0), (6, 0)],
        ),
        (
            "the second pane too narrow for its curves",
            |t| {
                t.st.apply(
                    Msg::PlotWidth {
                        pane: PaneId(2),
                        px: 800.0,
                    },
                    &t.keys,
                );
            },
            &[(1, 0)],
        ),
    ];
    for (name, setup, want) in cases {
        let mut t = three_curves();
        t.st.apply(
            Msg::PlotWidth {
                pane: PaneId(1),
                px: 1900.0,
            },
            &t.keys,
        );
        setup(&mut t);
        assert_eq!(stages(&t), want, "{name}");
    }
}

/// The second rig, as its hello names it.
const PUPU: RigId = RigId(0x9e9e);

#[test]
fn a_pane_on_another_rig_asks_that_rigs_link() {
    let mut s = daemon_state();
    for m in &mut s.measurements {
        if let MeasKind::Transfer { config } = &mut m.config.kind {
            config.resolution = TransferResolution::Bins;
        }
    }
    let mut t = T::new();
    t.conn(mirror(paired_with(s.clone(), PUPU, "pupu")));
    let slot = t.st.add_rig("pupu".into(), None, None);
    let to =
        |t: &mut T, rig: RigId, e: ConnEvent| t.st.update(Msg::Conn(rig, Box::new(e)), &t.keys);
    to(
        &mut t,
        RigId(u64::from(slot)),
        ConnEvent::Connected {
            target: "pupu".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c2".into()),
            rig_id: PUPU,
            name: "pupu".into(),
        },
    );
    to(&mut t, PUPU, mirror(s));
    to(&mut t, PUPU, laid_out(&[1]));
    // The transfer pane shows pupu's Main L; the rig shown has no frame of its own.
    let pane = t.pane(PaneKind::Transfer);
    let v = t.st.layout.view_mut(pane).expect("pane");
    (v.rig, v.meas) = (PaneRig::Rig(PUPU), Some(MeasId(1)));
    view(&mut t, 10_000.0, 20_000.0);
    width(&mut t, 1900.0);
    let sync = t.st.sync_link();
    let bins = |rig: RigId| -> Vec<Topic> {
        sync.iter()
            .filter(|(r, _)| *r == rig)
            .flat_map(|(_, q)| match q {
                Request::Topics(x) => x.iter().cloned().collect(),
                _ => Vec::new(),
            })
            .filter(|tp| matches!(tp, Topic::Bins { .. }))
            .collect()
    };
    assert_eq!(
        bins(PUPU),
        [Topic::Bins {
            meas: MeasId(1),
            stage: 0
        }],
        "{sync:?}"
    );
    assert_eq!(bins(RIG), [], "{sync:?}");
}
