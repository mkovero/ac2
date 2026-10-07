use super::*;
use crate::canvas::tests::{intersects, label_box};
use crate::leq::{LeqHistory, leq_tiles, run_text};
use crate::primitives::{Label, Scene};
use crate::spl::spl_readout;
use crate::time::Freshness;
use crate::view::{LeqLayout, LeqStyle};
use ac2_proto::frame::{LeqFlags, LeqFrame, LeqMeta, LeqRun, SplFrame, SplMeta};
use ac2_proto::model::{
    CalBasis, CalStatus, LeqConfig, LeqWindow, LevelScale, PeakWeighting, TimeWeighting, Weighting,
};
use ac2_proto::units::{Db, DbSpl, MeasId, Seconds, WallNs};

const CAL: CalStatus = CalStatus::Verified {
    calibrated_at: WallNs(1),
    basis: CalBasis::Acoustic {
        calibrator_level: DbSpl(94.0),
    },
};

fn readout(stale: Option<f64>) -> SplReadout {
    let f = SplFrame {
        meas: MeasId(4),
        meta: SplMeta {
            scale: LevelScale::DbSpl,
            weighting: Weighting::A,
            time_weighting: TimeWeighting::Fast,
            peak_weighting: PeakWeighting::C,
            level: 94.04,
            lmax: 97.25,
            lmin: 60.0,
            leq: 92.06,
            lpeak: 110.31,
            duration: Seconds(83.9),
            cal: CAL,
            mic_curve: false,
            position: None,
        },
    };
    spl_readout(
        &f,
        95.0,
        "M30 · cal 3 h ago".into(),
        stale.map(Freshness::from_age),
        "meter since 4:01 · R resets".into(),
    )
}

/// Five LAeq windows, 1 … 60 min, limited at 99 dB: over, near, ok, filling, ok.
fn leq() -> (LeqConfig, LeqFrame) {
    let minutes = [1.0, 5.0, 10.0, 30.0, 60.0];
    let windows = minutes
        .iter()
        .map(|m| LeqWindow {
            duration: Seconds(m * 60.0),
            weighting: Weighting::A,
            limit: Some(DbSpl(99.0)),
            warn_margin: Db(3.0),
        })
        .collect();
    let judged = |x| LeqFlags::LIMIT.with(LeqFlags::JUDGED).with(x);
    let f = LeqFrame {
        meas: MeasId(4),
        meta: LeqMeta {
            scale: LevelScale::DbSpl,
            cal: CAL,
            mic_curve: false,
            horizon: Seconds(60.0),
            logged: 3600,
            run: Some(LeqRun {
                started_at: WallNs(1_791_046_920_000_000_000),
                until: WallNs(1_791_054_965_000_000_000),
                measured: Seconds(8033.0),
                gaps: Seconds(12.0),
                trimmed: false,
                laeq: 97.84,
                lceq: 110.2,
                lzeq: 112.0,
            }),
            lcpeak: None,
            lafmax: None,
            position: None,
        },
        leq: vec![101.3, 97.2, 88.4, 96.9, 95.1],
        elapsed: vec![60.0, 300.0, 600.0, 750.0, 3600.0],
        measured: vec![60.0, 300.0, 600.0, 750.0, 3600.0],
        allowed: vec![f32::NAN, 101.5, 105.0, 100.0, 99.5],
        recover: vec![450.0, f32::NAN, f32::NAN, f32::NAN, f32::NAN],
        least: vec![101.3, 97.2, 88.4, 93.1, 95.1],
        over_in: vec![f32::NAN; 5],
        flags: vec![
            judged(LeqFlags::OVER.with(LeqFlags::CANNOT_RECOVER)),
            judged(LeqFlags::NEAR),
            judged(LeqFlags::NONE),
            judged(LeqFlags::NONE),
            judged(LeqFlags::NONE),
        ],
    };
    (
        LeqConfig {
            windows,
            horizon: Seconds(60.0),
            peaks: Default::default(),
        },
        f,
    )
}

fn view<'a>(
    c: &'a LeqConfig,
    f: &LeqFrame,
    h: Option<&'a LeqHistory>,
    l: LeqLayout,
) -> LeqView<'a> {
    LeqView {
        meter: "FOH SPL".into(),
        cal: "M30 · cal 3 h ago".into(),
        cfg: c,
        tiles: leq_tiles(c, f),
        history: h,
        stale: None,
        scale: f.meta.scale,
        layout: l,
        run: f.meta.run.map(|r| run_text(&r, c, |_| 7200)),
    }
}

fn history(c: &LeqConfig, f: &LeqFrame) -> LeqHistory {
    let mut h = LeqHistory::default();
    for k in 0..30 {
        h.push(c, f, 1000.0 + f64::from(k));
    }
    h
}

fn vp(w: f32, h: f32) -> Viewport {
    Viewport {
        width: w,
        height: h,
    }
}

fn labels(s: &Scene) -> Vec<&Label> {
    s.layers.iter().flat_map(|l| &l.labels).collect()
}

/// The rectangles the windows take: columns, tiles, the history plot.
fn window_rects(s: &LeqScene) -> Vec<Rect> {
    s.tiles
        .iter()
        .copied()
        .chain(
            s.columns
                .iter()
                .flat_map(|k| k.columns.iter().map(|x| x.rect)),
        )
        .chain(s.history.iter().map(|h| h.plot))
        .collect()
}

fn layouts() -> [LeqLayout; 4] {
    [
        LeqLayout {
            style: LeqStyle::Columns,
            history: false,
        },
        LeqLayout {
            style: LeqStyle::Columns,
            history: true,
        },
        LeqLayout {
            style: LeqStyle::Tiles,
            history: false,
        },
        LeqLayout {
            style: LeqStyle::Tiles,
            history: true,
        },
    ]
}

/// A full-screen pane: the number centred in the top third under the caption, its name
/// and unit and the live bar under it, the windows below — as large as on their own
/// minus the third.
#[test]
fn meter_takes_the_top_third_and_the_windows_the_rest() {
    let th = Theme::dark();
    let r = readout(None);
    let (c, f) = leq();
    let h = history(&c, &f);
    for layout in layouts() {
        for (w, ht) in [(1920.0, 1080.0), (1280.0, 720.0), (800.0, 600.0)] {
            let v = view(&c, &f, Some(&h), layout);
            let s = meter_leq_scene(&r, &v, &Status::default(), &th, vp(w, ht));
            let at = format!("{layout:?} at {w}×{ht}");
            let m = s.meter;
            assert_eq!(m.form, MeterForm::Block, "{at}");
            // A third of the pane under the caption.
            let under = ht - s.leq.caption.bottom() - 10.0;
            assert!(
                (m.region.h - under / 3.0).abs() < 1.0,
                "{at}: {:?}",
                m.region
            );
            assert!(m.region.y >= s.leq.caption.bottom() - 0.5, "{at}");
            let all = labels(&s.leq.scene);
            let number = all.iter().find(|l| l.text == r.value).expect("number");
            let b = label_box(number);
            let cx = w / 2.0;
            assert!((b.x + b.w / 2.0 - cx).abs() < 0.6, "{at}: not centred");
            assert!(b.y >= m.region.y - 0.5 && b.bottom() <= m.region.bottom() + 0.5);
            // The number reads from across the room on a full screen.
            if w >= 1920.0 {
                assert!(number.size > 150.0, "{at}: {}", number.size);
            }
            // The meter's number is the one big number: every text of the windows, their
            // values first, stays well under it.
            for l in all.iter().filter(|l| l.pos[1] >= m.region.bottom()) {
                assert!(
                    l.size <= number.size * 0.3,
                    "{at}: {:?} at {} next to {}",
                    l.text,
                    l.size,
                    number.size
                );
            }
            let bar = m.bar.expect("bar");
            assert!(bar.y > b.bottom() && bar.bottom() <= m.region.bottom() + 0.5);
            for x in window_rects(&s.leq) {
                assert!(
                    x.y >= m.region.bottom() - 0.5,
                    "{at}: {x:?} under the meter"
                );
                assert!(x.bottom() <= ht + 0.5, "{at}: {x:?}");
            }
            assert!(!window_rects(&s.leq).is_empty(), "{at}");
            if layout.history {
                assert!(s.leq.history.is_some(), "{at}: history");
            }
        }
    }
}

/// One caption for the pane: the meter's name, the run (with the history on) and the
/// calibration once each, the number's name and unit once under it; none of the meter
/// view's statistics, its heading, its calibration footer or its own STALE.
#[test]
fn one_caption_for_both_parts() {
    let th = Theme::dark();
    let (c, f) = leq();
    for stale in [None, Some(4.0)] {
        let r = readout(stale);
        let mut v = view(
            &c,
            &f,
            None,
            LeqLayout {
                history: true,
                ..LeqLayout::default()
            },
        );
        v.stale = stale.map(|a| format!("STALE {}", crate::format::age(a)));
        for (w, ht) in [(1920.0, 1080.0), (640.0, 400.0), (320.0, 200.0)] {
            let s = meter_leq_scene(&r, &v, &Status::default(), &th, vp(w, ht));
            let texts: Vec<&str> = labels(&s.leq.scene)
                .iter()
                .map(|l| l.text.as_str())
                .collect();
            let count = |p: &dyn Fn(&str) -> bool| texts.iter().filter(|t| p(t)).count();
            let at = format!("{w}×{ht} ({stale:?})");
            assert_eq!(count(&|t| t.starts_with("FOH SPL")), 1, "{at}: {texts:?}");
            // Narrow and stale, the calibration is cut short (its STALE first).
            let cal = count(&|t| t.contains("cal 3 h ago"));
            assert_eq!(
                cal,
                usize::from(w >= 640.0 || stale.is_none()),
                "{at}: {texts:?}"
            );
            assert_eq!(count(&|t| t.contains("2:14:05")), 1, "{at}: {texts:?}");
            assert_eq!(
                count(&|t| t.contains("STALE")),
                usize::from(stale.is_some())
            );
            assert_eq!(count(&|t| t.contains("LAF")), 1, "{at}: {texts:?}");
            assert_eq!(count(&|t| t == "94.0"), 1, "{at}: {texts:?}");
            for gone in ["meter since", "LCpeak", "LAFmax", "LAFmin", "LAeq 92.1"] {
                assert_eq!(count(&|t| t.contains(gone)), 0, "{at}: {gone} in {texts:?}");
            }
        }
    }
}

/// The number dims with a stale frame, as in the meter view.
#[test]
fn stale_meter_dims_its_number() {
    let th = Theme::dark();
    let (c, f) = leq();
    let v = view(&c, &f, None, LeqLayout::default());
    for (stale, want) in [(None, th.text), (Some(4.0), th.text_dim)] {
        let r = readout(stale);
        let s = meter_leq_scene(&r, &v, &Status::default(), &th, vp(1280.0, 720.0));
        let n = labels(&s.leq.scene)
            .into_iter()
            .find(|l| l.text == "94.0")
            .expect("number");
        assert_eq!(n.color, want);
    }
}

/// Shorter panes: one line (number, name and unit) over the windows, then the windows
/// alone; the meter's part never grows past its share.
#[test]
fn short_panes_take_one_line_then_the_windows_alone() {
    let th = Theme::dark();
    let r = readout(None);
    let (c, f) = leq();
    let v = view(&c, &f, None, LeqLayout::default());
    let form = |w: f32, h: f32| {
        meter_leq_scene(&r, &v, &Status::default(), &th, vp(w, h))
            .meter
            .form
    };
    assert_eq!(form(960.0, 540.0), MeterForm::Block);
    assert_eq!(form(960.0, 260.0), MeterForm::Line);
    assert_eq!(form(320.0, 200.0), MeterForm::Line);
    assert_eq!(form(960.0, 150.0), MeterForm::Hidden);
    let s = meter_leq_scene(&r, &v, &Status::default(), &th, vp(960.0, 260.0));
    let all = labels(&s.leq.scene);
    let n = all.iter().find(|l| l.text == "94.0").expect("number");
    let cap = all.iter().find(|l| l.text == "LAF · dB SPL").expect("name");
    assert_eq!(n.pos[1], cap.pos[1], "one baseline");
    assert!(cap.pos[0] > n.pos[0]);
    assert!(s.meter.bar.is_none());
    let s = meter_leq_scene(&r, &v, &Status::default(), &th, vp(960.0, 150.0));
    assert!(!labels(&s.leq.scene).iter().any(|l| l.text == "94.0"));
    assert_eq!(s.leq.columns.as_ref().map(|k| k.columns.len()), Some(5));
}

/// From a small pane of the grid to a full screen, columns and tiles, with and without the
/// history strip: the meter's labels stay in their region and the pane and overlap nothing,
/// the caption's labels stay apart and above both parts, the windows start under the meter;
/// with columns and no strip, no two labels of the scene overlap at all.
#[test]
fn meter_and_windows_never_overlap() {
    let th = Theme::dark();
    let (c, f) = leq();
    let h = history(&c, &f);
    for stale in [None, Some(9.0)] {
        let r = readout(stale);
        for layout in layouts() {
            let widths = (320..=960).step_by(64).chain([1280, 1600, 1920]);
            for w in widths {
                let heights = (200..=440).step_by(20).chain((500..=1080).step_by(60));
                for ht in heights {
                    let (w, ht) = (w as f32, ht as f32);
                    let mut v = view(&c, &f, Some(&h), layout);
                    v.stale = stale.map(|a| format!("STALE {}", crate::format::age(a)));
                    let s = meter_leq_scene(&r, &v, &Status::default(), &th, vp(w, ht));
                    let at = format!("{layout:?} at {w}×{ht} ({stale:?})");
                    check(&s, &r, w, ht, layout, &at);
                }
            }
        }
    }
}

fn check(s: &MeterLeqScene, r: &SplReadout, w: f32, ht: f32, layout: LeqLayout, at: &str) {
    let all = labels(&s.leq.scene);
    let boxes: Vec<(&str, Rect)> = all
        .iter()
        .map(|l| (l.text.as_str(), label_box(l)))
        .collect();
    let cap = s.leq.caption;
    let is_caption = |l: &Label| l.pos[1] >= cap.y - 0.5 && l.pos[1] < cap.bottom();
    let caption: Vec<(&str, Rect)> = all
        .iter()
        .filter(|l| is_caption(l))
        .map(|l| (l.text.as_str(), label_box(l)))
        .collect();
    // The run only with the history on.
    assert_eq!(
        caption.len(),
        2 + usize::from(layout.history),
        "{at}: {caption:?}"
    );
    for (i, (t, b)) in caption.iter().enumerate() {
        assert!(b.x >= -0.5 && b.right() <= w + 0.5, "{at}: {t:?} {b:?}");
        assert!(
            b.bottom() <= cap.bottom() + 0.5,
            "{at}: {t:?} under the caption"
        );
        for (u, d) in &caption[i + 1..] {
            assert!(!intersects(*b, *d), "{at}: {t:?} overlaps {u:?}");
        }
    }
    let m = s.meter;
    let meter: Vec<(&str, Rect)> = boxes
        .iter()
        .filter(|(t, _)| *t == r.value || (m.form != MeterForm::Hidden && t.starts_with("LAF")))
        .copied()
        .collect();
    match m.form {
        MeterForm::Hidden => assert!(meter.is_empty(), "{at}: {meter:?}"),
        _ => assert_eq!(meter.len(), 2, "{at}: {meter:?}"),
    }
    assert!(
        m.region.y >= cap.bottom() - 0.5,
        "{at}: meter over the caption"
    );
    for (t, b) in &meter {
        assert!(
            b.x >= m.region.x - 0.5
                && b.right() <= m.region.right() + 0.5
                && b.y >= m.region.y - 0.5
                && b.bottom() <= m.region.bottom() + 0.5,
            "{at}: {t:?} {b:?} outside {:?}",
            m.region
        );
        for (u, d) in &boxes {
            if meter.iter().any(|(mt, mb)| mt == u && mb == d) {
                continue;
            }
            assert!(!intersects(*b, *d), "{at}: {t:?} overlaps {u:?}");
        }
        if let Some(bar) = m.bar {
            assert!(!intersects(*b, bar), "{at}: bar over {t:?}");
        }
    }
    for x in window_rects(&s.leq) {
        assert!(
            x.y >= m.region.bottom() - 0.5,
            "{at}: {x:?} under the meter"
        );
        assert!(
            x.bottom() <= ht + 0.5 && x.right() <= w + 0.5,
            "{at}: {x:?}"
        );
    }
    if let Some(k) = &s.leq.columns {
        assert_eq!(k.columns.len(), 5, "{at}");
        for x in &k.columns {
            assert!(x.track.h >= 20.0, "{at}: track {:?}", x.track);
        }
    } else {
        assert_eq!(s.leq.tiles.len(), 5, "{at}");
    }
    if layout.style == LeqStyle::Columns && s.leq.history.is_none() {
        for (i, (t, b)) in boxes.iter().enumerate() {
            for (u, d) in &boxes[i + 1..] {
                assert!(!intersects(*b, *d), "{at}: {t:?} overlaps {u:?}");
            }
        }
    }
    // A pane of the grid keeps the meter; only a very short one gives it up.
    if ht >= 200.0 {
        assert_ne!(m.form, MeterForm::Hidden, "{at}");
    }
    if ht >= 480.0 {
        assert_eq!(m.form, MeterForm::Block, "{at}");
    }
}
