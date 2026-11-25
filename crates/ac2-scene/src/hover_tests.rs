//! Hover text reads at a glance: one line of at most 80 characters, a list at most four
//! such lines. Every hover-producing function is called here with long typical inputs (a
//! long rig name, long keys), so a new string that grows past the rule fails here first.

use ac2_proto::frame::{AudioHealth, DaemonHealth};
use ac2_proto::model::{RecordingEnd, RecordingRun, RecordingStatus};
use ac2_proto::units::{ClientId, SampleIndex, Seconds, StreamEpoch, WallNs};

use crate::health::{HealthInputs, UiFrames, health_line};
use crate::network::{ChoiceFrom, choice_tip, network_tip};
use crate::pairing::{Half, Halves, Side, Unseen};
use crate::plate::{PlateHover, PlateKind};
use crate::spectrum::{BinWidth, density_unit_help, tone_unit_help};
use crate::stim_item::{StimInputs, StimItem, StimKeys, StimState, stim_item};

const RIG: &str = "ac2 on ac2pi-0ab1c6";
const KEY: &str = "Ctrl+Shift+Esc";

fn check(what: &str, text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() <= 4, "{what}: {} lines\n{text}", lines.len());
    for l in lines {
        let n = l.chars().count();
        assert!(n <= 80, "{what}: {n} chars: {l}");
    }
}

fn recording(status: RecordingStatus) -> RecordingRun {
    RecordingRun {
        name: "take 12".into(),
        path: "/home/op/.local/share/ac2/recordings/take 12.wav".into(),
        inputs: vec![0, 1],
        sample_rate_hz: 96_000,
        stream_epoch: StreamEpoch(1),
        start_sample: SampleIndex(0),
        started_at: WallNs(0),
        started_by: ClientId("a".into()),
        frames: 96_000 * 600,
        bytes: 460_800_116,
        discontinuities: 3,
        remote_gaps: ac2_proto::model::RemoteGaps {
            spans: 2,
            frames: 960,
        },
        max_duration: Seconds(3600.0),
        max_bytes: Some(4_000_000_000),
        status,
    }
}

#[test]
fn every_hover_is_short() {
    let mut tips: Vec<(String, String)> = vec![
        ("stop".into(), crate::chrome::stop_tip(KEY)),
        ("keys".into(), crate::chrome::keys_tip(KEY)),
        ("rig menu".into(), crate::chrome::RIG_MENU_TIP.into()),
        ("level".into(), crate::stimulus::level_tip(KEY)),
        ("stop all".into(), crate::stimulus::stop_every_rig_tip(KEY)),
        ("meas row".into(), crate::meas_list::meas_row_tip(Some(KEY))),
        (
            "meas row, no key".into(),
            crate::meas_list::meas_row_tip(None),
        ),
        ("trace row".into(), crate::meas_list::TRACE_ROW_TIP.into()),
        ("edit".into(), crate::meas_list::edit_tip(Some(KEY))),
        ("replay".into(), crate::recording::REPLAY_DETAIL.into()),
        ("density".into(), density_unit_help()),
        ("multi-res".into(), tone_unit_help(BinWidth::MultiRes)),
        ("per bin".into(), tone_unit_help(BinWidth::Hz(1.4648))),
        ("network fixed".into(), crate::network::FIXED_TIP.into()),
        (
            "network embedded".into(),
            crate::network::EMBEDDED_TIP.into(),
        ),
        (
            "network no key".into(),
            crate::network::NO_KEY_STORE_TIP.into(),
        ),
        ("network peer".into(), crate::network::PEER_TIP.into()),
        (
            "inputs".into(),
            crate::programme::INPUTS_LOUDNESS_TIP.into(),
        ),
    ];
    for on in [false, true] {
        tips.push((format!("network {on}"), network_tip(on).into()));
    }
    for from in [
        ChoiceFrom::InForce,
        ChoiceFrom::Picked,
        ChoiceFrom::Kept,
        ChoiceFrom::Default,
    ] {
        tips.push((format!("{from:?}"), choice_tip(from).into()));
    }
    for kind in PlateKind::ALL {
        for grip in [false, true] {
            tips.push((
                format!("plate {kind:?}"),
                crate::plate::hint(PlateHover { kind, grip }),
            ));
        }
    }

    // A pairing at its fullest: a half that cannot be told, the clocks, the key.
    let unknown = Half::Unknown {
        side: Side::There,
        why: Unseen::NotHeld,
    };
    let h = Halves {
        here_lists: Half::Yes,
        there_admits: unknown,
        there_lists: unknown,
        here_admits: Half::No,
    };
    let clocks = "clocks agree (+0.12 ppm over 60 s) · from the rigs' level frames";
    tips.push((
        "pairing".into(),
        crate::pairing::tip(RIG, &h, Some(clocks), true),
    ));

    // The health meter with a remote daemon and every figure.
    let d = DaemonHealth {
        host: "ac2pi-0ab1c6".into(),
        cpu: Some(1.0),
        audio: Some(AudioHealth {
            load: Some(1.0),
            peak: Some(2.0),
            stream_load: Some(0.99),
            stream_peak: Some(2.5),
            xruns: 12_345,
        }),
    };
    let health = health_line(&HealthInputs {
        daemon: Some(&d),
        local_host: "ketunkolo",
        daemon_addr: Some("192.168.100.133"),
        local_cpu: Some(1.0),
        received_per_s: Some(1.8e6),
        frames: Some(UiFrames {
            fps: 144.0,
            longest_gap_s: 0.25,
            streaming: true,
            wanted_fps: 144.0,
        }),
        since_xrun_s: None,
        rate_unit: None,
    })
    .expect("figures");
    tips.push(("health".into(), health.detail));

    // A recording that lost audio, while it runs and once it ended badly.
    let name = |i: u16| ["FOH L measurement mic", "FOH R measurement mic"][usize::from(i)].into();
    for status in [
        RecordingStatus::Recording,
        RecordingStatus::Ended {
            reason: RecordingEnd::AudioStopped,
        },
        RecordingStatus::Ended {
            reason: RecordingEnd::Interrupted,
        },
    ] {
        let l = crate::recording::recording_label(&recording(status), name);
        tips.push(("recording".into(), l.detail));
    }

    // The stimulus item, every state, at its narrowest too.
    for state in [StimState::Off, StimState::Armed, StimState::Firing] {
        let level = crate::stimulus::level_tip(KEY);
        let item = stim_item(&StimInputs {
            state,
            plays: "sine 999.76 Hz −20.0 dBFS → out 1 Genelec L, 2 Genelec R",
            held_by: Some("alice"),
            off_keys: Some("Space arms"),
            keys: StimKeys::None,
            rig: Some(RIG),
            next_long: None,
            tip: &level,
        });
        tips.push((format!("stim {state:?}"), item.hover.clone()));
        tips.push((
            format!("stim {state:?} narrow"),
            item.narrowed(StimItem::STEPS - 1).hover,
        ));
    }

    for (what, text) in &tips {
        check(what, text);
    }
}
