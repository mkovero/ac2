use super::*;
use crate::health::{HealthInputs, UiFrames, health_line};
use ac2_proto::frame::{AudioHealth, DaemonHealth};

fn session_line(changed: bool) -> SessionLine {
    SessionLine {
        full: "Fireface USB · 96 kHz · 256 frames".into(),
        short: "96 kHz · 256 frames".into(),
        change: changed.then(|| "driver changed from 48 kHz".to_owned()),
        changed,
        detail: changed.then(|| "the driver changed the rate to 96 kHz (was 48 kHz)".to_owned()),
    }
}

/// The meter of a daemon on `host` seen from `ketunkolo`.
fn health(host: &str) -> HealthLine {
    let d = DaemonHealth {
        host: host.into(),
        cpu: Some(0.43),
        audio: Some(AudioHealth {
            load: Some(0.0),
            peak: None,
            session_load: None,
            session_peak: None,
            xruns: 0,
        }),
    };
    health_line(&HealthInputs {
        daemon: Some(&d),
        local_host: "ketunkolo",
        daemon_addr: None,
        local_cpu: None,
        received_per_s: None,
        frames: Some(UiFrames {
            fps: 85.0,
            longest_gap_s: 0.02,
            streaming: true,
            wanted_fps: 60.0,
        }),
        since_xrun_s: None,
        rate_unit: None,
    })
    .expect("figures")
}

const SAVED: BarSession<'static> = BarSession {
    text: "untitled · unsaved changes",
    unsaved: true,
    detail: "Not saved yet.",
};

/// pupu's daemon on host `ac2pi-0ab1c6`, link fine, a session open and nothing playing.
fn remote<'a>(line: &'a SessionLine) -> BarInputs<'a> {
    BarInputs {
        rig: "pupu",
        place: Place::Remote {
            host: "ac2pi-0ab1c6",
        },
        target: "pupu (192.168.9.27:7700)",
        server: Some("ac2d 0.9.1 (build 85507b49)"),
        link: BarLink::Connected {
            responding: true,
            synced: true,
        },
        audio: Some(BarAudio::Open {
            device: "Fireface USB",
            line,
        }),
        health: Some(health("ac2pi-0ab1c6")),
        session: Some(SAVED),
        notes: Vec::new(),
    }
}

/// A host is shown by the name its advert carries, without the mDNS suffix; the address
/// typed only while no advert was seen for the rig.
#[test]
fn a_host_reads_by_its_mdns_name_without_the_suffix() {
    assert_eq!(host_name("192.168.9.27", Some("pupu.local")), "pupu");
    assert_eq!(
        host_name("192.168.9.27", Some("ac2pi-0ab1c6.local.")),
        "ac2pi-0ab1c6"
    );
    assert_eq!(host_name("192.168.9.27", Some("FOH.LOCAL")), "FOH");
    assert_eq!(host_name("192.168.9.27", None), "192.168.9.27");
    // A name typed with the suffix reads as the advert's would.
    assert_eq!(host_name("pupu.local", None), "pupu");
    assert_eq!(host_name("rig.example.org", None), "rig.example.org");
    assert_eq!(host_name("fe80::1", None), "fe80::1");
    // Nothing but the suffix is no name to strip to.
    assert_eq!(host_name(".local", None), ".local");
}

/// The line of a remote rig in good health: where the daemon runs, the audio settings, the meter, then the session after a rule. The link's dot is on the rig.
#[test]
fn a_remote_rig_in_good_health() {
    let l = session_line(false);
    let b = bar(&remote(&l));
    assert_eq!(b.rig, "pupu");
    assert_eq!(b.tone, LinkTone::Ok);
    assert_eq!(
        b.text(),
        "remote: ac2pi-0ab1c6 · 96 kHz · 256 frames · CPU 43 % · DSP 0 % · \
         0 xruns · 85 fps | untitled · unsaved changes"
    );
    // The daemon's software and the numeric address are a hover away, never in the line.
    let place = &b.line[0];
    assert_eq!(
        place.hover.as_deref(),
        Some(
            "ac2d 0.9.1 (build 85507b49) · pupu (192.168.9.27:7700) · click: Settings › \
             Connection"
        )
    );
    assert_eq!(place.click, Some(Click::Connection));
    assert!(!b.text().contains("ac2d") && !b.text().contains("192.168"));
    let session = b.line.last().expect("the session");
    assert_eq!((session.lead, session.tone), (Lead::Rule, Tone::Warn));
    assert_eq!(session.click, Some(Click::Sessions));
    // The audio part carries the device and the meter its spans.
    assert_eq!(b.line[1].hover.as_deref(), Some("Fireface USB"));
    assert!(b.line[2].figures.is_some());
}

/// A host named as the rig is not said twice.
#[test]
fn a_host_called_as_its_rig_is_only_remote() {
    let l = session_line(false);
    let mut i = remote(&l);
    i.place = Place::Remote { host: "Pupu" };
    i.health = None;
    i.session = None;
    assert_eq!(bar(&i).text(), "remote · 96 kHz · 256 frames");
    i.place = Place::Remote { host: "pupu-2" };
    assert_eq!(bar(&i).text(), "remote: pupu-2 · 96 kHz · 256 frames");
}

/// The daemon the app started beside itself is `local`, with one CPU figure.
#[test]
fn a_local_daemon() {
    let l = session_line(false);
    let mut i = remote(&l);
    i.rig = "ketunkolo";
    i.place = Place::Local;
    i.target = "local daemon";
    i.health = Some(health("ketunkolo"));
    i.session = Some(BarSession {
        text: "friday",
        unsaved: false,
        detail: "Saved 2 min ago.",
    });
    let b = bar(&i);
    assert_eq!(
        b.text(),
        "local · 96 kHz · 256 frames · CPU 43 % · DSP 0 % · 0 xruns · 85 fps | friday"
    );
    assert_eq!(b.line.last().map(|p| p.tone), Some(Tone::Plain));
}

/// The stimulus is never on the scrolling line: it is fixed at the bar's right end.
#[test]
fn the_stimulus_is_not_on_the_line() {
    let l = session_line(false);
    let b = bar(&remote(&l));
    assert!(!b.text().contains("STIM"), "{}", b.text());
}

/// Link trouble takes the daemon's segment over, in its tone; the session stays.
#[test]
fn link_trouble_takes_the_segment_over() {
    let l = session_line(false);
    let mut i = remote(&l);
    let mut says = |link| {
        i.link = link;
        let b = bar(&i);
        assert_eq!(b.line[0].click, Some(Click::Connection));
        (b.tone, b.text())
    };
    let after = " | untitled · unsaved changes";
    assert_eq!(
        says(BarLink::Connected {
            responding: false,
            synced: true
        }),
        (
            LinkTone::Fault,
            format!("remote: ac2pi-0ab1c6 · not responding{after}")
        )
    );
    assert_eq!(
        says(BarLink::Connected {
            responding: true,
            synced: false
        }),
        (
            LinkTone::Warn,
            format!("remote: ac2pi-0ab1c6 · syncing{after}")
        )
    );
    assert_eq!(
        says(BarLink::Connecting),
        (
            LinkTone::Warn,
            format!("connecting to ac2pi-0ab1c6…{after}")
        )
    );
    assert_eq!(
        says(BarLink::Failed {
            error: "connection refused",
            down: RigDown::Unreachable
        }),
        (
            LinkTone::Fault,
            format!("remote: ac2pi-0ab1c6 · connection refused · retrying{after}")
        )
    );
    // Turned off on purpose: said so, and not in the fault colour.
    assert_eq!(
        says(BarLink::Failed {
            error: "not responding",
            down: RigDown::NetworkOff
        }),
        (
            LinkTone::Warn,
            format!("remote: ac2pi-0ab1c6 · turned its network off · retrying{after}")
        )
    );
    assert_eq!(
        says(BarLink::Failed {
            error: "the daemon refused this app's key",
            down: RigDown::NotAuthorized
        }),
        (
            LinkTone::Fault,
            format!("remote: ac2pi-0ab1c6 · has not authorized this app{after}")
        )
    );
    // What the daemon said is in the hover, before the address.
    let b = bar(&i);
    assert!(
        b.line[0]
            .hover
            .as_deref()
            .is_some_and(|h| h.starts_with("the daemon refused this app's key\nac2d 0.9.1")),
        "{:?}",
        b.line[0].hover
    );
    // Before the first hello there is no mirrored state: the trouble alone.
    i.session = None;
    i.server = None;
    i.place = Place::Local;
    i.link = BarLink::Connecting;
    assert_eq!(bar(&i).text(), "connecting to the local daemon…");
}

/// No audio I/O: what is missing and its key, where the rate would be; the meter stays.
/// A stopped stream, a driver's change and a transfer function stopped across rigs each
/// keep their colour.
#[test]
fn the_audio_part_says_what_is_wrong_in_its_tone() {
    let l = session_line(false);
    let mut i = remote(&l);
    i.session = None;
    i.health = None;
    i.audio = Some(BarAudio::None {
        text: "no session · Shift+S, then New session",
    });
    let b = bar(&i);
    assert_eq!(
        b.text(),
        "remote: ac2pi-0ab1c6 · no session · Shift+S, then New session"
    );
    assert_eq!(b.line[1].tone, Tone::Dim);

    i.audio = Some(BarAudio::Stopped {
        text: "audio stopped · reopening (attempt 3, next in 8 s)",
        hover: "AUDIO STOPPED · device not delivering since 20:36",
    });
    let b = bar(&i);
    assert_eq!(
        b.text(),
        "remote: ac2pi-0ab1c6 · audio stopped · reopening (attempt 3, next in 8 s)"
    );
    assert_eq!(b.line[1].tone, Tone::Fault);

    let changed = session_line(true);
    i.audio = Some(BarAudio::Open {
        device: "Fireface USB",
        line: &changed,
    });
    let b = bar(&i);
    assert_eq!(
        b.text(),
        "remote: ac2pi-0ab1c6 · 96 kHz · 256 frames · driver changed from 48 kHz"
    );
    assert_eq!(b.line[1].tone, Tone::Warn);
    assert_eq!(
        b.line[1].hover.as_deref(),
        Some("Fireface USB · the driver changed the rate to 96 kHz (was 48 kHz)")
    );
}

/// A rig reached at a typed address reads by that address until an advert is seen for it.
#[test]
fn a_typed_address_until_an_advert_is_seen() {
    let l = session_line(false);
    let mut i = remote(&l);
    i.health = None;
    i.session = None;
    let typed = host_name("192.168.9.27", None);
    i.place = Place::Remote { host: &typed };
    assert_eq!(bar(&i).text(), "remote: 192.168.9.27 · 96 kHz · 256 frames");
    let seen = host_name("192.168.9.27", Some("ac2pi-0ab1c6.local"));
    i.place = Place::Remote { host: &seen };
    assert_eq!(bar(&i).text(), "remote: ac2pi-0ab1c6 · 96 kHz · 256 frames");
}

/// What the line says after the session: each note in its tone.
#[test]
fn notes_follow_the_session() {
    let l = session_line(false);
    let mut i = remote(&l);
    i.health = None;
    i.notes = vec![
        Note {
            text: "REC 1:23".into(),
            tone: Tone::Fault,
            hover: None,
        },
        Note {
            text: "PLAY replay.wav".into(),
            tone: Tone::Dim,
            hover: None,
        },
    ];
    let b = bar(&i);
    assert!(
        b.text()
            .ends_with(" | untitled · unsaved changes · REC 1:23 · PLAY replay.wav"),
        "{}",
        b.text()
    );
    assert_eq!(b.line[b.line.len() - 2].tone, Tone::Fault);
}
