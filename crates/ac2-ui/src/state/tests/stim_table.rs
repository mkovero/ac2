//! Reducer tests that the keys the bar offers are what Space and Enter do, in every state
//! of this app's stimulus against every mirror, and that Esc always sends its stop.

use super::*;
use ac2_scene::stim_item::StimKeys;
use ac2_scene::stimulus::Drive;

/// The daemon's generator as the mirror shows it.
#[derive(Clone, Copy, Debug)]
enum Mirror {
    Off,
    MineArmed,
    MineFiring,
    OtherArmed,
    OtherFiring,
}

/// What a key sent to the stimulus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent {
    Nothing,
    Arm,
    Fire,
    Stop,
}

fn sent(r: &[Request]) -> Sent {
    match r {
        [] => Sent::Nothing,
        [Request::StimArm { .. }] => Sent::Arm,
        [Request::StimSet(d)] if d.firing => Sent::Fire,
        [Request::StimStop] => Sent::Stop,
        r => panic!("not a stimulus key's: {r:?}"),
    }
}

fn cell(phase: StimPhase, m: Mirror, unmirrored: bool) -> T {
    let mut t = T::new();
    t.pink_at(-20.0);
    t.remirror(|s| {
        let g = &mut s.generator;
        let (owner, armed, firing) = match m {
            Mirror::Off => (None, false, false),
            Mirror::MineArmed => (Some("c1"), true, false),
            Mirror::MineFiring => (Some("c1"), true, true),
            Mirror::OtherArmed => (Some("c2"), true, false),
            Mirror::OtherFiring => (Some("c2"), true, true),
        };
        g.owner = owner.map(|o| ClientId(o.into()));
        g.armed = armed;
        g.firing = firing;
    });
    t.st.rig.stimulus.phase = phase;
    t.st.rig.stimulus.stopped_unmirrored = unmirrored;
    t
}

/// In every cell the bar offers exactly what Space and Enter then send, and names a next
/// stimulus (and the NO REFERENCE banner a key that starts one) only where a key starts it.
#[test]
fn the_bars_keys_are_what_the_keys_do() {
    let phases = [
        StimPhase::Idle,
        StimPhase::Arming,
        StimPhase::Armed,
        StimPhase::FireRequested,
        StimPhase::Firing,
        StimPhase::Stopping,
    ];
    let mirrors = [
        Mirror::Off,
        Mirror::MineArmed,
        Mirror::MineFiring,
        Mirror::OtherArmed,
        Mirror::OtherFiring,
    ];
    for phase in phases {
        for m in mirrors {
            for unmirrored in [false, true] {
                let at = format!("{phase:?} × {m:?} × unmirrored {unmirrored}");
                let t = cell(phase, m, unmirrored);
                let keys = t.st.stim_keys();
                let want = match keys {
                    StimKeys::None | StimKeys::Run => (Sent::Nothing, Sent::Nothing),
                    StimKeys::Off => (Sent::Arm, Sent::Nothing),
                    StimKeys::Armed => (Sent::Stop, Sent::Fire),
                    StimKeys::Playing => (Sent::Stop, Sent::Stop),
                    StimKeys::ElsewhereArmed | StimKeys::ElsewherePlaying => {
                        (Sent::Stop, Sent::Nothing)
                    }
                };
                let starts = matches!(keys, StimKeys::Off | StimKeys::Armed);
                assert_eq!(t.st.stimulus_next().is_some(), starts, "{at}");
                assert!(
                    starts || matches!(t.st.drive(), Drive::Playing),
                    "{at}: {:?}",
                    t.st.drive()
                );
                let space = sent(&cell(phase, m, unmirrored).key("Space"));
                let enter = sent(&cell(phase, m, unmirrored).key("Enter"));
                assert_eq!((space, enter), want, "{at}: {keys:?}");
            }
        }
    }
    // The cells the review named: arming or stopping offer nothing whatever the mirror;
    // armed here with the mirror still firing offers the armed keys.
    for m in [Mirror::MineArmed, Mirror::MineFiring] {
        for phase in [StimPhase::Arming, StimPhase::Stopping] {
            assert_eq!(cell(phase, m, false).st.stim_keys(), StimKeys::None);
        }
    }
    let t = cell(StimPhase::Armed, Mirror::MineFiring, false);
    assert_eq!(t.st.stim_keys(), StimKeys::Armed);
}

/// A same-name client takes the generator right after this app's stop lands: the first
/// mirror ends the wait, and Space stops what plays.
#[test]
fn the_first_mirror_after_a_stop_ends_the_wait() {
    let mut t = cell(StimPhase::Firing, Mirror::MineFiring, false);
    assert_eq!(sent(&t.key("Space")), Sent::Stop);
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.stim_keys(), StimKeys::None);
    t.remirror(|_| {});
    assert_eq!(t.st.own_stim(), crate::state::OwnStim::Elsewhere);
    assert_eq!(t.st.stim_keys(), StimKeys::ElsewherePlaying);
    assert_eq!(sent(&t.key("Space")), Sent::Stop);
}

/// Connected, Esc sends its stop whatever the app believes plays, before the first mirror
/// too; with nothing live the stop lands quietly.
#[test]
fn esc_always_sends_a_stop() {
    let mut t = T::disconnected();
    assert!(t.key("Esc").is_empty());
    t.conn(ConnEvent::Connected {
        target: "local daemon".into(),
        server: "ac2d test".into(),
        client_id: ClientId("c1".into()),
        rig_id: RIG,
        name: "test rig".into(),
    });
    assert!(t.st.daemon().is_none());
    for key in ["Esc", "Shift+Esc"] {
        assert_eq!(sent(&t.key(key)), Sent::Stop, "{key}");
        assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    }
    let mut t = T::new();
    assert!(!t.st.stimulus_live());
    let n = t.st.toasts.list.len();
    assert_eq!(sent(&t.key("Esc")), Sent::Stop);
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Stopped)).is_empty());
    assert_eq!(t.st.toasts.list.len(), n, "{}", t.last_toast());
}

/// Space then Esc within one round trip: the arm lands behind the stop sent after it, stays
/// stopping (the stop, queued behind the arm, ends the lease it took) and says nothing.
#[test]
fn an_arm_landing_behind_a_stop_stays_stopped() {
    let mut t = T::new();
    t.pink_at(-20.0);
    assert_eq!(sent(&t.key("Space")), Sent::Arm);
    assert_eq!(sent(&t.key("Esc")), Sent::Stop);
    let n = t.st.toasts.list.len();
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Armed)).is_empty());
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Stopping);
    assert_eq!(t.st.toasts.list.len(), n);
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    assert_eq!(t.last_toast(), "stimulus stopped");
}

/// Esc twice, then a Space on the sweep view between the two stops landing: the second
/// stop leaves the new arm alone and says nothing.
#[test]
fn a_second_stop_landing_leaves_the_next_arm_alone() {
    let mut t = T::new();
    t.pink_at(-30.0);
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(sent(&t.key("Esc")), Sent::Stop);
    assert_eq!(sent(&t.key("Esc")), Sent::Stop);
    t.conn(mirror(super::sweep::sweep_state()));
    t.go(PaneKind::Distortion);
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.last_toast(), "stimulus stopped");
    let r = t.key("Space");
    assert!(
        r.iter().any(|x| matches!(x, Request::StimArm { .. })),
        "{r:?}"
    );
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Arming);
    let n = t.st.toasts.list.len();
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Stopped)).is_empty());
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Arming);
    assert!(t.st.rig.sweep.plan.is_some(), "the sweep mode stays");
    assert_eq!(t.st.toasts.list.len(), n);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Armed);
}
