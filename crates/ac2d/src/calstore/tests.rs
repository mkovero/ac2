use super::*;
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

fn entry(k: CalKey, s: Option<SplCal>, curve_at: Option<u64>) -> CalEntry {
    CalEntry {
        key: k,
        spl: s,
        mic_curve: curve_at.map(|at| MicCurveRef {
            name: "c".into(),
            file_name: "c.frd".into(),
            content_hash: content_hash(b"x"),
            points: 2,
            f_lo: Hz(20.0),
            f_hi: Hz(20_000.0),
            imported_at: WallNs(at),
        }),
    }
}

fn setup(ch: u16, mic: Option<&str>, on: bool) -> InputSetup {
    InputSetup {
        channel: ch,
        mic: mic.map(Into::into),
        mic_curve: on,
    }
}

fn curve(gain_hi: f64) -> Arc<MicCurve> {
    Arc::new(MicCurve::from_points(&[(20.0, 0.0), (1000.0, 0.0), (10_000.0, gain_hi)]).expect("c"))
}

#[test]
fn matching_rules() {
    let dev = DeviceId("hw:A".into());
    let entries = vec![
        entry(key("hw:A", 2, "M30"), Some(spl(120.0, 10, 1000.0)), None),
        entry(key("hw:A", 2, "ECM"), Some(spl(110.0, 20, 1000.0)), None),
        entry(key("hw:B", 5, "UMIK"), Some(spl(100.0, 5, 250.0)), None),
    ];
    let store = CalStore::memory();
    let r = |inputs: &[InputSetup], ch: u16| resolve(&entries, inputs, &store, &dev, ch);

    // Exact device + channel + mic.
    let x = r(&[setup(2, Some("M30"), true)], 2);
    assert_eq!(
        x.status,
        CalStatus::Verified {
            calibrated_at: WallNs(10)
        }
    );
    assert_eq!(x.sensitivity, Some(120.0));
    // Another mic on this input: the newest calibration of this input, flagged.
    let x = r(&[setup(2, Some("Other"), true)], 2);
    assert_eq!(
        x.status,
        CalStatus::OtherMicOrInput {
            calibrated_at: WallNs(20)
        }
    );
    assert_eq!(x.sensitivity, Some(110.0));
    // No mic name set: same, flagged (never verified without a name).
    let x = r(&[], 2);
    assert_eq!(
        x.status,
        CalStatus::OtherMicOrInput {
            calibrated_at: WallNs(20)
        }
    );
    // The mic moved to another input / device: its calibration, flagged.
    let x = r(&[setup(7, Some("UMIK"), true)], 7);
    assert_eq!(
        x.status,
        CalStatus::OtherMicOrInput {
            calibrated_at: WallNs(5)
        }
    );
    assert_eq!(x.sensitivity, Some(100.0));
    // Nothing applies.
    let x = r(&[setup(7, Some("New"), true)], 7);
    assert_eq!(x.status, CalStatus::Uncalibrated);
    assert_eq!(x.sensitivity, None);
}

#[test]
fn curve_follows_the_mic_and_its_switch() {
    let dev = DeviceId("hw:A".into());
    let entries = vec![
        // M30 on input 2: calibrated at 250 Hz, curve imported.
        entry(key("hw:A", 2, "M30"), Some(spl(120.0, 10, 250.0)), Some(1)),
        // Another curve for M30, imported later on another device.
        entry(key("hw:B", 0, "M30"), None, Some(2)),
        // ECM: calibration only.
        entry(key("hw:A", 3, "ECM"), Some(spl(110.0, 20, 1000.0)), None),
    ];
    let mut store = CalStore::memory();
    let mut curves = HashMap::new();
    curves.insert(key("hw:A", 2, "M30"), curve(6.0));
    curves.insert(key("hw:B", 0, "M30"), curve(3.0));
    store
        .persist(&entries, &[], curves)
        .expect("memory persist");
    let r = |inputs: &[InputSetup], ch: u16| resolve(&entries, inputs, &store, &dev, ch);

    // Exact entry's curve, normalised at that entry's calibrator frequency.
    let x = r(&[setup(2, Some("M30"), true)], 2);
    let c = x.correction.expect("curve");
    assert_eq!(c.f_norm(), 250.0);
    assert!((c.db(10_000.0) - 6.0).abs() < 1e-12);
    // Switched off.
    assert!(r(&[setup(2, Some("M30"), false)], 2).correction.is_none());
    // M30 moved to input 3 (ECM's input): sensitivity of input 3 flagged, the newest M30
    // curve, normalised at the ECM calibration's 1 kHz.
    let x = r(&[setup(3, Some("M30"), true)], 3);
    assert!(matches!(x.status, CalStatus::OtherMicOrInput { .. }));
    let c = x.correction.expect("curve");
    assert_eq!(c.f_norm(), 1000.0);
    assert!((c.db(10_000.0) - 3.0).abs() < 1e-12);
    // Never another mic's curve; none without a mic name.
    assert!(r(&[setup(2, Some("ECM"), true)], 2).correction.is_none());
    assert!(r(&[], 2).correction.is_none());
}

#[test]
fn file_round_trip_and_atomic_write() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("cal").join("calibrations.json");
    let (mut s, e, i) = CalStore::open(&path);
    assert!(e.is_empty() && i.is_empty());
    s.check().expect("missing file is fine");
    let entries = vec![
        entry(
            key("hw:A", 2, "M30 #1"),
            Some(spl(120.050_816_734_789_08, 10, 1000.0)),
            Some(3),
        ),
        entry(key("hw:A", 3, "ECM"), Some(spl(110.0, 20, 1000.0)), None),
    ];
    let inputs = vec![setup(2, Some("M30 #1"), false), setup(3, None, true)];
    let mut curves = HashMap::new();
    curves.insert(key("hw:A", 2, "M30 #1"), curve(4.5));
    s.persist(&entries, &inputs, curves).expect("persist");
    // Only the file itself: no temporary left behind.
    let names: Vec<_> = std::fs::read_dir(path.parent().expect("dir"))
        .expect("ls")
        .map(|d| d.expect("e").file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");

    let (s2, e2, i2) = CalStore::open(&path);
    s2.check().expect("readable");
    assert_eq!(e2, entries);
    assert_eq!(i2, inputs);
    assert_eq!(
        s2.curve(&key("hw:A", 2, "M30 #1"))
            .map(|c| c.gains().to_vec()),
        Some(vec![0.0, 0.0, 4.5])
    );
}

#[test]
fn unreadable_file_is_refused_and_never_written() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("calibrations.json");
    for bad in [
        &b"{ not json"[..],
        br#"{"format":"ac2-calibrations","version":2,"entries":[],"inputs":[]}"#,
        br#"{"format":"ac2-calibrations","version":1,"entries":[{"key":{"device":"d","channel":0,"mic":"m"},"spl":null,"mic_curve":{"reference":{"name":"c","file_name":"c","content_hash":"0","points":2,"f_lo":20.0,"f_hi":10.0,"imported_at":1},"points":[[20.0,0.0],[10.0,0.0]]}}],"inputs":[]}"#,
    ] {
        std::fs::write(&path, bad).expect("write");
        let (mut s, e, _) = CalStore::open(&path);
        assert!(e.is_empty());
        let err = s.check().expect_err("refused");
        assert_eq!(err.code, ErrorCode::Refused);
        assert!(matches!(err.detail, Some(ErrorDetail::CalStore { .. })), "{err:?}");
        assert!(err.msg.contains("never overwritten"), "{}", err.msg);
        let entries = vec![entry(key("hw:A", 2, "M30"), Some(spl(1.0, 1, 1000.0)), None)];
        assert!(s.persist(&entries, &[], HashMap::new()).is_err());
        assert_eq!(std::fs::read(&path).expect("read"), bad, "file must be untouched");
    }
}

#[test]
fn mic_names_and_curve_errors() {
    for ok in ["M30", "M30 #1234", "Ä-mic"] {
        check_mic_name(ok).expect(ok);
    }
    let long = "x".repeat(MAX_MIC_NAME + 1);
    for bad in ["", " M30", "M30 ", "a\tb", long.as_str()] {
        assert_eq!(check_mic_name(bad).expect_err(bad).code, ErrorCode::Invalid);
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
