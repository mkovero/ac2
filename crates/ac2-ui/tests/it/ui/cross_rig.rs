//! A transfer function across two rigs against the fake daemon (`docs/design/remote-inputs.md`
//! §5): the top bar's chip and banner, the pane's title and banner say the clocks between the
//! rigs in words; the chip opens the Connection page with its link offset line.

use super::*;
use ac2_proto::event::Change;
use ac2_proto::model::{
    CrossRigTransfer, CrossTfState, NotLockedReason, PeerClock, PeerRelation, REMOTE_BASE,
    RemoteAudio, RemoteInput, RemoteInputStatus, RemoteLink, RemoteRig, RemoteSide, RemoteState,
    StopWhy,
};
use ac2_proto::units::{RigId, Seconds};

const KETUNKOLO: RigId = RigId(0x6b65_7475);

/// The fake rig's session takes ketunkolo's input 1; their clocks are not locked and Main L,
/// measuring on that input, holds its result.
fn not_locked(rig: &common::Rig) {
    let mut s = rig.fake.lock();
    let mut session = s.state.session.clone();
    if let Some(o) = session.open.as_mut() {
        o.config.remote_inputs = vec![RemoteInput {
            slot: 0,
            rig: KETUNKOLO,
            rig_name: "ketunkolo".into(),
            channel: 0,
        }];
        o.config.input_channels.push(REMOTE_BASE);
    }
    s.commit(Change::Session(session));
    s.commit(Change::Remote(RemoteState {
        rigs: vec![RemoteRig {
            rig: KETUNKOLO,
            name: "ketunkolo".into(),
            link: RemoteLink::Up,
            clock: PeerClock {
                relation: PeerRelation::NotLocked {
                    reason: NotLockedReason::CrossRate {
                        ppm: 5.6,
                        over: Seconds(40.0),
                    },
                },
                ppm: Some(5.6),
                span: Some(Seconds(40.0)),
                last_change: None,
            },
            bytes_per_s: 390_000.0,
            window: 262_144,
        }],
        inputs: vec![RemoteInputStatus {
            slot: 0,
            audio: RemoteAudio::Streaming,
            anchor: None,
            late_frames: 0,
            late_frames_hour: 0,
            slips: 0,
            last_slip: None,
            arrival_margin: None,
            setup: None,
            cal_status: ac2_proto::model::CalStatus::Uncalibrated,
            calibration: None,
            curve: None,
        }],
        link_offset: Some(Seconds(0.01)),
        transfers: vec![CrossRigTransfer {
            meas: MeasId(1),
            rig: KETUNKOLO,
            side: RemoteSide::Measurement,
            state: CrossTfState::Stopped {
                why: StopWhy::NotLocked,
                held: true,
            },
            relock: None,
            slip: None,
            delay_drift: None,
            late: false,
        }],
        ..RemoteState::default()
    }));
}

/// Clocks not locked: the chip and the banner say it in the top bar, the pane names the
/// remote side; a click on the chip opens the Connection page, which offers the link offset.
#[test]
fn clocks_not_locked_across_rigs() {
    if !have_gpu("clocks_not_locked_across_rigs") {
        return;
    }
    let rig = common::Rig::start();
    not_locked(&rig);
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    step_until(&mut h, "the remote state", |a| {
        a.state
            .daemon()
            .is_some_and(|d| !d.remote.transfers.is_empty())
    });
    h.run_steps(2);
    assert!(
        h.query_by_label("1  Transfer · Main L · meas from ketunkolo")
            .is_some(),
        "the transfer pane's title"
    );
    h.get_by_label_contains(
        "Main L stopped: fake rig and ketunkolo are not on one clock (+5.6 ppm)",
    );
    h.get_by_label("ketunkolo · CLOCKS NOT LOCKED +5.6 ppm")
        .click();
    step_until(&mut h, "the Connection page", |a| {
        matches!(&a.state.overlay, Overlay::Settings(s)
            if s.page == ac2_ui::settings::Page::Connection)
    });
    h.run_steps(2);
    assert!(
        h.query_by_label_contains("Wait for remote inputs (link offset): 10 ms")
            .is_none(),
        "not set in the session: default"
    );
    h.get_by_label_contains("Wait for remote inputs (link offset): default (10 ms)");
}
