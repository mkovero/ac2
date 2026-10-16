//! Reducer tests of the Leq history strip looked back at, and the Leq log export.

use super::*;
use crate::state::leq_strip::StripMsg;

/// The meter 4 of `with_spl`, logging 40 000 s up to wall second 50 000, its newest 600 s
/// rebuilt into the strip; the SPL pane focused on its windows with the strip on.
fn browsing() -> (T, PaneId) {
    let mut t = T::new();
    let r = t.conn(mirror(with_spl()));
    let (meas, ask) = super::leq::backfill_asked(&r).expect("history asked for");
    t.conn(ConnEvent::LeqBackfill {
        meas,
        ask,
        result: Ok(Box::new(super::leq::rebuilt(600, 50_000))),
    });
    t.conn(leq_data_logged(
        40_001,
        40_000,
        50_000,
        80.0,
        ac2_proto::frame::LeqFlags::LIMIT,
    ));
    t.key("Alt+4");
    t.st.view.spl.layout.history = true;
    t.st.modes_mut().spl = SplMode::Leq;
    let pane = t.st.layout.focus;
    assert!(t.st.leq_strip_target().is_some());
    (t, pane)
}

fn span_asked(r: &[Request]) -> Option<(f64, f64, u32, u64)> {
    r.iter().find_map(|r| match r {
        Request::LeqSpan {
            from,
            to,
            step_s,
            ask,
            ..
        } => Some((from.0 as f64 / 1e9, to.0 as f64 / 1e9, *step_s, *ask)),
        _ => None,
    })
}

/// A history reply of `n` points `step` s apart ending at `end` s.
fn span_reply(end: u64, step: u32, n: u64) -> SplHistory {
    let windows = LeqConfig::default_windows().windows;
    let w = windows.len();
    SplHistory {
        meas: MeasId(4),
        windows,
        scale: LevelScale::DbSpl,
        step_s: step,
        log_start: Some(WallNs(10_000 * 1_000_000_000)),
        log_until: Some(WallNs(50_000 * 1_000_000_000)),
        at: (0..n)
            .map(|k| WallNs((end - (n - 1 - k) * u64::from(step)) * 1_000_000_000))
            .collect(),
        leq: vec![vec![90.0; n as usize]; w],
        over: vec![vec![false; n as usize]; w],
    }
}

/// O / I zoom the focused strip's time, ← / → pan it, as on the other graphs; looking back
/// past what was received asks the daemon for that span at the strip's resolution, its
/// reply draws it; Home returns to live.
#[test]
fn keys_walk_the_strip_and_fetch_what_is_not_held() {
    let (mut t, pane) = browsing();
    let cfg = LeqConfig::default_windows();
    let live = ac2_scene::leq::history_span(&cfg);
    // Zoomed out about the middle, the right edge stays at the newest second.
    let r = t.key("O");
    let (from, to) = t.st.leq_strips[&pane];
    assert_eq!(to, 50_000.0);
    assert!((to - from - (live * ZOOM_FACTOR).min(40_000.0 * 1.05)).abs() < 1e-6);
    // The newest 600 s are held, the rest is asked for, with room to pan.
    let (a, b, step, ask) = span_asked(&r).expect("asked for the span");
    assert!(
        a <= from.max(10_000.0) && b >= to,
        "{a}..{b} for {from}..{to}"
    );
    assert!(step >= 1 && f64::from(step) <= (to - from) / 100.0);
    assert!(t.st.strip_view(pane, MeasId(4), None).loading);
    // The same span again: already asked.
    assert_eq!(span_asked(&t.key("ArrowRight")), None);
    // Its reply: the strip draws it.
    let n = ((b - a) / f64::from(step)) as u64;
    t.conn(ConnEvent::LeqSpan {
        meas: MeasId(4),
        ask,
        result: Ok(Box::new(span_reply(b as u64, step, n))),
    });
    let v = t.st.strip_view(pane, MeasId(4), None);
    assert!(!v.loading);
    assert_eq!(v.points.map(|p| p.step()), Some(step));
    // ← moves a tenth of the span back.
    let before = t.st.leq_strips[&pane];
    t.key("ArrowLeft");
    let after = t.st.leq_strips[&pane];
    assert!((before.0 - after.0 - (before.1 - before.0) / 10.0).abs() < 1e-6);
    // I zooms in again.
    t.key("I");
    let z = t.st.leq_strips[&pane];
    assert!(z.1 - z.0 < after.1 - after.0);
    // Home: live, said.
    t.key("Home");
    assert!(!t.st.leq_strips.contains_key(&pane));
    assert_eq!(t.last_toast(), "Leq history: live");
    let v = t.st.strip_view(pane, MeasId(4), None);
    assert_eq!(v.span.span, None);
}

/// The wheel zooms time about the pointer, a drag pans it, never past the newest second or
/// the log's start; the pointer's time is the cursor.
#[test]
fn the_mouse_zooms_pans_and_reads_the_strip() {
    let (mut t, pane) = browsing();
    let shown = (49_000.0, 50_000.0);
    let m = |m| Msg::LeqStrip(pane, m);
    // Zoomed in ×2 about 49 800: that time stays where it was on the strip.
    t.st.update(
        m(StripMsg::Zoom {
            shown,
            about: 49_800.0,
            factor: 2.0,
            width_px: 800.0,
        }),
        &t.keys,
    );
    let (a, b) = t.st.leq_strips[&pane];
    assert!((b - a - 500.0).abs() < 1e-6);
    assert!(((49_800.0 - a) / (b - a) - 0.8).abs() < 1e-6, "{a}..{b}");
    assert_eq!(t.st.leq_strip_px, 800.0);
    // Dragged right by 50 s' worth (the content moves left): later; all of it received.
    let r = t.st.update(
        m(StripMsg::Pan {
            shown: (a, b),
            by: 50.0,
            width_px: 800.0,
        }),
        &t.keys,
    );
    assert_eq!(t.st.leq_strips[&pane], (a + 50.0, b + 50.0));
    assert_eq!(span_asked(&r), None, "the newest 600 s are held");
    // Far into the future: the strip ends at the newest second.
    t.st.update(
        m(StripMsg::Pan {
            shown: (a, b),
            by: 1e6,
            width_px: 800.0,
        }),
        &t.keys,
    );
    assert_eq!(t.st.leq_strips[&pane].1, 50_000.0);
    // Far into the past: the log's first second stays on the strip, and that span is
    // asked for.
    let r = t.st.update(
        m(StripMsg::Pan {
            shown: (a, b),
            by: -1e6,
            width_px: 800.0,
        }),
        &t.keys,
    );
    let (a, b) = t.st.leq_strips[&pane];
    assert!(a < 10_000.0 && b > 10_000.0, "{a}..{b}");
    let (fa, fb, step, _) = span_asked(&r).expect("asked");
    assert!(fa <= 10_000.0 && fb >= b, "{fa}..{fb}");
    assert_eq!(step, 1, "500 s over 800 px: a point a second");
    // Never narrower than the shortest span.
    t.st.update(
        m(StripMsg::Zoom {
            shown: (a, b),
            about: a,
            factor: 1e6,
            width_px: 800.0,
        }),
        &t.keys,
    );
    let (a, b) = t.st.leq_strips[&pane];
    assert!((b - a - STRIP_MIN_S).abs() < 1e-6);
    // The pointer on the strip, and off it.
    t.st.update(m(StripMsg::Hover(Some(a + 5.0))), &t.keys);
    assert_eq!(t.st.leq_cursor, Some((pane, a + 5.0)));
    assert_eq!(t.st.strip_view(pane, MeasId(4), None).cursor, Some(a + 5.0));
    t.st.update(m(StripMsg::Hover(None)), &t.keys);
    assert_eq!(t.st.leq_cursor, None);
}

/// A stale or failed fetch: a reply to an earlier ask is not kept; a failure is said once.
#[test]
fn a_failed_span_fetch_is_said() {
    let (mut t, _) = browsing();
    let r = t.key("O");
    let (_, _, _, ask) = span_asked(&r).expect("asked");
    t.conn(ConnEvent::LeqSpan {
        meas: MeasId(4),
        ask: ask + 9,
        result: Err("gone".into()),
    });
    assert!(!t.last_toast().contains("gone"));
    t.conn(ConnEvent::LeqSpan {
        meas: MeasId(4),
        ask,
        result: Err("no such meter".into()),
    });
    assert!(
        t.last_toast().contains("no such meter"),
        "{}",
        t.last_toast()
    );
    assert!(t.st.leq_spans.is_empty());
}

/// The export prompt's name: the meter's name made a file name, local date and time.
#[test]
fn the_leq_export_name_is_dated_in_local_time() {
    // 2026-10-16 21:12 UTC is 00:12 the next day at UTC+3.
    let at = WallNs(1_792_185_120 * 1_000_000_000);
    assert_eq!(
        leq_export_name("FOH SPL", at, 3 * 3600),
        "FOH-SPL-leq-2026-10-17-0012.csv"
    );
    assert_eq!(
        leq_export_name("Stage L/R", at, 0),
        "Stage-L-R-leq-2026-10-16-2112.csv"
    );
    let home = std::path::Path::new("/home/op");
    assert_eq!(
        export_path("~/ac2-exports/a.csv", Some(home)),
        Ok(home.join("ac2-exports/a.csv"))
    );
    assert_eq!(export_path("b.csv", Some(home)), Ok(home.join("b.csv")));
    assert_eq!(
        export_path("/tmp/c.csv", Some(home)),
        Ok(std::path::PathBuf::from("/tmp/c.csv"))
    );
    assert!(export_path("  ", Some(home)).is_err());
}

/// "Export Leq log…" (Ctrl+K) asks where, with `~/ac2-exports/<meter>-leq-<date>.csv`;
/// Enter sends the export with the meter's facts for the CSV header. The previous log's
/// entry asks the same for that log. No meter: it says so.
#[test]
fn the_leq_export_prompt_sends_the_export() {
    let mut t = T::new();
    t.st.local_zone = crate::scenes::LocalZone::Fixed { offset_s: 7200 };
    t.st.update(Msg::Command(CommandId::SplLogExport), &t.keys);
    assert!(
        t.last_toast().contains("no SPL meter"),
        "{}",
        t.last_toast()
    );
    t.conn(mirror(with_spl()));
    t.st.update(Msg::Command(CommandId::SplLogExport), &t.keys);
    let Overlay::Prompt(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(
        p.kind,
        PromptKind::LeqExport {
            meas: MeasId(4),
            previous: false
        }
    );
    assert!(
        p.text.starts_with("~/ac2-exports/FOH-SPL-leq-20"),
        "{}",
        p.text
    );
    assert!(p.text.ends_with(".csv"), "{}", p.text);
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("x").join("foh.csv");
    if let Overlay::Prompt(p) = &mut t.st.overlay {
        p.text = path.display().to_string();
    }
    let r = t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    let (info, log, to) = r
        .iter()
        .find_map(|r| match r {
            Request::ExportLeqLog { info, log, path } => Some((info.clone(), *log, path.clone())),
            _ => None,
        })
        .expect("export asked");
    assert_eq!(
        (info.meas, info.name.as_str(), info.input),
        (MeasId(4), "FOH SPL", 1)
    );
    assert_eq!(log, SplLogWhich::Current);
    assert_eq!(to, path);
    // The app writes the file where asked, making its folder.
    let wrote = crate::conn::write_new_file(&to, b"x").expect("written");
    assert_eq!(std::fs::read(&wrote).expect("read"), b"x");
    // The previous log.
    t.st.update(Msg::Command(CommandId::SplLogExportPrevious), &t.keys);
    let Overlay::Prompt(p) = &t.st.overlay else {
        panic!()
    };
    assert_eq!(
        p.kind,
        PromptKind::LeqExport {
            meas: MeasId(4),
            previous: true
        }
    );
    let r = t.key("Enter");
    assert!(r.iter().any(|r| matches!(
        r,
        Request::ExportLeqLog {
            log: SplLogWhich::Previous,
            ..
        }
    )));
}
