//! A crafted trace file or session is refused before its data is kept: declared lengths
//! are checked before anything is allocated for them, grids before anything is formed on
//! them, values against what a capture produces.

use super::*;

/// `text` with the line starting `prefix` replaced by `prefix` + `f(its rest)`.
fn with_line(text: &str, prefix: &str, f: impl Fn(&str) -> String) -> String {
    let mut found = false;
    let out: String = text
        .lines()
        .map(|l| match l.strip_prefix(prefix) {
            Some(rest) if !found => {
                found = true;
                format!("{prefix}{}\n", f(rest))
            }
            _ => format!("{l}\n"),
        })
        .collect();
    assert!(found, "no line {prefix:?}");
    out
}

/// `text` with the JSON on the line starting `prefix` edited by `f`.
fn with_json(text: &str, prefix: &str, f: impl Fn(&mut serde_json::Value)) -> String {
    with_line(text, prefix, |rest| {
        let mut v: serde_json::Value = serde_json::from_str(rest).unwrap();
        f(&mut v);
        v.to_string()
    })
}

/// `text` with field `col` of the first data row after the line `header…` set to `v`.
fn with_field(text: &str, header: &str, col: usize, v: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().position(|l| l.starts_with(header)).unwrap() + 1;
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == at {
                let mut f: Vec<String> = l.split(',').map(str::to_owned).collect();
                f[col] = v.to_owned();
                format!("{}\n", f.join(","))
            } else {
                format!("{l}\n")
            }
        })
        .collect()
}

fn refused(text: &str, what: &str) -> ImportProblem {
    match import(text.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace) {
        Ok(_) => panic!("{what}: imported"),
        Err(e) => e.problem,
    }
}

/// The impulse-response announcement is checked before its table is read: a length past
/// [`TraceIr::MAX_POINTS`] (the vectors are sized from it), no points, a zero or non-finite
/// spacing, a zero sample rate; and a non-finite sample in the table.
#[test]
fn crafted_impulse_responses_refuse_before_allocating() {
    let sweep = export_csv(&sweep_trace(3));
    for (what, edit) in [
        ("points", serde_json::json!(u32::MAX)),
        ("no points", serde_json::json!(0)),
    ] {
        let t = with_json(&sweep, "# sweep_ir: ", |v| v["points"] = edit.clone());
        assert_eq!(refused(&t, what), ImportProblem::BadHeader, "{what}");
    }
    let t = with_json(&sweep, "# sweep_ir: ", |v| v["dt"] = serde_json::json!(0.0));
    assert_eq!(refused(&t, "dt"), ImportProblem::BadHeader);
    let t = with_json(&sweep, "# sweep_info: ", |v| {
        v["sample_rate"] = serde_json::json!(0.0);
    });
    assert_eq!(refused(&t, "sweep rate"), ImportProblem::BadHeader);

    let tf = export_csv(&super::sweep::transfer_with_ir(4));
    let t = with_json(&tf, "# transfer_ir: ", |v| {
        v["sample_rate"] = serde_json::json!(0.0);
    });
    assert_eq!(refused(&t, "sample rate"), ImportProblem::BadHeader);
    let t = with_json(&tf, "# transfer_ir: ", |v| {
        v["points"] = serde_json::json!(u32::MAX);
    });
    assert_eq!(refused(&t, "transfer points"), ImportProblem::BadHeader);
    for bad in ["NaN", "inf"] {
        let t = with_field(&tf, "t_s,", 1, bad);
        assert_eq!(refused(&t, bad), ImportProblem::OutOfRange, "{bad}");
    }
    // The ETC of an all-zero stretch is −∞ and passes; NaN does not.
    let t = with_field(&tf, "t_s,", 2, "-inf");
    import(t.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap();
    let t = with_field(&tf, "t_s,", 2, "NaN");
    assert_eq!(refused(&t, "etc NaN"), ImportProblem::OutOfRange);
}

/// A header grid is checked before its frequencies are formed: billions of columns, none
/// per octave, an FFT of no length, columns at no finite frequency.
#[test]
fn crafted_grids_refuse_before_forming_columns() {
    let csv = export_csv(&delayed(1, 1, 0.0, 0.0, 0.9));
    for g in [
        GridDef::Log {
            ppo: 48,
            k_min: i32::MIN,
            k_max: i32::MAX,
        },
        GridDef::Log {
            ppo: 0,
            k_min: 0,
            k_max: 10,
        },
        // Columns at 2^(2^30) Hz: not a finite frequency.
        GridDef::Log {
            ppo: 1,
            k_min: 1 << 30,
            k_max: (1 << 30) + 4,
        },
        GridDef::Linear {
            fs: Hz(48_000.0),
            n: 0,
        },
        GridDef::Linear {
            fs: Hz(48_000.0),
            n: u32::MAX,
        },
        GridDef::LogBins {
            fs: Hz(0.0),
            n: 4096,
            ppo: 48,
        },
        GridDef::IecBands {
            fraction: BandFraction::Third,
            centres: vec![Hz(1000.0), Hz(500.0)],
        },
    ] {
        let t = with_line(&csv, "# grid: ", |_| serde_json::to_string(&g).unwrap());
        assert_eq!(refused(&t, "grid"), ImportProblem::BadHeader, "{g:?}");
    }
}

/// Magnitudes: NaN is "no value" and passes; a value that is ±∞ as f32 or beyond what
/// later math keeps finite is refused, in the response and the distortion columns alike.
#[test]
fn crafted_magnitudes_refuse() {
    let csv = export_csv(&delayed(1, 1, 0.0, 0.0, 0.9));
    let t = with_field(&csv, "freq_hz,", 1, "NaN");
    import(t.as_bytes(), ImportFormat::Ac2Csv, ImportRole::Trace).unwrap();
    for bad in ["1e30", "5000", "-3000.5"] {
        let t = with_field(&csv, "freq_hz,", 1, bad);
        assert_eq!(refused(&t, bad), ImportProblem::OutOfRange, "{bad}");
    }
    let sweep = export_csv(&sweep_trace(3));
    let header = sweep
        .lines()
        .find(|l| l.starts_with("freq_hz,"))
        .unwrap()
        .to_owned();
    let h2 = header.split(',').position(|c| c == "h2_db").unwrap();
    let t = with_field(&sweep, "freq_hz,", h2, "1e30");
    assert_eq!(refused(&t, "h2"), ImportProblem::OutOfRange);
}

/// The whole-trace check: columns as long as the grid, values in range, a mic curve's
/// normalisation at a frequency.
#[test]
fn stored_trace_check() {
    use ac2_traces::check;
    let t = delayed(1, 1, 0.0, 0.0, 0.9);
    check::trace(&t).unwrap();
    let mut short = t.clone();
    short.columns.mag_db.pop();
    assert!(check::trace(&short).is_err());
    let mut phase = t.clone();
    phase.columns.phase_deg.as_mut().unwrap().pop();
    assert!(check::trace(&phase).is_err());
    let mut inf = t.clone();
    inf.columns.mag_db[3] = f32::INFINITY;
    assert!(check::trace(&inf).is_err());
    let mut coh = t.clone();
    coh.columns.coherence.as_mut().unwrap()[0] = 1.5;
    assert!(check::trace(&coh).is_err());
    let mut nan = t.clone();
    nan.columns.mag_db[3] = f32::NAN;
    check::trace(&nan).unwrap();
    let mut delay = t.clone();
    delay.meta.delay = Seconds(f64::NAN);
    assert!(check::trace(&delay).is_err());

    let mut s = sweep_trace(2);
    check::trace(&s).unwrap();
    s.sweep.as_mut().unwrap().thd.level_db.pop();
    assert!(check::trace(&s).is_err());
    let mut s = sweep_trace(2);
    s.sweep.as_mut().unwrap().ir.dt = Seconds(0.0);
    assert!(check::trace(&s).is_err());
    let mut s = sweep_trace(2);
    s.sweep.as_mut().unwrap().harmonics.reverse();
    assert!(check::trace(&s).is_err());

    let mut m = super::mic_curve::with_curve(t);
    check::trace(&m).unwrap();
    m.meta.mic_curve.as_mut().unwrap().f_norm = Hz(0.0);
    assert!(check::trace(&m).is_err());
}

fn saved() -> (tempfile::TempDir, std::path::PathBuf, session::Manifest) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("s");
    let mut s = super::sessions::session_sample();
    s.traces[1] = super::mic_curve::with_curve(s.traces[1].clone());
    session::save(&dir, &s).unwrap();
    session::load(&dir).unwrap();
    let m = session::read_manifest(&dir).unwrap();
    (tmp, dir, m)
}

fn corrupt(dir: &Path, what: &str) -> String {
    match session::load(dir) {
        Err(SessionError::Corrupt { msg, .. }) => msg,
        other => panic!("{what}: {other:?}"),
    }
}

fn edit_manifest(dir: &Path, f: impl Fn(&mut serde_json::Value)) {
    let p = dir.join(session::MANIFEST);
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    f(&mut v);
    std::fs::write(&p, v.to_string()).unwrap();
}

/// An SPL log names an SPL meter of the session, one log per meter.
#[test]
fn session_spl_logs_belong_to_spl_meters() {
    for (what, edit) in [
        ("orphan", serde_json::json!(99)),
        ("transfer", serde_json::json!(1)),
    ] {
        let (_tmp, dir, _) = saved();
        edit_manifest(&dir, |v| v["spl_logs"][0]["meas"] = edit.clone());
        let msg = corrupt(&dir, what);
        assert!(msg.contains("SPL log of measurement"), "{what}: {msg}");
    }
    let (_tmp, dir, _) = saved();
    edit_manifest(&dir, |v| {
        let l = v["spl_logs"][0].clone();
        v["spl_logs"].as_array_mut().unwrap().push(l);
    });
    assert!(corrupt(&dir, "twice").contains("two SPL logs"));
}

/// SPL and band log rows hold what the meter writes: no NaN or +∞ level, no level past
/// what f32 samples can reach, a measured second, a start in the wall clock's range, a
/// correction within the band meter's.
#[test]
fn session_spl_log_rows_are_checked() {
    let (_tmp, dir, m) = saved();
    let log = dir.join(&m.spl_logs[0].file);
    let good = std::fs::read_to_string(&log).unwrap();
    for (what, col, v) in [
        ("nan level", 4, "nan"),
        ("inf level", 5, "inf"),
        ("level past f32", 6, "1000"),
        ("nothing measured", 2, "0"),
        ("start past the clock", 1, "18446744073709551615"),
        ("position", 10, "1e9"),
    ] {
        let t = with_field(&good, "start_utc,", col, v);
        std::fs::write(&log, t).unwrap();
        let msg = corrupt(&dir, what);
        assert!(msg.contains("line"), "{what}: {msg}");
    }
    // −∞ (a second without energy) is a level the meter writes.
    std::fs::write(&log, with_field(&good, "start_utc,", 8, "-inf")).unwrap();
    session::load(&dir).unwrap();
    std::fs::write(&log, &good).unwrap();

    let bands = dir.join(m.spl_logs[0].bands.as_ref().unwrap());
    let good = std::fs::read_to_string(&bands).unwrap();
    for (what, col, v) in [("nan band", 9, "nan"), ("correction", 5, "20")] {
        std::fs::write(&bands, with_field(&good, "start_utc,", col, v)).unwrap();
        corrupt(&dir, what);
    }
}

/// Trace data in a session: columns off the grid, a crafted grid in the manifest, a mic
/// curve with no normalisation frequency or points out of order.
#[test]
fn session_trace_data_is_checked() {
    let (_tmp, dir, m) = saved();
    let file = dir.join(&m.traces[0].file);
    let good = std::fs::read_to_string(&file).unwrap();
    // One data row short of the grid.
    let at = good.find("\nfreq_hz,").unwrap() + 1;
    let row = at + good[at..].find('\n').unwrap() + 1;
    let end = row + good[row..].find('\n').unwrap() + 1;
    std::fs::write(&file, format!("{}{}", &good[..row], &good[end..])).unwrap();
    corrupt(&dir, "short");
    std::fs::write(&file, &good).unwrap();

    let (_tmp, dir, _) = saved();
    edit_manifest(&dir, |v| {
        v["traces"][1]["meta"]["mic_curve"]["f_norm"] = serde_json::json!(0.0);
    });
    assert!(corrupt(&dir, "f_norm").contains("normalisation"));
    let (_tmp, dir, _) = saved();
    edit_manifest(&dir, |v| {
        v["traces"][1]["mic_curve_points"] = serde_json::json!([[1000.0, 0.0], [50.0, -2.0]]);
    });
    assert!(corrupt(&dir, "points").contains("mic curve"));
}
