use ac2_proto::model::{DeviceClock, DeviceId, RemoteLink};
use ac2_proto::units::Seconds;

use super::*;

const PUPU: RigId = RigId(1);
const KETUNKOLO: RigId = RigId(2);

fn synced(maxerror_us: u32) -> SystemClock {
    SystemClock::Synced { maxerror_us }
}

fn clocks(rate: SampleClock, system: SystemClock) -> Clocks {
    Clocks {
        system,
        capture: Some(DeviceClock {
            device: DeviceId("system".into()),
            rate,
        }),
        stamp_map: None,
    }
}

fn measured_against(ppm: f64, against: ClockBasis) -> SampleClock {
    SampleClock::Measured {
        ppm,
        span: Seconds(59.9),
        against,
    }
}

/// A sample clock at `ppm` against a system clock synced within `maxerror_us`.
fn at(ppm: f64, maxerror_us: u32) -> Clocks {
    clocks(
        measured_against(ppm, ClockBasis::SystemClock),
        synced(maxerror_us),
    )
}

fn rig<'a>(name: &'a str, rate_hz: u32, clocks: &'a Clocks) -> RigClockFacts<'a> {
    RigClockFacts {
        id: None,
        name,
        rate_hz: Some(rate_hz),
        clocks,
        between: &[],
        levels: None,
    }
}

/// What a daemon says of the clocks between it and rig `id`.
fn remote(id: RigId, relation: PeerRelation, ppm: Option<f64>) -> Vec<RemoteRig> {
    vec![RemoteRig {
        rig: id,
        name: "as the daemon names it".into(),
        link: RemoteLink::Up,
        clock: PeerClock {
            relation,
            ppm,
            span: ppm.map(|_| Seconds(60.0)),
            last_change: None,
        },
        bytes_per_s: 0.0,
        window: 0,
    }]
}

fn tags(d: &ClockDomains) -> Vec<Option<&str>> {
    d.rigs.iter().map(|r| r.tag.as_deref()).collect()
}

fn tip(d: &ClockDomains, i: usize) -> &str {
    d.rigs[i].tip.as_deref().expect("a tooltip")
}

/// pupu and ketunkolo paired, each with `between` empty unless given.
fn pair_with<'a>(
    a: &'a Clocks,
    b: &'a Clocks,
    pupu_sees: &'a [RemoteRig],
) -> [RigClockFacts<'a>; 2] {
    [
        RigClockFacts {
            id: Some(PUPU),
            between: pupu_sees,
            ..rig("pupu", 96_000, a)
        },
        RigClockFacts {
            id: Some(KETUNKOLO),
            ..rig("ketunkolo", 96_000, b)
        },
    ]
}

#[test]
fn the_operators_rigs_two_agree_and_the_meter_has_its_own_rate() {
    let (pupu, ketunkolo, cm4) = (at(0.2, 3), at(-0.1, 3027), at(6.0, 629_500));
    let d = clock_domains(&[
        rig("pupu", 96_000, &pupu),
        rig("ketunkolo", 96_000, &ketunkolo),
        rig("cm4", 48_000, &cm4),
    ]);
    assert_eq!(tags(&d), [Some("AGREE"), Some("AGREE"), Some("FREE")]);
    assert_eq!(
        tip(&d, 0),
        "rates agree with ketunkolo within 2 ppm by system clocks"
    );
    assert_eq!(
        tip(&d, 1),
        "rates agree with pupu within 2 ppm by system clocks"
    );
    assert_eq!(tip(&d, 2), "48 kHz here; pupu and ketunkolo at 96 kHz");
    assert_eq!(
        d.rigs[0].domain,
        Domain::Group {
            index: 0,
            basis: Basis::SystemClocks
        }
    );
    assert_eq!(d.rigs[2].domain, Domain::Own);
    assert_eq!(d.same_domain(0, 1), SameDomain::Yes);
    assert_eq!(
        d.same_domain(0, 2),
        SameDomain::No {
            reason: "pupu runs at 96 kHz, cm4 at 48 kHz".into()
        }
    );
}

#[test]
fn a_loose_os_bound_at_the_same_rate_reads_cant_tell() {
    // The meter's +6 ppm is against a clock that may have gone 21 minutes unsteered: it
    // neither joins the group nor counts as a difference.
    let (pupu, ketunkolo, cm4) = (at(0.2, 3), at(-0.1, 3027), at(6.0, 629_500));
    let d = clock_domains(&[
        rig("pupu", 96_000, &pupu),
        rig("ketunkolo", 96_000, &ketunkolo),
        rig("cm4", 96_000, &cm4),
    ]);
    assert_eq!(tags(&d), [Some("AGREE"), Some("AGREE"), Some("UNKNOWN")]);
    let why = "cm4's system clock is synced, OS bound ±629.5 ms: too loose to compare rates \
               between rigs";
    assert_eq!(tip(&d, 2), format!("can't tell: {why}"));
    assert_eq!(d.rigs[2].domain, Domain::CantTell);
    assert_eq!(
        d.same_domain(2, 0),
        SameDomain::CantTell { reason: why.into() }
    );
}

#[test]
fn the_bound_must_be_under_what_one_window_adds() {
    // 59.9 s at 500 µs/s: 29.95 ms.
    let good = at(0.0, 3);
    for (us, tag) in [(29_900, "AGREE"), (30_000, "UNKNOWN")] {
        let edge = at(0.0, us);
        let d = clock_domains(&[rig("a", 48_000, &good), rig("b", 48_000, &edge)]);
        assert_eq!(tags(&d), [Some(tag), Some(tag)], "{us} µs");
    }
}

#[test]
fn two_rates_are_never_one_clock_whatever_the_daemon_says() {
    let (a, b) = (at(0.0, 3), at(0.0, 3));
    let sees = remote(KETUNKOLO, PeerRelation::Agree, Some(0.0));
    let mut rigs = pair_with(&a, &b, &sees);
    rigs[1].rate_hz = Some(48_000);
    let d = clock_domains(&rigs);
    assert_eq!(tags(&d), [Some("FREE"), Some("FREE")]);
    assert_eq!(tip(&d, 0), "96 kHz here; ketunkolo at 48 kHz");
    assert_eq!(tip(&d, 1), "48 kHz here; pupu at 96 kHz");
}

#[test]
fn a_daemons_agreement_reads_agree_with_its_figure() {
    // The verdict stands on its own: neither OS states its system clock here.
    let a = clocks(
        measured_against(0.0, ClockBasis::HostCrystal),
        SystemClock::NotStated,
    );
    let sees = remote(KETUNKOLO, PeerRelation::Agree, Some(-0.3));
    let d = clock_domains(&pair_with(&a, &a, &sees));
    assert_eq!(tags(&d), [Some("AGREE"), Some("AGREE")]);
    assert_eq!(
        tip(&d, 0),
        "clocks agree with ketunkolo within 2 ppm (+0.3 ppm over 60 s)"
    );
    assert_eq!(
        tip(&d, 1),
        "clocks agree with pupu within 2 ppm (−0.3 ppm over 60 s)"
    );
    assert_eq!(
        d.rigs[1].domain,
        Domain::Group {
            index: 0,
            basis: Basis::BetweenRigs
        }
    );
    assert_eq!(d.same_domain(1, 0), SameDomain::Yes);
}

#[test]
fn a_daemons_difference_outweighs_figures_that_agree() {
    let a = at(0.0, 3);
    let sees = remote(
        KETUNKOLO,
        PeerRelation::NotLocked {
            reason: NotLockedReason::CrossRate {
                ppm: 5.6,
                over: Seconds(40.0),
            },
        },
        Some(5.6),
    );
    let d = clock_domains(&pair_with(&a, &a, &sees));
    assert_eq!(tags(&d), [Some("FREE"), Some("FREE")]);
    assert_eq!(tip(&d, 0), "pupu runs −5.6 ppm against ketunkolo over 40 s");
    assert_eq!(tip(&d, 1), "ketunkolo runs +5.6 ppm against pupu over 40 s");
    assert_eq!(
        d.same_domain(1, 0),
        SameDomain::No {
            reason: tip(&d, 1).to_owned()
        }
    );
}

#[test]
fn an_interface_off_its_own_system_clock_is_named() {
    let a = at(0.0, 3);
    for (rig, who) in [(KETUNKOLO, "ketunkolo"), (PUPU, "pupu")] {
        let sees = remote(
            KETUNKOLO,
            PeerRelation::NotLocked {
                reason: NotLockedReason::OwnClock { rig, ppm: 5.6 },
            },
            None,
        );
        let d = clock_domains(&pair_with(&a, &a, &sees));
        assert_eq!(tags(&d), [Some("FREE"), Some("FREE")]);
        let text = format!("{who}'s interface runs +5.6 ppm off its system clock");
        assert_eq!(tip(&d, 0), text);
        assert_eq!(tip(&d, 1), text);
    }
}

#[test]
fn a_daemon_that_cannot_tell_is_not_overruled_and_one_still_measuring_is_no_verdict() {
    let a = at(0.0, 3);
    let sees = remote(
        KETUNKOLO,
        PeerRelation::CantTell {
            reason: CantTellReason::ClocksDisagree { by: Seconds(-0.25) },
        },
        None,
    );
    let d = clock_domains(&pair_with(&a, &a, &sees));
    assert_eq!(tags(&d), [Some("UNKNOWN"), Some("UNKNOWN")]);
    assert_eq!(
        tip(&d, 1),
        "can't tell: the system clocks of pupu and ketunkolo disagree by 250.0 ms"
    );

    let sees = remote(KETUNKOLO, PeerRelation::Measuring, None);
    let d = clock_domains(&pair_with(&a, &a, &sees));
    assert_eq!(tags(&d), [Some("AGREE"), Some("AGREE")]);
}

/// A stream still measuring where the system clocks cannot place the two: its verdict is
/// on its way, so the rigs read TIMING, the tip still saying what the system clocks lack.
#[test]
fn a_stream_still_measuring_reads_timing_where_the_system_clocks_cannot_tell() {
    let loose = at(0.0, 30_000);
    let sees = remote(KETUNKOLO, PeerRelation::Measuring, None);
    let d = clock_domains(&pair_with(&loose, &loose, &sees));
    assert_eq!(tags(&d), [Some("TIMING"), Some("TIMING")]);
    assert!(tip(&d, 1).starts_with("can't tell: "), "{}", tip(&d, 1));

    let d = clock_domains(&pair_with(&loose, &loose, &[]));
    assert_eq!(tags(&d), [Some("UNKNOWN"), Some("UNKNOWN")]);
}

#[test]
fn figures_that_differ_by_each_system_clock_are_own_clocks() {
    let (a, b) = (at(0.2, 3), at(6.2, 3027));
    let d = clock_domains(&[rig("pupu", 48_000, &a), rig("usb mic", 48_000, &b)]);
    assert_eq!(tags(&d), [Some("FREE"), Some("FREE")]);
    assert_eq!(
        tip(&d, 1),
        "usb mic runs +6.0 ppm against pupu by system clocks"
    );
}

#[test]
fn a_figure_with_no_common_reference_is_no_evidence() {
    let good = at(0.0, 3);
    let cases = [
        (
            clocks(
                measured_against(0.0, ClockBasis::SystemClock),
                SystemClock::Unsynced { maxerror_us: 3 },
            ),
            "b's system clock is not synced",
        ),
        (
            clocks(
                measured_against(0.0, ClockBasis::SystemClock),
                SystemClock::NotStated,
            ),
            "b's system clock does not say",
        ),
        (
            clocks(measured_against(0.0, ClockBasis::HostCrystal), synced(3)),
            "b's interface is measured against its own crystal",
        ),
        (
            clocks(SampleClock::Measuring, synced(3)),
            "b's sample clock is still being measured (less than a minute of audio)",
        ),
        (
            clocks(SampleClock::Synthetic, synced(3)),
            "b plays a recording: no device clock",
        ),
    ];
    for (bad, why) in &cases {
        let d = clock_domains(&[rig("a", 48_000, &good), rig("b", 48_000, bad)]);
        assert_eq!(tags(&d), [Some("UNKNOWN"), Some("UNKNOWN")], "{why}");
        assert_eq!(tip(&d, 0), format!("can't tell: {why}"));
        assert_eq!(tip(&d, 1), format!("can't tell: {why}"));
    }
    // Both rigs short of a fact: each is said, once.
    let d = clock_domains(&[rig("a", 48_000, &cases[0].0), rig("b", 48_000, &cases[1].0)]);
    assert_eq!(
        tip(&d, 0),
        "can't tell: a's system clock is not synced · b's system clock does not say"
    );
}

#[test]
fn a_chain_of_agreements_holding_a_difference_is_no_group() {
    let (a, b, c) = (at(0.0, 3), at(1.5, 3), at(3.0, 3));
    let d = clock_domains(&[
        rig("a", 48_000, &a),
        rig("b", 48_000, &b),
        rig("c", 48_000, &c),
    ]);
    assert_eq!(tags(&d), [Some("UNKNOWN"); 3]);
    let why = "the rates agree in a chain within 2 ppm, but a runs −3.0 ppm against c by \
               system clocks";
    for i in 0..3 {
        assert_eq!(tip(&d, i), format!("can't tell: {why}"));
    }
    assert_eq!(
        d.same_domain(0, 1),
        SameDomain::CantTell { reason: why.into() }
    );
    assert!(matches!(d.same_domain(0, 2), SameDomain::No { .. }));
}

#[test]
fn a_broken_chain_keeps_the_group_the_daemon_measured() {
    let (a, b, c) = (at(0.0, 3), at(1.5, 3), at(3.0, 3));
    let sees = remote(KETUNKOLO, PeerRelation::Agree, Some(1.4));
    let [pupu, ketunkolo] = pair_with(&a, &b, &sees);
    let d = clock_domains(&[pupu, ketunkolo, rig("c", 96_000, &c)]);
    assert_eq!(tags(&d), [Some("AGREE"), Some("AGREE"), Some("UNKNOWN")]);
    assert_eq!(
        tip(&d, 2),
        "can't tell: the rates agree in a chain within 2 ppm, but pupu runs −3.0 ppm against c by \
         system clocks"
    );
    assert!(matches!(d.same_domain(1, 2), SameDomain::CantTell { .. }));
}

#[test]
fn a_rig_joined_only_by_its_system_clock_weakens_the_groups_tag() {
    let (a, b, c) = (at(0.0, 3), at(0.5, 3), at(1.0, 3));
    let sees = remote(KETUNKOLO, PeerRelation::Agree, Some(0.5));
    let [pupu, ketunkolo] = pair_with(&a, &b, &sees);
    let d = clock_domains(&[pupu, ketunkolo, rig("c", 96_000, &c)]);
    assert_eq!(tags(&d), [Some("AGREE"); 3]);
    assert_eq!(
        tip(&d, 0),
        "rates agree with ketunkolo and c within 2 ppm by system clocks"
    );
}

#[test]
fn two_groups_are_lettered_in_list_order() {
    let (zero, ten) = (at(0.0, 3), at(10.0, 3));
    let d = clock_domains(&[
        rig("a", 48_000, &ten),
        rig("b", 48_000, &zero),
        rig("c", 48_000, &ten),
        rig("d", 48_000, &zero),
    ]);
    assert_eq!(
        tags(&d),
        [
            Some("AGREE A"),
            Some("AGREE B"),
            Some("AGREE A"),
            Some("AGREE B")
        ]
    );
    assert!(tip(&d, 3).starts_with("rates agree with b within 2 ppm"));
    assert!(matches!(d.same_domain(0, 1), SameDomain::No { .. }));

    // One pair measured by its daemon, the other by system clocks: one word, the basis kept
    // apart in the data and the tooltip.
    let sees = remote(KETUNKOLO, PeerRelation::Agree, None);
    let [pupu, ketunkolo] = pair_with(&zero, &zero, &sees);
    let d = clock_domains(&[
        rig("a", 48_000, &zero),
        pupu,
        rig("b", 48_000, &zero),
        ketunkolo,
    ]);
    assert_eq!(
        tags(&d),
        [
            Some("AGREE A"),
            Some("AGREE B"),
            Some("AGREE A"),
            Some("AGREE B")
        ]
    );
    assert_eq!(tip(&d, 1), "clocks agree with ketunkolo within 2 ppm");
    let basis = |i: usize| match d.rigs[i].domain {
        Domain::Group { basis, .. } => Some(basis),
        _ => None,
    };
    assert_eq!(basis(0), Some(Basis::SystemClocks));
    assert_eq!(basis(1), Some(Basis::BetweenRigs));
    assert!(
        d.rigs
            .iter()
            .all(|r| r.tag.as_deref().is_none_or(|t| !t.contains("LOCKED"))),
        "no evidence here shows a lock"
    );
}

#[test]
fn nothing_to_compare_is_no_tag() {
    let c = at(0.0, 3);
    let none = Clocks::default();
    let idle = RigClockFacts {
        rate_hz: None,
        ..rig("idle", 0, &none)
    };

    assert!(clock_domains(&[]).rigs.is_empty());
    let d = clock_domains(&[rig("pupu", 96_000, &c)]);
    assert_eq!((tags(&d), d.rigs[0].tip.as_deref()), (vec![None], None));
    assert_eq!(d.rigs[0].domain, Domain::NotCompared);

    // A rig with no stream leaves the other alone in the list.
    let d = clock_domains(&[rig("pupu", 96_000, &c), idle]);
    assert_eq!(tags(&d), [None, None]);
    assert_eq!(
        d.rigs[1],
        DomainTag {
            tag: None,
            tip: None,
            domain: Domain::NoStream
        }
    );
    assert_eq!(
        d.same_domain(0, 1),
        SameDomain::CantTell {
            reason: "idle has no audio stream".into()
        }
    );
    assert_eq!(d.same_domain(1, 1), SameDomain::Yes);

    // Between two that have one, it takes no part.
    let d = clock_domains(&[rig("pupu", 96_000, &c), idle, rig("ketunkolo", 96_000, &c)]);
    assert_eq!(tags(&d), [Some("AGREE"), None, Some("AGREE")]);
    assert!(tip(&d, 0).starts_with("rates agree with ketunkolo within"));
}
