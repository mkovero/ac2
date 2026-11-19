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
        lines[0].ends_with("clock: no audio open · system clock synced, OS bound ±1.0 ms"),
        "{lines:?}"
    );
}

/// A sample clock read `ppm` off a system clock held closely enough to compare rigs by.
fn measured(ppm: f64) -> Clocks {
    Clocks {
        system: SystemClock::Synced { maxerror_us: 504 },
        capture: Some(DeviceClock {
            device: DeviceId("system".into()),
            rate: SampleClock::Measured {
                ppm,
                span: Seconds(59.9),
                against: ClockBasis::SystemClock,
            },
        }),
        stamp_map: None,
    }
}

/// The list's rig headings: the first line and the tooltip of each.
/// Each heading as `name / its narrowest second line`, with its tooltip.
fn headings(t: &T) -> Vec<(String, String)> {
    t.st.list_rows()
        .into_iter()
        .filter(|(_, r)| matches!(r.key, ac2_scene::meas_list::TreeKey::RigName(_)))
        .map(|(_, r)| {
            let line = r.details.last().map_or("", String::as_str);
            (format!("{} / {line}", r.name), r.describe)
        })
        .collect()
}

fn pupu_clocks(t: &mut T, clocks: Clocks) {
    let mut s = pupu(t).daemon().cloned().expect("pupu's state");
    s.clocks = clocks;
    to(t, PUPU, mirror(s));
}

/// Two paired rigs with a session each: every heading carries its clock tag after the
/// rate, and its tooltip the words behind the tag. The tags are judged over the rigs
/// listed together, so both change when one rig's clock does.
#[test]
fn the_lists_headings_say_which_rigs_share_a_clock() {
    let (mut t, _) = two_lists();
    t.remirror(|s| s.clocks = measured(6.2));
    pupu_clocks(&mut t, measured(5.4));
    let h = headings(&t);
    assert_eq!(h[0].0, "test rig / 48 kHz AGREE");
    assert_eq!(h[1].0, "pupu / 48 kHz AGREE");
    assert!(
        h[0].1.ends_with(
            "\nclock AGREE: rates agree with pupu within 2 ppm (by each rig's system clock); \
             two free clocks can meet that by chance"
        ),
        "{}",
        h[0].1
    );
    assert!(
        h[1].1
            .contains("\nclock AGREE: rates agree with test rig within 2 ppm"),
        "{}",
        h[1].1
    );

    // Pupu's crystal 40 ppm away: two clocks.
    pupu_clocks(&mut t, measured(46.2));
    let h = headings(&t);
    assert_eq!(h[0].0, "test rig / 48 kHz FREE");
    assert_eq!(h[1].0, "pupu / 48 kHz FREE");
    assert!(h[1].1.contains("\nclock FREE: "), "{}", h[1].1);

    // Pupu's clock not measured yet: neither rig can be placed, and says why.
    pupu_clocks(&mut t, Clocks::default());
    let h = headings(&t);
    assert_eq!(h[0].0, "test rig / 48 kHz UNKNOWN");
    assert_eq!(h[1].0, "pupu / 48 kHz UNKNOWN");
    assert!(
        h[0].1
            .ends_with("\nclock UNKNOWN: can't tell: pupu reports no sample clock"),
        "{}",
        h[0].1
    );
}

/// A paired rig this app does not read gives no evidence: it gets no tag, and with it the
/// only other rig, the rig shown has none to be compared with.
#[test]
fn a_paired_rig_the_app_does_not_read_has_no_clock_tag() {
    let (mut t, _) = two_lists();
    t.remirror(|s| s.clocks = measured(6.2));
    pupu_clocks(&mut t, measured(6.2));
    to(
        &mut t,
        PUPU,
        ConnEvent::Failed {
            target: "pupu".into(),
            error: "gone".into(),
            down: ac2_scene::chrome::RigDown::Unreachable,
            retry_in: std::time::Duration::from_secs(2),
        },
    );
    let h = headings(&t);
    assert_eq!(h[0].0, "test rig / 48 kHz");
    assert_eq!(h[1].0, "pupu / paired · unreachable · retrying");
    assert!(h.iter().all(|(_, tip)| !tip.contains("clock")), "{h:?}");
}
