//! How a link reads the other rig: told its network went off, back, lost without a word,
//! and what it finds on return taking precedence.

use std::time::{Duration, Instant};

use ac2_proto::model::RemoteLink;
use ac2_proto::units::RigId;
use ac2_zmq::MonitorEvent;

use super::{Health, KEY_FAILURES, SILENT_AFTER};

const PAIRED: RigId = RigId(7);

/// A link up and answering at `t0`.
fn up(t0: Instant) -> Health {
    let mut h = Health::new(t0);
    for ctrl in [true, false] {
        h.event(ctrl, MonitorEvent::Connected);
        h.event(ctrl, MonitorEvent::HandshakeSucceeded);
    }
    assert!(h.welcome(PAIRED, PAIRED));
    assert_eq!(h.status(t0), RemoteLink::Up);
    h
}

/// Its connections end as a closing pair ends them.
fn dropped(h: &mut Health) {
    for ctrl in [true, false] {
        h.event(ctrl, MonitorEvent::Disconnected);
    }
}

#[test]
fn told_network_off_it_reads_so_until_welcomed_again() {
    let t0 = Instant::now();
    let mut h = up(t0);
    h.told_network_off();
    dropped(&mut h);
    assert_eq!(h.status(t0), RemoteLink::NetworkOff);
    // Long past the time a silent link reads unreachable.
    let later = t0 + SILENT_AFTER * 4;
    assert_eq!(h.status(later), RemoteLink::NetworkOff);
    for ctrl in [true, false] {
        h.event(ctrl, MonitorEvent::Connected);
        h.event(ctrl, MonitorEvent::HandshakeSucceeded);
    }
    h.heard = later;
    assert!(h.welcome(PAIRED, PAIRED));
    assert_eq!(h.status(later), RemoteLink::Up);
}

#[test]
fn lost_without_a_word_it_reads_unreachable() {
    let t0 = Instant::now();
    let mut h = up(t0);
    dropped(&mut h);
    let later = t0 + SILENT_AFTER + Duration::from_secs(1);
    assert!(matches!(h.status(later), RemoteLink::Unreachable { .. }));
}

#[test]
fn what_it_finds_on_return_comes_before_network_off() {
    let t0 = Instant::now();

    // Another rig answers at the address.
    let mut h = up(t0);
    h.told_network_off();
    dropped(&mut h);
    assert!(!h.welcome(RigId(9), PAIRED));
    assert_eq!(h.status(t0), RemoteLink::OtherRig { found: RigId(9) });

    // It came back without this rig admitted.
    let mut h = up(t0);
    h.told_network_off();
    dropped(&mut h);
    h.event(true, MonitorEvent::Connected);
    h.event(true, MonitorEvent::HandshakeFailedAuth { status: 400 });
    assert_eq!(h.status(t0), RemoteLink::NotAdmitted);

    // It came back with another key.
    let mut h = up(t0);
    h.told_network_off();
    dropped(&mut h);
    for _ in 0..KEY_FAILURES {
        h.event(true, MonitorEvent::Connected);
        h.event(true, MonitorEvent::HandshakeFailedNoDetail { errno: 0 });
    }
    assert_eq!(h.status(t0), RemoteLink::KeyChanged);
}
