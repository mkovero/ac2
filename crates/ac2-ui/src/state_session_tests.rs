//! Reducer tests of the audio session and measurement dialogs, the meters and recording.

use super::*;

fn real_backends() -> Vec<BackendInfo> {
    let mut b = backends(false);
    b.push(BackendInfo {
        kind: BackendKind::Cpal,
        description: "System audio".into(),
        availability: Availability::Available,
        devices: vec![
            DeviceInfo {
                backend: BackendKind::Cpal,
                id: DeviceId("card".into()),
                name: "card".into(),
                ..b[0].devices[0].clone()
            },
            DeviceInfo {
                backend: BackendKind::Cpal,
                id: DeviceId("other".into()),
                name: "other".into(),
                ..b[0].devices[0].clone()
            },
        ],
    });
    b
}

fn previewed(r: &[Request]) -> Option<&DeviceId> {
    r.iter().find_map(|r| match r {
        Request::Preview { device, .. } => Some(device),
        _ => None,
    })
}

/// Key labels as this platform shows them (`Shift+O` / `⇧O`, `Ctrl+K` / `⌘K`).
fn open_key() -> String {
    Chord::parse("Shift+O").expect("chord").label()
}

fn palette_key() -> String {
    Chord::parse("Ctrl+K").expect("chord").label()
}

fn connected_to(t: &mut T, target: &str) {
    t.conn(ConnEvent::Connected {
        target: target.into(),
        server: "ac2d test".into(),
        client_id: ClientId("c1".into()),
    });
}

/// Stored curves on the transfer pane move the hint out of the plot into the title strip;
/// hidden ones (or no data yet) leave it centred.
#[test]
fn empty_hint_yields_to_stored_traces() {
    let mut t = T::disconnected();
    connected_to(&mut t, "local daemon");
    let meta = stored(10, Some(1), 2);
    let mut s = no_session_state();
    s.traces = vec![meta.clone()];
    t.conn(mirror(s));
    let place = |t: &T| t.st.empty_hint(&t.keys).map(|h| h.place);
    // Listed, but its data has not arrived: nothing drawn yet.
    assert_eq!(place(&t), Some(HintPlace::Centre));
    let grid = Arc::new(GridDef::Log {
        ppo: 1,
        k_min: 0,
        k_max: 3,
    });
    let data = |meta: &TraceMeta| {
        Arc::new(TraceData {
            meta: meta.clone(),
            mag_db: vec![0.0; 4],
            phase_deg: None,
            coherence: None,
            sweep: None,
        })
    };
    t.conn(ConnEvent::Trace(data(&meta), grid.clone()));
    assert!(t.st.transfer_shows_stored());
    let hint = t.st.empty_hint(&t.keys).expect("hint");
    assert_eq!(hint.place, HintPlace::Title);
    assert!(
        hint.text.starts_with("No audio session — "),
        "{}",
        hint.text
    );
    // Hidden: the plot is empty again and the hint goes back to its centre.
    let mut hidden = meta.clone();
    hidden.edit.visible = false;
    let mut s = no_session_state();
    s.traces = vec![hidden.clone()];
    t.conn(mirror(s));
    t.conn(ConnEvent::Trace(data(&hidden), grid));
    assert_eq!(place(&t), Some(HintPlace::Centre));
}

#[test]
fn empty_hints_guide_to_a_session_then_a_measurement() {
    let mut t = T::disconnected();
    assert_eq!(t.st.empty_hint(&t.keys), None);
    connected_to(&mut t, "local daemon");
    // Connected but not synced: nothing to say yet.
    assert_eq!(t.st.empty_hint(&t.keys), None);
    t.conn(mirror(no_session_state()));
    assert_eq!(
        t.st.empty_hint(&t.keys).map(|h| h.text).as_deref(),
        Some(format!(
            "No audio session — press {} (or {} → Open audio session)",
            open_key(),
            palette_key()
        ))
        .as_deref()
    );
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    let hint = t.st.empty_hint(&t.keys).map(|h| h.text).unwrap_or_default();
    assert!(
        hint.starts_with(&format!(
            "No measurements — {} → New transfer measurement…",
            palette_key()
        )),
        "{hint}"
    );
    t.conn(mirror(daemon_state()));
    assert_eq!(t.st.empty_hint(&t.keys), None);
    // The hint follows the keymap: palette only when Shift+O is unbound.
    let keys = Keymap::from_toml("[global]\nsession_open = []").expect("keys");
    t.conn(mirror(no_session_state()));
    assert_eq!(
        t.st.empty_hint(&keys).map(|h| h.text).as_deref(),
        Some(format!(
            "No audio session — {} → Open audio session",
            palette_key()
        ))
        .as_deref()
    );
}

#[test]
fn arming_without_a_session_names_the_key() {
    let mut t = T::new();
    t.conn(mirror(no_session_state()));
    t.st.stimulus.level = Some(Dbfs(-20.0));
    let r = t.key("Space");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(
        t.last_toast().contains(&format!(
            "press {} (or {} → Open audio session)",
            open_key(),
            palette_key()
        )),
        "{}",
        t.last_toast()
    );
}

/// Shift+O, the simulated rig's own wiring as roles, Enter: the session, its mic names and
/// one transfer per mic, without a channel number typed.
#[test]
fn session_dialog_opens_a_session_from_roles() {
    let mut t = T::new();
    t.conn(mirror(no_session_state()));
    // Shift+O opens the dialog, asks for the devices and subscribes to the meters; the O it
    // types is swallowed.
    let r = t.type_key("Shift+O", "O");
    assert!(
        matches!(r.as_slice(), [Request::Devices, Request::Meters(true)]),
        "{r:?}"
    );
    assert!(dialog(&t).backends.is_none());
    // Enter before the list arrives says so and sends nothing.
    assert!(t.key("Enter").is_empty());
    assert!(dialog(&t).error.is_some());
    let r = t.conn(ConnEvent::Devices(Ok(backends(true))));
    // A daemon offering only the simulated rig preselects it, its device previewed.
    assert_eq!(dialog(&t).backend_kind(), Some(BackendKind::Fake));
    assert_eq!(previewed(&r), Some(&DeviceId("fake:loop".into())));
    let d = dialog(&t);
    assert_eq!(d.inputs[0].role, InputRole::Reference);
    assert_eq!(d.inputs[1].role, InputRole::Mic);
    assert_eq!(d.inputs[1].label(), "Room mic");
    assert!(!d.inputs[2].in_session);
    assert!(d.outputs[0].stimulus);
    // Name the mic: N on its row, type, Enter ends the edit (it does not open).
    focus(&mut t, Row::Input(1));
    let r = t.type_key("N", "n");
    assert!(r.is_empty(), "{r:?}");
    t.text("M30 FOH");
    assert!(t.key("Enter").is_empty());
    assert_eq!(dialog(&t).inputs[1].mic, "M30 FOH");
    assert_eq!(dialog(&t).edit, None);
    let r = t.key("Enter");
    let (config, inputs, transfers) = opened(&r).expect("session.open");
    assert_eq!(config.backend, Some(BackendKind::Fake));
    assert_eq!(config.input_channels, vec![0, 1]);
    assert_eq!(config.output_channels, 2);
    assert_eq!(
        config.loopback,
        Some(LoopbackRoute {
            output: 0,
            input: 0
        })
    );
    assert_eq!(
        config.input_device,
        DeviceSelector::Id {
            id: DeviceId("fake:loop".into())
        }
    );
    assert_eq!(
        inputs,
        &[
            InputSetup {
                channel: 0,
                mic: None,
                curve: CurveChoice::NotChosen,
            },
            InputSetup {
                channel: 1,
                mic: Some("M30 FOH".into()),
                curve: CurveChoice::NotChosen,
            },
        ]
    );
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].name, "Reference → M30 FOH");
    // Closing the dialog closes the preview and the meter subscription.
    assert!(r.iter().any(|x| matches!(x, Request::PreviewStop)), "{r:?}");
    assert!(
        r.iter().any(|x| matches!(x, Request::Meters(false))),
        "{r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
    // Remembered for the device, the stimulus follows the S output (K4).
    let roles = &t.st.prefs.sessions["fake/fake:loop"];
    assert_eq!(roles.reference, Some(0));
    assert_eq!(roles.mics, vec![1]);
    assert_eq!(roles.mic_names[&1], "M30 FOH");
    assert_eq!(t.st.prefs.outputs["fake:loop"], vec![0]);
    assert!(t.st.prefs_dirty);
}

#[test]
fn roles_move_toggle_and_explain_themselves() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    // R moves the reference; the old row keeps its place in the session.
    focus(&mut t, Row::Input(2));
    t.key("R");
    let d = dialog(&t);
    assert_eq!(d.inputs[2].role, InputRole::Reference);
    assert!(d.inputs[2].in_session);
    assert_eq!(d.inputs[0].role, InputRole::None);
    assert!(d.inputs[0].in_session);
    // M marks several mics; Space takes a row out of the session with its role.
    focus(&mut t, Row::Input(3));
    t.key("M");
    assert_eq!(dialog(&t).inputs[3].role, InputRole::Mic);
    assert_eq!(dialog(&t).inputs[1].role, InputRole::Mic);
    t.type_key("Space", " ");
    assert!(!dialog(&t).inputs[3].in_session);
    assert_eq!(dialog(&t).inputs[3].role, InputRole::None);
    // S belongs on an output: on an input it says so and changes nothing.
    t.key("S");
    assert!(
        dialog(&t)
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("S marks an output")),
        "{:?}",
        dialog(&t).notice
    );
    // Without the reference, mics have nothing to be measured against.
    focus(&mut t, Row::Input(2));
    t.key("R");
    assert!(
        t.key("Enter")
            .iter()
            .all(|r| opened(std::slice::from_ref(r)).is_none())
    );
    let e = dialog(&t).error.clone().unwrap_or_default();
    assert!(e.starts_with("Pick a reference input: the loopback"), "{e}");
    t.key("R");
    // Without a stimulus output the loopback has no source.
    focus(&mut t, Row::Output(0));
    t.key("S");
    assert!(opened(&t.key("Enter")).is_none());
    let e = dialog(&t).error.clone().unwrap_or_default();
    assert!(e.starts_with("Pick the stimulus output"), "{e}");
    t.key("S");
    // Space on output 2 shrinks the session to output 1, and back.
    focus(&mut t, Row::Output(1));
    t.type_key("Space", " ");
    assert_eq!(dialog(&t).out_count, 1);
    t.type_key("Space", " ");
    assert_eq!(dialog(&t).out_count, 2);
    // Every input out of the session: refused in plain words.
    for i in 0..4 {
        if dialog(&t).inputs[i].in_session {
            focus(&mut t, Row::Input(i));
            t.type_key("Space", " ");
        }
    }
    assert!(opened(&t.key("Enter")).is_none());
    let e = dialog(&t).error.clone().unwrap_or_default();
    assert!(e.starts_with("Choose at least one input"), "{e}");
}

#[test]
fn mic_names_must_differ_and_the_mouse_assigns_roles() {
    let mut t = T::new();
    open_dialog(&mut t, backends(false));
    // Unnamed channels read Input N.
    assert_eq!(dialog(&t).inputs[2].label(), "Input 3");
    for i in [1, 2] {
        t.st.update(Msg::Session(SessionMsg::EditMic(Row::Input(i))), &t.keys);
        t.text("ECM");
        t.key("Enter");
    }
    assert!(opened(&t.key("Enter")).is_none());
    let e = dialog(&t).error.clone().unwrap_or_default();
    assert!(e.contains("Two mics are named \"ECM\""), "{e}");
    // The R chip of input 3 by mouse; N on the reference refuses a mic name.
    t.st.update(
        Msg::Session(SessionMsg::Role(Row::Input(2), RoleKey::Reference)),
        &t.keys,
    );
    assert_eq!(dialog(&t).inputs[2].role, InputRole::Reference);
    t.key("N");
    assert_eq!(dialog(&t).edit, None);
    let r = t.st.update(Msg::Session(SessionMsg::Submit), &t.keys);
    let (config, inputs, transfers) = opened(&r).expect("open");
    assert_eq!(config.loopback.map(|l| l.input), Some(2));
    assert_eq!(config.input_channels, vec![0, 1, 2]);
    assert_eq!(inputs[1].mic.as_deref(), Some("ECM"));
    assert_eq!(inputs[2].mic, None);
    assert_eq!(transfers[0].name, "Reference → ECM");
    // Cancel closes without touching the stimulus (Esc would stop it).
    t.st.update(Msg::Command(CommandId::OpenSession), &t.keys);
    t.st.stimulus.phase = StimPhase::Armed;
    let r = t.st.update(Msg::Session(SessionMsg::Cancel), &t.keys);
    assert!(!r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.stimulus.phase, StimPhase::Armed);
}

#[test]
fn roles_are_remembered_per_device() {
    let mut t = T::new();
    t.st.prefs.sessions.insert(
        "fake/fake:loop".into(),
        crate::prefs::DeviceRoles {
            inputs: vec![0, 2, 3],
            outputs: 2,
            reference: Some(3),
            mics: vec![0, 2],
            stimulus: vec![1],
            mic_names: [(0, "M30".to_owned()), (2, "KM184".to_owned())].into(),
            output_device: None,
        },
    );
    open_dialog(&mut t, backends(true));
    let d = dialog(&t);
    assert_eq!(d.inputs[3].role, InputRole::Reference);
    assert_eq!(d.inputs[0].role, InputRole::Mic);
    assert_eq!(d.inputs[0].mic, "M30");
    assert_eq!(d.inputs[0].label(), "M30");
    assert!(!d.inputs[1].in_session);
    assert!(d.outputs[1].stimulus && !d.outputs[0].stimulus);
    let r = t.key("Enter");
    let (config, _, transfers) = opened(&r).expect("open");
    assert_eq!(
        config.loopback,
        Some(LoopbackRoute {
            output: 1,
            input: 3
        })
    );
    let names: Vec<&str> = transfers.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Reference → M30", "Reference → KM184"]);
    assert!(matches!(
        &transfers[1].kind,
        MeasKind::Transfer { config } if config.reference_input == 3 && config.measurement_input == 2
    ));
}

#[test]
fn a_real_backend_is_preferred_and_an_unavailable_one_says_why() {
    let mut t = T::new();
    let r = open_dialog(&mut t, real_backends());
    assert_eq!(dialog(&t).backend_kind(), Some(BackendKind::Cpal));
    assert_eq!(previewed(&r), Some(&DeviceId("card".into())));
    // A real interface starts without roles: its wiring is the operator's to say.
    assert!(dialog(&t).inputs.iter().all(|i| i.role == InputRole::None));
    // → on the device row previews the other device.
    t.key("Down");
    let r = t.key("Right");
    assert_eq!(previewed(&r), Some(&DeviceId("other".into())));
    // The backend row: JACK is listed but not available.
    t.key("Up");
    let r = t.key("Left");
    assert_eq!(dialog(&t).backend_kind(), Some(BackendKind::Jack));
    assert!(r.iter().any(|x| matches!(x, Request::PreviewStop)), "{r:?}");
    assert!(opened(&t.key("Enter")).is_none());
    let e = dialog(&t).error.clone().unwrap_or_default();
    assert_eq!(
        e,
        "JACK is not available: JACK server not running. Or pick another backend (← → on the first row)."
    );
}

/// A Linux daemon offers JACK alone; with no server the dialog says why and what to do,
/// and never points at another backend that is not there.
#[test]
fn jack_alone_and_unavailable_says_what_to_do() {
    let reason = "PipeWire is running but its JACK library isn't in use: install pipewire-jack \
                  (e.g. `sudo apt install pipewire-jack`, `sudo pacman -S pipewire-jack`) or \
                  start the daemon with `pw-jack ac2d`";
    let mut t = T::new();
    let r = open_dialog(
        &mut t,
        vec![BackendInfo {
            kind: BackendKind::Jack,
            description: "JACK".into(),
            availability: Availability::Unavailable {
                reason: reason.into(),
            },
            devices: vec![],
        }],
    );
    assert_eq!(dialog(&t).backend_kind(), Some(BackendKind::Jack));
    assert_eq!(previewed(&r), None);
    assert!(opened(&t.key("Enter")).is_none());
    assert_eq!(
        dialog(&t).error.as_deref(),
        Some(format!("JACK is not available: {reason}.").as_str())
    );
}

#[test]
fn preview_renews_and_the_open_device_uses_the_session_meters() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    let tick = |t: &mut T, s: f64| {
        t.st.update(
            Msg::Tick {
                now_s: s,
                dt_s: 0.1,
            },
            &t.keys,
        )
    };
    assert!(previewed(&tick(&mut t, 1.0)).is_none());
    assert!(previewed(&tick(&mut t, 2.5)).is_some());
    assert!(previewed(&tick(&mut t, 3.0)).is_none());
    let target = dialog(&t).preview_target().expect("previewed");
    // A late answer for a device the dialog has left is not this device's.
    t.conn(ConnEvent::Preview {
        backend: BackendKind::Jack,
        device: DeviceId("another".into()),
        result: Err("device busy".into()),
    });
    assert_eq!(dialog(&t).preview_error, None);
    t.conn(ConnEvent::Preview {
        backend: target.0,
        device: target.1.clone(),
        result: Err("device busy".into()),
    });
    assert_eq!(
        dialog(&t).preview_error.as_deref(),
        Some("meters unavailable: device busy")
    );
    t.key("Escape");
    // The open session's own device: its session meters, no preview.
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
    }
    t.conn(mirror(s));
    t.st.update(Msg::Command(CommandId::OpenSession), &t.keys);
    let r = t.conn(ConnEvent::Devices(Ok(backends(true))));
    assert!(dialog(&t).is_open_device());
    assert!(previewed(&r).is_none(), "{r:?}");
    // Prefilled from the session: inputs 1–2, no loopback.
    assert!(dialog(&t).inputs[0].in_session && dialog(&t).inputs[1].in_session);
    assert!(dialog(&t).inputs.iter().all(|i| i.role == InputRole::None));
}

#[test]
fn esc_closes_the_dialog_and_the_preview_and_leaves_the_stimulus() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    t.st.stimulus.phase = StimPhase::Firing;
    let r = t.key("Escape");
    assert!(!r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);
    assert!(r.iter().any(|x| matches!(x, Request::PreviewStop)), "{r:?}");
    assert!(
        r.iter().any(|x| matches!(x, Request::Meters(false))),
        "{r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn detect_loopback_needs_a_stimulus_output_and_a_typed_level() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    // Without a stimulus output there is nothing to play on.
    focus(&mut t, Row::Output(0));
    t.key("S");
    t.type_key("D", "d");
    let e = dialog(&t).error.clone().unwrap_or_default();
    assert!(e.starts_with("Pick the stimulus output first"), "{e}");
    assert!(dialog(&t).detect.is_none());
    t.key("S");
    // D asks first: no level, no burst.
    let r = t.type_key("D", "d");
    assert!(r.is_empty(), "{r:?}");
    let p = dialog(&t).detect.clone().expect("panel");
    assert_eq!(p.level, "", "never a default level");
    assert_eq!(p.output, 0);
    assert!(t.key("Enter").is_empty());
    assert!(
        dialog(&t)
            .detect
            .as_ref()
            .and_then(|d| d.error.as_deref())
            .is_some_and(|e| e.contains("no default level"))
    );
    // Above the ceiling (−6 dBFS on this daemon) is refused before anything is sent.
    t.text("-3");
    assert!(t.key("Enter").is_empty());
    for _ in 0..2 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("-30");
    let r = t.key("Enter");
    let req = r
        .iter()
        .find_map(|x| match x {
            Request::DetectLoopback(d) => Some(d.clone()),
            _ => None,
        })
        .expect("detect");
    assert_eq!(req.level, Dbfs(-30.0));
    assert_eq!(req.output, 0);
    assert_eq!(req.device, DeviceId("fake:loop".into()));
    // While the burst plays, Enter does not open a session on the same device.
    assert!(opened(&t.key("Enter")).is_none());
    assert!(
        dialog(&t)
            .error
            .as_deref()
            .is_some_and(|e| e.contains("still playing"))
    );
    // The burst holds the device: no preview meanwhile.
    assert!(r.iter().any(|x| matches!(x, Request::PreviewStop)), "{r:?}");
    // The answer: input 3 is the loopback; it becomes the Reference and the preview resumes.
    let mut det = ac2_client::fake::fake_detection();
    det.loopback = Some(2);
    det.ranked[0].input = 2;
    let r = t.conn(ConnEvent::LoopbackDetected(Ok(det)));
    assert!(previewed(&r).is_some(), "{r:?}");
    let d = dialog(&t);
    assert_eq!(d.inputs[2].role, InputRole::Reference);
    assert_eq!(d.inputs[0].role, InputRole::None);
    assert_eq!(d.focus, Row::Input(2));
    let text = d.detect_text().unwrap_or_default();
    assert!(
        text.starts_with("Loopback found on input 3 (Line 3)"),
        "{text}"
    );
    // A typed stimulus level is offered again for the next detection.
    t.st.stimulus.level = Some(Dbfs(-24.0));
    t.type_key("D", "d");
    assert_eq!(
        dialog(&t).detect.as_ref().map(|d| d.level.as_str()),
        Some("-24.0")
    );
    // ↑ leaves the confirmation without playing.
    t.key("Up");
    assert!(dialog(&t).detect.is_none());
}

#[test]
fn opening_offers_one_transfer_per_mic_on_an_empty_daemon() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    focus(&mut t, Row::Input(2));
    t.key("M");
    let r = t.key("Enter");
    let (_, _, transfers) = opened(&r).expect("open");
    assert_eq!(transfers.len(), 2);
    let transfers = transfers.to_vec();
    t.conn(ConnEvent::SessionOpened {
        transfers: transfers.clone(),
    });
    assert!(matches!(&t.st.overlay, Overlay::Offer(o) if o.transfers == transfers));
    let r = t.key("Enter");
    let made: Vec<&str> = r
        .iter()
        .filter_map(|x| match x {
            Request::CreateMeas { config } => Some(config.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(made, ["Reference → Room mic", "Reference → Line 3"]);
    assert_eq!(t.st.overlay, Overlay::None);
    // N skips; a daemon that has measurements by then gets no offer.
    t.conn(ConnEvent::SessionOpened {
        transfers: transfers.clone(),
    });
    assert!(t.key("N").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    t.conn(mirror(daemon_state()));
    t.conn(ConnEvent::SessionOpened { transfers });
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn session_dialog_errors() {
    let mut t = T::new();
    t.st.update(Msg::Command(CommandId::OpenSession), &t.keys);
    t.conn(ConnEvent::Devices(Err("not connected".into())));
    assert!(
        dialog(&t)
            .error
            .as_deref()
            .is_some_and(|e| e.contains("not connected"))
    );
    t.key("Escape");
    // Not connected: no dialog.
    let mut t = T::disconnected();
    assert!(
        t.st.update(Msg::Command(CommandId::OpenSession), &t.keys)
            .is_empty()
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(t.last_toast().contains("not connected"));
}

#[test]
fn input_meters_follow_the_dialog() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{ClipFlags, PreviewLevelsFrame, PreviewLevelsMeta, SessionLevelsFrame};
    use ac2_proto::topic::Topic;
    use ac2_proto::{Frame, FrameData};
    let mut t = T::new();
    let frame = |data: FrameData| TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame {
            stamp: ac2_proto::samples::stamp(None),
            data,
        }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    for f in [
        frame(FrameData::SessionLevels(SessionLevelsFrame {
            meta: ac2_proto::frame::LevelsMeta {
                channels: vec![0, 1],
            },
            peak: vec![-10.0, -30.0],
            rms: vec![-20.0, -40.0],
            clip: vec![ClipFlags::NONE, ClipFlags::HELD],
        })),
        frame(FrameData::PreviewLevels(PreviewLevelsFrame {
            meta: PreviewLevelsMeta {
                backend: BackendKind::Fake,
                device: DeviceId("fake:loop".into()),
                channels: vec![0, 1, 2, 3],
            },
            peak: vec![-6.0, f32::NEG_INFINITY, -50.0, -50.0],
            rms: vec![-12.0, f32::NEG_INFINITY, -60.0, -60.0],
            clip: vec![ClipFlags::NONE; 4],
        })),
    ] {
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    assert!(latest.get(&Topic::PreviewLevels).is_some());
    t.st.data = Some(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }));
    // A measurement dialog shows the session's meters.
    t.st.update(Msg::Command(CommandId::NewTransfer), &t.keys);
    let m = t.st.input_meters();
    assert_eq!(m.len(), 2);
    assert_eq!(m[&0].text, "\u{2212}20.0");
    assert_eq!(m[&1].state, ac2_scene::meter::MeterState::Clip);
    t.key("Escape");
    // The session dialog on a device the session does not capture: its preview.
    open_dialog(&mut t, backends(true));
    let m = t.st.input_meters();
    assert_eq!(m.len(), 4);
    assert_eq!(m[&0].text, "\u{2212}12.0");
    assert_eq!(m[&1].state, ac2_scene::meter::MeterState::Silent);
}

/// The sidebar's always-on meters: every captured input by name and role, its reading,
/// and what the running sweep (else the selected measurement) uses it as.
#[test]
fn sidebar_meters_name_every_session_input_by_role() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{ClipFlags, SessionLevelsFrame};
    use ac2_proto::{Frame, FrameData};
    use ac2_scene::meter::{InputUse, MeterState};
    let mut t = T::new();
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
        o.config.input_channels = vec![0, 1, 2];
        o.config.loopback = Some(LoopbackRoute {
            output: 0,
            input: 0,
        });
    }
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve: CurveChoice::Curve {
            label: "90°".into(),
        },
    }];
    s.mics = vec![mm1()];
    t.conn(mirror(s.clone()));
    let labels = |t: &T| -> Vec<(String, Option<InputUse>)> {
        t.st.session_inputs()
            .into_iter()
            .map(|r| (r.label, r.used))
            .collect()
    };
    // Before the device list: the mic's name, else `Input N`, with the role.
    assert_eq!(
        labels(&t),
        vec![
            ("Input 1 · reference".into(), Some(InputUse::Reference)),
            (
                "MM1 34804 · 90° · mic (in 2)".into(),
                Some(InputUse::Measurement)
            ),
            ("Input 3".into(), None),
        ]
    );
    // The device's channel names once listed.
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    assert_eq!(
        labels(&t),
        vec![
            (
                "Loop return · reference (in 1)".into(),
                Some(InputUse::Reference)
            ),
            (
                "MM1 34804 · 90° · mic (in 2)".into(),
                Some(InputUse::Measurement)
            ),
            ("Line 3 (in 3)".into(), None),
        ]
    );
    // Readings from the session's meters; an input without one reads as no data.
    let mut latest = Latest::default();
    let data = FrameData::SessionLevels(SessionLevelsFrame {
        meta: ac2_proto::frame::LevelsMeta {
            channels: vec![0, 1],
        },
        peak: vec![-10.0, -1.0],
        rms: vec![-20.0, -12.0],
        clip: vec![ClipFlags::NONE, ClipFlags::HELD],
    });
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame {
            stamp: ac2_proto::samples::stamp(None),
            data,
        }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    latest.frames.insert(f.topic.to_string().into(), f);
    t.st.data = Some(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }));
    let rows = t.st.session_inputs();
    assert_eq!(rows[0].reading.with_unit(), "\u{2212}20.0 dBFS");
    assert_eq!(rows[0].reading.state, MeterState::Signal);
    assert_eq!(rows[1].reading.state, MeterState::Clip);
    assert_eq!(rows[2].reading.state, MeterState::NoData);

    // A running sweep's inputs are the marked ones while it runs.
    let mut run = sweep_run(SweepStatus::Playing { repeat: 1 });
    run.reference_input = 2;
    s.sweep = Some(run);
    t.conn(mirror(s.clone()));
    let used: Vec<_> = labels(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(
        used,
        vec![None, Some(InputUse::Measurement), Some(InputUse::Reference)]
    );
}

/// The meters stay subscribed while a session is open, whatever dialog opens and closes,
/// and the subscription ends with the session.
#[test]
fn session_meters_stay_subscribed_while_a_session_is_open() {
    let mut t = T::disconnected();
    connected_to(&mut t, "local daemon");
    let r = t.conn(mirror(daemon_state()));
    assert!(
        r.iter().any(|x| matches!(x, Request::Meters(true))),
        "{r:?}"
    );
    let r = t.st.update(Msg::Command(CommandId::NewTransfer), &t.keys);
    assert!(!r.iter().any(|x| matches!(x, Request::Meters(_))), "{r:?}");
    let r = t.key("Escape");
    assert!(!r.iter().any(|x| matches!(x, Request::Meters(_))), "{r:?}");
    let r = t.conn(mirror(no_session_state()));
    assert!(
        r.iter().any(|x| matches!(x, Request::Meters(false))),
        "{r:?}"
    );
}

/// A set of sweeps on the progress strip: which one of how many, the bar, the time left
/// counted from when each step was seen, analysing, and gone once it ends.
#[test]
fn sweep_progress_strip_counts_steps_and_time_left() {
    let mut t = T::new();
    let tick = |t: &mut T, now_s: f64| {
        t.st.update(Msg::Tick { now_s, dt_s: 0.02 }, &t.keys);
    };
    tick(&mut t, 10.0);
    assert_eq!(t.st.operation(), None);
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("c1".into()));
    s.generator.armed = true;
    s.generator.firing = true;
    let mut run = sweep_run(SweepStatus::Playing { repeat: 1 });
    run.repeats = 2;
    // Each step: 3.1 s of sweep and 1 s of silence.
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.title, "sweep \"Run 1\"");
    assert_eq!(p.step, "sweep 1 of 2");
    assert_eq!(p.fraction, 0.0);
    assert_eq!(p.remaining.as_deref(), Some("about 9 s left"));
    tick(&mut t, 12.05);
    let p = t.st.operation().expect("progress");
    assert!((p.fraction - 0.25).abs() < 1e-3, "{}", p.fraction);
    assert_eq!(p.remaining.as_deref(), Some("about 7 s left"));

    tick(&mut t, 14.0);
    run.status = SweepStatus::Playing { repeat: 2 };
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.step, "sweep 2 of 2");
    assert!((p.fraction - 0.5).abs() < 1e-3, "{}", p.fraction);
    assert_eq!(p.remaining.as_deref(), Some("about 5 s left"));

    // Stop (the strip's button sends the same command as Esc).
    let r = t.st.update(Msg::Command(CommandId::StimulusStop), &t.keys);
    assert!(r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");

    run.status = SweepStatus::Analysing;
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.step, "analysing…");
    assert_eq!(p.remaining, None);

    run.status = SweepStatus::Failed {
        reason: SweepFailure::Stopped,
        msg: "stopped".into(),
    };
    s.sweep = Some(run);
    t.conn(mirror(s));
    assert_eq!(t.st.operation(), None);
    assert_eq!(t.st.sweep.step_seen, None);
}

#[test]
fn record_toggles_and_the_indicator_names_the_inputs() {
    let mut t = T::new();
    assert_eq!(t.st.recording_label(), None);
    let r = t.st.update(Msg::Command(CommandId::Record), &t.keys);
    let [
        Request::Call {
            cmd: Command::RecStart { request },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}");
    };
    assert_eq!(request.inputs, vec![0, 1], "every input of the session");
    assert_eq!(request.max_duration, Seconds(RECORD_MAX_S));

    let mut s = daemon_state();
    s.recording = Some(RecordingRun {
        name: "rec-x".into(),
        path: "/r/rec-x.wav".into(),
        inputs: vec![0, 1],
        sample_rate_hz: 48_000,
        session_epoch: s.session.epoch,
        start_sample: SampleIndex(0),
        started_at: WallNs(0),
        started_by: ClientId("c1".into()),
        frames: 48_000 * 5,
        bytes: 116 + 48_000 * 5 * 8,
        discontinuities: 0,
        max_duration: Seconds(RECORD_MAX_S),
        max_bytes: None,
        status: RecordingStatus::Recording,
    });
    t.conn(mirror(s.clone()));
    let l = t.st.recording_label().expect("indicator");
    assert_eq!(l.text, "REC 0:05 · 1.9 MB");
    assert!(
        l.detail
            .starts_with("Recording Input 1, Input 2 to /r/rec-x.wav"),
        "{}",
        l.detail
    );
    let r = t.st.update(Msg::Command(CommandId::Record), &t.keys);
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call {
                cmd: Command::RecStop,
                ..
            }]
        ),
        "{r:?}"
    );

    // No session, nothing recording: the toggle says how to open one.
    t.conn(mirror(no_session_state()));
    assert!(
        t.st.update(Msg::Command(CommandId::Record), &t.keys)
            .is_empty()
    );
    assert!(t.last_toast().contains("no audio session to record"));

    // Replay asks for the recording and plays it in real time.
    t.st.update(Msg::Command(CommandId::ReplayRecording), &t.keys);
    t.text("rec-x");
    let r = t.key("Enter");
    assert!(
        matches!(r.as_slice(), [Request::Call { cmd: Command::SessionReplay {
            recording: RecordingRef::Name { name },
            pace: ReplayPace::Realtime,
        }, .. }] if name == "rec-x"),
        "{r:?}"
    );
}

#[test]
fn close_session_and_delete_measurement() {
    let mut t = T::new();
    let r = t.st.update(Msg::Command(CommandId::CloseSession), &t.keys);
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call {
                cmd: Command::SessionClose,
                ..
            }]
        ),
        "{r:?}"
    );
    t.st.update(Msg::Command(CommandId::DeleteSelected), &t.keys);
    let r = t.key("Enter");
    assert!(
        matches!(r.as_slice(), [Request::Call { cmd: Command::MeasDelete { meas: MeasId(1), traces: ac2_proto::model::OwnedTraces::Keep }, what }]
            if what == "Main L deleted"),
        "{r:?}"
    );
    t.conn(mirror(no_session_state()));
    assert!(
        t.st.update(Msg::Command(CommandId::CloseSession), &t.keys)
            .is_empty()
    );
    assert!(t.last_toast().contains("no audio session"));
    assert!(
        t.st.update(Msg::Command(CommandId::DeleteSelected), &t.keys)
            .is_empty()
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(t.last_toast().contains("select a measurement"));
}

#[test]
fn new_measurements_start_and_become_selected() {
    let mut t = T::new();
    assert_eq!(t.st.selected, Some(MeasId(1)));
    t.st.update(Msg::Command(CommandId::NewTransfer), &t.keys);
    assert_eq!(form(&t).kind, FormKind::Transfer);
    let r = t.key("Enter");
    let c = created(&r).expect("meas.create");
    // One transfer exists, so this is the second; inputs from the session (1 → 2).
    assert_eq!(c.name, "TF 2");
    assert!(matches!(
        &c.kind,
        MeasKind::Transfer { config } if config.reference_input == 0
            && config.measurement_input == 1
            && config.depth == DepthPolicy::EqualConfidence
    ));
    assert_eq!(t.st.overlay, Overlay::None);
    // Created: selected at once, kept while the mirror has not caught up, then confirmed.
    let m = meas(7, "TF 2", transfer());
    t.conn(ConnEvent::MeasCreated(Box::new(m.clone())));
    assert_eq!(t.st.selected, Some(MeasId(7)));
    t.conn(mirror(daemon_state()));
    assert_eq!(t.st.selected, Some(MeasId(7)));
    let mut s = daemon_state();
    s.measurements.push(m);
    t.conn(mirror(s.clone()));
    assert_eq!(t.st.selected, Some(MeasId(7)));
    // The selection is the operator's again: N moves on and a later mirror keeps it.
    t.key("N");
    assert_ne!(t.st.selected, Some(MeasId(7)));
    let sel = t.st.selected;
    t.conn(mirror(s));
    assert_eq!(t.st.selected, sel);

    // Spectrum, RTA and SPL meter dialogs make their kinds on the first non-reference input.
    for (cmd, want) in [
        (CommandId::NewSpectrum, "spectrum"),
        (CommandId::NewRta, "rta"),
        (CommandId::NewSpl, "spl"),
    ] {
        t.st.update(Msg::Command(cmd), &t.keys);
        let r = t.key("Enter");
        let c = created(&r).unwrap_or_else(|| panic!("{want}: {r:?}"));
        let kind = match &c.kind {
            MeasKind::Spectrum { config } => ("spectrum", config.input),
            MeasKind::Rta { config } => ("rta", config.input),
            MeasKind::Spl { config } => ("spl", config.input),
            MeasKind::Transfer { .. } | MeasKind::Math { .. } | MeasKind::Sweep { .. } => {
                ("tf", 99)
            }
        };
        assert_eq!(kind, (want, 1));
    }
    // Inputs are picked by name among the captured ones: ←/→ walks them, stopping at the
    // ends.
    t.st.update(Msg::Command(CommandId::NewSpl), &t.keys);
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(1));
    assert_eq!(form(&t).fields[0].display(), "2 · Input 2");
    t.key("Right");
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(1));
    t.key("Left");
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(0));
    t.key("Left");
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(0));
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);

    // No session: nothing to measure, and the toast says how to open one.
    t.conn(mirror(no_session_state()));
    assert!(
        t.st.update(Msg::Command(CommandId::NewRta), &t.keys)
            .is_empty()
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(t.last_toast().contains(&open_key()), "{}", t.last_toast());
}

#[test]
fn real_audio_embedded_daemon_opens_the_session_dialog_once() {
    let mut t = T::disconnected();
    t.st.open_session_when_empty = true;
    connected_to(&mut t, "embedded daemon (cpal)");
    let r = t.conn(mirror(no_session_state()));
    assert!(
        matches!(r.as_slice(), [Request::Devices, Request::Meters(true)]),
        "{r:?}"
    );
    assert!(dialog(&t).backends.is_none());
    t.key("Escape");
    // Only once: the operator closed it.
    assert!(t.conn(mirror(no_session_state())).is_empty());
    assert_eq!(t.st.overlay, Overlay::None);

    // A daemon that already has a session: no dialog; its input meters and the channel
    // names for them.
    let mut t = T::disconnected();
    t.st.open_session_when_empty = true;
    connected_to(&mut t, "embedded daemon (cpal)");
    let r = t.conn(mirror(daemon_state()));
    assert!(
        matches!(r.as_slice(), [Request::Meters(true), Request::Devices]),
        "{r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(!t.st.open_session_when_empty);
}

/// System audio listing each direction as its own endpoint (WASAPI): an output-only device
/// listed first and the system's default output, an input-only USB mic, and another
/// output-only device.
fn endpoint_backends() -> Vec<BackendInfo> {
    let mut b = backends(false);
    let base = b[0].devices[0].clone();
    let endpoint = |id: &str, input: bool, output: Option<bool>| DeviceInfo {
        backend: BackendKind::Cpal,
        id: DeviceId(id.into()),
        name: id.into(),
        input: base.input.clone().filter(|_| input),
        output: output.and_then(|default| {
            base.output.clone().map(|o| DirectionInfo {
                system_default: default,
                ..o
            })
        }),
        duplex_clock: ClockRelation::Unknown,
        ..base.clone()
    };
    b.push(BackendInfo {
        kind: BackendKind::Cpal,
        description: "System audio".into(),
        availability: Availability::Available,
        devices: vec![
            endpoint("Speakers", false, Some(true)),
            endpoint("USB mic", true, None),
            endpoint("Interface out", false, Some(false)),
        ],
    });
    b
}

/// An input device without outputs plays on the system's default output; ←/→ on the
/// Output row choose another output device, whose rows the Inputs & outputs page shows;
/// the session opens with both devices, the loopback detection plays on the output device,
/// and the choice is remembered for the input device.
#[test]
fn an_input_only_device_plays_on_another_output_device() {
    use crate::session_dialog::OutputDevice;
    let mut t = T::new();
    let r = open_dialog(&mut t, endpoint_backends());
    let d = dialog(&t);
    // The output-only endpoint is never the input device.
    assert_eq!(d.device_info().map(|x| x.name.as_str()), Some("USB mic"));
    assert_eq!(previewed(&r), Some(&DeviceId("USB mic".into())));
    assert_eq!(d.out_device, OutputDevice::Other(0));
    assert_eq!(d.output_info().map(|x| x.name.as_str()), Some("Speakers"));
    assert_eq!(d.output_choices().len(), 2, "no same-as-input choice");
    assert_eq!(d.outputs.len(), 2);
    assert!(
        d.clock_note()
            .is_some_and(|n| n.starts_with("Output plays on Speakers, another device")),
        "{:?}",
        d.clock_note()
    );
    // → on the Output row: the other output device; → again stays at the end; the input
    // device's preview is untouched.
    focus(&mut t, Row::OutputDevice);
    let r = t.key("Right");
    assert_eq!(previewed(&r), None, "{r:?}");
    assert_eq!(
        dialog(&t).output_info().map(|x| x.name.as_str()),
        Some("Interface out")
    );
    t.key("Right");
    assert_eq!(dialog(&t).out_device, OutputDevice::Other(2));
    t.key("Left");
    assert_eq!(dialog(&t).out_device, OutputDevice::Other(0));
    // ← on the input row never lands on an output-only endpoint.
    focus(&mut t, Row::Device);
    t.key("Left");
    assert_eq!(
        dialog(&t).device_info().map(|x| x.name.as_str()),
        Some("USB mic")
    );

    // The stimulus on output 1 of the output device, and a loopback detection over both.
    focus(&mut t, Row::Output(0));
    t.key("S");
    assert!(dialog(&t).outputs[0].stimulus);
    t.type_key("D", "d");
    t.text("-30");
    let r = t.key("Enter");
    let req = r
        .iter()
        .find_map(|x| match x {
            Request::DetectLoopback(d) => Some(d.clone()),
            _ => None,
        })
        .expect("detect");
    assert_eq!(req.device, DeviceId("USB mic".into()));
    assert_eq!(req.output_device, DeviceId("Speakers".into()));
    let mut det = ac2_client::fake::fake_detection();
    det.input_device = req.device.clone();
    det.output_device = req.output_device.clone();
    t.conn(ConnEvent::LoopbackDetected(Ok(det)));
    assert_eq!(dialog(&t).inputs[0].role, InputRole::Reference);

    let r = t.key("Enter");
    let (config, _, _) = opened(&r).expect("session.open");
    assert_eq!(
        config.input_device,
        DeviceSelector::Id {
            id: DeviceId("USB mic".into())
        }
    );
    assert_eq!(
        config.output_device,
        DeviceSelector::Id {
            id: DeviceId("Speakers".into())
        }
    );
    assert_eq!(config.output_channels, 2);
    assert_eq!(config.loopback.map(|l| l.output), Some(0));
    let roles = t.st.prefs.sessions.get("cpal/USB mic").expect("remembered");
    assert_eq!(roles.output_device.as_deref(), Some("Speakers"));
    // The stimulus outputs belong to the device that plays them.
    assert_eq!(t.st.prefs.outputs_for("Speakers"), Some(&[0][..]));

    // Remembered: the next dialog comes back with the same output device.
    t.key("Escape");
    open_dialog(&mut t, endpoint_backends());
    assert_eq!(
        dialog(&t).output_info().map(|x| x.name.as_str()),
        Some("Speakers")
    );
    assert!(dialog(&t).outputs[0].stimulus);
}

/// A device with both directions (JACK, the simulated rig) plays on itself: the Output row
/// offers only that, and the session names the one device both ways.
#[test]
fn a_duplex_device_plays_on_itself() {
    use crate::session_dialog::OutputDevice;
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    assert_eq!(dialog(&t).out_device, OutputDevice::SameAsInput);
    assert_eq!(dialog(&t).output_choices(), vec![OutputDevice::SameAsInput]);
    assert_eq!(dialog(&t).clock_note(), None);
    focus(&mut t, Row::OutputDevice);
    t.key("Right");
    assert_eq!(dialog(&t).out_device, OutputDevice::SameAsInput);
    let r = t.key("Enter");
    let (config, _, _) = opened(&r).expect("session.open");
    assert_eq!(config.input_device, config.output_device);
}
