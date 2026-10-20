use super::*;

#[test]
fn autosave_line() {
    const S: u64 = 1_000_000_000;
    let now = WallNs(1_000 * S);
    let a = |state, saved_at: Option<u64>| Autosave {
        state,
        saved_at: saved_at.map(WallNs),
        log_failures: Vec::new(),
    };
    assert_eq!(
        autosave(&a(AutosaveState::Off, None), now),
        "autosave     off"
    );
    assert_eq!(
        autosave(&a(AutosaveState::Saved, Some(400 * S)), now),
        "autosave     autosaved 10 min ago"
    );
    assert_eq!(
        autosave(&a(AutosaveState::Pending, None), now),
        "autosave     saving…"
    );
    let reason = "x".repeat(100);
    assert_eq!(
        autosave(
            &a(
                AutosaveState::Failed {
                    reason: reason.clone()
                },
                None
            ),
            now
        ),
        format!("autosave     FAILED: {reason}")
    );
}

#[test]
fn log_failure_lines() {
    use ac2_proto::model::{LogWriteFailure, MeterLog};
    const S: u64 = 1_000_000_000;
    let now = WallNs(1_000 * S);
    let mut a = Autosave {
        state: AutosaveState::Saved,
        saved_at: None,
        log_failures: Vec::new(),
    };
    assert!(log_failures(&a, &[], now).is_empty());
    a.log_failures.push(LogWriteFailure {
        meas: ac2_proto::units::MeasId(3),
        log: MeterLog::Bands,
        reason: "disk full".into(),
        since: WallNs(400 * S),
    });
    assert_eq!(
        log_failures(&a, &[], now),
        ["log          NOT SAVED: band log of measurement 3 not saved for 10 min: disk full"]
    );
}

#[test]
fn damaged_file_lines() {
    use ac2_proto::model::{DamagedFile, Rig, StateFile};
    let mut rig = Rig::default();
    assert!(damaged_files(&rig).is_empty());
    rig.damaged_files.push(DamagedFile {
        kind: StateFile::Calibrations,
        path: "/c/calibrations.json".into(),
        set_aside_as: Some("/c/calibrations.json.damaged-5".into()),
        reason: "expected value; started without it".into(),
        at: WallNs(0),
    });
    assert_eq!(
        damaged_files(&rig),
        [
            "damaged      calibrations calibrations.json damaged: expected value; started \
          without it; set aside as calibrations.json.damaged-5"
        ]
    );
}
