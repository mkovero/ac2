use super::*;
use ac2_proto::model::{
    CantTellReason, DelaySlip, PeerClock, Relock, RemoteLink, RemoteRig, StopWhy,
};
use ac2_proto::units::{MeasId, RigId, Seconds};

const KETUNKOLO: RigId = RigId(2);

fn remote(relation: PeerRelation) -> RemoteState {
    RemoteState {
        rigs: vec![RemoteRig {
            rig: KETUNKOLO,
            name: "ketunkolo".into(),
            link: RemoteLink::Up,
            clock: PeerClock {
                relation,
                ppm: Some(5.6),
                span: Some(Seconds(40.0)),
                last_change: None,
            },
            bytes_per_s: 0.0,
            window: 0,
        }],
        link_offset: Some(Seconds(0.01)),
        ..RemoteState::default()
    }
}

fn tf(side: RemoteSide, state: CrossTfState) -> CrossRigTransfer {
    CrossRigTransfer {
        meas: MeasId(3),
        rig: KETUNKOLO,
        side,
        state,
        relock: None,
        slip: None,
        delay_drift: None,
        late: false,
    }
}

const STOPPED_HELD: CrossTfState = CrossTfState::Stopped {
    why: StopWhy::NotLocked,
    held: true,
};

#[test]
fn the_pane_title_names_the_remote_side() {
    let r = remote(PeerRelation::Agree);
    let x = tf(RemoteSide::Measurement, CrossTfState::Running);
    assert_eq!(pane_title("TF 3", &x, &r), "TF 3 · meas from ketunkolo");
    let x = tf(RemoteSide::Reference, CrossTfState::Running);
    assert_eq!(pane_title("TF 3", &x, &r), "TF 3 · ref from ketunkolo");
}

#[test]
fn agreeing_clocks_say_so_on_the_pane_and_nowhere_loudly() {
    let r = remote(PeerRelation::Agree);
    let x = tf(RemoteSide::Measurement, CrossTfState::Running);
    assert_eq!(
        status_lines(&x, &r),
        vec!["clocks agree within 2 ppm · ketunkolo"]
    );
    assert_eq!(banner("TF 3", &x, &r, "pupu"), None);
    assert_eq!(list_cue(&x), None);
    assert_eq!(faint_phase(&x), None);
}

#[test]
fn not_locked_stops_holds_and_says_what_to_do() {
    let r = remote(PeerRelation::NotLocked {
        reason: NotLockedReason::CrossRate {
            ppm: 5.6,
            over: Seconds(40.0),
        },
    });
    let x = tf(RemoteSide::Measurement, STOPPED_HELD);
    assert_eq!(
        status_lines(&x, &r)[0],
        "Clocks not locked: averaging stopped · ketunkolo"
    );
    assert_eq!(faint_phase(&x), Some("clocks not locked"));
    assert_eq!(
        banner("TF 3", &x, &r, "pupu").as_deref(),
        Some(
            "TF 3 stopped: pupu and ketunkolo are not on one clock (+5.6 ppm). Lock both \
         interfaces to one reference."
        )
    );
    assert_eq!(list_cue(&x).as_deref(), Some("clocks"));

    // One rig's interface off its own host: that rig is named, not the pair.
    for (rig, name) in [(KETUNKOLO, "ketunkolo"), (RigId(1), "pupu")] {
        let r = remote(PeerRelation::NotLocked {
            reason: NotLockedReason::OwnClock { rig, ppm: 5.6 },
        });
        assert_eq!(
            banner("TF 3", &x, &r, "pupu"),
            Some(format!(
                "TF 3 stopped: {name}'s interface runs +5.6 ppm against its host. Lock it to \
                 the reference its host is disciplined from."
            ))
        );
    }

    // Nothing held: nothing drawn, so no faint phase either; the words stay.
    let x = tf(
        RemoteSide::Measurement,
        CrossTfState::Stopped {
            why: StopWhy::NotLocked,
            held: false,
        },
    );
    assert_eq!(faint_phase(&x), None);
    assert_eq!(
        stop_verdict(&x),
        Some("Clocks not locked: averaging stopped")
    );
}

#[test]
fn cant_tell_is_refused_with_its_reason() {
    let r = remote(PeerRelation::CantTell {
        reason: CantTellReason::SystemClockUnsynced { rig: KETUNKOLO },
    });
    let x = tf(
        RemoteSide::Reference,
        CrossTfState::Stopped {
            why: StopWhy::CantTell,
            held: true,
        },
    );
    assert_eq!(
        banner("TF 3", &x, &r, "pupu").as_deref(),
        Some(
            "TF 3 stopped: can't tell whether pupu and ketunkolo share a clock: ketunkolo's \
         system clock is not synced."
        )
    );
    assert_eq!(faint_phase(&x), Some("clocks: can't tell"));
    assert_eq!(list_cue(&x).as_deref(), Some("clocks"));
}

#[test]
fn measuring_waits_without_a_banner() {
    let r = remote(PeerRelation::Measuring);
    let x = tf(
        RemoteSide::Measurement,
        CrossTfState::Stopped {
            why: StopWhy::Measuring,
            held: false,
        },
    );
    assert_eq!(
        status_lines(&x, &r)[0],
        "Measuring clocks…: averaging waits · ketunkolo"
    );
    assert_eq!(banner("TF 3", &x, &r, "pupu"), None);
    assert_eq!(list_cue(&x).as_deref(), Some("measuring clocks…"));
}

#[test]
fn a_relock_says_how_long_and_where_the_delay_went() {
    let r = remote(PeerRelation::Agree);
    let mut x = tf(RemoteSide::Measurement, CrossTfState::Running);
    x.relock = Some(Relock {
        after: Seconds(190.0),
        from: StopWhy::NotLocked,
        outcome: RelockOutcome::Tracked {
            delay: Seconds(0.01341),
            was: Seconds(0.01238),
        },
    });
    assert_eq!(
        status_lines(&x, &r),
        vec![
            "clocks agree within 2 ppm · ketunkolo",
            "clocks were not locked for 3 min 10 s · delay tracked again: 13.41 ms (was \
             12.38 ms)"
        ]
    );
    assert_eq!(faint_phase(&x), None);

    // Tracking off: the delay is not touched, the phase faint until a find runs.
    x.relock = Some(Relock {
        after: Seconds(16.0),
        from: StopWhy::CantTell,
        outcome: RelockOutcome::FindAgain,
    });
    assert_eq!(
        relock_text(&x).as_deref(),
        Some("clocks could not be told for 16.0 s · delay may have moved: find it again")
    );
    assert_eq!(faint_phase(&x), Some("delay may have moved: find it again"));
    assert_eq!(list_cue(&x).as_deref(), Some("delay may have moved"));

    // Ambiguous: the operator's pick, the phase faint until answered.
    x.relock = Some(Relock {
        after: Seconds(16.0),
        from: StopWhy::NotLocked,
        outcome: RelockOutcome::Ambiguous,
    });
    assert_eq!(faint_phase(&x), Some("delay ambiguous: pick one"));
}

#[test]
fn slip_drift_and_late_are_cues_not_refusals() {
    let r = remote(PeerRelation::Agree);
    let mut x = tf(RemoteSide::Measurement, CrossTfState::Running);
    x.slip = Some(DelaySlip {
        by: Seconds(-0.0049),
    });
    x.delay_drift = Some(Seconds(0.09e-3));
    x.late = true;
    assert_eq!(
        status_lines(&x, &r),
        vec![
            "clocks agree within 2 ppm · ketunkolo",
            "ketunkolo's audio slipped 4.90 ms · delay moved with it",
            "delay drifting 0.09 ms/min: path or clocks",
            "link offset too small: raise Wait for remote inputs (now 10 ms)",
        ]
    );
    assert_eq!(banner("TF 3", &x, &r, "pupu"), None);
    assert_eq!(list_cue(&x).as_deref(), Some("audio slipped 4.90 ms"));
    x.slip = None;
    assert_eq!(list_cue(&x).as_deref(), Some("delay drifting 0.09 ms/min"));
    x.delay_drift = None;
    assert_eq!(list_cue(&x).as_deref(), Some("link offset too small"));
}

#[test]
fn the_delay_says_its_basis() {
    assert_eq!(
        delay_basis(0.01238, "pupu", "ketunkolo"),
        "12.38 ms · includes the clock offset between pupu and ketunkolo (not measured)"
    );
}
