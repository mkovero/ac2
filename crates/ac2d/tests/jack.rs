//! `session.devices` on JACK: names from the server's ports, or the reason it cannot be
//! used. Runs against whatever server is reachable (`jackd -d dummy` locally) and checks
//! the unavailable reason when none is.
#![cfg(all(feature = "jack", target_os = "linux"))]

mod common;

use ac2_proto::model::{Availability, BackendKind};
use ac2_proto::{Command, ReplyBody};
use ac2d::{BackendChoice, Daemon, DaemonConfig};

use common::*;

#[test]
fn jack_lists_port_names_or_says_why_not() {
    init_log();
    let backends = ac2d::backends(BackendChoice::Jack).expect("backends");
    let mut cfg = DaemonConfig::new(backends[0].clone(), inproc("jack"), -10.0);
    cfg.backends = backends;
    let h = Daemon::start(cfg).expect("daemon");
    let (mut c, _s) = connect(&h, &[]);
    let ReplyBody::Backends(b) = c.ok(Command::SessionDevices) else {
        panic!("backends");
    };
    assert_eq!(b[0].kind, BackendKind::Jack);
    assert!(
        b.iter().all(|x| x.kind != BackendKind::Fake),
        "real audio never offers the simulated rig"
    );
    match &b[0].availability {
        Availability::Available => {
            let d = &b[0].devices[0];
            let input = d.input.as_ref().expect("capture ports");
            let names = input.channel_names.as_ref().expect("port names");
            assert_eq!(names.len(), usize::from(input.max_channels));
            eprintln!("JACK capture ports: {names:?}");
        }
        Availability::Unavailable { reason } => {
            assert_eq!(reason, "JACK server not running");
            assert!(b[0].devices.is_empty());
        }
    }
}
