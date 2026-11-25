use super::*;
use crate::model::{LogClock, LogGap};

fn rows() -> Vec<SplLogRow> {
    vec![
        SplLogRow {
            second: 0,
            run: 0,
            start: WallNs(1_790_000_245_000_000_000),
            clock: LogClock::Synced,
            gap: Some(LogGap::Known),
            measured: Seconds(1.0),
            laeq: Dbfs(-24.9),
            lceq: Dbfs(-22.0),
            lzeq: Dbfs(-20.5),
            lcpeak: Dbfs(1.23),
            lafmax: Dbfs(-19.02),
            sensitivity: Some(Db(120.02)),
            position: Some(PositionCorrection {
                level: Db(3.0),
                peak: Db(2.0),
            }),
        },
        SplLogRow {
            second: 1,
            run: 0,
            start: WallNs(1_790_000_246_000_000_000),
            clock: LogClock::Synced,
            gap: None,
            measured: Seconds(0.25),
            laeq: Dbfs(f64::NEG_INFINITY),
            lceq: Dbfs(-90.125),
            lzeq: Dbfs(-80.0),
            lcpeak: Dbfs(-70.0),
            lafmax: Dbfs(f64::NEG_INFINITY),
            sensitivity: None,
            position: None,
        },
    ]
}

#[test]
fn utc_text() {
    assert_eq!(utc_iso(0), "1970-01-01T00:00:00.000Z");
    // 2026-10-03 14:57:25.5 UTC.
    assert_eq!(
        utc_iso(1_791_039_445_500_000_000),
        "2026-10-03T14:57:25.500Z"
    );
    assert_eq!(utc_iso(951_782_400_000_000_000), "2000-02-29T00:00:00.000Z");
}

#[test]
fn civil_dates() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(-1), (1969, 12, 31));
    assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    assert_eq!(civil_from_days(-719_468), (0, 3, 1));
    // Every day of 1900–2100 follows the one before.
    let mut prev = civil_from_days(-25_567);
    assert_eq!(prev, (1900, 1, 1));
    for days in -25_566..47_482 {
        let (y, m, d) = civil_from_days(days);
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let len = [
            31,
            if leap { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let next = if prev.2 < len[(prev.1 - 1) as usize] {
            (prev.0, prev.1, prev.2 + 1)
        } else if prev.1 < 12 {
            (prev.0, prev.1 + 1, 1)
        } else {
            (prev.0 + 1, 1, 1)
        };
        assert_eq!((y, m, d), next, "{days}");
        prev = (y, m, d);
    }
    assert_eq!(prev, (2099, 12, 31));
}

fn info() -> SplLogInfo {
    SplLogInfo {
        meas: MeasId(4),
        name: "FOH SPL".into(),
        input: 1,
        mic: Some("M30".into()),
        zone: "Europe/Helsinki".into(),
        started: WallNs(1_790_000_000_000_000_000),
    }
}

#[test]
fn round_trip() {
    let csv = export_csv(&info(), &rows());
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], HEADER_LINE);
    assert_eq!(lines[4], "# mic: M30");
    assert_eq!(lines[5], "# zone: Europe/Helsinki");
    assert_eq!(lines[6], "# started: 2026-09-21T14:13:20.000Z");
    assert_eq!(lines[7], COLUMNS);
    assert_eq!(
        lines[8],
        "0,0,2026-09-21T14:17:25.000Z,1790000245000000000,synced,known,1,dB SPL,95.1200,\
         98.0200,99.5200,121.2500,101.0000,120.0200,3.0000,2.0000"
    );
    assert_eq!(
        lines[9],
        "1,0,2026-09-21T14:17:26.000Z,1790000246000000000,synced,,0.25,dBFS,-inf,-90.1250,\
         -80.0000,-70.0000,-inf,,,"
    );
    let read = import_csv(csv.as_bytes()).expect("reads back");
    assert_eq!(read.info, info());
    let back = read.rows;
    assert_eq!(back.len(), 2);
    for (a, b) in back.iter().zip(rows()) {
        assert_eq!(
            (a.second, a.run, a.start, a.clock, a.gap),
            (b.second, b.run, b.start, b.clock, b.gap)
        );
        assert_eq!(a.measured, b.measured);
        assert_eq!(a.sensitivity, b.sensitivity);
        assert_eq!(a.position, b.position);
        for (x, y) in [
            (a.laeq, b.laeq),
            (a.lceq, b.lceq),
            (a.lzeq, b.lzeq),
            (a.lcpeak, b.lcpeak),
            (a.lafmax, b.lafmax),
        ] {
            assert!(x.0 == y.0 || (x.0 - y.0).abs() < 1e-9, "{x:?} {y:?}");
        }
    }
}

/// A log cut short by a power cut reads up to its last whole line and says where that
/// ends, so an append carries on from there.
#[test]
fn an_unterminated_last_line_is_left_out() {
    let csv = export_csv(&info(), &rows());
    let whole = import_csv(csv.as_bytes()).expect("whole");
    assert_eq!((whole.rows.len(), whole.complete_len), (2, csv.len()));
    let one = csv.len() - csv.lines().last().expect("row").len() - 1;
    for cut in [csv.len() - 1, csv.len() - 30, one + 1] {
        let r = import_csv(&csv.as_bytes()[..cut]).expect("cut");
        assert_eq!((r.rows.len(), r.complete_len), (1, one), "cut at {cut}");
    }
    let mut zeros = csv.clone().into_bytes();
    zeros.truncate(one + 7);
    zeros.extend([0u8; 300]);
    let r = import_csv(&zeros).expect("zeros");
    assert_eq!((r.rows.len(), r.complete_len), (1, one));
    // Zeros after a torn row, with newlines among them, several KiB: the rows before.
    for tail in [
        [
            &b"2,0,2026-10-1"[..],
            &[0u8; 4096],
            b"\n",
            &[0u8; 512],
            b"\n",
        ]
        .concat(),
        [&[0u8; 8192][..], b"\n"].concat(),
        b"\xff\xfe\x00garbage\n\x01\n".to_vec(),
    ] {
        let mut b = csv.clone().into_bytes();
        b.extend(&tail);
        let r = import_csv(&b).expect("zero tail");
        assert_eq!((r.rows.len(), r.complete_len), (2, csv.len()));
        assert_eq!(r.tail, tail.len());
    }
    // A malformed whole line is damage, not a cut.
    let bad = format!("{csv}x,1\n");
    assert!(import_csv(bad.as_bytes()).is_err());
}

fn head() -> String {
    header(&info())
}

#[test]
fn refuses_what_it_did_not_write() {
    assert_eq!(import_csv(b"start,leq\n"), Err(SplLogError::Header));
    assert_eq!(import_csv(HEADER_LINE.as_bytes()), Err(SplLogError::Header));
    let h = head();
    let bad = format!("{h}1,2,3\n");
    assert!(matches!(
        import_csv(bad.as_bytes()),
        Err(SplLogError::Line { line: 9, .. })
    ));
    let bad = format!("{h}0,0,x,1,synced,known,1,dBFS,1,1,1,1,1,120,,\n");
    assert!(matches!(
        import_csv(bad.as_bytes()),
        Err(SplLogError::Line { line: 9, .. })
    ));
    // A correction is energy and peak together.
    let bad = format!("{h}0,0,x,1,synced,known,1,dB SPL,1,1,1,1,1,120,3,\n");
    assert!(matches!(
        import_csv(bad.as_bytes()),
        Err(SplLogError::Line { line: 9, .. })
    ));
    // A header without its zone or start is not one this build wrote.
    let no_zone = h.replace("# zone: Europe/Helsinki\n", "");
    assert!(matches!(
        import_csv(no_zone.as_bytes()),
        Err(SplLogError::Line { .. })
    ));
}

/// A log of the previous version is named as such, never read as this one.
#[test]
fn another_version_is_named() {
    let v2 = "# ac2 spl log v2\n# meas: 4\nstart_utc,start_ns,measured_s\n";
    assert_eq!(
        import_csv(v2.as_bytes()),
        Err(SplLogError::Version { found: 2, reads: 3 })
    );
}

fn row_at(second: u64, run: u32, start_s: u64, gap: Option<LogGap>) -> SplLogRow {
    SplLogRow {
        second,
        run,
        start: WallNs(start_s * 1_000_000_000),
        gap,
        ..rows()[0]
    }
}

/// Seconds rise and only a run's first row carries a gap: rows out of that order were
/// written so, and are damage.
#[test]
fn rows_out_of_key_order_are_refused() {
    let ok = [
        row_at(5, 0, 100, Some(LogGap::Known)),
        row_at(6, 0, 101, None),
        row_at(9, 1, 50, Some(LogGap::Unknown)),
    ];
    assert_eq!(
        import_csv(export_csv(&info(), &ok).as_bytes())
            .expect("the log reads back")
            .rows
            .len(),
        3
    );
    for bad in [
        [row_at(5, 0, 1, None), row_at(5, 0, 2, None)],
        [row_at(5, 1, 1, None), row_at(6, 0, 2, None)],
        [row_at(5, 0, 1, None), row_at(6, 1, 2, None)],
        [row_at(5, 0, 1, None), row_at(6, 0, 2, Some(LogGap::Known))],
    ] {
        let csv = export_csv(&info(), &bad);
        assert!(
            matches!(
                import_csv(csv.as_bytes()),
                Err(SplLogError::Line { line: 10, .. })
            ),
            "{bad:?}"
        );
    }
}

/// A wall clock stepped back within a run changes the rows' annotation only: they read
/// back in index order with their wall times as logged, and so do rows across a year's end.
#[test]
fn a_clock_step_back_and_a_new_year_read_back() {
    // 2026-12-31T23:59:58Z.
    let eve = 1_798_761_598;
    let r = [
        row_at(10, 0, eve, Some(LogGap::Known)),
        row_at(11, 0, eve + 1, None),
        row_at(12, 0, eve + 2, None),
        // Stepped back an hour.
        row_at(13, 0, eve - 3600 + 3, None),
    ];
    let csv = export_csv(&info(), &r);
    assert!(csv.contains(",2026-12-31T23:59:59.000Z,"));
    assert!(csv.contains(",2027-01-01T00:00:00.000Z,"));
    let back = import_csv(csv.as_bytes()).expect("the log reads back").rows;
    assert_eq!(
        back.iter()
            .map(|r| (r.second, r.start.0 / 1_000_000_000))
            .collect::<Vec<_>>(),
        vec![(10, eve), (11, eve + 1), (12, eve + 2), (13, eve - 3597)]
    );
}

#[test]
fn utc_text_parses_back() {
    for ns in [
        0,
        951_782_400_000_000_000,
        1_798_761_599_999_000_000,
        1_798_761_600_000_000_000,
        4_102_444_800_123_000_000,
    ] {
        assert_eq!(parse_utc_iso(&utc_iso(ns)), Some(ns), "{ns}");
    }
    for bad in [
        "2026-02-30T00:00:00.000Z",
        "2026-13-01T00:00:00.000Z",
        "2026-01-01T24:00:00.000Z",
        "2026-01-01T00:00:00Z",
        "+026-01-01T00:00:00.000Z",
    ] {
        assert_eq!(parse_utc_iso(bad), None, "{bad}");
    }
    for d in [-800_000, -1, 0, 59, 60, 11_016, 20_819, 2_000_000] {
        let (y, m, dd) = civil_from_days(d);
        assert_eq!(days_from_civil(y, m, dd), d);
    }
}
