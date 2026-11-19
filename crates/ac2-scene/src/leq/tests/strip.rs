//! The history strip looking back over the log: its caption, clock axis, the points of a
//! span fetched in buckets, and the cursor readout.

use super::*;

/// A span of the log in buckets of `step` s: `n` points ending at wall second `end_s`,
/// window 1 (15 min, limit 100) at 101 and over in the third bucket, else 95; window 0
/// at 90.
fn bucketed(n: u64, end_s: u64, step: u32) -> SplHistory {
    let c = cfg();
    let at: Vec<WallNs> = (0..n)
        .map(|k| WallNs((end_s - (n - 1 - k) * u64::from(step)) * 1_000_000_000))
        .collect();
    let leq = (0..c.windows.len())
        .map(|i| {
            (0..n)
                .map(|k| match (i, k) {
                    (1, 2) => 101.0,
                    (0, _) => 90.0,
                    _ => 95.0,
                })
                .collect()
        })
        .collect();
    let over = (0..c.windows.len())
        .map(|i| (0..n).map(|k| i == 1 && k == 2).collect())
        .collect();
    SplHistory {
        meas: MeasId(4),
        windows: c.windows.clone(),
        scale: LevelScale::DbSpl,
        step_s: step,
        log_start: Some(WallNs((end_s - 86_400) * 1_000_000_000)),
        log_until: Some(WallNs(end_s * 1_000_000_000)),
        at,
        leq,
        over,
    }
}

fn view<'a>(c: &'a LeqConfig, f: &LeqFrame, h: &'a LeqHistory) -> LeqView<'a> {
    LeqView {
        meter: "FOH SPL".into(),
        cal: "M30 · cal 3 h ago".into(),
        cfg: c,
        tiles: leq_tiles(c, f),
        history: Some(h),
        stale: None,
        scale: f.meta.scale,
        layout: LeqLayout {
            style: LeqStyle::Columns,
            history: true,
        },
        run: run_of(c, f),
        stage: false,
        chrome: crate::view::PlotChrome::Full,
        strip: Default::default(),
        shared: None,
    }
}

/// Looking back: the strip draws the fetched span on a local clock axis, says so with the
/// key back to live, and the cursor reads each window's highest Leq in the bucket under
/// it, with its limit and over.
#[test]
fn a_strip_looking_back_says_history_and_reads_the_cursor() {
    let c = cfg();
    let f = frame(LevelScale::DbSpl);
    let live = LeqHistory::default();
    // 30 buckets of a minute ending 15:10 UTC (17:10 at UTC+2).
    let end_s = START_S - 17 * 3600 - 2 * 60 + 15 * 3600 + 10 * 60;
    let span = LeqHistory::from_span(&bucketed(30, end_s, 60));
    assert_eq!(span.step(), 60);
    let (from, to) = ((end_s - 30 * 60) as f64, end_s as f64);
    let mut v = view(&c, &f, &live);
    v.strip = LeqStripView {
        span: StripSpan {
            span: Some((from, to)),
            utc_offset_s: UTC_PLUS_2,
        },
        points: Some(&span),
        loading: false,
        // In the third bucket (16:42–16:43 local).
        cursor: Some(from + 2.5 * 60.0),
        live_key: Some("Home".into()),
        ..Default::default()
    };
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 800.0));
    assert_eq!(
        s.strip_caption.as_deref(),
        Some("history · 16:40–17:10 · Home: live")
    );
    let hist = s.history.as_ref().expect("strip");
    assert_eq!((hist.from, hist.to), (from, to));
    let labels = hist.x.labels();
    assert!(
        labels.contains(&"16:50") && labels.contains(&"17:00"),
        "{labels:?}"
    );
    // Every bucket drawn: one point each on the 15 min line (nothing decimated away).
    assert_eq!(hist.lines[1].points.len(), 30);
    assert_eq!(hist.lines[1].over.len(), 1);
    let r = s.strip_readout.as_ref().expect("readout");
    assert_eq!(r.time, "16:42:00–16:43:00 · highest");
    assert_eq!(r.rows.len(), 4);
    assert_eq!(r.rows[0].text, "LAeq 1 min  90.0 dB SPL");
    assert_eq!(
        r.rows[1].text,
        "LAeq 15 min  101.0 dB SPL · limit 100.0 dB SPL · over"
    );
    assert!(r.rows[1].over && !r.rows[2].over);
    assert_eq!(
        r.rows[2].text,
        "LAeq 30 min  95.0 dB SPL · limit 99.0 dB SPL"
    );
    // The axis says the unit the levels are in, never a bare dB.
    assert_eq!(hist.y.title, "dB SPL");
    let texts: Vec<&str> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .map(|l| l.text.as_str())
        .collect();
    assert!(texts.contains(&"history · 16:40–17:10 · Home: live"));
    assert!(texts.contains(&"LAeq 15 min  101.0 dB SPL · limit 100.0 dB SPL · over"));
    assert!(texts.contains(&"dB SPL"), "the axis title: {texts:?}");

    // Traces only (T): the caption stays, the cursor goes.
    v.chrome = crate::view::PlotChrome::Bare;
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 800.0));
    assert!(s.strip_readout.is_none() && s.readout_box.is_none());
    assert!(s.strip_caption.is_some());

    // While the span is fetched; under ten minutes the times carry seconds.
    v.chrome = crate::view::PlotChrome::Full;
    v.strip.loading = true;
    v.strip.span.span = Some((from, from + 300.0));
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 800.0));
    assert_eq!(
        s.strip_caption.as_deref(),
        Some("history · 16:40:00–16:45:00 · loading · Home: live")
    );
}

/// Live, the strip has no history caption, and the cursor reads a second: its start in
/// local time and each window's Leq.
#[test]
fn a_live_strip_reads_the_second_under_the_cursor() {
    let c = cfg();
    let f = frame(LevelScale::DbSpl);
    let mut h = LeqHistory::default();
    h.backfill(&super::caption::rebuilt(600, START_S + 600, 80.0));
    let mut v = view(&c, &f, &h);
    v.strip.span.utc_offset_s = UTC_PLUS_2;
    // In the second ending at START_S + 300 (19:07:00 local at its end).
    v.strip.cursor = Some((START_S + 299) as f64 + 0.4);
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 800.0));
    assert!(s.strip_caption.is_none());
    let r = s.strip_readout.as_ref().expect("readout");
    assert_eq!(r.time, "19:06:59");
    assert_eq!(
        r.rows[1].text,
        "LAeq 15 min  80.0 dB SPL · limit 100.0 dB SPL · over"
    );
    // Off the strip's span: no readout.
    v.strip.cursor = Some((START_S - 7200) as f64);
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 800.0));
    assert!(s.strip_readout.is_none());
}

/// The scene with a 30-minute span looked back at and the cursor in its third bucket.
fn looking_back<'a>(
    c: &'a LeqConfig,
    f: &LeqFrame,
    live: &'a LeqHistory,
    span: &'a LeqHistory,
) -> LeqView<'a> {
    let end_s = START_S;
    let (from, to) = ((end_s - 30 * 60) as f64, end_s as f64);
    let mut v = view(c, f, live);
    v.strip = LeqStripView {
        span: StripSpan {
            span: Some((from, to)),
            utc_offset_s: 0,
        },
        points: Some(span),
        cursor: Some(from + 2.5 * 60.0),
        ..Default::default()
    };
    v
}

/// The strip's cursor is every plot's cursor: its line in the cursor's colour, its values
/// on the cursor values' plate (the time, then a row per window), which the operator moves
/// and scales and the solid plate setting fills.
#[test]
fn the_strip_cursor_is_the_cursor_line_and_the_readout_plate() {
    use crate::plate::{PlateKind, PlatePlace};
    let c = cfg();
    let f = frame(LevelScale::DbSpl);
    let live = LeqHistory::default();
    let span = LeqHistory::from_span(&bucketed(30, START_S, 60));
    let mut v = looking_back(&c, &f, &live, &span);
    let theme = Theme::dark();
    let sz = size(1200.0, 800.0);
    let s = leq_scene(&v, &Status::default(), &theme, sz);
    let hist = s.history.as_ref().expect("strip");
    // One vertical line across the strip, in the cursor's stroke.
    let x = hist.plot.x + hist.plot.w * 2.5 / 30.0;
    let line = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.polylines)
        .find(|p| {
            p.points.len() == 2 && p.points[0][0] == p.points[1][0] && p.stroke == theme.cursor
        })
        .expect("a line in the cursor's stroke");
    assert!((line.points[0][0] - x).abs() < 0.5, "{:?}", line.points);
    // The plate: placed by the scene inside the strip's right edge, a row per line.
    let b = s.readout_box.expect("plate");
    assert_eq!(s.plates(), vec![b]);
    assert_eq!(b.kind, PlateKind::Readout);
    assert!(b.rect.right() <= hist.plot.right() && b.rect.x >= hist.plot.x);
    assert!(b.rect.y >= hist.plot.y && b.rect.bottom() <= sz.height);
    let r = s.strip_readout.as_ref().expect("readout");
    let on_plate = |s: &LeqScene, b: &crate::plate::PlateBox| -> Vec<String> {
        s.scene
            .layers
            .last()
            .expect("layers")
            .labels
            .iter()
            .filter(|l| {
                (b.rect.x..=b.rect.right()).contains(&l.pos[0])
                    && (b.rect.y..=b.rect.bottom()).contains(&l.pos[1])
            })
            .map(|l| l.text.clone())
            .collect()
    };
    let mut want = vec![r.time.clone()];
    want.extend(r.rows.iter().map(|row| row.text.clone()));
    assert_eq!(on_plate(&s, &b), want);
    // Its fill is the plates' own: see-through by default, solid when the theme says so.
    let fill = |s: &LeqScene, b: &crate::plate::PlateBox| {
        s.scene
            .layers
            .last()
            .expect("layers")
            .rects
            .iter()
            .find(|q| q.rect == b.rect)
            .expect("fill")
            .color
    };
    assert_eq!(
        fill(&s, &b),
        theme.plot_background.with_alpha(theme.plate_alpha)
    );
    let mut solid = theme.clone();
    solid.plate_alpha = 1.0;
    let s2 = leq_scene(&v, &Status::default(), &solid, sz);
    assert_eq!(
        fill(&s2, &s2.readout_box.expect("plate")),
        solid.plot_background.with_alpha(1.0)
    );
    // Dragged to the pane's top left and enlarged: it sits there, over the columns.
    let mut place = PlatePlace::default();
    place.move_to(0.0, 0.0);
    place.set_scale(1.5);
    v.strip.plate = place;
    let moved = leq_scene(&v, &Status::default(), &theme, sz);
    let m = moved.readout_box.expect("plate");
    assert!(m.rect.x < 20.0 && m.rect.y < hist.plot.y, "{:?}", m.rect);
    assert!(m.rect.h > b.rect.h);
    assert_eq!(on_plate(&moved, &m), want);
    // In a gap of the log the line stays and nothing is read.
    v.strip.plate = PlatePlace::default();
    let empty = LeqHistory::default();
    v.strip.points = Some(&empty);
    let gap = leq_scene(&v, &Status::default(), &theme, sz);
    assert!(gap.strip_readout.is_none() && gap.readout_box.is_none());
    assert!(
        gap.scene
            .layers
            .iter()
            .flat_map(|l| &l.polylines)
            .any(|p| p.stroke == theme.cursor)
    );
}

/// An uncalibrated meter's strip reads dBFS, signed, on the axis and the cursor.
#[test]
fn an_uncalibrated_strip_reads_signed_dbfs() {
    let c = cfg();
    let f = frame(LevelScale::Dbfs);
    let live = LeqHistory::default();
    let mut log = bucketed(30, START_S, 60);
    log.scale = LevelScale::Dbfs;
    for w in &mut log.leq {
        for v in w.iter_mut() {
            *v -= 110.0;
        }
    }
    log.leq[0][2] = 3.0;
    let span = LeqHistory::from_span(&log);
    let v = looking_back(&c, &f, &live, &span);
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 800.0));
    assert_eq!(s.history.as_ref().expect("strip").y.title, "dBFS");
    let r = s.strip_readout.as_ref().expect("readout");
    assert_eq!(r.rows[0].text, "LAeq 1 min  +3.0 dBFS");
    assert!(
        r.rows[2].text.starts_with("LAeq 30 min  \u{2212}15.0 dBFS"),
        "{}",
        r.rows[2].text
    );
}
