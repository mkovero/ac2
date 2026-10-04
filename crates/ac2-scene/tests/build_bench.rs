//! Bench-style test: how long a rebuild of the heaviest views takes and how many line
//! points it hands the renderer. Prints figures and never fails on them; meaningful in an
//! optimized build:
//!
//! ```text
//! cargo test -p ac2-scene --release --test build_bench -- --nocapture
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_proto::GridDef;
use ac2_proto::frame::{LeqFlags, LeqFrame, LeqMeta, ValidityMask};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_scene::banner::Status;
use ac2_scene::grid::column_frequencies;
use ac2_scene::leq::{LeqHistory, LeqView, leq_scene, leq_tiles};
use ac2_scene::primitives::{Scene, Viewport};
use ac2_scene::tf::transfer_scene;
use ac2_scene::theme::Theme;
use ac2_scene::time::Freshness;
use ac2_scene::trace::{DisplayCache, TfTrace, TimeBase, TraceKey};
use ac2_scene::view::{LeqLayout, LeqStyle, ViewState};

const SIZE: Viewport = Viewport {
    width: 1280.0,
    height: 800.0,
};

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn line_points(s: &Scene) -> usize {
    s.layers
        .iter()
        .flat_map(|l| &l.polylines)
        .map(|p| p.points.len())
        .sum()
}

fn time(n: usize, mut f: impl FnMut()) -> Duration {
    median(
        (0..n)
            .map(|_| {
                let t0 = Instant::now();
                f();
                t0.elapsed()
            })
            .collect(),
    )
}

fn leq_window(minutes: f64, limit: f64) -> LeqWindow {
    LeqWindow {
        duration: Seconds(minutes * 60.0),
        weighting: Weighting::A,
        limit: Some(DbSpl(limit)),
        warn_margin: Db(3.0),
    }
}

fn leq_frame(levels: &[f32]) -> LeqFrame {
    let n = levels.len();
    LeqFrame {
        meas: MeasId(4),
        meta: LeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            horizon: Seconds(60.0),
            logged: 3600,
            run: None,
        },
        leq: levels.to_vec(),
        elapsed: vec![3600.0; n],
        measured: vec![3600.0; n],
        allowed: vec![f32::NAN; n],
        recover: vec![f32::NAN; n],
        least: levels.to_vec(),
        over_in: vec![f32::NAN; n],
        flags: levels
            .iter()
            .map(|l| {
                let f = LeqFlags::LIMIT.with(LeqFlags::JUDGED);
                if *l > 98.0 { f.with(LeqFlags::OVER) } else { f }
            })
            .collect(),
    }
}

#[test]
fn leq_view_with_four_hours_of_four_windows() {
    let cfg = LeqConfig {
        windows: vec![
            leq_window(5.0, 102.0),
            leq_window(15.0, 100.0),
            leq_window(30.0, 99.0),
            leq_window(60.0, 98.0),
        ],
        horizon: Seconds(60.0),
    };
    let mut h = LeqHistory::default();
    let level = |k: u32, w: usize| 95.0 + 4.0 * ((f64::from(k) * 0.01 + w as f64).sin() as f32);
    let n = 4 * 3600;
    for k in 0..n {
        let levels: Vec<f32> = (0..4).map(|w| level(k, w)).collect();
        h.push(&cfg, &leq_frame(&levels), f64::from(k));
    }
    let f = leq_frame(&[95.0; 4]);
    let theme = Theme::dark();
    let view = |h: &LeqHistory| {
        let v = LeqView {
            meter: "FOH SPL".into(),
            cal: "not calibrated".into(),
            cfg: &cfg,
            tiles: leq_tiles(&cfg, &f),
            history: Some(h),
            stale: None,
            scale: LevelScale::DbSpl,
            horizon: "1 min".into(),
            layout: LeqLayout {
                style: LeqStyle::Columns,
                history: true,
            },
            run: None,
        };
        leq_scene(&v, &Status::default(), &theme, SIZE)
    };
    let s = view(&h);
    let points = line_points(&s.scene);
    // A new frame each time: the strip is laid out again.
    let mut k = n;
    let fresh = time(20, || {
        let levels: Vec<f32> = (0..4).map(|w| level(k, w)).collect();
        h.push(&cfg, &leq_frame(&levels), f64::from(k));
        k += 1;
        std::hint::black_box(view(&h));
    });
    // The same history: a rebuild for another reason (a key, the stale clock).
    let same = time(50, || {
        std::hint::black_box(view(&h));
    });
    eprintln!(
        "leq 4 windows x 4 h @{}x{}: {points} line points; build with a new frame {fresh:?}, \
         rebuild without one {same:?}",
        SIZE.width, SIZE.height
    );
}

fn grid() -> GridDef {
    GridDef::Log {
        ppo: 48,
        k_min: -240,
        k_max: 200,
    }
}

fn stored(id: u32, freqs: &[f64]) -> TraceData {
    let n = freqs.len();
    TraceData {
        meta: TraceMeta {
            id: TraceId(id),
            edit: TraceEdit {
                name: format!("t{id}"),
                color: Rgb { r: 1, g: 2, b: 3 },
                visible: true,
                locked: false,
                order: id,
                offset: Db(0.0),
                polarity: Polarity::Normal,
                delay_nudge: Seconds(0.0),
                slot: None,
                smoothing: None,
            },
            kind: TraceKind::Transfer,
            source: TraceSource::Captured {
                meas: MeasId(1),
                meas_name: "live".into(),
                epoch: SessionEpoch(1),
                at_sample: SampleIndex(0),
            },
            grid_id: grid().id(),
            delay: Seconds(0.0),
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(0),
        },
        mag_db: (0..n)
            .map(|i| ((i as f32) * 0.05 + id as f32).sin() * 6.0)
            .collect(),
        phase_deg: Some(
            (0..n)
                .map(|i| ((i as f32) * (3.0 + id as f32)) % 360.0 - 180.0)
                .collect(),
        ),
        coherence: Some(
            (0..n)
                .map(|i| 0.5 + 0.5 * ((i as f32) * 0.1).cos())
                .collect(),
        ),
        sweep: None,
    }
}

#[test]
fn transfer_view_with_a_live_trace_and_eight_stored() {
    let freqs = column_frequencies(&grid());
    let n = freqs.len();
    let traces_data: Vec<Arc<TraceData>> = (1..=8).map(|i| Arc::new(stored(i, &freqs))).collect();
    let mag = vec![0.0f32; n];
    let phase = vec![10.0f32; n];
    let coh = vec![0.9f32; n];
    let validity = vec![ValidityMask::NONE; n];
    let theme = Theme::dark();
    let view = ViewState::default();
    let live = TfTrace {
        key: TraceKey::Live(MeasId(1)),
        name: "live".into(),
        color: theme.trace_color(0),
        freqs: &freqs,
        mag_db: &mag,
        phase_deg: Some(&phase),
        coherence: Some(&coh),
        validity: Some(&validity),
        offset_db: 0.0,
        polarity: Polarity::Normal,
        nudge: Seconds(0.0),
        time_base: TimeBase::Shared {
            epoch: SessionEpoch(1),
            delay: Seconds(0.001),
        },
        freshness: Some(Freshness::from_age(0.1)),
        smoothing: None,
        stored: None,
    };
    let traces: Vec<TfTrace<'_>> = std::iter::once(live)
        .chain(traces_data.iter().map(|d| TfTrace::stored(d, &freqs)))
        .collect();
    let cache = DisplayCache::default();
    let s = transfer_scene(&traces, &cache, &Status::default(), &view, &theme, SIZE);
    let points = line_points(&s.scene);
    let cold = time(20, || {
        std::hint::black_box(transfer_scene(
            &traces,
            &DisplayCache::default(),
            &Status::default(),
            &view,
            &theme,
            SIZE,
        ));
    });
    let kept = time(20, || {
        std::hint::black_box(transfer_scene(
            &traces,
            &cache,
            &Status::default(),
            &view,
            &theme,
            SIZE,
        ));
    });
    eprintln!(
        "transfer 1 live + 8 stored x {n} columns: {points} line points; build working out \
         every stored trace {cold:?}, with them kept {kept:?}"
    );
}
