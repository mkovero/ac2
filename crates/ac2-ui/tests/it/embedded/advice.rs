//! Headroom advice from an empty daemon: a stimulus too quiet for the simulated rig's room
//! mic (its noise floor about −80 dBFS) brings RAISE STIMULUS on the transfer pane, and the
//! advice changes no level.

use super::harness::{Driver, NAME, R, first_transfer};
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::keys::CommandId;
use ac2_ui::settings::DisplayRow;
use ac2_ui::state::{AppState, Msg, Overlay, PaneKind, SettingsMsg, StimPhase};
use std::time::{Duration, Instant};

/// The transfer pane's banners now.
fn banners(s: &AppState) -> Vec<String> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    ac2_ui::scenes::transfer(
        s,
        crate::common::pane(s, PaneKind::Transfer),
        &Theme::dark(),
        size,
        now,
    )
    .banners
    .into_iter()
    .map(|b| b.text)
    .collect()
}

#[test]
fn a_quiet_stimulus_brings_raise_advice() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    // A session on the simulated rig's roles and a transfer measurement, from the app.
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the device list", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.session.device_info().is_some())
    })?;
    d.key("Enter");
    first_transfer(&mut d)?;
    // A short average's coherence swings by tenths from frame to frame at this SNR; a
    // higher threshold keeps it under for the 2 s the advice waits.
    d.send(Msg::Settings(SettingsMsg::Display(
        DisplayRow::AdviceLowCoherence,
        2,
    )));
    assert_eq!(d.st.prefs.advice.low_coherence, 0.9);
    // Pink noise at −76 dBFS: the room mic reads it 6 dB down, at its own noise floor.
    d.send(Msg::Command(CommandId::StimulusLevel));
    while matches!(&d.st.overlay, Overlay::Prompt(p) if !p.text.is_empty()) {
        d.send(Msg::Backspace);
    }
    d.send(Msg::Text("-76".into()));
    d.key("Enter");
    d.until("level", |s| {
        s.rig.stimulus.level.is_some_and(|l| l.0 == -76.0)
    })?;
    d.key("Space");
    d.until("armed", |s| s.rig.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    // The app's clock runs on its frame ticks; the advice's hold times count on it.
    let start = Instant::now();
    let shown = loop {
        d.send(Msg::Tick {
            now_s: start.elapsed().as_secs_f64(),
            dt_s: 0.02,
        });
        d.pump();
        if let Some(b) = banners(&d.st)
            .into_iter()
            .find(|b| b.starts_with("RAISE STIMULUS"))
        {
            break b;
        }
        if start.elapsed() > super::harness::DEADLINE {
            return Err(format!("no raise advice; banners {:?}", banners(&d.st)).into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(shown.starts_with("RAISE STIMULUS · about +"), "{shown}");
    // Advice only: the stimulus still plays at the level typed.
    let level =
        d.st.daemon()
            .and_then(|x| x.generator.settings.as_ref().map(|g| g.level.0));
    assert_eq!(level, Some(-76.0));
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}
