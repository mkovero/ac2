//! The harmonics plate, end to end: a sine fired as `ac2 gen sine` fires it or chosen in
//! the app, read by a spectrum of the app's making and shown on its pane's plate; gone
//! when the sine stops.

use super::new_spectrum;
use crate::harness::{Cli, Driver, R, measuring};
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::{Dbfs, Hz};
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use ac2_ui::forms::FormKind;
use ac2_ui::state::{AppState, Msg, Overlay, PaneId, PromptKind, StimPhase};
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

/// The app's own sine, with a spectrum running: the first Space asks the signal (`1k`,
/// moved onto a bin of the spectrum, which 1 kHz is not at 48 kHz), then the level; Space
/// and Enter then arm and fire. The generator plays the sine at the bin's frequency and
/// the plate reads it *on bin*; Esc stops it and the plate goes.
#[test]
fn a_sine_chosen_at_the_first_arm_is_fired_on_a_bin_of_the_running_spectrum() -> R {
    use ac2_proto::model::{MeasKind, SpecResolution, nearest_bin};
    let (_daemon, mut d) = measuring()?;
    let spectrum = new_spectrum(&mut d, false)?;
    d.key("Alt+2");
    let p = d.st.layout.focus;
    d.drop_meas(p, spectrum);
    d.until("the spectrum running", |s| {
        s.measurements()
            .iter()
            .any(|m| m.id == spectrum && m.running)
    })?;
    let fs =
        d.st.daemon()
            .and_then(|s| s.stream.open.as_ref())
            .map(|o| f64::from(o.sample_rate_hz))
            .ok_or("no stream")?;
    let running: Vec<SpecResolution> =
        d.st.measurements()
            .iter()
            .filter(|m| m.running)
            .filter_map(|m| match &m.config.kind {
                MeasKind::Spectrum { config } => Some(config.resolution),
                _ => None,
            })
            .collect();
    let n = SpecResolution::shared_sine_fft_len(running, fs).ok_or("no spectrum")?;
    let bin = nearest_bin(1000.0, fs, n);
    assert!(bin != 1000.0 && (bin - 1000.0).abs() < 3.0, "{bin}");

    // The pane focused is the transfer's: the bin is the running spectrum's all the same.
    d.key("Alt+1");
    // The noise the measurement was started with stopped, and the stimulus as an app just
    // started has it: no signal chosen, no level typed.
    d.stop()?;
    d.st.rig.stimulus.generator = None;
    d.st.rig.stimulus.level = None;
    d.key("Space");
    let rows = |d: &Driver| match &d.st.overlay {
        Overlay::Form(f) if f.kind == FormKind::StimulusSignal => Ok(f
            .fields
            .iter()
            .map(|x| format!("{}: {} {}", x.label, x.display(), x.hint))
            .collect::<Vec<_>>()),
        o => Err(format!("no signal dialog: {o:?}")),
    };
    assert_eq!(rows(&d)?, ["Signal: pink noise "]);
    // ← from pink noise is the sine, at 1 kHz on a bin of the running spectrum.
    d.key("ArrowLeft");
    assert_eq!(
        rows(&d)?,
        [
            "Signal: sine ".to_owned(),
            "Sine frequency: 1000 Hz 997, 1k, 1.5 kHz".to_owned(),
            format!(
                "On an FFT bin: on · bins of the {n}-point FFT plays 1000 Hz → {}",
                ac2_scene::format::freq_exact(bin)
            ),
        ]
    );
    d.key("Enter");
    let toast = d.st.toasts.list.last().map(|t| t.text.clone());
    assert_eq!(
        toast,
        Some(format!(
            "sine {} · on an FFT bin of the spectrum · type a level; nothing is armed without one",
            ac2_scene::format::freq_exact(bin)
        ))
    );
    match &d.st.overlay {
        Overlay::Prompt(prompt) if prompt.kind == PromptKind::StimulusLevel => {}
        o => return Err(format!("no level prompt: {o:?}").into()),
    }
    d.send(Msg::Text("-20".into()));
    d.key("Enter");
    assert_eq!(d.st.overlay, Overlay::None);
    assert_eq!(d.st.rig.stimulus.phase, StimPhase::Idle);
    d.arm()?;
    d.play()?;
    d.until("the sine playing", |s| {
        s.daemon()
            .and_then(|s| s.generator.settings.as_ref())
            .map(|s| s.signal)
            == Some(Signal::Sine { freq: Hz(bin) })
    })?;
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
