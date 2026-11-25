//! A transfer at the `bins` resolution measures on the 1/96 grid: the bins are offered beside
//! that grid, so its columns, grid and unresolved ranges are the 1/96 ones.
#![allow(clippy::unwrap_used)]

use crate::common;
use crate::transfer::{run_tf, tf};

use std::time::Duration;

use ac2_proto::Command;
use ac2_proto::model::{MeasKind, Resolution, TransferResolution};
use ac2_proto::units::MeasId;
use ac2d::Daemon;
use common::*;

#[test]
fn bins_measures_on_the_ninety_sixth_grid() {
    init_log();
    let backend = manual_rig();
    let mut cfg = config(backend.clone(), inproc("tf-bins"));
    cfg.lease_expiry = Duration::from_secs(60);
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/1/tf"]);
    c.ok(Command::StreamOpen {
        config: stream(true),
    });
    let mut m = transfer("main");
    if let MeasKind::Transfer { config } = &mut m.kind {
        config.resolution = Resolution::NinetySixth.into();
    }
    c.ok(Command::MeasCreate { config: m.clone() });
    c.ok(Command::MeasStart { meas: MeasId(1) });
    let mut d = driver(&backend);
    let fine = run_tf(&mut d, &sub, 0.5, 0);
    let rev = fine.stamp.config_rev.0;

    if let MeasKind::Transfer { config } = &mut m.kind {
        config.resolution = TransferResolution::Bins;
    }
    c.update_ok(MeasId(1), m);
    let bins = run_tf(&mut d, &sub, 0.5, rev + 1);
    assert!(fine.stamp.grid_id.is_some());
    assert_eq!(bins.stamp.grid_id, fine.stamp.grid_id);
    assert_eq!(tf(&bins).mag.len(), 1440);
    assert_eq!(tf(&bins).mag.len(), tf(&fine).mag.len());
    assert_eq!(tf(&bins).meta.unresolved, tf(&fine).meta.unresolved);
    h.shutdown();
}
