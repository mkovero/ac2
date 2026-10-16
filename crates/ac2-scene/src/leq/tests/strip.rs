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
    assert_eq!(r.rows[0].text, "LAeq 1 min 90.0 dB");
    assert_eq!(r.rows[1].text, "LAeq 15 min 101.0 dB · limit 100.0 · over");
    assert!(r.rows[1].over && !r.rows[2].over);
    assert_eq!(r.rows[2].text, "LAeq 30 min 95.0 dB · limit 99.0");
    let texts: Vec<&str> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .map(|l| l.text.as_str())
        .collect();
    assert!(texts.contains(&"history · 16:40–17:10 · Home: live"));
    assert!(texts.contains(&"LAeq 15 min 101.0 dB · limit 100.0 · over"));

    // Traces only (T): the caption stays, the cursor goes.
    v.chrome = crate::view::PlotChrome::Bare;
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 800.0));
    assert!(s.strip_readout.is_none());
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
    assert_eq!(r.rows[1].text, "LAeq 15 min 80.0 dB · limit 100.0 · over");
    // Off the strip's span: no readout.
    v.strip.cursor = Some((START_S - 7200) as f64);
    let s = leq_scene(&v, &Status::default(), &Theme::dark(), size(1200.0, 800.0));
    assert!(s.strip_readout.is_none());
}
