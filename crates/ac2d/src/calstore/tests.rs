use super::*;
use ac2_proto::model::{CalKey, CurveChoice, SplCal};
use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, WallNs};

fn key(dev: &str, ch: u16, mic: &str) -> CalKey {
    CalKey {
        device: DeviceId(dev.into()),
        channel: ch,
        mic: mic.into(),
    }
}

fn spl(sens: f64, at: u64, freq: f64) -> SplCal {
    SplCal {
        sensitivity: Db(sens),
        calibrator_level: DbSpl(94.0),
        calibrator_freq: Hz(freq),
        measured: Dbfs(94.0 - sens),
        calibrated_at: WallNs(at),
    }
}

fn entry(k: CalKey, s: SplCal) -> CalEntry {
    CalEntry { key: k, spl: s }
}

fn curve_ref(label: &str, at: u64) -> MicCurveRef {
    MicCurveRef {
        label: label.into(),
        file_name: format!("{label}.frd"),
        content_hash: content_hash(label.as_bytes()),
        points: 3,
        f_lo: Hz(20.0),
        f_hi: Hz(10_000.0),
        imported_at: WallNs(at),
        stated_sensitivity: Some(15.0),
    }
}

fn setup(ch: u16, mic: Option<&str>, curve: CurveChoice) -> InputSetup {
    InputSetup {
        channel: ch,
        mic: mic.map(Into::into),
        curve,
    }
}

fn label(l: &str) -> CurveChoice {
    CurveChoice::Curve { label: l.into() }
}

fn points(gain_hi: f64) -> Arc<MicCurve> {
    Arc::new(MicCurve::from_points(&[(20.0, 0.0), (1000.0, 0.0), (10_000.0, gain_hi)]).expect("c"))
}

fn id(mic: &str, label: &str) -> MicCurveId {
    MicCurveId {
        mic: mic.into(),
        label: label.into(),
    }
}

fn state(c: &Contents) -> State {
    let mut s = ac2_proto::samples::state();
    s.calibrations = c.calibrations.clone();
    s.mics = c.mics.clone();
    s.inputs = c.inputs.clone();
    s
}

#[test]
fn sensitivity_matching_rules() {
    let dev = DeviceId("hw:A".into());
    let mut c = Contents {
        calibrations: vec![
            entry(key("hw:A", 2, "M30"), spl(120.0, 10, 1000.0)),
            entry(key("hw:A", 2, "ECM"), spl(110.0, 20, 1000.0)),
            entry(key("hw:B", 5, "UMIK"), spl(100.0, 5, 250.0)),
        ],
        ..Contents::default()
    };
    let store = CalStore::memory();
    let mut r = |inputs: Vec<InputSetup>, ch: u16| {
        c.inputs = inputs;
        resolve(&state(&c), &store, &dev, ch)
    };

    // Exact device + channel + mic.
    let x = r(vec![setup(2, Some("M30"), CurveChoice::Off)], 2);
    assert_eq!(
        x.status,
        CalStatus::Verified {
            calibrated_at: WallNs(10)
        }
    );
    assert_eq!(x.sensitivity, Some(120.0));
    // Another mic on this input: the newest calibration of this input, flagged.
    let x = r(vec![setup(2, Some("Other"), CurveChoice::Off)], 2);
    assert_eq!(
        x.status,
        CalStatus::OtherMicOrInput {
            calibrated_at: WallNs(20)
        }
    );
    assert_eq!(x.sensitivity, Some(110.0));
    // No mic name set: same, flagged (never verified without a name).
    let x = r(vec![], 2);
    assert!(matches!(x.status, CalStatus::OtherMicOrInput { .. }));
    // The mic moved to another input / device: its calibration, flagged.
    let x = r(vec![setup(7, Some("UMIK"), CurveChoice::Off)], 7);
    assert_eq!(
        x.status,
        CalStatus::OtherMicOrInput {
            calibrated_at: WallNs(5)
        }
    );
    assert_eq!(x.sensitivity, Some(100.0));
    // Nothing applies.
    let x = r(vec![setup(7, Some("New"), CurveChoice::Off)], 7);
    assert_eq!(x.status, CalStatus::Uncalibrated);
    assert_eq!(x.sensitivity, None);
}

#[test]
fn only_the_chosen_curve_applies() {
    let dev = DeviceId("hw:A".into());
    let mut c = Contents {
        calibrations: vec![entry(key("hw:A", 2, "MM1"), spl(120.0, 10, 250.0))],
        mics: vec![Mic {
            name: "MM1".into(),
            curves: vec![curve_ref("0°", 1), curve_ref("90°", 2)],
        }],
        inputs: vec![],
    };
    let mut store = CalStore::memory();
    let mut curves = HashMap::new();
    curves.insert(id("MM1", "0°"), points(1.0));
    curves.insert(id("MM1", "90°"), points(6.0));
    store.persist(&c, curves).expect("memory persist");
    let mut r = |row: InputSetup| {
        let ch = row.channel;
        c.inputs = vec![row];
        resolve(&state(&c), &store, &dev, ch)
    };

    // The chosen curve, normalised at the calibration's calibrator frequency.
    let x = r(setup(2, Some("MM1"), label("90°")));
    let k = x.correction.expect("curve");
    assert_eq!(k.f_norm(), 250.0);
    assert!((k.db(10_000.0) - 6.0).abs() < 1e-12);
    assert_eq!(x.curve.map(|c| c.label), Some("90°".into()));
    let x = r(setup(2, Some("MM1"), label("0°")));
    assert!((x.correction.expect("curve").db(10_000.0) - 1.0).abs() < 1e-12);
    // Off, none chosen among two, a label not stored, another mic: nothing applies.
    for row in [
        setup(2, Some("MM1"), CurveChoice::Off),
        setup(2, Some("MM1"), CurveChoice::NotChosen),
        setup(2, Some("MM1"), label("45°")),
        setup(2, Some("ECM"), label("0°")),
        setup(2, None, CurveChoice::NotChosen),
    ] {
        let x = r(row.clone());
        assert!(x.correction.is_none() && x.curve.is_none(), "{row:?}");
    }
    // The curve follows the mic to an uncalibrated input: normalised at 1 kHz.
    let x = r(setup(5, Some("MM1"), label("90°")));
    assert_eq!(x.correction.expect("curve").f_norm(), 250.0);
    c.calibrations.clear();
    let mut store2 = CalStore::memory();
    let mut curves = HashMap::new();
    curves.insert(id("MM1", "90°"), points(6.0));
    store2.persist(&c, curves).expect("memory persist");
    c.inputs = vec![setup(5, Some("MM1"), label("90°"))];
    let x = resolve(&state(&c), &store2, &dev, 5);
    assert_eq!(x.correction.expect("curve").f_norm(), DEFAULT_F_NORM);
}

#[test]
fn file_round_trip_and_atomic_write() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("cal").join("calibrations.json");
    let (mut s, c) = CalStore::open(&path);
    assert_eq!(c, Contents::default());
    s.check().expect("missing file is fine");
    let c = Contents {
        calibrations: vec![
            entry(
                key("hw:A", 2, "M30 #1"),
                spl(120.050_816_734_789_08, 10, 1000.0),
            ),
            entry(key("hw:A", 3, "ECM"), spl(110.0, 20, 1000.0)),
        ],
        mics: vec![Mic {
            name: "M30 #1".into(),
            curves: vec![curve_ref("0°", 3), curve_ref("90°", 4)],
        }],
        inputs: vec![
            setup(2, Some("M30 #1"), label("90°")),
            setup(3, None, CurveChoice::NotChosen),
            setup(4, Some("ECM"), CurveChoice::Off),
        ],
    };
    let mut curves = HashMap::new();
    curves.insert(id("M30 #1", "0°"), points(1.5));
    curves.insert(id("M30 #1", "90°"), points(4.5));
    s.persist(&c, curves).expect("persist");
    // Only the file itself: no temporary left behind.
    let names: Vec<_> = std::fs::read_dir(path.parent().expect("dir"))
        .expect("ls")
        .map(|d| d.expect("e").file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");

    let (s2, c2) = CalStore::open(&path);
    s2.check().expect("readable");
    assert_eq!(c2, c);
    assert_eq!(
        s2.curve("M30 #1", "90°").map(|c| c.gains().to_vec()),
        Some(vec![0.0, 0.0, 4.5])
    );
    assert_eq!(
        s2.curve("M30 #1", "0°").map(|c| c.gains().to_vec()),
        Some(vec![0.0, 0.0, 1.5])
    );
}

#[test]
fn a_store_of_the_previous_format_is_set_aside() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("calibrations.json");
    let old = br#"{"format":"ac2-calibrations","version":1,"entries":[{"key":{"device":"d","channel":0,"mic":"MM1 34804"},"spl":null,"mic_curve":{"reference":{"name":"c","file_name":"c.txt","content_hash":"0","points":2,"f_lo":20.0,"f_hi":200.0,"imported_at":1},"points":[[20.0,0.0],[200.0,0.0]]}}],"inputs":[{"channel":0,"mic":"MM1 34804","mic_curve":true}]}"#;
    std::fs::write(&path, old).expect("write");
    let (mut s, c) = CalStore::open(&path);
    // Empty and writable: the old file is kept beside, never deleted or read as this format.
    assert_eq!(c, Contents::default());
    s.check().expect("writable");
    let aside = dir.path().join("calibrations.json.v1");
    assert_eq!(std::fs::read(&aside).expect("set aside"), old);
    assert!(!path.exists());
    s.persist(&Contents::default(), HashMap::new())
        .expect("persist");
    assert!(path.exists());
    // A second old file does not overwrite the first one set aside.
    std::fs::write(&path, old).expect("write");
    let (s, _) = CalStore::open(&path);
    s.check().expect("writable");
    let n = std::fs::read_dir(dir.path())
        .expect("ls")
        .filter(|e| {
            e.as_ref().is_ok_and(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("calibrations.json.v1")
            })
        })
        .count();
    assert_eq!(n, 2);
}

#[test]
fn unreadable_file_is_refused_and_never_written() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("calibrations.json");
    for bad in [
        &b"{ not json"[..],
        br#"{"format":"other","version":2,"sensitivities":[],"mics":[],"inputs":[]}"#,
        br#"{"format":"ac2-calibrations","version":2,"sensitivities":[],"mics":[{"name":"m","curves":[{"reference":{"label":"0deg","file_name":"c","content_hash":"0","points":2,"f_lo":20.0,"f_hi":10.0,"imported_at":1,"stated_sensitivity":null},"points":[[20.0,0.0],[10.0,0.0]]}]}],"inputs":[]}"#,
        br#"{"format":"ac2-calibrations","version":2,"sensitivities":[],"mics":[{"name":"m","curves":[]}],"inputs":[]}"#,
    ] {
        std::fs::write(&path, bad).expect("write");
        let (mut s, c) = CalStore::open(&path);
        assert_eq!(c, Contents::default());
        let err = s.check().expect_err("refused");
        assert_eq!(err.code, ErrorCode::Refused);
        assert!(matches!(err.detail, Some(ErrorDetail::CalStore { .. })), "{err:?}");
        assert!(err.msg.contains("never overwritten"), "{}", err.msg);
        let c = Contents {
            calibrations: vec![entry(key("hw:A", 2, "M30"), spl(1.0, 1, 1000.0))],
            ..Contents::default()
        };
        assert!(s.persist(&c, HashMap::new()).is_err());
        assert_eq!(std::fs::read(&path).expect("read"), bad, "file must be untouched");
    }
}

#[test]
fn labels_and_stated_sensitivity_from_the_real_headers() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mic_curves");
    for (file, want) in [
        ("449350_34804_0Grad.txt", "0°"),
        ("449350_34804_90Grad.txt", "90°"),
    ] {
        let bytes = std::fs::read(dir.join(file)).expect("fixture");
        let info = ac2_core::mic_curve::file_info(&bytes, file);
        assert_eq!(cal::default_label(info.angle_deg, file), want);
        assert_eq!(info.stated_sensitivity_mv_per_pa, Some(15.0));
        let c = MicCurve::parse(&bytes).expect("parses");
        assert_eq!(c.len(), 100);
    }
}

#[test]
fn mic_names_labels_and_curve_errors() {
    for ok in ["M30", "M30 #1234", "Ä-mic"] {
        check_mic_name(ok).expect(ok);
    }
    let long = "x".repeat(cal::MAX_MIC_NAME + 1);
    for bad in ["", " M30", "M30 ", "a\tb", long.as_str()] {
        assert_eq!(check_mic_name(bad).expect_err(bad).code, ErrorCode::Invalid);
    }
    for bad in ["", "off", "None", " 0°"] {
        assert_eq!(check_label(bad).expect_err(bad).code, ErrorCode::Invalid);
    }
    let e = curve_error(MicCurveFileError::NotAscending { line: 7 });
    assert_eq!(e.code, ErrorCode::Invalid);
    assert_eq!(
        e.detail,
        Some(ErrorDetail::MicCurveFile {
            line: Some(7),
            reason: MicCurveFileReason::NotAscending
        })
    );
    assert_eq!(content_hash(b""), "cbf29ce484222325");
    assert_eq!(content_hash(b"a"), "af63dc4c8601ec8c");
}
