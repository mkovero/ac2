//! Reducer tests of the first-start audio setup and of devices playing on another output.

use super::*;

/// A fresh start on a daemon with no audio I/O lands on the Sessions dialog, once: its
/// New leads to the audio setup; Esc leaves a calm empty app naming the Sessions key. A
/// daemon with audio open (a rig, the kiosk) opens nothing.
#[test]
fn a_daemon_without_audio_opens_the_sessions_dialog_once() {
    let mut t = T::disconnected();
    t.st.open_session_when_empty = true;
    connected_to(&mut t, "embedded daemon (coreaudio)");
    let r = t.conn(mirror(no_session_state()));
    assert!(r.iter().any(|x| matches!(x, Request::SessionList)), "{r:?}");
    assert!(t.st.sessions_dialog().is_some(), "{:?}", t.st.overlay);
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
    // Only once: the operator closed it.
    t.conn(mirror(no_session_state()));
    assert_eq!(t.st.overlay, Overlay::None);
    let hint =
        t.st.empty_hint(t.pane(PaneKind::Transfer), &t.keys)
            .expect("hint");
    assert_eq!(hint.text, "No session open — Alt+S, then New session");

    // A daemon that already has a session: no dialog; its input meters and the channel
    // names for them.
    let mut t = T::disconnected();
    t.st.open_session_when_empty = true;
    connected_to(&mut t, "embedded daemon (coreaudio)");
    let r = t.conn(mirror(daemon_state()));
    assert!(
        matches!(r.as_slice(), [Request::Meters(true), Request::Devices]),
        "{r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(!t.st.open_session_when_empty);
}

/// A session kept from before (an autosave restored, measurements listed, its audio not
/// reopened) is no fresh start: nothing opens at connect, and a command needing the audio
/// opens the audio setup, not the Sessions dialog whose New would discard the session.
#[test]
fn a_kept_session_without_audio_opens_no_sessions_dialog() {
    let mut t = T::disconnected();
    t.st.open_session_when_empty = true;
    connected_to(&mut t, "embedded daemon (coreaudio)");
    let mut kept = daemon_state();
    kept.session.open = None;
    t.conn(mirror(kept));
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(!t.st.open_session_when_empty);
    t.key("Ctrl+M");
    assert!(t.st.sessions_dialog().is_none(), "{:?}", t.st.overlay);
    assert!(
        matches!(&t.st.overlay, Overlay::Settings(s) if s.page == crate::settings::Page::AudioIo && !s.new_session),
        "{:?}",
        t.st.overlay
    );
}

/// Core Audio listing each direction as its own device: an output-only device
/// listed first and the system's default output, an input-only USB mic, and another
/// output-only device.
fn endpoint_backends() -> Vec<BackendInfo> {
    let mut b = backends(false);
    let base = b[0].devices[0].clone();
    let endpoint = |id: &str, input: bool, output: Option<bool>| DeviceInfo {
        backend: BackendKind::CoreAudio,
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
        kind: BackendKind::CoreAudio,
        description: "Core Audio".into(),
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
/// Output row choose another output device, whose rows the Audio I/O page shows;
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
    assert!(dialog(&t).inputs[0].reference);

    let r = t.key("Enter");
    let (config, _) = opened(&r).expect("session.open");
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
    let roles =
        t.st.prefs
            .sessions
            .get("coreaudio/USB mic")
            .expect("remembered");
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
    let (config, _) = opened(&r).expect("session.open");
    assert_eq!(config.input_device, config.output_device);
}

/// An ASIO device: its clock sources are listed with the one in use marked, and P opens its
/// driver's control panel on the daemon; a device of another backend has neither.
#[test]
fn an_asio_device_lists_its_clocks_and_p_opens_its_driver_panel() {
    let b = backends(false);
    let mut dev = b[0].devices[0].clone();
    dev.backend = BackendKind::Asio;
    dev.id = DeviceId("Fireface USB".into());
    dev.name = "Fireface USB".into();
    dev.clock_sources = vec![
        ClockSource {
            name: "Internal".into(),
            current: true,
        },
        ClockSource {
            name: "Word Clock".into(),
            current: false,
        },
    ];
    let asio = BackendInfo {
        kind: BackendKind::Asio,
        description: "ASIO".into(),
        availability: Availability::Available,
        devices: vec![dev],
    };
    let mut t = T::new();
    open_dialog(&mut t, vec![asio]);
    assert_eq!(
        dialog(&t).clock_sources_text().as_deref(),
        Some("Internal (in use) · Word Clock")
    );
    focus(&mut t, Row::Device);
    let r = t.type_key("P", "p");
    let calls: Vec<_> = r
        .iter()
        .filter_map(|r| match r {
            Request::Call { what, cmd } => Some((what.as_str(), cmd)),
            _ => None,
        })
        .collect();
    assert!(
        matches!(
            calls.as_slice(),
            [(what, Command::SessionControlPanel { backend: BackendKind::Asio, device })]
                if device.0 == "Fireface USB" && *what == "driver control panel of Fireface USB"
        ),
        "{r:?}"
    );

    let mut t = T::new();
    open_dialog(&mut t, backends(false));
    assert_eq!(dialog(&t).clock_sources_text(), None);
    focus(&mut t, Row::Device);
    let r = t.type_key("P", "p");
    assert!(
        !r.iter().any(|r| matches!(r, Request::Call { .. })),
        "{r:?}"
    );
}

/// A Core Audio device: P brings up Audio MIDI Setup on the daemon's host.
#[test]
fn p_on_a_core_audio_device_opens_audio_midi_setup() {
    let b = backends(false);
    let mut dev = b[0].devices[0].clone();
    dev.backend = BackendKind::CoreAudio;
    dev.id = DeviceId("coreaudio:BlackHole2ch_UID".into());
    let ca = BackendInfo {
        kind: BackendKind::CoreAudio,
        description: "Core Audio".into(),
        availability: Availability::Available,
        devices: vec![dev],
    };
    let mut t = T::new();
    open_dialog(&mut t, vec![ca]);
    assert_eq!(
        crate::session_dialog::panel_label(BackendKind::CoreAudio).0,
        "Audio MIDI Setup… (P)"
    );
    focus(&mut t, Row::Device);
    let r = t.type_key("P", "p");
    assert!(
        r.iter().any(|r| matches!(r,
            Request::Call { what, cmd: Command::SessionControlPanel { backend: BackendKind::CoreAudio, device } }
                if device.0 == "coreaudio:BlackHole2ch_UID"
                    && what == "Audio MIDI Setup for coreaudio:BlackHole2ch_UID")),
        "{r:?}"
    );
}
