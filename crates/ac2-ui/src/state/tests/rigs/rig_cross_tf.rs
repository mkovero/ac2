//! Multi-rig stage 3c (`docs/design/remote-inputs.md` §5): a transfer function with one
//! channel on another rig says its clock relation wherever it appears — the top bar's chip
//! and banner, the pane's title, caption, banner strip and faint phase, its cue in the
//! measurement list — and the Connection page sets the link offset as `ac2 session
//! link-offset` does.

use super::rig_remote::with_remote;
use super::*;
use ac2_proto::model::{
    CrossRigTransfer, CrossTfState, NotLockedReason, PeerRelation, Relock, RelockOutcome,
    RemoteAudio, RemoteLink, RemoteSide, StopWhy,
};
use ac2_proto::units::Seconds;
use ac2_scene::banner::{BannerKind, banners};

fn key(t: &mut T, s: &str) -> Vec<(RigId, Request)> {
    let c = Chord::parse(s).expect(s);
    t.st.update(Msg::Key(c), &t.keys)
}

/// This rig's session with pupu's input 2 (see `with_remote`), the clocks between the rigs
/// `relation`, and Main L a transfer function measuring on pupu's input, `state`.
fn cross_tf(t: &mut T, relation: PeerRelation, state: CrossTfState) {
    with_remote(t, RemoteLink::Up, RemoteAudio::Streaming);
    t.remirror(|s| {
        // The pairing the inputs are taken through, as the daemon lists it.
        s.remote.peers = vec![ac2_proto::model::RigPeer {
            rig: PUPU,
            name: "pupu".into(),
            ctrl: "tcp://192.168.9.27:7700".into(),
            data: "tcp://192.168.9.27:7701".into(),
            key: "Yne@$w-vo<fVvi]a<NY6T1ed:M$fCG*[IaLV{hID".into(),
        }];
        s.remote.rigs[0].clock.relation = relation;
        s.remote.transfers = vec![CrossRigTransfer {
            meas: MeasId(1),
            rig: PUPU,
            side: RemoteSide::Measurement,
            state,
            relock: None,
            slip: None,
            delay_drift: None,
            late: false,
        }];
    });
}

fn edit_transfer(t: &mut T, f: impl FnOnce(&mut CrossRigTransfer)) {
    t.remirror(|s| f(&mut s.remote.transfers[0]));
}

const NOT_LOCKED: PeerRelation = PeerRelation::NotLocked {
    reason: NotLockedReason::CrossRate {
        ppm: 5.6,
        over: Seconds(40.0),
    },
};

const HELD: CrossTfState = CrossTfState::Stopped {
    why: StopWhy::NotLocked,
    held: true,
};

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: std::time::Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    }
}

const SIZE: ac2_scene::primitives::Viewport = ac2_scene::primitives::Viewport::new(900.0, 500.0);

/// The texts of the banner strip as transfer pane `p` draws it.
fn drawn_banners(st: &AppState, p: PaneId) -> Vec<String> {
    crate::scenes::transfer(st, p, &ac2_scene::theme::Theme::dark(), SIZE, now())
        .banners
        .into_iter()
        .map(|b| b.text)
        .collect()
}

/// The banner strip of the pane showing Main L.
fn pane_banners(t: &T) -> Vec<(BannerKind, String)> {
    let m = t.st.meas(MeasId(1));
    let s = crate::scenes::status(&t.st, &[], m, now());
    banners(&s)
        .into_iter()
        .filter(|b| {
            matches!(
                b.kind,
                BannerKind::BetweenRigs | BannerKind::BetweenRigsNote
            )
        })
        .map(|b| (b.kind, b.text))
        .collect()
}

/// What pupu's heading in the measurement list says of its pairing.
fn pupu_pairing(t: &T) -> String {
    let rows = t.st.list_rows();
    let head = rows
        .iter()
        .map(|(_, r)| r)
        .find(|r| r.depth == 0 && r.name == "pupu")
        .unwrap_or_else(|| panic!("no heading for pupu in {rows:?}"));
    head.details[0].clone()
}

/// Agreeing clocks: said once on the rig's heading, by its clock tag with the words behind
/// it in the tooltip (the pairing line gives no second verdict); the pane names the remote
/// side and says the relation in its title, no banner and no cue.
#[test]
fn agreeing_clocks_are_said_quietly() {
    let mut t = two_mirrored();
    cross_tf(&mut t, PeerRelation::Agree, CrossTfState::Running);
    let rows = t.st.list_rows();
    let head = rows
        .iter()
        .map(|(_, r)| r)
        .find(|r| r.depth == 0 && r.name == "pupu")
        .expect("pupu's heading");
    assert_eq!(
        head.details,
        ["48 kHz AGREE · paired · streaming", "48 kHz AGREE"]
    );
    assert!(
        head.describe
            .contains("\nclock AGREE: clocks agree with test rig: measured between the rigs"),
        "{}",
        head.describe
    );
    assert!(t.st.cross_rig_banners().is_empty());
    let p = t.pane(PaneKind::Transfer);
    assert!(
        t.st.pane_heading(p).ends_with("Main L · meas from pupu"),
        "{}",
        t.st.pane_heading(p)
    );
    // First, and kept as the title narrows.
    let caption = t.st.pane_caption_variants(p);
    assert_eq!(
        caption.first().map(String::as_str),
        Some("clocks agree within 2 ppm · pupu · smoothing off")
    );
    assert!(
        caption
            .iter()
            .any(|c| c == "clocks agree within 2 ppm · pupu"),
        "{caption:?}"
    );
    assert!(pane_banners(&t).is_empty());
    let row = t.st.meas_rows().into_iter().find(|r| r.id == MeasId(1));
    assert!(
        !row.expect("Main L's row").text.contains("clocks"),
        "no cue while it averages"
    );
}

/// Not locked: the rig's heading says it first and in capitals, the top-bar banner says why and what to do, the
/// pane's verdict is a banner (it stays with the plot chrome hidden), the list row says
/// *clocks* with the pane closed.
#[test]
fn clocks_not_locked_stop_it_and_say_so_everywhere() {
    let mut t = two_mirrored();
    cross_tf(&mut t, NOT_LOCKED, HELD);
    assert_eq!(
        pupu_pairing(&t),
        "48 kHz FREE · CLOCKS NOT LOCKED +5.6 ppm · paired · streaming"
    );
    let ours = t.st.rig_name(t.st.active());
    assert_eq!(
        t.st.cross_rig_banners(),
        vec![format!(
            "Main L stopped: {ours} and pupu are not on one clock (+5.6 ppm). Lock both \
             interfaces to one reference."
        )]
    );
    assert_eq!(
        pane_banners(&t),
        vec![(
            BannerKind::BetweenRigs,
            "Clocks not locked: averaging stopped".to_string()
        )]
    );
    // The verdict is said in the strip, not repeated in the title.
    let p = t.pane(PaneKind::Transfer);
    assert!(
        !t.st
            .pane_caption(p)
            .unwrap_or_default()
            .contains("clocks agree"),
        "{:?}",
        t.st.pane_caption(p)
    );
    let row = t.st.meas_rows().into_iter().find(|r| r.id == MeasId(1));
    assert!(
        row.expect("Main L's row").text.contains("running · clocks"),
        "the short cue"
    );
    // T hides the plot chrome, down to the traces alone; banners are not chrome.
    t.st.layout.set_focus(p);
    for _ in 0..2 {
        t.st.update(Msg::Command(CommandId::PlotChrome), &t.keys);
    }
    assert_eq!(t.st.view_for(p).chrome, ac2_scene::view::PlotChrome::Bare);
    let drawn = drawn_banners(&t.st, p);
    assert!(
        drawn
            .iter()
            .any(|b| b == "Clocks not locked: averaging stopped"),
        "{drawn:?}"
    );
}

/// A pane of a rig in the background is drawn with that rig swapped in; its cross-rig
/// transfer function is that rig's, not the one of the rig shown, whose measurement ids
/// are another rig's.
#[test]
fn a_cross_rig_transfer_in_another_rigs_pane_is_that_rigs() {
    let mut t = two_mirrored();
    cross_tf(&mut t, NOT_LOCKED, HELD);
    let tab = t.st.active();
    pick(&mut t, RigRow::Rig(PUPU));
    // The first rig's transfer function dragged onto a pane of pupu's screen.
    let p = t.st.layout.focus;
    let key = ac2_scene::meas_list::TreeKey::Meas(MeasId(1));
    t.drag_drop(DragItem::Row { rig: tab, key }, p, None);
    assert_eq!(
        t.st.layout.view(p).expect("pane").rig,
        PaneRig::Rig(tab),
        "the pane is the first rig's"
    );
    // Pupu, shown, measures nothing across rigs: what the pane shows is the tab rig's.
    let drawn =
        t.st.visit(tab, |st| drawn_banners(st, p))
            .expect("the pane's rig");
    assert!(
        drawn
            .iter()
            .any(|b| b == "Clocks not locked: averaging stopped"),
        "{drawn:?}"
    );
    let caption = t.st.visit(tab, |st| st.pane_caption(p)).flatten();
    assert!(
        !caption.unwrap_or_default().contains("clocks agree"),
        "a stopped transfer function's pane never says the clocks agree"
    );
}

/// After a relock with tracking off, the pane says what the passage did and that the delay
/// may have moved, as a warning; drift and late are cues; a late link warns on the chip.
#[test]
fn a_relock_and_the_cues_are_said_on_the_pane() {
    let mut t = two_mirrored();
    cross_tf(&mut t, PeerRelation::Agree, CrossTfState::Running);
    edit_transfer(&mut t, |x| {
        x.relock = Some(Relock {
            after: Seconds(190.0),
            from: StopWhy::NotLocked,
            outcome: RelockOutcome::FindAgain,
        });
        x.delay_drift = Some(Seconds(0.3e-3));
        x.late = true;
    });
    assert_eq!(
        pane_banners(&t),
        vec![
            (
                BannerKind::BetweenRigs,
                "clocks were not locked for 3 min 10 s · delay may have moved: X to find it \
                 again"
                    .to_string()
            ),
            (
                BannerKind::BetweenRigs,
                "link offset too small: raise Wait for remote inputs (now 10 ms)".to_string()
            ),
            (
                BannerKind::BetweenRigsNote,
                "delay drifting 0.30 ms/min: path or clocks".to_string()
            ),
        ]
    );
    assert_eq!(
        pupu_pairing(&t),
        "48 kHz AGREE · LINK OFFSET TOO SMALL · paired · streaming"
    );
    let row = t.st.meas_rows().into_iter().find(|r| r.id == MeasId(1));
    assert!(
        row.expect("Main L's row")
            .text
            .contains("running · delay may have moved"),
    );
}

/// An ambiguous relock asks for the operator's pick through the decision-1c candidates: the
/// pane's banner names the key the key table binds to the delay finder, that key in the
/// pane finds, an ambiguous answer opens the list for the cross-rig measurement as for any.
#[test]
fn an_ambiguous_relock_opens_the_pick() {
    use ac2_proto::model::DelayPick;
    let mut t = two_mirrored();
    cross_tf(&mut t, PeerRelation::Agree, CrossTfState::Running);
    edit_transfer(&mut t, |x| {
        x.relock = Some(Relock {
            after: Seconds(16.0),
            from: StopWhy::NotLocked,
            outcome: RelockOutcome::Ambiguous,
        });
    });
    let k = t
        .keys
        .label(CommandId::InsertDelay, crate::keys::Scope::Transfer)
        .expect("the finder has a key in a transfer pane");
    assert_eq!(k, "X");
    assert_eq!(
        pane_banners(&t),
        vec![(
            BannerKind::BetweenRigs,
            format!("clocks were not locked for 16.0 s · delay ambiguous: {k} to pick one")
        )]
    );
    let p = t.pane(PaneKind::Transfer);
    t.st.layout.set_focus(p);
    let r = key(&mut t, &k);
    assert!(
        r.iter().any(|(_, q)| matches!(
            q,
            Request::FindDelay {
                meas: MeasId(1),
                pick: DelayPick::FirstArrival,
                ..
            }
        )),
        "{r:?}"
    );
    found(&mut t, DelayPick::FirstArrival, ambiguous());
    assert!(
        matches!(t.st.overlay, Overlay::DelayPick(_)),
        "{:?}",
        t.st.overlay
    );
}

/// The chip opens the Connection page; its link offset line types a time and reopens the
/// session with it, everything else as it was; a bad time is refused in words.
#[test]
fn the_connection_page_sets_the_link_offset() {
    let mut t = two_mirrored();
    cross_tf(&mut t, PeerRelation::Agree, CrossTfState::Running);
    t.st.update(Msg::Command(CommandId::BetweenRigs), &t.keys);
    let Overlay::Settings(s) = &t.st.overlay else {
        panic!("settings open: {:?}", t.st.overlay);
    };
    assert_eq!(s.page, crate::settings::Page::Connection);
    let lines = s.connection.lines();
    let i = lines
        .iter()
        .position(|l| *l == crate::settings::ConnLine::LinkOffset)
        .expect("the link offset line");
    t.st.update(Msg::Settings(SettingsMsg::Connection(i)), &t.keys);
    t.st.update(Msg::Text("900ms".into()), &t.keys);
    let r = key(&mut t, "Enter");
    assert!(r.is_empty(), "{r:?}");
    let Overlay::Settings(s) = &t.st.overlay else {
        panic!("settings open");
    };
    assert!(
        s.connection
            .error
            .as_deref()
            .is_some_and(|e| e.contains("at most 500 ms")),
        "{:?}",
        s.connection.error
    );
    for _ in 0.."900ms".len() {
        key(&mut t, "Backspace");
    }
    t.st.update(Msg::Text("30ms".into()), &t.keys);
    let r = key(&mut t, "Enter");
    let before = t.st.open_session().expect("open").config.clone();
    let (config, what) = r
        .iter()
        .find_map(|(_, q)| match q {
            Request::RemoteSession {
                pair: None,
                config,
                what,
            } => Some((config.clone(), what.clone())),
            _ => None,
        })
        .expect("reopens the session");
    assert_eq!(
        what,
        "link offset 30 ms: the remote inputs anchor again, their averages restart"
    );
    assert_eq!(config.link_offset, Some(Seconds(0.03)));
    assert_eq!(
        ac2_proto::model::SessionConfig {
            link_offset: None,
            ..config
        },
        before
    );
}
