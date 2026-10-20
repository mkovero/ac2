//! Reducer tests of the audio I/O and measurement dialogs, the meters and recording.

use super::*;

fn real_backends() -> Vec<BackendInfo> {
    let mut b = backends(false);
    b.push(BackendInfo {
        kind: BackendKind::CoreAudio,
        description: "Core Audio".into(),
        availability: Availability::Available,
        devices: vec![
            DeviceInfo {
                backend: BackendKind::CoreAudio,
                id: DeviceId("card".into()),
                name: "card".into(),
                ..b[0].devices[0].clone()
            },
            DeviceInfo {
                backend: BackendKind::CoreAudio,
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
    let meta = captured(10, Some(1), 2);
    // Main L, kept from an earlier session: the pane draws its capture.
    let mut s = no_session_state();
    s.measurements = vec![meas(1, "Main L", transfer())];
    s.traces = vec![meta.clone()];
    t.conn(mirror(s));
    let place = |t: &T| {
        t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys)
            .map(|h| h.place)
    };
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
            ir: None,
        })
    };
    t.conn(ConnEvent::Trace(data(&meta), grid.clone()));
    assert!(t.st.pane_shows_stored(t.pane(PaneKind::Transfer)));
    let hint =
        t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys)
            .expect("hint");
    assert_eq!(hint.place, HintPlace::Title);
    assert!(hint.text.starts_with("No audio I/O — "), "{}", hint.text);
    // Hidden: the plot is empty again and the hint goes back to its centre.
    let mut hidden = meta.clone();
    hidden.edit.visible = false;
    let mut s = no_session_state();
    s.measurements = vec![meas(1, "Main L", transfer())];
    s.traces = vec![hidden.clone()];
    t.conn(mirror(s));
    t.conn(ConnEvent::Trace(data(&hidden), grid));
    assert_eq!(place(&t), Some(HintPlace::Centre));
}

#[test]
fn empty_hints_guide_to_a_session_then_a_measurement() {
    let mut t = T::disconnected();
    assert_eq!(t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys), None);
    connected_to(&mut t, "local daemon");
    // Connected but not synced: nothing to say yet.
    assert_eq!(t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys), None);
    t.conn(mirror(no_session_state()));
    assert_eq!(
        t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys)
            .map(|h| h.text)
            .as_deref(),
        Some("No session open — Alt+S, then New session")
    );
    assert_eq!(
        t.st.no_audio_line(&t.keys),
        "no session · Alt+S, then New session"
    );
    // Measurements kept without audio: the audio is what is missing.
    let mut s = no_session_state();
    s.measurements = vec![meas(1, "Main L", transfer())];
    t.conn(mirror(s));
    assert_eq!(
        t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys)
            .map(|h| h.text)
            .as_deref(),
        Some(format!(
            "No audio I/O — press {} (or {} → Open audio I/O)",
            open_key(),
            palette_key()
        ))
        .as_deref()
    );
    assert_eq!(
        t.st.no_audio_line(&t.keys),
        format!("no audio I/O · {} opens it", open_key())
    );
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    let hint =
        t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys)
            .map(|h| h.text)
            .unwrap_or_default();
    assert!(
        hint.starts_with("No measurements — Ctrl+M → New measurement…"),
        "{hint}"
    );
    t.conn(mirror(daemon_state()));
    assert_eq!(t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys), None);
    // The hint follows the keymap: the palette entry when Alt+S is unbound.
    let keys = Keymap::from_toml("[global]\nsessions = []").expect("keys");
    t.conn(mirror(no_session_state()));
    assert_eq!(
        t.st.empty_hint(t.pane(PaneKind::Transfer), &keys)
            .map(|h| h.text)
            .as_deref(),
        Some(format!(
            "No session open — Sessions… in {}, then New session",
            palette_key()
        ))
        .as_deref()
    );
}

/// Every key a hint or a refusal names is the one the keymap binds: rebound, the text
/// follows, and no default chord is left behind.
#[test]
fn hints_name_the_keys_the_keymap_binds() {
    let keys = Keymap::from_toml(
        "[global]\nmeas_new = \"F5\"\nsplit_pane = \"F6\"\nclose_pane = \"F7\"\n\
         next_trace = \"F8\"\nstimulus_level = \"F9\"\n",
    )
    .expect("keys");
    let mut t = T::new();
    t.keys = keys;
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    let hint =
        t.st.empty_hint(t.st.layout.focus, &t.keys)
            .map(|h| h.text)
            .unwrap_or_default();
    assert!(
        hint.starts_with("No measurements — F5 → New measurement…"),
        "{hint}"
    );
    // Q, as rebound, down to the last pane: it stays, and says what brings more.
    let panes = t.st.layout.views.len();
    for _ in 0..panes {
        t.key("F7");
    }
    assert_eq!(t.st.layout.views.len(), 1);
    assert_eq!(
        t.last_toast(),
        "the last pane stays: F6 splits it · F5: New measurement…"
    );
    t.key("Alt+9");
    assert_eq!(t.last_toast(), "no pane 9: 1 on screen (F6 splits)");
    t.st.stimulus.level = None;
    t.key("Up");
    assert_eq!(t.last_toast(), "no stimulus level yet: type one (F9)");
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
            "press {} (or {} → Open audio I/O)",
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
    let (config, inputs) = opened(&r).expect("session.open");
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
    let (config, inputs) = opened(&r).expect("open");
    assert_eq!(config.loopback.map(|l| l.input), Some(2));
    assert_eq!(config.input_channels, vec![0, 1, 2]);
    assert_eq!(inputs[1].mic.as_deref(), Some("ECM"));
    assert_eq!(inputs[2].mic, None);
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
    let (config, _) = opened(&r).expect("open");
    assert_eq!(
        config.loopback,
        Some(LoopbackRoute {
            output: 1,
            input: 3
        })
    );
}

#[test]
fn a_real_backend_is_preferred_and_an_unavailable_one_says_why() {
    let mut t = T::new();
    let r = open_dialog(&mut t, real_backends());
    assert_eq!(dialog(&t).backend_kind(), Some(BackendKind::CoreAudio));
    assert_eq!(previewed(&r), Some(&DeviceId("card".into())));
    // A real interface starts without roles: its wiring is the operator's to say.
    assert!(dialog(&t).inputs.iter().all(|i| i.role == InputRole::None));
    // → on the device row (where Shift+O lands) previews the other device.
    assert_eq!(dialog(&t).focus, Row::Device);
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
fn opening_on_an_empty_daemon_asks_what_to_measure() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    let r = t.key("Enter");
    assert!(opened(&r).is_some(), "{r:?}");
    // The reply can come before the state shows the session: the chooser waits for it, as
    // which kinds can be made is read from that state.
    t.conn(ConnEvent::SessionOpened);
    assert_eq!(t.st.overlay, Overlay::None);
    let mut s = daemon_state();
    s.measurements.clear();
    t.conn(mirror(s));
    assert!(
        matches!(&t.st.overlay, Overlay::Choose(c) if c.purpose == ChoicePurpose::NewMeasurement),
        "{:?}",
        t.st.overlay
    );
    // Esc leaves the empty state as it was, nothing created.
    let r = t.key("Escape");
    assert!(
        !r.iter().any(|x| matches!(x, Request::CreateMeas { .. })),
        "{r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
    // A daemon that has measurements by then asks nothing.
    t.conn(mirror(daemon_state()));
    t.conn(ConnEvent::SessionOpened);
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

#[path = "state_session_meters_tests.rs"]
mod meters;

#[path = "state_session_devices_tests.rs"]
mod devices;
