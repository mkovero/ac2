//! A transfer's own analysis bins: which stages a view draws and receives, the line through
//! them, when a stage frame is too old or of another configuration, and what the legend
//! and the cursor say in each case.

use super::*;
use crate::tf::bins::{self, BinSource, Forced, StageFrame};
use ac2_proto::frame::TfMeta;
use ac2_proto::frame::{BinLayout, BinStage, BinsFrame, BinsMeta, FrameStamp, ProtectionFlags};
use ac2_proto::model::{BinRange, FreqRange, RangeBand, RangeSpread};
use ac2_proto::units::{DaemonIncarnation, Hz, Rev, SampleIndex, Samples, WallNs};

const RATE: f64 = 48_000.0;
/// The 48 kHz standard ladder: bin spacing, first bin, bins, hop and decimation per stage.
const DF: [f64; 3] = [RATE / 4096.0, RATE / 16_384.0, RATE / 49_152.0];
const FIRST: [u32; 3] = [88, 88, 1];
const BINS: [u32; 3] = [1756, 189, 207];
const HOP: [i64; 3] = [2048, 4096, 6144];
const FACTOR: [u32; 3] = [1, 4, 12];
const T: u64 = 480_000;

fn fr(lo: f64, hi: f64) -> FreqRange {
    FreqRange {
        lo: Hz(lo),
        hi: Hz(hi),
    }
}

/// Columns coarser than the serving stage's bins: up to 138.5 bins of each stage.
fn unresolved() -> Vec<BinRange> {
    [
        (0.5, 135.3, DF[2]),
        (202.2, 405.8, DF[1]),
        (811.0, 1623.0, DF[0]),
    ]
    .into_iter()
    .map(|(lo, hi, b)| BinRange {
        lo: Hz(lo),
        hi: Hz(hi),
        bin_hz: Hz(b),
    })
    .collect()
}

fn layout() -> BinLayout {
    BinLayout {
        stages: (0..3)
            .map(|b| BinStage {
                first_bin: FIRST[b],
                bins: BINS[b],
                bin_hz: Hz(DF[b]),
            })
            .collect(),
        blends: vec![fr(811.0, 1031.25), fr(202.2, 257.8)],
    }
}

fn stamp(at: u64, rev: u64) -> FrameStamp {
    FrameStamp {
        seq: 1,
        audio_sample: SampleIndex(at),
        session_epoch: SessionEpoch(1),
        daemon_incarnation: DaemonIncarnation(1),
        config_rev: Rev(rev),
        config_applied_at: SampleIndex(0),
        capture_wall_ns: WallNs(0),
        grid_id: None,
        protection: ProtectionFlags::NONE,
    }
}

/// Bin `k` of stage `b`: a level of its own (−6 dB less the stage, a tenth per `k % 7`),
/// and a peak at stage 0's bin 1300 (15.2 kHz) that only the bins show.
fn bin_db(b: usize, k: u32) -> f32 {
    if (b, k) == (0, PEAK_BIN) {
        return 6.0;
    }
    -6.0 - b as f32 + (k % 7) as f32 * 0.1
}

const PEAK_BIN: u32 = 1300;

fn stage(b: usize) -> BinsFrame {
    let n = BINS[b] as usize;
    BinsFrame {
        meas: MeasId(1),
        meta: BinsMeta {
            stage: b as u8,
            factor: FACTOR[b],
            rate: Hz(RATE / f64::from(FACTOR[b])),
            bin_hz: Hz(DF[b]),
            first_bin: FIRST[b],
            hop: Samples(HOP[b]),
            delay: Seconds(0.0),
            nudged: Seconds(0.0),
            mic_curve: false,
            mic_range: None,
            blocks: 8,
            eff_avg: 8.0,
        },
        mag: (0..n).map(|i| bin_db(b, FIRST[b] + i as u32)).collect(),
        phase: vec![30.0; n],
        coh: vec![0.9; n],
    }
}

/// A transfer at `bins`: its 1/96-octave grid at 0 dB, its layout, and every stage held.
struct Fx {
    c: Cols,
    meta: TfMeta,
    stamp: FrameStamp,
    stages: Vec<(FrameStamp, BinsFrame)>,
    running: bool,
}

impl Fx {
    fn new() -> Self {
        let freqs: Vec<f64> = (0..)
            .map(|i| 10.0 * 2f64.powf(f64::from(i) / 96.0))
            .take_while(|f| *f <= 24_000.0)
            .collect();
        let n = freqs.len();
        Self {
            c: Cols {
                freqs,
                mag: vec![0.0; n],
                phase: vec![0.0; n],
                coh: vec![1.0; n],
                validity: vec![ValidityMask::NONE; n],
            },
            meta: TfMeta {
                delay: Seconds(0.0),
                nudged: Seconds(0.0),
                smoothing: None,
                mic_curve: false,
                mic_range: None,
                math: None,
                unresolved: Some(unresolved()),
                bins: Some(Box::new(layout())),
                settling: None,
                filled: 1.0,
            },
            stamp: stamp(T, 5),
            stages: (0..3).map(|b| (stamp(T, 5), stage(b))).collect(),
            running: true,
        }
    }

    fn trace(&self) -> TfTrace<'_> {
        let src = BinSource {
            layout: self.meta.bins.as_deref().expect("layout"),
            tf_meta: &self.meta,
            tf_stamp: &self.stamp,
            stages: self
                .stages
                .iter()
                .map(|(stamp, frame)| Some(StageFrame { stamp, frame }))
                .collect(),
            running: self.running,
            holds: false,
        };
        TfTrace {
            bins: Some(src),
            unresolved: self
                .meta
                .unresolved
                .as_deref()
                .map(crate::unresolved::Source::Joined),
            ..trace(&self.c, TraceKey::Live(MeasId(1)), 0.0)
        }
    }
}

fn view(lo: f64, hi: f64) -> ViewState {
    ViewState {
        freq: crate::view::FreqRange { lo, hi },
        unresolved: true,
        ..ViewState::default()
    }
}

/// A viewport whose transfer plot is `plot_px` wide.
fn size(plot_px: f32) -> Viewport {
    let probe = scene(&[], &ViewState::default(), Viewport::new(1000.0, 600.0));
    Viewport::new(plot_px + 1000.0 - probe.panes[0].plot.w, 600.0)
}

fn scene(traces: &[TfTrace<'_>], v: &ViewState, size: Viewport) -> TfScene {
    transfer_scene(
        traces,
        &DisplayCache::default(),
        &Status::default(),
        v,
        &Theme::dark(),
        size,
    )
}

/// The stages and bins `t` draws in `v` on a plot `plot_px` wide.
fn drawn(t: &TfTrace<'_>, v: &ViewState, plot_px: f64) -> Vec<(u8, u32, u32)> {
    let shown = [crate::trace::display_trace(t, None, &v.tf.coherence)];
    let Some(s) = bins::splice(std::slice::from_ref(t), &shown, v, plot_px) else {
        return Vec::new();
    };
    s.spans[0]
        .iter()
        .map(|sp| {
            let k = |c: usize| (s.traces[0].freqs[c] / sp.bin_hz).round() as u32;
            (sp.stage, k(sp.cols.start), k(sp.cols.end - 1))
        })
        .collect()
}

fn readout_freq(s: &TfScene) -> String {
    s.cursor.as_ref().expect("cursor").freq.clone()
}

#[test]
fn each_stage_is_fine_above_its_unresolved_range() {
    let (l, u) = (layout(), unresolved());
    let fine: Vec<_> = (0..3).map(|b| bins::fine_bins(&l, Some(&u), b)).collect();
    assert_eq!(
        fine,
        [Some((139, 1843)), Some((139, 276)), Some((139, 207))]
    );
    // No unresolved ranges stated: no bin is known to be finer, none is wanted or drawn.
    assert!((0..3).all(|b| bins::fine_bins(&l, None, b).is_none()));
    let full = Range::new(20.0, 20_000.0);
    assert_eq!(bins::wanted(&l, None, full, 2500.0), Vec::<u8>::new());
    let mut fx = Fx::new();
    fx.meta.unresolved = None;
    assert_eq!(drawn(&fx.trace(), &view(10_000.0, 20_000.0), 1900.0), []);
}

#[test]
fn the_default_view_wants_and_draws_per_the_threshold_table() {
    let (l, u) = (layout(), unresolved());
    let full = Range::new(20.0, 20_000.0);
    // 800 px: 0.07 / 0.42 / 0.56 px; 1280: stage 2 at 0.89; 1900: 1 at 0.99, 2 at 1.32;
    // 2500: 1 at 1.31, 2 at 1.74.
    assert_eq!(bins::wanted(&l, Some(&u), full, 800.0), Vec::<u8>::new());
    assert_eq!(bins::wanted(&l, Some(&u), full, 1280.0), [2]);
    assert_eq!(bins::wanted(&l, Some(&u), full, 1900.0), [1, 2]);
    assert_eq!(bins::wanted(&l, Some(&u), full, 2500.0), [1, 2]);
    let fx = Fx::new();
    let v = view(20.0, 20_000.0);
    assert_eq!(drawn(&fx.trace(), &v, 800.0), []);
    assert_eq!(drawn(&fx.trace(), &v, 1280.0), []);
    assert_eq!(drawn(&fx.trace(), &v, 1900.0), [(2, 139, 207)]);
    assert_eq!(
        drawn(&fx.trace(), &v, 2500.0),
        [(2, 139, 207), (1, 139, 276)]
    );
}

#[test]
fn a_zoom_draws_the_bins_in_view_and_the_first_beyond_each_edge() {
    let fx = Fx::new();
    // One octave on 1900 px: stage 0 bins 1.6 px apart at 20 kHz.
    let v = view(10_000.0, 20_000.0);
    assert_eq!(drawn(&fx.trace(), &v, 1900.0), [(0, 853, 1707)]);
    const { assert!(853.0 * DF[0] < 10_000.0 && 854.0 * DF[0] > 10_000.0) };
    const { assert!(1707.0 * DF[0] > 20_000.0 && 1706.0 * DF[0] < 20_000.0) };
    // On 800 px they are 0.68 px apart: the grid.
    assert_eq!(drawn(&fx.trace(), &v, 800.0), []);
    // Under 1 kHz stage 0 is never drawn, however wide.
    let low = view(300.0, 900.0);
    assert!(
        drawn(&fx.trace(), &low, 4000.0)
            .iter()
            .all(|(b, ..)| *b != 0)
    );
    assert_eq!(drawn(&fx.trace(), &low, 4000.0), [(1, 139, 276)]);
}

#[test]
fn the_blend_keeps_the_grid_and_says_so_while_a_neighbour_draws_bins() {
    let fx = Fx::new();
    let t = fx.trace();
    let v = ViewState {
        cursor_hz: Some(900.0),
        ..view(700.0, 1200.0)
    };
    let s = scene(&[t], &v, size(1900.0));
    let sp = s.bins.as_ref().expect("bins drawn");
    assert_eq!(sp.spans[0].len(), 1);
    assert_eq!(sp.spans[0][0].stage, 1);
    // Between the last bin of stage 1 and the first of stage 0 every point is a grid column.
    let d = &sp.traces[0];
    for (i, f) in d.freqs.iter().enumerate() {
        if *f > 276.0 * DF[1] && *f < 1031.25 {
            assert!(sp.span_at(0, i).is_none());
            assert!(fx.c.freqs.contains(f), "{f}");
        }
    }
    let blend = format!(
        "{}–{}: grid",
        format::freq_rough(811.0),
        format::freq_rough(1031.25)
    );
    assert!(
        s.unresolved.iter().any(|m| m.text == blend),
        "{:?}",
        s.unresolved
    );
    // Inside it the cursor reads the grid: the unresolved note, then the blend's.
    assert!(
        readout_freq(&s).ends_with(" · 12 Hz bins · grid"),
        "{}",
        readout_freq(&s)
    );
    // With the resolution marker off the blend is still said: it is why a column is read.
    let unmarked = ViewState {
        unresolved: false,
        ..v
    };
    let off = scene(&[fx.trace()], &unmarked, size(1900.0));
    assert_eq!(
        readout_freq(&off),
        readout_freq(&s).replace(" · 12 Hz bins", "")
    );
    assert!(readout_freq(&off).ends_with(" · grid"));
    // Not shaded: the unresolved ranges alone are.
    let theme = Theme::dark();
    let shades = |s: &TfScene| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.rects)
            .filter(|r| r.color == theme.unresolved_shade)
            .count()
    };
    let without = scene(
        &[TfTrace {
            bins: None,
            ..fx.trace()
        }],
        &v,
        size(1900.0),
    );
    assert_eq!(shades(&s), shades(&without));
    // Zoomed out, no stage draws: no blend mark.
    let s = scene(&[fx.trace()], &view(20.0, 20_000.0), size(800.0));
    assert!(!s.unresolved.iter().any(|m| m.note == "grid"));
}

/// `fx`'s curve as a second measurement's.
fn second(fx: &Fx) -> TfTrace<'_> {
    TfTrace {
        key: TraceKey::Live(MeasId(2)),
        ..fx.trace()
    }
}

/// `Fx::new()` on a ladder of twice the FFT lengths (a long-LF transfer's): every bin,
/// unresolved range and blend at half the frequency, stage 0 covering the band again.
fn long_lf() -> Fx {
    let mut fx = Fx::new();
    let half = |r: &mut FreqRange| {
        r.lo = Hz(r.lo.0 / 2.0);
        r.hi = Hz(r.hi.0 / 2.0);
    };
    if let Some(l) = fx.meta.bins.as_mut() {
        for (b, st) in l.stages.iter_mut().enumerate() {
            st.bin_hz = Hz(DF[b] / 2.0);
            if b == 0 {
                st.bins = 2 * BINS[0];
            }
        }
        l.blends.iter_mut().for_each(half);
    }
    for r in fx.meta.unresolved.iter_mut().flatten() {
        r.lo = Hz(r.lo.0 / 2.0);
        r.hi = Hz(r.hi.0 / 2.0);
        r.bin_hz = Hz(r.bin_hz.0 / 2.0);
    }
    for (b, (_, f)) in fx.stages.iter_mut().enumerate() {
        f.meta.bin_hz = Hz(DF[b] / 2.0);
        if b == 0 {
            let n = 2 * BINS[0] as usize;
            f.mag = (0..n).map(|i| bin_db(0, FIRST[0] + i as u32)).collect();
            f.phase = vec![30.0; n];
            f.coh = vec![0.9; n];
        }
    }
    fx
}

#[test]
fn blends_are_marked_on_the_marked_curves_own_ladder() {
    let (std, long) = (Fx::new(), long_lf());
    let mut stale = Fx::new();
    stale.stages.iter_mut().for_each(|(_, f)| f.meta.blocks = 0);
    let mark = |lo: f64, hi: f64| {
        format!(
            "{}–{}: grid",
            format::freq_rough(lo),
            format::freq_rough(hi)
        )
    };
    let blends = |s: &TfScene| -> Vec<String> {
        s.unresolved
            .iter()
            .filter(|m| m.note == "grid")
            .map(|m| m.text.clone())
            .collect()
    };
    let v = view(300.0, 900.0);
    // The standard curve first (marked) draws stage 1: its own two blends, never the
    // long-LF curve's at half their frequencies.
    let s = scene(&[std.trace(), second(&long)], &v, size(1900.0));
    let sp = s.bins.as_ref().expect("bins drawn");
    assert!(!sp.spans[0].is_empty() && !sp.spans[1].is_empty());
    assert_eq!(
        blends(&s),
        [mark(811.0, 1031.25), mark(202.2, 257.8)],
        "{:?}",
        s.unresolved
    );
    // The marked curve draws none of its stages: no blend is marked, whatever the other
    // curve draws.
    let s = scene(&[stale.trace(), second(&long)], &v, size(1900.0));
    assert!(s.bins.is_some());
    assert_eq!(blends(&s), Vec::<String>::new());
    // The long-LF curve selected: its own blends.
    let selected = TfTrace {
        selected: true,
        ..second(&long)
    };
    let s = scene(&[std.trace(), selected], &v, size(1900.0));
    assert_eq!(
        blends(&s),
        [mark(405.5, 515.625), mark(101.1, 128.9)],
        "{:?}",
        s.unresolved
    );
}

#[test]
fn grid_and_bins_join_in_one_line() {
    let fx = Fx::new();
    let v = view(1000.0, 3000.0);
    let s = scene(&[fx.trace()], &v, size(1900.0));
    let sp = s.bins.as_ref().expect("bins drawn");
    assert_eq!(sp.spans[0][0].stage, 0);
    let mag = &s.scene.layers[1].polylines[0];
    let segs = segments(&mag.points);
    assert_eq!(segs.len(), 1, "one line through grid and bins");
    // The first bin follows the last grid column below it.
    let xm = s.x_axis.mapping;
    let first = xm.to_px(139.0 * DF[0]);
    let below =
        fx.c.freqs
            .iter()
            .rev()
            .find(|f| **f < 139.0 * DF[0])
            .map(|f| xm.to_px(*f))
            .expect("a column below");
    let at = segs[0]
        .iter()
        .position(|p| (p[0] - first).abs() < 1e-3)
        .expect("first bin drawn");
    assert!((segs[0][at - 1][0] - below).abs() < 1e-3);
    // Each bin is a vertex at its level.
    let ym = s.panes[0].y_axis.mapping;
    assert!((segs[0][at][1] - ym.to_px(f64::from(bin_db(0, 139)))).abs() < 1e-3);
    let bins_in_line = segs[0].iter().filter(|p| p[0] >= first - 1e-3).count();
    // Bins 139..=256 (256 · Δf = 3 kHz, the view's top), then the grid column beyond.
    assert_eq!(bins_in_line, 256 - 139 + 1 + 1);
}

#[test]
fn a_stage_frame_not_of_the_shown_curve_draws_the_grid() {
    let v = view(10_000.0, 20_000.0);
    let draws = |fx: &Fx| !drawn(&fx.trace(), &v, 1900.0).is_empty();
    assert!(draws(&Fx::new()));
    let mut fx = Fx::new();
    fx.stages[0].0.config_rev = Rev(4);
    assert!(!draws(&fx), "another configuration");
    let mut fx = Fx::new();
    fx.stages[0].1.meta.delay = Seconds(0.001);
    assert!(!draws(&fx), "another delay");
    let mut fx = Fx::new();
    fx.stages[0].1.meta.nudged = Seconds(0.001);
    assert!(!draws(&fx), "another nudge");
    let mut fx = Fx::new();
    fx.stages[0].1.meta.blocks = 0;
    assert!(!draws(&fx), "no average yet");
    // Running: within a hop and the 250 ms refresh behind the `tf`.
    let slack = HOP[0] as u64 + 12_000;
    let mut fx = Fx::new();
    fx.stages[0].0.audio_sample = SampleIndex(T - slack);
    assert!(draws(&fx));
    fx.stages[0].0.audio_sample = SampleIndex(T - slack - 1);
    assert!(!draws(&fx), "too old");
    // A stage sent alone after the `tf`: ahead by as much, no more.
    fx.stages[0].0.audio_sample = SampleIndex(T + slack);
    assert!(draws(&fx));
    fx.stages[0].0.audio_sample = SampleIndex(T + slack + 1);
    assert!(!draws(&fx), "too far ahead");
    // Stopped: the final frames carry the final `tf`'s stamp, and only those are drawn.
    let mut fx = Fx::new();
    fx.running = false;
    assert!(draws(&fx));
    fx.stages[0].0.audio_sample = SampleIndex(T - 1);
    assert!(!draws(&fx));
    // No frame yet.
    let mut fx = Fx::new();
    fx.stages.clear();
    assert!(!draws(&fx));
}

#[test]
fn the_cursor_reads_the_nearest_bin_with_its_width() {
    let fx = Fx::new();
    let v = ViewState {
        cursor_hz: Some(15_003.0),
        ..view(10_000.0, 20_000.0)
    };
    let s = scene(&[fx.trace()], &v, size(1900.0));
    let cr = s.cursor.as_ref().expect("cursor");
    // Bin 1280 is at 15 kHz exactly.
    assert_eq!(cr.freq_hz, 1280.0 * DF[0]);
    assert_eq!(
        cr.freq,
        format!("{} · 12 Hz bin", format::freq_readout(15_000.0))
    );
    assert_eq!(
        cr.rows[0].magnitude,
        format::db_readout(f64::from(bin_db(0, 1280)))
    );
    assert_eq!(cr.rows[0].phase, format::phase_readout(30.0));
    assert_eq!(
        cr.rows[0].coherence,
        format::coherence_readout(0.9_f32.into())
    );
    // The jump runs over the bins as drawn: the next peak up from 15 kHz is the bin's.
    let found = crate::tf::jump(
        &[fx.trace()],
        &DisplayCache::default(),
        &v,
        s.panes[0].plot.w,
        v.freq.range(),
        crate::cursor_jump::Landmark::Peak,
        crate::cursor_jump::Step::Up,
    )
    .expect("a peak");
    assert_eq!(found, f64::from(PEAK_BIN) * DF[0]);
    // On a plot too narrow for the bins the grid has no such peak.
    let narrow = crate::tf::jump(
        &[fx.trace()],
        &DisplayCache::default(),
        &v,
        800.0,
        v.freq.range(),
        crate::cursor_jump::Landmark::Peak,
        crate::cursor_jump::Step::Up,
    );
    assert_eq!(narrow, None);
}

/// The cursor line in `v` on a plot `plot_px` wide, of the curve `t`.
fn said(t: TfTrace<'_>, v: &ViewState, plot_px: f32) -> String {
    readout_freq(&scene(&[t], v, size(plot_px)))
}

#[test]
fn what_the_cursor_says_in_each_case() {
    let fx = Fx::new();
    let at = |hz: f64, lo: f64, hi: f64| ViewState {
        cursor_hz: Some(hz),
        ..view(lo, hi)
    };
    let zoom = at(15_000.0, 10_000.0, 20_000.0);
    let column = |hz: f64| {
        let i =
            fx.c.freqs
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    (a.1.ln() - hz.ln())
                        .abs()
                        .total_cmp(&(b.1.ln() - hz.ln()).abs())
                })
                .map(|(i, _)| i)
                .expect("a column");
        format::freq_readout(fx.c.freqs[i])
    };
    let grid = format!("{} · 1/96 oct", column(15_000.0));
    // Below 138.5 bins: the unresolved note.
    let s = said(fx.trace(), &at(300.0, 20.0, 20_000.0), 1900.0);
    assert_eq!(s, format!("{} · 2.9 Hz bins", column(300.0)));
    // Less than a pixel apart: a resolved column.
    assert_eq!(said(fx.trace(), &zoom, 800.0), grid);
    // Before the first stage frame.
    let mut none = Fx::new();
    none.stages.clear();
    assert_eq!(said(none.trace(), &zoom, 1900.0), grid);
    // Smoothed.
    let smoothed = TfTrace {
        smoothing: Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::Magnitude,
        }),
        ..fx.trace()
    };
    assert_eq!(said(smoothed, &zoom, 1900.0), grid);
    // A peak hold or peak tracking in the pane.
    let hold = vec![0.0_f32; fx.c.freqs.len()];
    let held = TfTrace {
        hold: Some(&hold),
        ..fx.trace()
    };
    assert_eq!(said(held, &zoom, 1900.0), grid);
    let tracking = TfTrace {
        tracks: true,
        ..fx.trace()
    };
    assert_eq!(said(tracking, &zoom, 1900.0), grid);
    // The unwrapped phase and group delay exist on the grid only: the whole pane draws and
    // reads the grid, magnitude and coherence too, and its jumps find no bin's peak.
    for phase in [
        PhaseView::Unwrapped {
            range: Range::new(-720.0, 180.0),
        },
        PhaseView::GroupDelay {
            range_ms: Range::new(-2.0, 10.0),
        },
    ] {
        let v = ViewState {
            tf: TfView { phase, ..zoom.tf },
            ..zoom
        };
        let s = scene(&[fx.trace()], &v, size(1900.0));
        assert_eq!(readout_freq(&s), grid, "{phase:?}");
        assert!(s.bins.is_none(), "{phase:?}");
        let why = if matches!(phase, PhaseView::Unwrapped { .. }) {
            "bins (grid: unwrapped phase)"
        } else {
            "bins (grid: group delay)"
        };
        assert!(s.legend[0].tags.iter().any(|t| t == why), "{phase:?}");
        assert_eq!(
            s.legend[0]
                .tags
                .iter()
                .filter(|t| t.contains("bins"))
                .count(),
            1
        );
        let found = crate::tf::jump(
            &[fx.trace()],
            &DisplayCache::default(),
            &v,
            1900.0,
            v.freq.range(),
            crate::cursor_jump::Landmark::Peak,
            crate::cursor_jump::Step::Up,
        );
        assert_eq!(found, None, "{phase:?}");
        // Nor is a blend marked as grid where every column is.
        let blend = ViewState {
            tf: TfView { phase, ..zoom.tf },
            cursor_hz: Some(900.0),
            ..view(700.0, 1200.0)
        };
        let s = scene(&[fx.trace()], &blend, size(1900.0));
        assert!(!s.unresolved.iter().any(|m| m.note == "grid"), "{phase:?}");
        assert!(
            !readout_freq(&s).ends_with(" · grid"),
            "{}",
            readout_freq(&s)
        );
    }
    // A stale stage.
    let mut stale = Fx::new();
    stale.stages[0].1.meta.blocks = 0;
    assert_eq!(said(stale.trace(), &zoom, 1900.0), grid);
    // Cross-rig, blank: no layout, so nothing is said of the form.
    let blank = TfTrace {
        bins: None,
        ..fx.trace()
    };
    assert_eq!(said(blank, &zoom, 1900.0), column(15_000.0));
    // A stored curve captured at bins is a 1/96 trace: no bins, nothing said.
    let stored = TfTrace {
        bins: None,
        unresolved: None,
        key: TraceKey::Stored(TraceId(3)),
        freshness: None,
        ..fx.trace()
    };
    assert_eq!(said(stored, &zoom, 1900.0), column(15_000.0));
}

/// A transfer's legend and a math's alike: what forces the grid is per pane, so the pane says
/// it, not the measurement's row.
#[test]
fn a_legend_says_bins_while_a_stage_is_drawn() {
    let fx = Fx::new();
    let zoom = view(10_000.0, 20_000.0);
    let tags =
        |t: TfTrace<'_>, v: &ViewState, px: f32| scene(&[t], v, size(px)).legend[0].tags.clone();
    let has = |tags: &[String], s: &str| tags.iter().any(|t| t == s);
    assert!(has(&tags(fx.trace(), &zoom, 1900.0), "bins"));
    // Its cursor reads the bin.
    let v = ViewState {
        cursor_hz: Some(15_000.0),
        ..zoom
    };
    assert!(said(fx.trace(), &v, 1900.0).ends_with(" · 12 Hz bin"));
    // No stage drawn for the zoom or a stale stage: nothing said, neither is a setting.
    assert!(
        !tags(fx.trace(), &zoom, 800.0)
            .iter()
            .any(|t| t.contains("bins"))
    );
    let mut stale = Fx::new();
    stale.stages[0].1.meta.blocks = 0;
    assert!(
        !tags(stale.trace(), &zoom, 1900.0)
            .iter()
            .any(|t| t.contains("bins"))
    );
    // A setting that keeps the grid: said, with which.
    let smoothed = TfTrace {
        smoothing: Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::Magnitude,
        }),
        ..fx.trace()
    };
    assert!(has(&tags(smoothed, &zoom, 1900.0), "bins (grid: smoothed)"));
    let hold = vec![0.0_f32; fx.c.freqs.len()];
    let held = TfTrace {
        hold: Some(&hold),
        ..fx.trace()
    };
    assert!(has(&tags(held, &zoom, 1900.0), "bins (grid: peak hold)"));
    let tracking = TfTrace {
        tracks: true,
        ..fx.trace()
    };
    assert!(has(&tags(tracking, &zoom, 1900.0), "bins (grid: peaks)"));
    // An average: its range band stays on the grid parts, and is not drawn under the bins.
    let n = fx.c.freqs.len();
    let band = RangeBand {
        lo: vec![-2.0; n],
        hi: vec![3.0; n],
        coh: vec![0.9; n],
    };
    let averaged = TfTrace {
        range: Some(crate::range_band::Source {
            spread: RangeSpread::of(4),
            band: Some(&band),
        }),
        ..fx.trace()
    };
    let v = ViewState {
        range_band: true,
        ..view(1000.0, 3000.0)
    };
    let s = scene(&[averaged], &v, size(1900.0));
    assert!(has(&s.legend[0].tags, "bins"));
    assert!(has(&s.legend[0].tags, "range band on grid parts"));
    assert!(
        s.legend[0]
            .text
            .ends_with(" · bins · range band on grid parts")
    );
    let band = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.bands)
        .next()
        .expect("a band");
    let xm = s.x_axis.mapping;
    let (first, last) = (xm.to_px(139.0 * DF[0]), xm.to_px(256.0 * DF[0]));
    let finite = |p: &&crate::primitives::BandPoint| p.y0.is_finite();
    assert!(band.points.iter().filter(finite).any(|p| p.x < first));
    assert!(
        !band
            .points
            .iter()
            .filter(finite)
            .any(|p| p.x >= first && p.x <= last),
        "no band under the bins"
    );
}

#[test]
fn forced_grid_names_its_reason() {
    let fx = Fx::new();
    let wrapped = PhaseView::Wrapped;
    let unwrapped = PhaseView::Unwrapped {
        range: Range::new(-720.0, 180.0),
    };
    let delay = PhaseView::GroupDelay {
        range_ms: Range::new(-2.0, 10.0),
    };
    assert_eq!(bins::forced(&[fx.trace()], wrapped), None);
    let tracking = || TfTrace {
        tracks: true,
        ..fx.trace()
    };
    assert_eq!(bins::forced(&[tracking()], wrapped), Some(Forced::Peaks));
    let held = || {
        let mut t = tracking();
        if let Some(b) = t.bins.as_mut() {
            b.holds = true;
        }
        t
    };
    // A peak hold before peaks, a phase view before both.
    assert_eq!(bins::forced(&[held()], wrapped), Some(Forced::PeakHold));
    assert_eq!(
        bins::forced(&[held()], unwrapped),
        Some(Forced::UnwrappedPhase)
    );
    assert_eq!(bins::forced(&[held()], delay), Some(Forced::GroupDelay));
    // The curve's own smoothing before anything the pane forces.
    let smoothed = TfTrace {
        smoothing: Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::Magnitude,
        }),
        ..held()
    };
    for phase in [wrapped, unwrapped, delay] {
        let pane = bins::forced(std::slice::from_ref(&smoothed), phase);
        assert_eq!(bins::kept_on_grid(&smoothed, pane), Some(Forced::Smoothed));
    }
    assert_eq!(bins::kept_on_grid(&fx.trace(), None), None);
    // The legend says the one that wins, and only it, zoomed in or out.
    let tags = |t: TfTrace<'_>, v: &ViewState, px: f32| -> Vec<String> {
        scene(&[t], v, size(px)).legend[0]
            .tags
            .iter()
            .filter(|t| t.contains("bins"))
            .cloned()
            .collect()
    };
    let at = |phase: PhaseView, lo: f64, hi: f64| {
        let v = view(lo, hi);
        ViewState {
            tf: TfView { phase, ..v.tf },
            ..v
        }
    };
    for px in [1900.0, 800.0] {
        let zoom = at(wrapped, 10_000.0, 20_000.0);
        assert_eq!(tags(smoothed.clone(), &zoom, px), ["bins (grid: smoothed)"]);
        assert_eq!(
            tags(held(), &at(unwrapped, 10_000.0, 20_000.0), px),
            ["bins (grid: unwrapped phase)"]
        );
        assert_eq!(
            tags(held(), &at(delay, 10_000.0, 20_000.0), px),
            ["bins (grid: group delay)"]
        );
        assert_eq!(tags(held(), &zoom, px), ["bins (grid: peak hold)"]);
        assert_eq!(tags(tracking(), &zoom, px), ["bins (grid: peaks)"]);
    }
    // Zoomed out with no setting: nothing.
    assert!(tags(fx.trace(), &at(wrapped, 20.0, 20_000.0), 800.0).is_empty());
}

#[test]
fn spacing_is_judged_at_the_top_of_the_view() {
    // 1900 px over 20 Hz – 20 kHz: 190.7 px per octave; stage 2's bins at 202 Hz.
    let ppo = bins::px_per_octave(Range::new(20.0, 20_000.0), 1900.0);
    assert!((ppo - 190.65).abs() < 0.01, "{ppo}");
    let s = bins::spacing_px(ppo, DF[2], 207.0 * DF[2]);
    assert!((s - 1.33).abs() < 0.01, "{s}");
}
