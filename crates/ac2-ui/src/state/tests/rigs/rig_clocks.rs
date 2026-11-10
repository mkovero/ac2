//! Multi-rig stage 2 (`docs/design/multi-rig.md`): a chip per rig not shown whose stimulus
//! plays, and each rig's clock line on the Connection page.

use super::*;
use ac2_proto::model::{
    ClockBasis, Clocks, DeviceClock, DeviceId, GeneratorSettings, SampleClock, Signal, SystemClock,
};
use ac2_proto::units::{Dbfs, Seconds};

/// pupu's generator as its mirror has it: `owner` holding it, `firing` or armed.
fn pupu_generator(t: &mut T, owner: &str, firing: bool) {
    let mut s = pupu(t).daemon().cloned().expect("pupu's state");
    s.generator.owner = Some(ClientId(owner.into()));
    s.generator.armed = true;
    s.generator.firing = firing;
    s.generator.settings = Some(GeneratorSettings {
        signal: Signal::Pink,
        level: Dbfs(-20.0),
        band: None,
        outputs: vec![0],
    });
    to(t, PUPU, mirror(s));
}

fn chips(t: &T) -> Vec<String> {
    t.st.live_elsewhere()
        .iter()
        .map(ac2_scene::stimulus::elsewhere_chip)
        .collect()
}

/// The stimulus this app holds on the rig in the background reads as a chip with its
/// level, armed or playing; the rig shown has none (its own badge says it), and switching
/// moves the chip to the other rig.
#[test]
fn a_background_rigs_stimulus_is_a_chip() {
    let mut t = two_mirrored();
    assert!(chips(&t).is_empty(), "nothing plays");
    // The mirrors' client id is this app's ("c1").
    pupu_generator(&mut t, "c1", false);
    assert_eq!(chips(&t), vec!["pupu armed −20 dBFS".to_string()]);
    pupu_generator(&mut t, "c1", true);
    assert_eq!(chips(&t), vec!["▶ pupu −20 dBFS".to_string()]);
    // Another client's stimulus there is said too, with who holds it.
    pupu_generator(&mut t, "alice", true);
    assert_eq!(
        chips(&t),
        vec!["▶ pupu −20 dBFS · held by alice".to_string()]
    );
    // Shown, pupu's stimulus is the bar's own: no chip for it.
    pick(&mut t, RigRow::Rig(PUPU));
    assert!(chips(&t).is_empty(), "{:?}", chips(&t));
}

/// One rig's line on its own, more each named; the line reads the daemon's clocks as
/// stated, a minute's measuring in words.
#[test]
fn the_connection_page_says_each_rigs_clock() {
    let mut t = T::new();
    t.remirror(|s| {
        s.clocks = Clocks {
            system: SystemClock::Synced { maxerror_us: 504 },
            capture: Some(DeviceClock {
                device: DeviceId("fake:loop".into()),
                rate: SampleClock::Measuring,
            }),
            stamp_map: None,
        }
    });
    assert_eq!(
        t.st.clock_lines(),
        vec!["clock: measuring… · system clock synced, OS bound ±0.5 ms".to_string()]
    );
    let mut t = two_mirrored();
    let mut s = pupu(&t).daemon().cloned().expect("pupu's state");
    s.clocks = Clocks {
        system: SystemClock::Synced { maxerror_us: 3027 },
        capture: Some(DeviceClock {
            device: DeviceId("system".into()),
            rate: SampleClock::Measured {
                ppm: 6.2,
                span: Seconds(59.9),
                against: ClockBasis::SystemClock,
            },
        }),
        stamp_map: None,
    };
    to(&mut t, PUPU, mirror(s));
    let lines = t.st.clock_lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        lines[1],
        "pupu · clock: +6 ppm against the system clock · system clock synced, OS bound ±3.0 ms"
    );
    assert!(
        lines[0].ends_with("clock: no audio open · system clock not stated by the OS"),
        "{lines:?}"
    );
}
