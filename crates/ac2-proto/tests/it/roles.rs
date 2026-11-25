//! Who may send what: the viewer set exactly, the rig set exactly, an operator everything
//! else.

use std::collections::BTreeSet;

use ac2_proto::Command;
use ac2_proto::model::{ClientRole, SessionRef};
use ac2_proto::units::{RigId, TransferId};

/// What a viewer may send, by wire name: reads that do not act on the rig, and `gen.stop`.
const VIEWER: [&str; 22] = [
    "hello",
    "state.snapshot",
    "state.since",
    "grid.get",
    "stream.status",
    "trace.list",
    "trace.get",
    "trace.export",
    "dynamics.table",
    "dynamics.envelope",
    "cal.list",
    "spl.log_get",
    "spl.history_get",
    "spl.band_log_get",
    "file.list",
    "rec.list",
    "plan.list",
    "server.info",
    "gen.stop",
    "file.download_begin",
    "file.download_chunk",
    "file.download_end",
];

#[test]
fn every_role_sends_exactly_its_set() {
    let rig = ClientRole::Rig { rig_id: RigId(1) };
    let all = ac2_proto::samples::commands();
    let names: BTreeSet<&str> = all.iter().map(Command::name).collect();
    for want in VIEWER {
        assert!(names.contains(want), "no sample of {want}");
    }
    for c in &all {
        let n = c.name();
        // The samples' download end names a file, so it is an operator's.
        let viewer = VIEWER.contains(&n) && n != "file.download_end";
        assert_eq!(c.allowed(&ClientRole::Viewer), viewer, "viewer: {n}");
        assert_eq!(
            c.allowed(&ClientRole::Operator),
            n != "source.state",
            "operator: {n}"
        );
        assert_eq!(
            c.allowed(&rig),
            matches!(n, "hello" | "source.state"),
            "rig: {n}"
        );
    }
}

#[test]
fn a_viewer_downloads_the_working_state_without_marking_it_saved() {
    let v = ClientRole::Viewer;
    assert!(Command::FileDownloadBegin { session: None }.allowed(&v));
    let named = Command::FileDownloadBegin {
        session: Some(SessionRef::Name {
            name: "friday".into(),
        }),
    };
    assert!(!named.allowed(&v));
    assert!(named.allowed(&ClientRole::Operator));
    let end = |saved_as: Option<String>| Command::FileDownloadEnd {
        transfer: TransferId(1),
        saved_as,
    };
    assert!(end(None).allowed(&v));
    assert!(!end(Some("friday.ac2session".into())).allowed(&v));
}
