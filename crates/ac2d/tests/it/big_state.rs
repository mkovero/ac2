//! A state larger than one ctrl message: `state.snapshot` sends it as a transfer, ac2-client
//! fetches it for `Client::snapshot` and for its mirror's resync, `state.since` answers
//! `resync_required` rather than a list too large to send, and an event over the event
//! bound is not replayed. The state is made large with plans of many markers, each plan's
//! event under the event bound unless a test wants one over it.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_proto::model::{Marker, MarkerKind, PlanSpec};
use ac2_proto::units::Rev;
use ac2_proto::{Command, ErrorCode, MAX_CTRL_BYTES, ReplyBody, SnapshotTransfer};
use ac2d::Daemon;
use common::*;

/// A ceiling, not an expectation: waits end on their condition, normally within a second.
const T: Duration = Duration::from_secs(30);
/// Markers of a plan whose event is a little under the 1 MiB event bound.
pub const UNDER_EVENT_BOUND: usize = 14_000;
/// Plans of [`UNDER_EVENT_BOUND`] markers that make a state over one ctrl message.
pub const PLANS_OVER_CTRL: usize = 5;

/// A plan `name` of `markers` named points (a point may lie anywhere, so nothing checks
/// them against each other or the outline).
pub fn big_plan(name: &str, markers: usize) -> PlanSpec {
    PlanSpec {
        name: name.into(),
        outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 7.0], [0.0, 7.0]],
        ceiling: Some(3.5),
        zones: vec![],
        markers: (0..markers)
            .map(|i| Marker {
                name: format!("p-{i:05}"),
                kind: MarkerKind::Point,
                at: [i as f64 * 0.001, 1.5, 1.2],
                heading_deg: None,
            })
            .collect(),
        surfaces: vec![],
    }
}

fn plans(c: &ac2_client::Client) -> usize {
    c.view().state.as_ref().map_or(0, |s| s.plans.len())
}

fn resync_required(r: Result<ReplyBody, ac2_client::ClientError>, why: &str) {
    let e = r.expect_err("resync_required");
    assert_eq!(e.code(), Some(ErrorCode::ResyncRequired), "{e}");
    assert!(e.to_string().contains(why), "{e}");
}

async fn stop(h: ac2d::Handle) {
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

/// Over one ctrl message the snapshot goes as a transfer of the msgpack `StateSnapshot`;
/// `Client::snapshot` and a mirror connecting then both fetch it, `state.since` over more
/// than one reply holds asks for a snapshot, and a mirror told to resync by an event over
/// the event bound resyncs through the transfer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_big_state_syncs_by_transfer() {
    init_log();
    let h = Daemon::start(config(manual_rig(), local_ipc())).unwrap();
    let c = mirror(&h, "big-state-test").await;
    let before = c.view().rev;
    for i in 0..PLANS_OVER_CTRL {
        c.call(Command::PlanSave {
            plan: big_plan(&format!("big {i}"), UNDER_EVENT_BOUND),
        })
        .await
        .unwrap();
    }
    // Each plan's event went live; the mirror took them without a snapshot.
    until("the plans' events", T, || plans(&c) == PLANS_OVER_CTRL).await;
    assert_eq!(c.view().snapshots, 1);

    // The reply holds no state: a transfer of it.
    let t = match c.call(Command::StateSnapshot).await.unwrap() {
        ReplyBody::SnapshotTransfer(t) => t,
        other => panic!("{}", ac2_client::body_name(&other)),
    };
    assert!(t.transfer.bytes as usize > MAX_CTRL_BYTES, "{t:?}");
    assert_eq!(t.transfer.file_name, SnapshotTransfer::file_name(t.rev));
    assert_eq!(t.transfer.file_name, format!("state-{}.msgpack", t.rev.0));
    // Ended, it is gone.
    c.call(Command::FileDownloadEnd {
        transfer: t.transfer.transfer,
        saved_as: None,
    })
    .await
    .unwrap();
    let e = c
        .call(Command::FileDownloadChunk {
            transfer: t.transfer.transfer,
            offset: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some(ErrorCode::NotFound), "{e}");

    // Fetched whole and decoded.
    let s = c.snapshot().await.unwrap();
    assert_eq!(s.rev, t.rev);
    assert_eq!(s.state.plans.len(), PLANS_OVER_CTRL);
    assert_eq!(s.state.plans[3], big_plan("big 3", UNDER_EVENT_BOUND));

    // The events since before the plans are more than one reply holds.
    resync_required(
        c.call(Command::StateSince { rev: before }).await,
        "over one ctrl reply",
    );
    // Those since the last one are not.
    let ReplyBody::Events(evs) = c
        .call(Command::StateSince {
            rev: Rev(t.rev.0 - 1),
        })
        .await
        .unwrap()
    else {
        panic!("events")
    };
    assert_eq!(evs[0].rev, t.rev);

    // A mirror connecting now syncs from the transfer.
    let late = mirror(&h, "big-state-late").await;
    assert_eq!(plans(&late), PLANS_OVER_CTRL);
    assert_eq!(late.view().snapshots, 1);
    assert_eq!(late.view().state.as_ref().unwrap().plans, s.state.plans);

    // A plan over the event bound is not sent and not replayed: every mirror is told to
    // resync, and resyncs by the transfer.
    let at = c.view().rev;
    c.call(Command::PlanSave {
        plan: big_plan("huge", 2 * UNDER_EVENT_BOUND),
    })
    .await
    .unwrap();
    resync_required(
        c.call(Command::StateSince { rev: at }).await,
        "no longer replayable",
    );
    for m in [&c, &late] {
        until("the resync", T, || plans(m) == PLANS_OVER_CTRL + 1).await;
        assert!(m.view().snapshots >= 2);
    }
    drop((c, late));
    stop(h).await;
}
