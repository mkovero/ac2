use super::*;
use ac2_proto::model::AlarmSubject;

/// A long calibration text (mic, data sheet, curve) on the right still leaves the run centred
/// on the pane, as the meter's number under it is: a shorter wording centred beats a longer
/// one pushed toward the meter's name.
#[test]
fn run_stays_centred_beside_a_long_calibration() {
    let th = Theme::dark();
    let (c, f) = many(5, false, LevelScale::DbSpl);
    for (w, h) in [(1920.0, 1080.0), (1280.0, 720.0)] {
        let v = LeqView {
            cal: "MM1 34804 · electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 17 h ago · mic curve: MM1 34804 90°".into(),
            layout: LeqLayout {
                style: LeqStyle::Columns,
                history: true,
            },
            ..columns_view(&c, &f, None)
        };
        let s = leq_scene(&v, &Status::default(), &th, size(w, h));
        let run = s.run.clone().expect("run");
        let l = s
            .scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .find(|l| l.text == run)
            .expect("run label");
        let b = crate::canvas::tests::label_box(l);
        assert!(
            (b.x + b.w / 2.0 - w / 2.0).abs() < 0.5,
            "{w}: {run:?} {b:?}"
        );
        assert_caption_fits(&s, w, true, &format!("{w}"));
    }
}

/// The run in the caption at every width, in columns and tiles: the longest wording that
/// fits, shortened to the clock and the total and then to the clock, never overlapping
/// the meter or the calibration; large on a full-screen pane.
#[test]
fn caption_run_fits_every_width() {
    let th = Theme::dark();
    for style in [LeqStyle::Columns, LeqStyle::Tiles] {
        for mixed in [false, true] {
            let (c, f) = many(5, mixed, LevelScale::DbSpl);
            for (w, h) in [
                (320.0, 240.0),
                (320.0, 480.0),
                (480.0, 320.0),
                (640.0, 400.0),
                (960.0, 540.0),
                (1280.0, 720.0),
                (1920.0, 1080.0),
            ] {
                let v = LeqView {
                    layout: LeqLayout {
                        style,
                        history: true,
                    },
                    ..columns_view(&c, &f, None)
                };
                let s = leq_scene(&v, &Status::default(), &th, size(w, h));
                let at = format!("{style:?} ({mixed}) at {w}×{h}");
                assert_caption_fits(&s, w, true, &at);
                // The windows start under the caption.
                for r in s.tiles.iter().chain(
                    s.columns
                        .iter()
                        .flat_map(|k| k.columns.iter().map(|x| &x.rect)),
                ) {
                    assert!(r.y >= s.caption.bottom() - 0.5, "{at}: {r:?}");
                }
                let run = s.run.clone().unwrap_or_default();
                assert!(
                    run.starts_with("running 2:14:05") || run.starts_with("2:14:05"),
                    "{at}: {run}"
                );
                // Centred on the pane when the sides leave room, as the meter's number is.
                if w >= 1280.0 {
                    let l = s
                        .scene
                        .layers
                        .iter()
                        .flat_map(|l| &l.labels)
                        .find(|l| l.text == run)
                        .expect("run");
                    let b = crate::canvas::tests::label_box(l);
                    assert!((b.x + b.w / 2.0 - w / 2.0).abs() < 0.5, "{at}: {b:?}");
                }
                if w >= 1280.0 {
                    assert!(
                        run.starts_with("running 2:14:05 since 19:02 · LAeq total 97.8"),
                        "{at}: {run}"
                    );
                }
                if w >= 1920.0 {
                    let want = if mixed {
                        "running 2:14:05 since 19:02 · LAeq total 97.8 · LCeq total 110.2 · offline 12 s"
                    } else {
                        "running 2:14:05 since 19:02 · LAeq total 97.8 · offline 12 s"
                    };
                    assert_eq!(run, want, "{at}");
                }
            }
        }
    }
    // The stage: a full-screen pane writes the run large, on the meter's row.
    let (c, f) = many(5, false, LevelScale::DbSpl);
    let s = leq_scene(
        &LeqView {
            layout: LeqLayout {
                style: LeqStyle::Columns,
                history: true,
            },
            ..columns_view(&c, &f, None)
        },
        &Status::default(),
        &th,
        size(1920.0, 1080.0),
    );
    let label = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .find(|l| Some(&l.text) == s.run.as_ref())
        .expect("run label");
    assert!(label.size >= 2.0 * th.font_size, "{}", label.size);
    assert!((label.pos[1] - s.caption.y).abs() < 0.5);
    // At 320 px it has a row of its own, shortened.
    let narrow = leq_scene(
        &LeqView {
            layout: LeqLayout {
                style: LeqStyle::Columns,
                history: true,
            },
            ..columns_view(&c, &f, None)
        },
        &Status::default(),
        &th,
        size(320.0, 480.0),
    );
    let run = narrow.run.clone().expect("run");
    assert!(
        run.len() < "running 2:14:05 since 19:02 · LAeq total 97.8".len(),
        "{run}"
    );
    // Without a run (nothing logged yet) the caption is the meter and the calibration.
    let none = LeqView {
        run: None,
        ..columns_view(&c, &f, None)
    };
    let s = leq_scene(&none, &Status::default(), &th, size(960.0, 540.0));
    assert_eq!(s.run, None);
}

#[test]
fn run_wordings() {
    let c = cfg();
    let r = run_text(&run(), &c, |_| UTC_PLUS_2);
    assert_eq!(r.clock, "2:14:05");
    assert_eq!(r.since, "19:02");
    assert_eq!(r.gaps.as_deref(), Some("12 s"));
    assert_eq!(
        r.variants(),
        [
            "running 2:14:05 since 19:02 · LAeq total 97.8 · offline 12 s",
            "2:14:05 since 19:02 · total 97.8 · offline 12 s",
            "2:14:05 · total 97.8 · offline 12 s",
            "2:14:05 · total 97.8",
            "2:14:05",
        ]
    );
    // No gaps: none named. A C-weighted window adds LCeq and names LAeq in the short form.
    let mut whole = run();
    whole.gaps = Seconds(0.4);
    let mut cc = c.clone();
    cc.windows[1].weighting = Weighting::C;
    let r = run_text(&whole, &cc, |_| UTC_PLUS_2);
    assert_eq!(
        r.variants(),
        [
            "running 2:14:05 since 19:02 · LAeq total 97.8 · LCeq total 110.2",
            "running 2:14:05 since 19:02 · LAeq total 97.8",
            "2:14:05 since 19:02 · LAeq total 97.8",
            "2:14:05 · LAeq total 97.8",
            "2:14:05",
        ]
    );
    // Started yesterday (local): the date is named. Under an hour: still h:mm:ss.
    let mut early = run();
    early.started_at = WallNs((START_S - 20 * 3600) * 1_000_000_000);
    let r = run_text(&early, &c, |_| UTC_PLUS_2);
    assert_eq!(r.since, "2 Oct 23:02");
    let mut short = run();
    short.until = WallNs((START_S + 75) * 1_000_000_000);
    assert_eq!(run_text(&short, &c, |_| UTC_PLUS_2).clock, "0:01:15");
    // The offset in force at each instant: a start before a DST change.
    let r = run_text(&run(), &c, |t| {
        if t.0 < (START_S + 60) * 1_000_000_000 {
            3600
        } else {
            7200
        }
    });
    assert_eq!(r.since, "18:02");
    // Trimmed at 48 h: said so, the clock is the span kept.
    let mut full = run();
    full.trimmed = true;
    full.until = WallNs((START_S + 48 * 3600 + 30) * 1_000_000_000);
    full.gaps = Seconds(30.0);
    let r = run_text(&full, &c, |_| UTC_PLUS_2);
    let v = r.variants();
    assert_eq!(
        v[0],
        "last 48 h: 48:00:30 since 3 Oct 19:02 · LAeq total 97.8 · offline 30 s"
    );
    assert_eq!(v[v.len() - 1], "last 48 h");
    // Nothing measured: no value.
    let mut empty = run();
    empty.laeq = f64::NAN;
    assert!(run_text(&empty, &c, |_| 0).line().contains("LAeq total —"));
}

#[test]
fn new_log_confirmation_names_what_ends() {
    let c = cfg();
    let r = run_text(&run(), &c, |_| UTC_PLUS_2);
    let k = new_log_confirm("FOH SPL", &c, Some(&r), "Shift+R");
    assert_eq!(
        k.hint,
        "Shift+R starts the new log · N or Esc keeps the current one"
    );
    assert_eq!(k.title, "Start a new SPL log for FOH SPL?");
    assert_eq!(
        k.lines[0],
        "The current log ends: running 2:14:05 since 19:02 · LAeq total 97.8 · offline 12 s."
    );
    assert!(
        k.lines[1].contains(
            "the 4 Leq windows and their states, the alarms, the run clock and the total"
        )
    );
    assert!(k.lines[2].starts_with("Kept: the windows, limits"));
    assert!(k.lines[3].contains("ac2 spl leq export --previous"));
    let k = new_log_confirm("FOH SPL", &c, None, "Shift+R");
    assert_eq!(k.lines[0], "The current log ends (no seconds logged yet).");
}

/// The history the daemon rebuilt from the log of `cfg()`'s meter: `n` seconds ending at
/// wall second `end_s`, each window at `leq`, the 15 min window over from the 5th second.
pub(super) fn rebuilt(n: u64, end_s: u64, leq: f32) -> SplHistory {
    let c = cfg();
    let at: Vec<WallNs> = (0..n)
        .map(|k| WallNs((end_s - n + 1 + k) * 1_000_000_000))
        .collect();
    SplHistory {
        meas: MeasId(4),
        windows: c.windows.clone(),
        scale: LevelScale::DbSpl,
        step_s: 1,
        log_start: at.first().map(|t| WallNs(t.0 - 1_000_000_000)),
        log_until: at.last().copied(),
        leq: vec![vec![leq; at.len()]; c.windows.len()],
        over: c
            .windows
            .iter()
            .enumerate()
            .map(|(i, _)| (0..n).map(|k| i == 1 && k >= 4).collect())
            .collect(),
        at,
    }
}

/// An app restarted part way: the frames it received go on from the history rebuilt
/// under them, no second twice (a frame stamped a few milliseconds after the end of its
/// second is that second), none lost; a frame of the newest rebuilt second arriving after
/// the rebuild replaces it.
#[test]
fn rebuilt_history_goes_under_the_live_frames() {
    let c = cfg();
    let mut h = LeqHistory::default();
    let mut f = frame(LevelScale::DbSpl);
    f.leq = vec![90.0; 4];
    // Live from second 1000 (frames stamped 12 ms after the second's end).
    for k in 0..3 {
        h.push(&c, &f, 1000.012 + f64::from(k));
    }
    // The daemon's history up to the end of second 1001.
    h.backfill(&rebuilt(600, 1001, 80.0));
    let p = h.points(&c.windows[1]).expect("series");
    let t: Vec<f64> = p.iter().map(|p| p.t).collect();
    assert_eq!(p.len(), 601, "{:?}", &t[595..]);
    assert_eq!(t[599], 1001.0);
    assert_eq!(t[600], 1002.012);
    assert!(t.windows(2).all(|w| w[1] - w[0] > 0.5));
    assert!(p[4].over && !p[3].over);
    assert_eq!(p[600].leq, 90.0);
    // A late frame of the newest second: the same point.
    let mut h = LeqHistory::default();
    h.backfill(&rebuilt(10, 1001, 80.0));
    h.push(&c, &f, 1001.012);
    h.push(&c, &f, 1002.011);
    let p = h.points(&c.windows[0]).expect("series");
    assert_eq!(p.len(), 11);
    assert_eq!(p[9].t, 1001.012);
}

/// A rebuilt history in another unit than the frames received since (the meter
/// calibrated meanwhile) is dropped; an empty one changes nothing.
#[test]
fn rebuilt_history_of_another_unit_is_dropped() {
    let c = cfg();
    let mut h = LeqHistory::default();
    let mut f = frame(LevelScale::DbSpl);
    f.leq = vec![90.0; 4];
    h.push(&c, &f, 2000.0);
    let before = h.clone();
    let mut r = rebuilt(10, 1001, -40.0);
    r.scale = LevelScale::Dbfs;
    h.backfill(&r);
    assert_eq!(h, before);
    r.at.clear();
    h.backfill(&r);
    assert_eq!(h, before);
}

/// `cfg()` with an LCpeak limit of 135 dB and an LAFmax limit of 125 dB, and a frame whose
/// LCpeak held is over and LAFmax ok, every level corrected by `position`.
fn with_peaks(position: Option<PositionCorrection>) -> (LeqConfig, LeqFrame) {
    use ac2_proto::model::{PeakLimit, PeakLimits};
    let mut c = cfg();
    let l = |v: f64| {
        Some(PeakLimit {
            cue: None,
            limit: DbSpl(v),
            warn_margin: Db(3.0),
        })
    };
    c.peaks = PeakLimits {
        lcpeak: l(135.0),
        lafmax: l(125.0),
    };
    let mut f = frame(LevelScale::DbSpl);
    f.meta.lcpeak = Some(LeqPeak {
        level: 136.24,
        judgement: LeqJudgement::Over,
    });
    f.meta.lafmax = Some(LeqPeak {
        level: 118.4,
        judgement: LeqJudgement::Ok,
    });
    f.meta.position = position;
    (c, f)
}

/// Peak limits come after the windows as tiles of their own, worded as limits on the
/// highest second of the hold; a correction marks every value and the caption says by how
/// much.
#[test]
fn peak_tiles_and_the_correction() {
    let (c, f) = with_peaks(Some(PositionCorrection {
        level: Db(3.0),
        peak: Db(1.5),
    }));
    let t = leq_tiles(&c, &f);
    assert_eq!(t.len(), 6);
    let lc = &t[4];
    assert_eq!(lc.kind, TileKind::Peak(PeakQuantity::LcPeak));
    assert_eq!(
        (
            lc.name.as_str(),
            lc.value.as_str(),
            lc.weighted_unit.as_str()
        ),
        ("LCpeak", "136.2", "dB SPL (C) corr.")
    );
    assert_eq!(lc.state, TileState::Over);
    assert_eq!(lc.state_text.as_deref(), Some("OVER"));
    assert_eq!(lc.limit.as_deref(), Some("limit 135.0 dB SPL"));
    assert_eq!(lc.held.as_deref(), Some("highest of the last 10 s"));
    assert_eq!(lc.limit_db, Some(135.0));
    assert!(lc.headroom.is_none() && lc.filling.is_none() && !lc.filling());
    let laf = &t[5];
    assert_eq!(
        (
            laf.name.as_str(),
            laf.weighted_unit.as_str(),
            laf.state_text.as_deref()
        ),
        ("LAFmax", "dB SPL (A) corr.", Some("OK"))
    );
    assert_eq!(t[1].weighted_unit, "dB SPL (A) corr.");
    assert!(
        t.iter()
            .all(|x| x.corrected.as_deref() == Some("corrected +3.0 dB, peaks +1.5 dB"))
    );
    assert_eq!(
        position_text(&PositionCorrection::both(-2.0)),
        "corrected -2.0 dB"
    );
    // Without a correction: plain units, no caption note.
    let (c0, f0) = with_peaks(None);
    let t0 = leq_tiles(&c0, &f0);
    assert_eq!(t0[4].weighted_unit, "dB SPL (C)");
    assert!(t0.iter().all(|x| x.corrected.is_none()));
    // A limit not configured has no tile even when a frame carries a state.
    let mut c1 = c0.clone();
    c1.peaks.lafmax = None;
    assert_eq!(leq_tiles(&c1, &f0).len(), 5);
    // Uncalibrated: not judged.
    let mut fu = f0.clone();
    fu.meta.scale = LevelScale::Dbfs;
    fu.meta.lcpeak = Some(LeqPeak {
        level: -3.0,
        judgement: LeqJudgement::NotCalibrated,
    });
    let tu = leq_tiles(&c0, &fu);
    assert_eq!(tu[4].state_text.as_deref(), Some("not calibrated"));
    assert_eq!(
        (tu[4].limit_db, tu[4].weighted_unit.as_str()),
        (None, "dBFS (C)")
    );

    // Columns: the windows by length, then LCpeak and LAFmax, whole names at any width,
    // the correction in the caption; nothing overlaps.
    let th = Theme::dark();
    for (w, h) in [
        (320.0, 480.0),
        (640.0, 400.0),
        (1280.0, 720.0),
        (1920.0, 1080.0),
    ] {
        let s = leq_scene(
            &columns_view(&c, &f, None),
            &Status::default(),
            &th,
            size(w, h),
        );
        let at = format!("{w}×{h}");
        let k = cols(&s);
        let order: Vec<usize> = k.columns.iter().map(|x| x.window).collect();
        assert_eq!(order, [0, 1, 2, 3, 4, 5], "{at}");
        assert_eq!(k.columns[4].name, "LCpeak", "{at}");
        assert_eq!(k.columns[5].name, "LAFmax", "{at}");
        // The limit lines of both kinds are on the scale.
        assert!(k.columns[4].limit_y.is_some() && k.columns[1].limit_y.is_some());
        assert!(
            k.range.hi >= 135.0 && k.range.lo <= 93.0,
            "{at}: {:?}",
            k.range
        );
        let labels = column_labels(&s);
        let boxes: Vec<Rect> = labels
            .iter()
            .map(|(l, _)| crate::canvas::tests::label_box(l))
            .collect();
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                assert!(
                    !crate::canvas::tests::intersects(boxes[i], boxes[j]),
                    "{at}: {:?} overlaps {:?}",
                    labels[i].0.text,
                    labels[j].0.text
                );
            }
        }
        assert_caption_fits(&s, w, false, &at);
        if w >= 1280.0 {
            let all = texts(&s.scene);
            assert!(
                all.iter()
                    .any(|t| t.contains("corrected +3.0 dB, peaks +1.5 dB")),
                "{at}: {all:?}"
            );
            assert!(
                all.contains(&"OVER") && all.iter().any(|t| t.ends_with("last 10 s")),
                "{at}: {all:?}"
            );
        }
    }
    // Tiles too.
    let v = LeqView {
        layout: LeqLayout {
            style: LeqStyle::Tiles,
            history: false,
        },
        ..columns_view(&c, &f, None)
    };
    let s = leq_scene(&v, &Status::default(), &th, size(1280.0, 720.0));
    assert_eq!(s.tiles.len(), 6);
    assert!(texts(&s.scene).contains(&"LCpeak"));
}

/// Alarms in words, windows and peak limits alike, a correction said.
#[test]
fn alarm_wording() {
    let window = LeqAlarm {
        at: WallNs(1),
        subject: AlarmSubject::Window {
            duration: Seconds(1800.0),
            weighting: Weighting::A,
        },
        kind: LeqAlarmKind::Over,
        level: DbSpl(101.24),
        limit: DbSpl(99.0),
        position: None,
    };
    assert_eq!(
        alarm_text("FOH SPL", &window),
        (
            true,
            "FOH SPL: LAeq 30 min over its limit — 101.2 dB SPL > 99.0 dB SPL".to_owned()
        )
    );
    let peak = LeqAlarm {
        subject: AlarmSubject::Peak {
            quantity: PeakQuantity::LcPeak,
        },
        kind: LeqAlarmKind::Recovered,
        level: DbSpl(133.0),
        limit: DbSpl(135.0),
        position: Some(Db(2.0)),
        ..window
    };
    assert_eq!(
        alarm_text("FOH SPL", &peak),
        (
            false,
            "FOH SPL: LCpeak back within its limit — 133.0 dB SPL (corrected +2.0 dB)".to_owned()
        )
    );
}

/// The run (clock, start, totals, time offline) belongs to the look back: without the
/// history it is not drawn in any style, and the row it took goes to the windows; with
/// the history on it is back.
#[test]
fn the_run_shows_only_with_the_history() {
    let th = Theme::dark();
    let (c, f) = many(5, true, LevelScale::DbSpl);
    for style in [LeqStyle::Columns, LeqStyle::Tiles] {
        for (w, h) in [(640.0, 400.0), (1280.0, 720.0), (1920.0, 1080.0)] {
            let at = format!("{style:?} at {w}×{h}");
            let scene = |history| {
                let v = LeqView {
                    layout: LeqLayout { style, history },
                    ..columns_view(&c, &f, None)
                };
                leq_scene(&v, &Status::default(), &th, size(w, h))
            };
            let (off, on) = (scene(false), scene(true));
            assert_eq!(off.run, None, "{at}");
            let all = texts(&off.scene);
            assert!(
                all.iter().all(|t| !t.contains("2:14:05")
                    && !t.contains("since 19:02")
                    && !t.contains("total")
                    && !t.contains("offline 12 s")),
                "{at}: {all:?}"
            );
            assert!(
                on.run.as_deref().is_some_and(|r| r.contains("2:14:05")),
                "{at}"
            );
            // The space it took is the windows'.
            assert!(off.caption.h <= on.caption.h, "{at}");
            let top = |s: &LeqScene| {
                s.tiles
                    .iter()
                    .chain(
                        s.columns
                            .iter()
                            .flat_map(|k| k.columns.iter().map(|x| &x.rect)),
                    )
                    .map(|r| r.y)
                    .fold(f32::INFINITY, f32::min)
            };
            assert!(top(&off) <= top(&on), "{at}");
            assert!(top(&off) >= off.caption.bottom() - 0.5, "{at}");
            assert_caption_fits(&off, w, false, &at);
            assert_caption_fits(&on, w, true, &at);
        }
    }
}

/// Full screen: the caption (meter, calibration) is gone without the history and the
/// windows take its room; with the history it is back with the run, and stale values keep
/// it (its STALE is the warning).
#[test]
fn the_stage_has_no_caption_without_the_history() {
    let th = Theme::dark();
    let (c, f) = many(5, false, LevelScale::DbSpl);
    for style in [LeqStyle::Columns, LeqStyle::Tiles] {
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0)] {
            let at = format!("{style:?} at {w}×{h}");
            let scene = |stage, history, stale: Option<&str>| {
                let v = LeqView {
                    layout: LeqLayout { style, history },
                    stage,
                    stale: stale.map(str::to_owned),
                    ..columns_view(&c, &f, None)
                };
                leq_scene(&v, &Status::default(), &th, size(w, h))
            };
            let bare = scene(true, false, None);
            let all = texts(&bare.scene);
            assert!(
                all.iter()
                    .all(|t| !t.contains("FOH SPL") && !t.contains("cal 3 h ago")),
                "{at}: {all:?}"
            );
            assert_eq!(bare.caption.h, 0.0, "{at}");
            assert_eq!(bare.run, None, "{at}");
            let top = |s: &LeqScene| {
                s.tiles
                    .iter()
                    .chain(
                        s.columns
                            .iter()
                            .flat_map(|k| k.columns.iter().map(|x| &x.rect)),
                    )
                    .map(|r| r.y)
                    .fold(f32::INFINITY, f32::min)
            };
            let windowed = scene(false, false, None);
            assert!(
                top(&bare) < top(&windowed),
                "{at}: the room goes to the windows"
            );
            assert_caption_fits(&windowed, w, false, &at);
            // The history on: caption and run, as in the window.
            let looking_back = scene(true, true, None);
            assert_caption_fits(&looking_back, w, true, &at);
            // Stale: the caption carries the warning.
            let stale = scene(true, false, Some("STALE 4.2 s"));
            assert!(
                texts(&stale.scene)
                    .iter()
                    .any(|t| t.starts_with("STALE 4.2 s")),
                "{at}"
            );
        }
    }
}
