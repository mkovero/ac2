//! The harmonics plate, end to end: a sine fired as `ac2 gen sine` fires it or chosen in
//! the app, read by a spectrum of the app's making and shown on its pane's plate; gone
//! when the sine stops.

use super::new_spectrum;
use crate::harness::{Cli, R, measuring};
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::{Dbfs, Hz};
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, PaneId, PromptKind};
use std::time::Instant;

const SIZE: Viewport = Viewport::new(1600.0, 900.0);

/// The harmonics plate's text on pane `p`.
fn plate(s: &AppState, p: PaneId) -> Option<String> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let sc = ac2_ui::scenes::spectrum(s, p, &Theme::dark(), SIZE, now);
    // The plate is one of the pane's, to drag like the others.
    assert_eq!(
        sc.harmonic_list.is_some(),
        sc.plates()
            .iter()
            .any(|b| b.kind == ac2_scene::plate::PlateKind::Harmonics)
    );
    sc.harmonic_list
}

/// From an empty daemon, a spectrum on its own pane and a sine from another client
/// through the simulated room (no real audio). 1.5 kHz is a bin centre of every
/// power-of-two FFT at 48 kHz, so the plate says *on bin* and reads a fundamental.
#[test]
fn a_fired_sine_shows_the_harmonics_plate_until_it_stops() -> R {
    let (daemon, mut d) = measuring()?;
    let spectrum = new_spectrum(&mut d, false)?;
    d.key("Alt+2");
    let p = d.st.layout.focus;
    d.drop_meas(p, spectrum);
    assert_eq!(plate(&d.st, p), None, "no sine, no plate");
    let cli = Cli::connect(daemon.endpoints())?;
    let lease = cli.lease(GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1500.0) },
            level: Dbfs(-20.0),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    })?;
    d.until("the sine read on the plate", |s| {
        plate(s, p).is_some_and(|t| t.lines().any(|l| l.starts_with("H1  ")))
    })?;
    let text = plate(&d.st, p).unwrap_or_default();
    assert!(text.starts_with("sine 1500 Hz · on bin\n"), "{text}");
    drop(lease);
    d.until("the plate gone with the sine", |s| plate(s, p).is_none())?;
    Ok(())
}

/// The app's own sine: on the spectrum's pane, a level, `1k` in the signal prompt (moved
/// onto a bin of that spectrum, which 1 kHz is not at 48 kHz), Space and Enter. The
/// generator plays the sine at the bin's frequency and the plate reads it *on bin*; Esc
/// stops it and the plate goes.
#[test]
fn a_sine_chosen_in_the_app_is_fired_on_a_bin_of_the_focused_spectrum() -> R {
    let (_daemon, mut d) = measuring()?;
    let spectrum = new_spectrum(&mut d, false)?;
    d.key("Alt+2");
    let p = d.st.layout.focus;
    d.drop_meas(p, spectrum);
    let (resolution, name) =
        d.st.measurements()
            .iter()
            .find(|m| m.id == spectrum)
            .and_then(|m| match &m.config.kind {
                ac2_proto::model::MeasKind::Spectrum { config } => {
                    Some((config.resolution, m.config.name.clone()))
                }
                _ => None,
            })
            .ok_or("no spectrum")?;
    let fs =
        d.st.daemon()
            .and_then(|s| s.session.open.as_ref())
            .map(|o| f64::from(o.sample_rate_hz))
            .ok_or("no session")?;
    let bin = resolution.nearest_bin(1000.0, fs);
    assert!(bin != 1000.0 && (bin - 1000.0).abs() < 3.0, "{bin}");

    d.send(Msg::Command(CommandId::StimulusLevel));
    d.send(Msg::Text("-20".into()));
    d.key("Enter");
    d.send(Msg::Command(CommandId::StimulusSignal));
    match &mut d.st.overlay {
        Overlay::Prompt(prompt) if prompt.kind == PromptKind::StimulusSignal => {
            assert_eq!(prompt.text, "pink");
            prompt.text = "1k".into();
        }
        o => return Err(format!("no signal prompt: {o:?}").into()),
    }
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    let toast = d.st.toasts.list.last().map(|t| t.text.clone());
    assert_eq!(
        toast,
        Some(format!(
            "sine {} · on a bin of {name}",
            ac2_scene::format::freq_exact(bin)
        ))
    );
    d.arm()?;
    d.play()?;
    let playing =
        d.st.daemon()
            .and_then(|s| s.generator.settings.as_ref())
            .map(|s| s.signal);
    assert_eq!(playing, Some(Signal::Sine { freq: Hz(bin) }));
    d.until("the sine read on the plate", |s| {
        plate(s, p).is_some_and(|t| t.lines().any(|l| l.starts_with("H1  ")))
    })?;
    let text = plate(&d.st, p).unwrap_or_default();
    let head = format!("sine {} · on bin\n", ac2_scene::format::freq_exact(bin));
    assert!(text.starts_with(&head), "{text}");
    d.stop()?;
    d.until("the plate gone with the sine", |s| plate(s, p).is_none())?;
    Ok(())
}
