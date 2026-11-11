//! Multi-rig stage 3a (`docs/design/remote-inputs.md` §2.3, §5): the app pairs two rigs it
//! holds and adds one's input to the other's session; the input's row, its name in the
//! pickers and the Connection page say what its rig and link do, in words.

use super::*;
use ac2_proto::model::{
    Anchor, AuthorizedClient, CalStatus, CurveChoice, InputKind, InputSetup, PeerClock,
    PeerRelation, REMOTE_BASE, RemoteAudio, RemoteInput, RemoteInputStatus, RemoteLink, RemoteRig,
    RemoteState, ServerInfo, ServerMode,
};
use ac2_proto::units::{Seconds, WallNs};
use ac2_scene::remote_inputs as words;

const KEY_A: &str = "rq:rM>}U?@Lns47E1%kR.o@n%FcmmsL/@{H8]yf7";
const KEY_B: &str = "Yne@$w-vo<fVvi]a<NY6T1ed:M$fCG*[IaLV{hID";

fn network(key: &str, authorized: Vec<AuthorizedClient>) -> ServerInfo {
    ServerInfo {
        mode: ServerMode::Network {
            ctrl: "tcp://0.0.0.0:7700".into(),
            data: "tcp://0.0.0.0:7701".into(),
            server_key: key.into(),
            fingerprint: "ab12".into(),
            advertised_as: None,
            authorized,
            refused: Vec::new(),
        },
        recording_dir: None,
    }
}

fn server(info: ServerInfo) -> ConnEvent {
    ConnEvent::Server {
        what: None,
        result: Ok(info),
    }
}

fn cmd(c: CommandId, t: &mut T) -> Vec<(RigId, Request)> {
    t.st.update(Msg::Command(c), &t.keys)
}

fn key(t: &mut T, s: &str) -> Vec<(RigId, Request)> {
    let c = Chord::parse(s).expect(s);
    t.st.update(Msg::Key(c), &t.keys)
}

/// Both rigs mirrored, both answered `server.info` in network mode; pupu's input 2 is its
/// mic `MM1 34804`, and the app reaches pupu at 192.168.9.27.
fn paired_ready() -> T {
    let mut t = two_mirrored();
    let mut s = pupu(&t).daemon().cloned().expect("pupu's state");
    s.inputs = vec![InputSetup {
        channel: 1,
        kind: InputKind::Mic {
            mic: Some("MM1 34804".into()),
            curve: CurveChoice::NotChosen,
        },
        pair: None,
    }];
    to(&mut t, PUPU, mirror(s));
    if let Some(b) = t.st.others.background.iter_mut().find(|r| r.id == PUPU) {
        b.daemon_addr = Some("192.168.9.27".into());
    }
    t.conn(server(network(KEY_A, Vec::new())));
    to(&mut t, PUPU, server(network(KEY_B, Vec::new())));
    t
}

fn texts(t: &T) -> Vec<String> {
    t.st.remote_pick_lines()
        .into_iter()
        .map(|l| match l {
            crate::remote_inputs::PickLine::Text { text, .. }
            | crate::remote_inputs::PickLine::Input { text, .. } => text,
        })
        .collect()
}

/// The palette's *Add an input from another rig…* opens the dialog on the Audio I/O page
/// and asks both rigs for their keys; the other rig's captured inputs are listed with its
/// row text; Enter on one has pupu admit this rig, and this rig pair with pupu and reopen
/// its session with the input added, the rest of the session as it was.
#[test]
fn picking_another_rigs_input_pairs_both_rigs_and_reopens_the_session() {
    let mut t = paired_ready();
    let r = cmd(CommandId::AddRemoteInput, &mut t);
    assert!(
        r.iter()
            .any(|(rig, q)| *rig == PUPU && matches!(q, Request::ServerInfo)),
        "{r:?}"
    );
    assert!(
        r.iter()
            .any(|(rig, q)| *rig == RIG && matches!(q, Request::ServerInfo)),
        "{r:?}"
    );
    assert_eq!(
        texts(&t),
        vec![
            "pupu · in 1 · mic".to_string(),
            "pupu · in 2 · mic MM1 34804".to_string(),
        ]
    );
    key(&mut t, "Down");
    let r = key(&mut t, "Enter");
    let me = t.st.rig_label(&t.st.rig);
    let admit = r
        .iter()
        .find_map(|(rig, q)| match q {
            Request::Call {
                cmd: Command::AuthAdmitRig { key, rig_id, name },
                what,
            } if *rig == PUPU => Some((key.clone(), *rig_id, name.clone(), what.clone())),
            _ => None,
        })
        .expect("pupu admits this rig");
    assert_eq!(admit.0, KEY_A);
    assert_eq!(admit.1, RIG);
    assert_eq!(admit.2, crate::remote_inputs::admit_name(&me));
    assert_eq!(admit.3, words::admitted_toast("pupu", &me));
    let (peers, config, what) = r
        .iter()
        .find_map(|(rig, q)| match q {
            Request::RemoteSession {
                peers,
                config,
                what,
            } if *rig == RIG => Some((peers.clone(), config.clone(), what.clone())),
            _ => None,
        })
        .expect("this rig pairs and reopens");
    let peers = peers.expect("pairs with pupu");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].rig, PUPU);
    assert_eq!(peers[0].key, KEY_B);
    assert_eq!(peers[0].ctrl, "tcp://192.168.9.27:7700");
    assert_eq!(peers[0].data, "tcp://192.168.9.27:7701");
    assert_eq!(
        config.remote_inputs,
        vec![RemoteInput {
            slot: 0,
            rig: PUPU,
            rig_name: "pupu".into(),
            channel: 1,
        }]
    );
    assert_eq!(config.input_channels, vec![0, 1, REMOTE_BASE]);
    assert_eq!(config.sample_rate_hz, Some(48_000));
    assert_eq!(what, "pupu in 2 added to the audio I/O");
    // The dialog closed; the page stays.
    assert!(matches!(&t.st.overlay, Overlay::Settings(s) if s.remote.pick.is_none()));
}

/// The dialog says why a rig's inputs cannot be taken, and Esc closes it alone.
#[test]
fn the_dialog_refuses_in_words() {
    let mut t = paired_ready();
    to(
        &mut t,
        PUPU,
        server(ServerInfo {
            mode: ServerMode::Local {
                ctrl: "ipc:///run/ac2".into(),
            },
            recording_dir: None,
        }),
    );
    cmd(CommandId::AddRemoteInput, &mut t);
    assert_eq!(
        texts(&t),
        vec![words::source_refusal(
            "pupu",
            &words::SourceRefusal::LocalOnly
        )]
    );
    // An app already authorized on pupu under the name this rig would take.
    let me = t.st.rig_label(&t.st.rig);
    let taken = crate::remote_inputs::admit_name(&me);
    to(
        &mut t,
        PUPU,
        server(network(
            KEY_B,
            vec![AuthorizedClient {
                name: taken.clone(),
                key: KEY_A.into(),
                fingerprint: "cd34".into(),
                rig: None,
            }],
        )),
    );
    assert!(texts(&t)[0].contains(&format!("already authorizes an app named {taken}")));
    // This rig on its own computer only.
    t.conn(server(ServerInfo {
        mode: ServerMode::Local {
            ctrl: "ipc:///run/ac2".into(),
        },
        recording_dir: None,
    }));
    assert_eq!(texts(&t)[0], words::this_rig_local(&me));
    key(&mut t, "Escape");
    assert!(
        matches!(&t.st.overlay, Overlay::Settings(s) if s.remote.pick.is_none()),
        "Esc closes the dialog, not the page"
    );
}

/// This rig's session with pupu's input 2, as its daemon reports it.
pub(super) fn with_remote(t: &mut T, link: RemoteLink, audio: RemoteAudio) {
    t.remirror(|s| {
        let o = s.session.open.as_mut().expect("open");
        o.config.remote_inputs = vec![RemoteInput {
            slot: 0,
            rig: PUPU,
            rig_name: "pupu".into(),
            channel: 1,
        }];
        o.config.input_channels.push(REMOTE_BASE);
        s.remote = RemoteState {
            peers: Vec::new(),
            rigs: vec![RemoteRig {
                rig: PUPU,
                name: "pupu".into(),
                link,
                clock: PeerClock {
                    relation: PeerRelation::Measuring,
                    ppm: None,
                    span: None,
                    last_change: None,
                },
                bytes_per_s: 390_000.0,
                window: 262_144,
            }],
            inputs: vec![RemoteInputStatus {
                slot: 0,
                audio,
                anchor: Some(Anchor::SystemTime),
                late_frames: 0,
                late_frames_hour: 0,
                slips: 0,
                last_slip: None,
                arrival_margin: Some(Seconds(0.004)),
                setup: Some(InputSetup {
                    channel: 1,
                    kind: InputKind::Mic {
                        mic: Some("MM1 34804".into()),
                        curve: CurveChoice::NotChosen,
                    },
                    pair: None,
                }),
                cal_status: CalStatus::Uncalibrated,
                calibration: None,
                curve: None,
            }],
            link_offset: Some(Seconds(0.01)),
            transfers: Vec::new(),
        };
    });
}

/// A remote input is a channel by name everywhere: its row under its rig with that rig's
/// row text and its state, the channel pickers, and the Connection page's block.
#[test]
fn a_remote_input_reads_by_its_rig_and_name() {
    let mut t = two_mirrored();
    with_remote(&mut t, RemoteLink::Up, RemoteAudio::Streaming);
    let rows = t.st.remote_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rig, "pupu");
    assert_eq!(rows[0].text, "in 2 · mic MM1 34804 · uncalibrated");
    assert_eq!(
        rows[0].state,
        "streaming, placed by capture time · 0 late frames in the last hour · 0 slips"
    );
    assert!(!rows[0].problem);
    let names = t.st.session_input_names();
    assert_eq!(
        names.last(),
        Some(&(REMOTE_BASE, "MM1 34804 · pupu in 2".to_string()))
    );
    let blocks = t.st.between_rigs();
    assert_eq!(blocks.len(), 1);
    assert!(
        blocks[0][0].starts_with("pupu: measuring clocks…"),
        "{blocks:?}"
    );
    assert!(
        blocks[0].iter().any(|l| l
            == "10 ms link offset · 0 late frames in the last hour · 0 slips · \
                         arrival margin 4.0 ms"),
        "{blocks:?}"
    );
    // pupu unreachable: the row says so, since when.
    with_remote(
        &mut t,
        RemoteLink::Unreachable {
            since: WallNs(1_000_000_000),
        },
        RemoteAudio::Streaming,
    );
    let rows = t.st.remote_rows();
    assert!(
        rows[0]
            .state
            .starts_with("no audio from pupu (unreachable since "),
        "{rows:?}"
    );
    assert!(rows[0].problem);
}

/// On a remote input's row the kind keys are refused (*set on pupu*); Delete asks, a
/// second Delete reopens the session without it.
#[test]
fn a_remote_inputs_row_refuses_setup_and_removes_on_a_second_delete() {
    let mut t = two_mirrored();
    with_remote(&mut t, RemoteLink::Up, RemoteAudio::Streaming);
    t.st.update(Msg::Command(CommandId::Settings), &t.keys);
    t.st.update(
        Msg::Session(SessionMsg::Focus(crate::session_dialog::Row::Remote(0))),
        &t.keys,
    );
    key(&mut t, "M");
    let d = t.st.overlay.session().expect("audio I/O page");
    assert_eq!(d.error.as_deref(), Some(words::set_on("pupu").as_str()));
    let r = key(&mut t, "Delete");
    assert!(r.is_empty(), "{r:?}");
    let d = t.st.overlay.session().expect("audio I/O page");
    assert_eq!(
        d.notice.as_deref(),
        Some(words::remove_confirm("MM1 34804 · pupu in 2").as_str())
    );
    let r = key(&mut t, "Delete");
    let config = r
        .iter()
        .find_map(|(_, q)| match q {
            Request::RemoteSession {
                peers: None,
                config,
                what,
            } => {
                assert_eq!(what, "MM1 34804 · pupu in 2 removed from the audio I/O");
                Some(config.clone())
            }
            _ => None,
        })
        .expect("reopens without it");
    assert!(config.remote_inputs.is_empty());
    assert_eq!(config.input_channels, vec![0, 1]);
}
