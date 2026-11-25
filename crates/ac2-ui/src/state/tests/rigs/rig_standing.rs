//! A paired rig's clock tag, rate and pairing words from the clocks and session the rig
//! shown's daemon hears in its always-on levels (`State.remote.standing`), where no stream
//! runs between the two; a stream's verdict stays the tag where one does.

use super::*;
use ac2_proto::model::{
    CantTellReason, NotLockedReason, PeerClock, PeerRelation, RemoteLink, RemoteRig, StandingRig,
    StandingSession,
};
use ac2_proto::units::Seconds;
use ac2_scene::standing::FROM_LEVELS;

/// pupu as the rig shown's daemon hears it in its levels: at 48 kHz when `session`, the
/// relation `relation` at `ppm`, its link `link`.
fn heard(relation: PeerRelation, ppm: Option<f64>, session: bool, link: RemoteLink) -> StandingRig {
    StandingRig {
        rig: PUPU,
        name: "pupu".into(),
        link,
        session: session.then(|| StandingSession {
            sample_rate_hz: 48_000,
            channels: vec![0, 1],
        }),
        clock: PeerClock {
            relation,
            ppm,
            span: ppm.map(|_| Seconds(60.0)),
            last_change: None,
        },
        marks_since: None,
        counting: None,
    }
}

fn up(relation: PeerRelation, ppm: Option<f64>) -> StandingRig {
    heard(relation, ppm, true, RemoteLink::Up)
}

/// The rig shown, paired with pupu, which this app does not hold; its daemon's standing
/// relation with pupu `s`, if any.
fn pupu_not_held(s: Option<StandingRig>) -> T {
    let mut t = T::new();
    t.conn(mirror(paired(with_spl())));
    t.remirror(|st| st.remote.standing = s.into_iter().collect());
    t
}

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

/// With no input from pupu, its rate and tag come from its levels: measuring, then agreeing,
/// then not locked; each tip says the evidence is the levels.
#[test]
fn a_paired_rig_with_no_input_is_tagged_from_its_levels() {
    let mut t = pupu_not_held(Some(up(PeerRelation::Measuring, None)));
    let h = headings(&t);
    assert_eq!(h[0].0, "test rig / 48 kHz TIMING");
    assert_eq!(h[1].0, "pupu / 48 kHz TIMING");
    assert!(
        h[1].1.ends_with(&format!(
            "\nclock TIMING: can't tell: measuring clocks… · {FROM_LEVELS}"
        )),
        "{}",
        h[1].1
    );
    // The widest line: rate and tag, then the pairing.
    let rows = t.st.list_rows();
    let pupu = rows
        .iter()
        .find(|(id, _)| *id == PUPU)
        .expect("pupu's heading");
    assert_eq!(
        pupu.1.details[0],
        "48 kHz TIMING · paired · this app is not connected to it"
    );

    t.remirror(|s| s.remote.standing = vec![up(PeerRelation::Agree, Some(0.12))]);
    let h = headings(&t);
    assert_eq!(h[1].0, "pupu / 48 kHz AGREE");
    assert!(
        h[1].1.ends_with(&format!(
            "\nclock AGREE: clocks agree with test rig: {FROM_LEVELS}, within 2 ppm (pupu runs \
             +0.12 ppm against test rig over 60 s); a lock is not proven below 2 ppm"
        )),
        "{}",
        h[1].1
    );

    let free = PeerRelation::NotLocked {
        reason: NotLockedReason::CrossRate {
            ppm: 3.12,
            over: Seconds(60.0),
        },
    };
    t.remirror(|s| s.remote.standing = vec![up(free, Some(3.12))]);
    let h = headings(&t);
    assert_eq!(h[0].0, "test rig / 48 kHz FREE");
    assert_eq!(h[1].0, "pupu / 48 kHz FREE");
}

/// Its levels say its session is closed: the heading says so in the top bar's words, and no
/// tag; the link between the daemons down: its words, nothing heard.
#[test]
fn a_paired_rigs_session_and_link_read_from_its_levels() {
    let no_session = CantTellReason::NoSession { rig: PUPU };
    let mut t = pupu_not_held(Some(heard(
        PeerRelation::CantTell { reason: no_session },
        None,
        false,
        RemoteLink::Up,
    )));
    let h = headings(&t);
    assert_eq!(h[0].0, "test rig / 48 kHz");
    assert_eq!(h[1].0, "pupu / no session");
    assert!(!h[1].1.contains("clock"), "{}", h[1].1);

    let down = CantTellReason::LinkDown;
    t.remirror(|s| {
        s.remote.standing = vec![heard(
            PeerRelation::CantTell { reason: down },
            None,
            false,
            RemoteLink::Unreachable {
                since: ac2_proto::units::WallNs(1),
            },
        )]
    });
    let h = headings(&t);
    assert_eq!(
        h[1].0,
        "pupu / link from test rig: unreachable · not connected"
    );
    // The sentence is the tooltip's.
    assert!(
        h[1].1.starts_with("pupu: another rig, unreachable; "),
        "{}",
        h[1].1
    );

    t.remirror(|s| {
        s.remote.standing = vec![heard(
            PeerRelation::CantTell { reason: down },
            None,
            false,
            RemoteLink::NoServerKey,
        )]
    });
    let rows = t.st.list_rows();
    let pupu = &rows.iter().find(|(id, _)| *id == PUPU).expect("pupu").1;
    assert_eq!(
        pupu.details,
        [
            "link from test rig: no server key · this app is not connected to it",
            "link from test rig: no server key · not connected"
        ]
    );
    assert!(
        pupu.describe
            .contains("this rig has no server key yet: turn its network on once"),
        "{}",
        pupu.describe
    );
}

/// A daemon that lists no standing relation (an older one, or the pairing not its) leaves
/// the heading as it was.
#[test]
fn no_standing_relation_leaves_the_heading_as_it_was() {
    let t = pupu_not_held(None);
    let h = headings(&t);
    assert_eq!(h[0].0, "test rig / 48 kHz");
    assert_eq!(h[1].0, "pupu / paired · not connected");
}

/// pupu held and read, its system clocks agreeing with the rig shown's: the levels outweigh
/// them, being a verdict between the two rigs; an input from pupu brings the stream's verdict,
/// which then is the tag, the levels' line beside it where they differ.
#[test]
fn levels_outweigh_system_clocks_and_a_stream_outweighs_levels() {
    let (mut t, _) = two_lists();
    let free = PeerRelation::NotLocked {
        reason: NotLockedReason::CrossRate {
            ppm: 3.12,
            over: Seconds(60.0),
        },
    };
    t.remirror(|s| s.remote.standing = vec![up(free, Some(3.12))]);
    let h = headings(&t);
    assert_eq!(h[1].0, "pupu / 48 kHz FREE");
    assert!(h[1].1.contains(FROM_LEVELS), "{}", h[1].1);

    t.remirror(|s| {
        s.remote.rigs = vec![RemoteRig {
            rig: PUPU,
            name: "pupu".into(),
            link: RemoteLink::Up,
            clock: PeerClock {
                relation: PeerRelation::Agree,
                ppm: Some(0.1),
                span: Some(Seconds(60.0)),
                last_change: None,
            },
            bytes_per_s: 0.0,
            window: 0,
        }]
    });
    let h = headings(&t);
    assert_eq!(h[1].0, "pupu / 48 kHz AGREE");
    assert!(
        h[1].1.ends_with(&format!(
            "\nclock AGREE: clocks agree with test rig: measured between the rigs, within 2 \
             ppm (+0.1 ppm over the last 60 s); a lock is not proven below 2 ppm\n\
             CLOCKS NOT LOCKED: pupu runs +3.1 ppm against test rig over 60 s · {FROM_LEVELS}"
        )),
        "{}",
        h[1].1
    );
}

/// The Connection page's *Paired rigs* row says the levels' relation in its hover, nothing
/// on the row.
#[test]
fn the_paired_rigs_row_says_the_clocks_from_levels_in_its_hover() {
    let mut t = pupu_not_held(Some(up(PeerRelation::Agree, Some(0.12))));
    t.st.open_settings(crate::settings::Page::Connection, &mut Vec::new());
    t.remirror(|_| {});
    let Overlay::Settings(s) = &t.st.overlay else {
        panic!("settings open");
    };
    let row = s.connection.pairing.row(PUPU).expect("pupu's row");
    assert!(!row.text.contains("clocks"), "{}", row.text);
    assert!(
        row.tip.contains(&format!(
            "\nclocks agree · pupu runs +0.12 ppm against this rig over 60 s · {FROM_LEVELS}\n"
        )),
        "{}",
        row.tip
    );
}

/// The marks' age reads on the reporting daemon's own clock, and not at all while the app
/// does not know that clock.
#[test]
fn marks_age_on_the_reporting_daemons_clock() {
    let since = super::super::sessions_impl::wall_now().0 - 312_400_000_000;
    let mut s = up(PeerRelation::Agree, Some(0.12));
    s.marks_since = Some(ac2_proto::units::WallNs(since));
    let t = pupu_not_held(Some(s.clone()));
    let h = headings(&t);
    assert!(
        h[1].1
            .contains("(test rig runs −0.12 ppm against pupu over 60 s · marks for 5 min 12 s)")
            || h[1].1.contains(
                "(pupu runs +0.12 ppm against test rig over 60 s · marks for 5 min 12 s)"
            ),
        "{}",
        h[1].1
    );

    // The same state, the link to the daemon not yet holding an offset.
    let mut t = T::new();
    let mut st = paired(with_spl());
    st.remote.standing = vec![s];
    let ConnEvent::Mirror(m) = mirror(st) else {
        unreachable!("a mirror event")
    };
    let mut v = (*m).clone();
    v.clock_offset_ns = None;
    t.conn(ConnEvent::Mirror(std::sync::Arc::new(v)));
    let h = headings(&t);
    assert_eq!(h[1].0, "pupu / 48 kHz AGREE");
    assert!(!h[1].1.contains("marks for"), "{}", h[1].1);
}

/// The rig shown reconnecting: its mirror's standing entry is what was, so nothing of it shows.
#[test]
fn a_lost_rig_shown_shows_nothing_from_its_levels() {
    let mut t = pupu_not_held(Some(up(PeerRelation::Agree, Some(0.12))));
    // Reconnecting keeps the last mirror until the daemon answers again.
    t.conn(ConnEvent::Connecting {
        target: "test".into(),
    });
    assert!(
        t.st.daemon().is_some(),
        "the lost rig keeps its last mirror"
    );
    let h = headings(&t);
    assert_eq!(h[1].0, "pupu / paired · not connected");
    assert!(!h[1].1.contains("clock"), "{}", h[1].1);
}
