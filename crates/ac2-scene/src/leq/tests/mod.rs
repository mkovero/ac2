use super::*;
use ac2_proto::frame::{LeqFlags, LeqMeta, LeqRun};
use ac2_proto::model::CalStatus;
use ac2_proto::units::{Db, DbSpl, MeasId, Seconds, WallNs};

mod caption;
mod colours;
mod columns;
mod tiles;

fn w(minutes: f64, limit: Option<f64>) -> LeqWindow {
    LeqWindow {
        cue: None,
        duration: Seconds(minutes * 60.0),
        weighting: Weighting::A,
        limit: limit.map(DbSpl),
        warn_margin: Db(3.0),
    }
}

fn cfg() -> LeqConfig {
    LeqConfig {
        windows: vec![
            w(1.0, None),
            w(15.0, Some(100.0)),
            w(30.0, Some(99.0)),
            w(60.0, Some(93.0)),
        ],
        horizon: Seconds(60.0),
        peaks: Default::default(),
    }
}

fn judged(extra: LeqFlags) -> LeqFlags {
    LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(extra)
}

/// 17:02 UTC on 3 October 2026: 19:02 at UTC+2.
const START_S: u64 = 1_791_046_920;
const UTC_PLUS_2: i32 = 7200;

/// A run of 2:14:05 from 19:02 local, 12 s of it not measured.
fn run() -> LeqRun {
    LeqRun {
        started_at: WallNs(START_S * 1_000_000_000),
        until: WallNs((START_S + 8045) * 1_000_000_000 + 400_000_000),
        measured: Seconds(8033.0),
        gaps: Seconds(12.4),
        trimmed: false,
        laeq: 97.84,
        lceq: 110.21,
        lzeq: 112.0,
    }
}

fn run_of(c: &LeqConfig, f: &LeqFrame) -> Option<LeqRunText> {
    f.meta.run.map(|r| run_text(&r, c, |_| UTC_PLUS_2))
}

fn frame(scale: LevelScale) -> LeqFrame {
    LeqFrame {
        meas: MeasId(4),
        meta: LeqMeta {
            scale,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(1),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0),
                },
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            logged: 3600,
            run: Some(run()),
            lcpeak: None,
            lafmax: None,
            position: None,
        },
        leq: vec![97.84, 98.26, 96.94, f32::NAN],
        elapsed: vec![60.0, 900.0, 750.0, 3600.0],
        measured: vec![60.0, 900.0, 750.0, 3480.0],
        allowed: vec![f32::NAN, 101.56, 100.03, f32::NAN],
        recover: vec![f32::NAN, f32::NAN, f32::NAN, 450.0],
        // The 30 min window at 12:30: its 750 s at 96.94 dB over 1800 s.
        least: vec![97.84, 98.26, 93.13, f32::NAN],
        over_in: vec![f32::NAN; 4],
        flags: vec![
            LeqFlags::NONE,
            judged(LeqFlags::NEAR),
            judged(LeqFlags::NONE),
            judged(LeqFlags::OVER)
                .with(LeqFlags::CANNOT_RECOVER)
                .with(LeqFlags::INCOMPLETE),
        ],
    }
}

fn size(w: f32, h: f32) -> Viewport {
    Viewport {
        width: w,
        height: h,
    }
}

fn texts(s: &Scene) -> Vec<&str> {
    s.layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|l| l.text.as_str()))
        .collect()
}

/// `n` windows (1 … 120 min, some C-weighted when `mixed`), listed longest first so the
/// columns must reorder them, each limited at 99 dB; the frame puts them over, near, ok,
/// filling, unmeasured and filling on course in turn.
fn many(n: usize, mixed: bool, scale: LevelScale) -> (LeqConfig, LeqFrame) {
    let minutes = [120.0, 60.0, 30.0, 15.0, 10.0, 5.0, 2.0, 1.0];
    let windows: Vec<LeqWindow> = minutes[8 - n..]
        .iter()
        .enumerate()
        .map(|(i, m)| LeqWindow {
            weighting: if mixed && i % 3 == 1 {
                Weighting::C
            } else {
                Weighting::A
            },
            ..w(*m, Some(99.0))
        })
        .collect();
    let judge = |extra| match scale {
        LevelScale::DbSpl => judged(extra),
        LevelScale::Dbfs => LeqFlags::LIMIT,
    };
    let mut f = frame(scale);
    f.leq.clear();
    f.elapsed.clear();
    f.measured.clear();
    f.allowed.clear();
    f.recover.clear();
    f.least.clear();
    f.over_in.clear();
    f.flags.clear();
    for (i, win) in windows.iter().enumerate() {
        let on_course = i % 6 == 5;
        let (leq, flags, elapsed) = match i % 6 {
            0 => (
                101.3,
                judge(LeqFlags::OVER.with(LeqFlags::CANNOT_RECOVER)),
                1.0,
            ),
            1 => (97.2, judge(LeqFlags::NEAR), 1.0),
            2 => (88.4, judge(LeqFlags::NONE), 0.4),
            3 => (100.6, judge(LeqFlags::OVER), 1.0),
            4 => (f32::NAN, judge(LeqFlags::NONE), 0.0),
            _ => (102.4, judge(LeqFlags::NEAR.with(LeqFlags::ON_COURSE)), 0.25),
        };
        let leq = match scale {
            LevelScale::DbSpl => leq,
            LevelScale::Dbfs => leq - 120.0,
        };
        f.leq.push(leq);
        f.elapsed.push((win.duration.0 * elapsed) as f32);
        f.measured.push((win.duration.0 * elapsed) as f32);
        f.allowed.push(if i % 6 == 0 { f32::NAN } else { 101.56 });
        f.recover.push(if i % 6 == 0 { 450.0 } else { f32::NAN });
        f.least.push(leq + (10.0 * elapsed.log10()) as f32);
        f.over_in.push(if on_course {
            (win.duration.0 * 0.5) as f32
        } else {
            f32::NAN
        });
        f.flags.push(flags);
    }
    (
        LeqConfig {
            windows,
            horizon: Seconds(60.0),
            peaks: Default::default(),
        },
        f,
    )
}

fn columns_view<'a>(c: &'a LeqConfig, f: &LeqFrame, h: Option<&'a LeqHistory>) -> LeqView<'a> {
    LeqView {
        meter: "FOH SPL".into(),
        cal: "M30 · cal 3 h ago".into(),
        cfg: c,
        tiles: leq_tiles(c, f),
        history: h,
        stale: None,
        scale: f.meta.scale,
        layout: LeqLayout::default(),
        run: run_of(c, f),
        stage: false,
        chrome: crate::view::PlotChrome::Full,
    }
}

fn cols(s: &LeqScene) -> &LeqColumns {
    s.columns.as_ref().expect("columns")
}

/// Every label of the scene that is not the caption, with the column it sits in.
fn column_labels(s: &LeqScene) -> Vec<(&crate::primitives::Label, Option<usize>)> {
    let k = cols(s);
    s.scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .filter(|l| l.pos[1] >= s.caption.bottom())
        .map(|l| {
            let b = crate::canvas::tests::label_box(l);
            let at = k
                .columns
                .iter()
                .position(|x| b.x >= x.rect.x - 0.5 && b.right() <= x.rect.right() + 0.5);
            (l, at)
        })
        .collect()
}

/// The caption's labels (meter, run, calibration) stay apart, inside the pane and above
/// the windows; with the history on the run is there at least as its clock, without it
/// the run is not there at all.
fn assert_caption_fits(s: &LeqScene, w: f32, history: bool, at: &str) {
    let caption: Vec<(&str, Rect)> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .filter(|l| l.pos[1] >= s.caption.y - 0.5 && l.pos[1] < s.caption.bottom())
        .map(|l| (l.text.as_str(), crate::canvas::tests::label_box(l)))
        .collect();
    assert_eq!(s.run.is_some(), history, "{at}: run {:?}", s.run);
    assert_eq!(caption.len(), 2 + usize::from(history), "{at}: {caption:?}");
    for (i, (t, b)) in caption.iter().enumerate() {
        assert!(b.x >= -0.5 && b.right() <= w + 0.5, "{at}: {t:?} {b:?}");
        assert!(
            b.bottom() <= s.caption.bottom() + 0.5,
            "{at}: {t:?} {b:?} below the caption"
        );
        for (u, c) in &caption[i + 1..] {
            assert!(
                !crate::canvas::tests::intersects(*b, *c),
                "{at}: {t:?} overlaps {u:?}"
            );
        }
    }
}
