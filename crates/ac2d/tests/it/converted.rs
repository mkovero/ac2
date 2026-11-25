//! The stored files `tools/proto-next/convert.py` writes are what this daemon reads: its
//! samples in the new shapes (`tools/proto-next/samples/new`, which its own test produces
//! from the old ones) load whole, nothing set aside.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::{Path, PathBuf};

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2d::{Daemon, NetworkSecurity};
use common::*;

fn samples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/proto-next/samples/new")
}

fn copy(from: &Path, to: &Path) {
    std::fs::copy(from, to).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // As the converter leaves an authorized-clients file: the daemon trusts no other.
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn the_converters_samples_load_whole() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    copy(&samples().join("rig.json"), &d.join("rig.json"));
    copy(
        &samples().join("calibrations.json"),
        &d.join("calibrations.json"),
    );
    copy(
        &samples().join("authorized_clients"),
        &d.join("authorized_clients"),
    );
    std::fs::create_dir(d.join("plans")).unwrap();
    copy(
        &samples().join("plans/Hall.json"),
        &d.join("plans/Hall.json"),
    );

    let mut cfg = net_config(
        manual_rig(),
        NetworkSecurity {
            server_key_file: d.join("server.key"),
            authorized_clients_file: d.join("authorized_clients"),
        },
    );
    cfg.session_dir = Some(d.join("sessions"));
    cfg.rig_settings = Some(d.join("rig.json"));
    cfg.rig_logs = Some(d.join("rig-logs"));
    cfg.cal_store = Some(d.join("calibrations.json"));
    cfg.plan_dir = Some(d.join("plans"));
    let h = Daemon::start(cfg).unwrap();
    net_endpoints(&h);
    let (mut c, _sub) = connect(&h, &[]);
    let st = c.state();
    assert!(
        st.rig.damaged_files.is_empty(),
        "{:?}",
        st.rig.damaged_files
    );
    let m = st.measurements.iter().find(|m| m.id == MeasId(1)).unwrap();
    assert_eq!(m.config.scope, MeasScope::Rig);
    let MeasKind::Spl { config } = &m.config.kind else {
        panic!("{m:?}")
    };
    assert!(config.bands.as_ref().unwrap().transfer.is_some());
    assert_eq!(
        st.remote.peers[0].rig,
        RigId(18_446_744_073_709_551_557),
        "{:?}",
        st.remote.peers
    );
    assert_eq!(st.calibrations.len(), 2);
    assert_eq!(st.line_calibrations.len(), 1);
    assert_eq!(st.mics.len(), 1);
    assert_eq!(st.plans.len(), 1);
    drop(c);
    h.shutdown();
}
