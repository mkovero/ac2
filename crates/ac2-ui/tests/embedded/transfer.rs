//! Transfer measurements from the app: starting, stopping, the IR pane and delay changes.

use crate::harness::{Driver, NAME, R, measure_from_empty, sweep_from_the_dialog};
use ac2_proto::FrameData;
use ac2_proto::model::MeasKind;
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded, start_embedded_with};
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay, StimPhase};
use std::time::{Duration, Instant};

/// An open window owns the keyboard: from an empty daemon, with the noise playing, the help
/// and the input setup take ↑/↓, the page keys and Esc and the noise plays on at its level;
/// the stop chord stops it from inside a window.
#[test]
fn windows_leave_the_stimulus_alone_and_the_stop_chord_stops_from_one() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    // The level typed there stays: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    let level = d.st.stimulus.level;
    let playing = |s: &AppState| {
        s.daemon()
            .is_some_and(|x| x.generator.firing && x.generator.armed)
    };

    d.key("H");
    assert_eq!(d.st.overlay, Overlay::Help);
    for k in [
        "ArrowDown",
        "ArrowDown",
        "PageDown",
        "ArrowUp",
        "End",
        "Home",
    ] {
        d.key(k);
    }
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);

    d.key("Ctrl+K");
    d.send(Msg::Text("input setup".into()));
    d.key("Enter");
    assert!(
        d.st.overlay
            .settings()
            .is_some_and(|s| s.page == ac2_ui::settings::Page::Io),
        "{:?}",
        d.st.overlay
    );
    for k in ["ArrowDown", "ArrowUp", "PageDown", "Home", "Shift+ArrowUp"] {
        d.key(k);
    }
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);

    // A while later the daemon still plays, at the level it was given.
    let end = Instant::now() + Duration::from_millis(600);
    while Instant::now() < end {
        d.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(playing(&d.st), "the windows touched the stimulus");
    assert_eq!(d.st.stimulus.level, level);
    assert_eq!(d.st.stimulus.phase, StimPhase::Firing);

    // The stop chord from inside a window: stopped, the window still open.
    d.key("H");
    d.key("Shift+Escape");
    assert_eq!(d.st.overlay, Overlay::Help);
    d.until("stopped by the stop chord", |s| {
        s.daemon()
            .is_some_and(|x| !x.generator.firing && !x.generator.armed)
    })?;
    drop(d);
    drop(daemon);
    Ok(())
}

#[test]
fn simulated_rig_starts_measuring() -> R {
    let daemon = start_embedded(EmbeddedBackend::Fake)?;
    assert_eq!(daemon.describe(), "embedded daemon (fake rig)");
    let config = daemon.client_config(NAME);
    // In-process: inproc endpoints in the daemon's own context.
    assert!(config.endpoints.ctrl.starts_with("inproc://"), "{config:?}");
    assert!(config.context.is_some());

    let mut d = Driver::connect(config, &daemon.describe())?;
    d.synced()?;
    // Session open as the rig is wired, "demo" running and selected: nothing to set up.
    let open = d.st.open_session().cloned().ok_or("no session")?;
    assert_eq!(open.config.input_channels, vec![0, 1]);
    assert_eq!(open.config.output_channels, 1);
    assert_eq!(
        open.config.loopback.map(|l| (l.output, l.input)),
        Some((0, 0))
    );
    let m = d.st.selected_meas().cloned().ok_or("nothing selected")?;
    assert_eq!(m.config.name, "demo");
    assert!(m.running);
    assert!(matches!(
        &m.config.kind,
        MeasKind::Transfer { config } if (config.reference_input, config.measurement_input) == (0, 1)
    ));
    assert_eq!(d.st.empty_hint(&d.keys), None);

    d.fire()?;
    d.tf_frames(m.id, 240)?;
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

#[test]
fn empty_embedded_daemon_measures_from_the_app() -> R {
    // As on real audio (no setup), but on the simulated rig.
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The transfer pane's NO REFERENCE detail, while it shows.
fn no_reference(s: &AppState) -> Option<String> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    ac2_ui::scenes::transfer(s, &Theme::dark(), size, now)
        .banners
        .into_iter()
        .find(|b| b.text == "NO REFERENCE")
        .and_then(|b| b.detail)
}

/// From an empty daemon: with the transfer measurement running and nothing playing, NO
/// REFERENCE names the keys (Space arms, then Enter plays); with the noise playing for the
/// only transfer measurement, S stops
/// the measurement and the stimulus with it (faded and released as Esc does), and one toast
/// says both.
#[test]
fn stopping_the_last_transfer_stops_the_noise_from_the_app() -> R {
    use ac2_ui::state::PaneKind;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    assert_eq!(d.st.layout.focus, PaneKind::Transfer);

    // Running with nothing playing: NO REFERENCE says which keys start the noise.
    d.until("the off reminder", |s| {
        no_reference(s).as_deref() == Some("stimulus off: Space arms, Enter starts it")
    })?;
    d.key("Space");
    d.until("armed and its reminder", |s| {
        s.stimulus.phase == StimPhase::Armed
            && no_reference(s).as_deref() == Some("stimulus armed: Enter starts it")
    })?;
    d.key("Enter");
    d.until("pink playing", |s| {
        s.daemon().is_some_and(|x| {
            x.generator.firing
                && x.generator.settings.as_ref().map(|g| g.signal)
                    == Some(ac2_proto::model::Signal::Pink)
        })
    })?;
    // Only what this stop says.
    d.st.toasts.clear();
    d.key("S");
    d.until("the measurement and the noise stopped", |s| {
        s.stimulus.phase == StimPhase::Idle
            && s.meas(m.id).is_some_and(|x| !x.running)
            && s.daemon().is_some_and(|x| {
                !x.generator.firing && !x.generator.armed && x.generator.owner.is_none()
            })
    })?;
    let stopped = format!(
        "{} stopped · stimulus stopped (no transfer measurement left running)",
        m.config.name
    );
    d.until("the toast", |s| s.toasts.iter().any(|t| t.text == stopped))?;
    assert!(
        !d.st.toasts.iter().any(|t| t.text == "stimulus stopped"),
        "said twice"
    );
    drop(d);
    drop(daemon);
    Ok(())
}

/// The IR pane from an empty daemon: the wheel zooms its time axis about the pointer, a drag
/// pans it, Ctrl+wheel zooms the amplitude, a click puts the cursor on the arrival and the
/// readout reads that sample of the IR as drawn.
#[test]
fn the_ir_pane_zooms_pans_and_reads_from_the_mouse() -> R {
    use ac2_scene::view::IrPane;
    use ac2_ui::state::IrNavMsg;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Alt+3");
    // The level typed before is kept: Space arms, Enter fires.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    // The rig's −6 dB path: an arrival of about 0.5 FS.
    d.until("an IR with its arrival", |s| {
        s.ir_frame_of(IrPane::Live)
            .is_some_and(|f| f.linear.iter().map(|x| x.abs()).fold(0.0, f32::max) > 0.3)
    })?;
    d.stop()?;
    let full = d.st.ir_extent(IrPane::Live).ok_or("extent")?;
    // Wheel up (about four notches) with the pointer on t = 0.
    d.send(Msg::IrNav(
        IrPane::Live,
        IrNavMsg::Zoom {
            about_ms: 0.0,
            factor: 4.0,
        },
    ));
    let r = d.st.view.ir.axes.time_ms.ok_or("zoomed")?;
    assert!((r.span() - full.full.span() / 4.0).abs() < 1e-9, "{r:?}");
    let frac = |r: ac2_scene::axis::Range| -r.lo / r.span();
    assert!((frac(r) - frac(full.full)).abs() < 1e-9, "t = 0 stays put");
    // A drag of a tenth of the width to the right shows earlier times.
    d.send(Msg::IrNav(
        IrPane::Live,
        IrNavMsg::Pan {
            ms: -r.span() / 10.0,
        },
    ));
    let p = d.st.view.ir.axes.time_ms.ok_or("panned")?;
    assert!((p.lo - (r.lo - r.span() / 10.0)).abs() < 1e-9, "{p:?}");
    d.send(Msg::IrNav(
        IrPane::Live,
        IrNavMsg::ValueZoom {
            about: Some(0.0),
            factor: 2.0,
        },
    ));
    let a = d.st.view.ir.axes.amplitude.ok_or("amplitude")?;
    assert!((a.lo + a.hi).abs() < 1e-9, "zoomed about 0: {a:?}");
    // A click at the arrival: the cursor on its sample, the readout that sample's value.
    d.send(Msg::IrNav(IrPane::Live, IrNavMsg::Cursor { t_ms: 0.0 }));
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    let s = ac2_ui::scenes::ir(&d.st, &d.keys, &Theme::dark(), size, now).ok_or("scene")?;
    let cur = s.cursor.ok_or("no cursor")?;
    assert!(cur.t_ms.abs() <= full.dt_ms / 2.0 + 1e-9, "{cur:?}");
    let f = d.st.ir_frame_of(IrPane::Live).ok_or("frame")?;
    let i = ac2_scene::ir::extent(&f).nearest(0.0).ok_or("sample")?;
    let want = ac2_scene::format::amplitude_readout(f64::from(f.linear[i]));
    assert_eq!(cur.value, want);
    assert!(
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .any(|l| l.text == cur.text()),
        "the readout is drawn"
    );
    drop(d);
    drop(daemon);
    Ok(())
}

/// The measurement's own delay from the keys: Ctrl+. a sample later, Alt+, a tenth earlier.
/// The daemon keeps the averages, so the very next frames carry the new delay with the
/// 1 kHz column still valid (never back to settling), and the measurement list shows the
/// delay to the microsecond.
#[test]
fn delay_nudges_from_the_keys_keep_the_curve() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    // The level typed there stays: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.tf_frames(m.id, 240)?;
    let applied = |s: &AppState| {
        s.selected_meas()
            .and_then(|m| m.delay.as_ref())
            .map(|d| (d.applied.0, d.applied_samples))
    };
    let (_, before) = applied(&d.st).ok_or("delay")?;
    for k in ["Ctrl+.", "Ctrl+.", "Ctrl+.", "Alt+,"] {
        d.key(k);
    }
    let want = before + 2.9;
    d.until("the nudged delay", |s| {
        applied(s).is_some_and(|(_, n)| (n - want).abs() < 1e-6)
    })?;
    let (secs, _) = applied(&d.st).ok_or("delay")?;
    let rate = f64::from(d.st.open_session().ok_or("session")?.sample_rate_hz);
    assert!((secs - want / rate).abs() < 1e-12);
    assert_eq!(
        ac2_scene::format::delay(secs),
        format!("{:.3} ms", want / rate * 1000.0)
    );
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Tf,
    };
    d.until("a frame at the new delay, 1 kHz still valid", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => {
                    (tf.meta.delay.0 - secs).abs() < 1e-12
                        && tf.validity[240] == ac2_proto::frame::ValidityMask::NONE
                }
                _ => false,
            })
        })
    })?;
    assert!(
        d.st.toasts
            .iter()
            // The total depends on whether the mirror caught up between the keys.
            .any(|t| t.text.contains(": delay −0.1 sample → ")),
        "{:?}",
        d.st.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}

/// The transfer pane as drawn: each curve's wrapped phase at column `col`, and its legend.
fn drawn_phase(s: &AppState, col: usize) -> Vec<(ac2_scene::trace::TraceKey, f64, String)> {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    let sc = ac2_ui::scenes::transfer(s, &Theme::dark(), size, now);
    sc.traces
        .iter()
        .map(|t| {
            let legend = sc
                .legend
                .iter()
                .find(|l| l.key == t.key)
                .map_or(String::new(), |l| l.text.clone());
            (t.key, t.phase_wrapped_deg[col], legend)
        })
        .collect()
}

/// The transfer pane's drawn curve `k`: its column frequencies and wrapped phases.
fn drawn_curve(s: &AppState, k: ac2_scene::trace::TraceKey) -> (Vec<f64>, Vec<f64>) {
    let now = ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 1100.0,
        height: 600.0,
    };
    let sc = ac2_ui::scenes::transfer(s, &Theme::dark(), size, now);
    sc.traces
        .iter()
        .find(|t| t.key == k)
        .map(|t| (t.freqs.clone(), t.phase_wrapped_deg.clone()))
        .unwrap_or_default()
}

/// How far a curve's phase moved between two drawings, less the rotation a step of `samples`
/// should give (360°·f·samples/fs): the median over 500 Hz – 2 kHz. Two live estimates scatter
/// by several degrees per column on the noisy rig, so one column alone cannot judge the step.
fn rotation_residual(
    before: &(Vec<f64>, Vec<f64>),
    after: &(Vec<f64>, Vec<f64>),
    samples: f64,
    rate: f64,
) -> f64 {
    let wrap = |x: f64| (x + 180.0).rem_euclid(360.0) - 180.0;
    let mut r: Vec<f64> = before
        .0
        .iter()
        .zip(before.1.iter().zip(&after.1))
        .filter(|(f, (a, b))| (500.0..=2000.0).contains(*f) && a.is_finite() && b.is_finite())
        .map(|(f, (a, b))| wrap(b - a - 360.0 * f * samples / rate))
        .collect();
    assert!(r.len() > 20, "{} columns in 500 Hz – 2 kHz", r.len());
    r.sort_by(f64::total_cmp);
    r[r.len() / 2]
}

fn phase_of(v: &[(ac2_scene::trace::TraceKey, f64, String)], k: ac2_scene::trace::TraceKey) -> f64 {
    v.iter().find(|x| x.0 == k).map_or(f64::NAN, |x| x.1)
}

/// The operator's case from an empty daemon: a running transfer measurement whose live
/// curve is the phase reference, a capture of it and a sweep run on the same pane. Ctrl+.
/// steps of the measurement's delay move its live curve the way `.` moves a stored trace
/// (both later: phase leads more, e^{+jωΔ}), and neither stored curve moves at all, though
/// the moved curve is the reference. Stopped, the keys say why they do nothing.
#[test]
fn a_measurement_delay_step_moves_only_its_live_curve() -> R {
    use ac2_scene::trace::TraceKey;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    let run = sweep_from_the_dialog(&mut d)?;
    d.send(Msg::Command(CommandId::FocusTransfer));
    for _ in 0..4 {
        if d.st.selected == Some(m.id) {
            break;
        }
        d.send(Msg::Command(CommandId::NextMeasurement));
    }
    assert_eq!(d.st.selected, Some(m.id));
    let running = |s: &AppState| {
        s.daemon()
            .is_some_and(|x| x.measurements.iter().any(|y| y.id == m.id && y.running))
    };
    if !running(&d.st) {
        d.send(Msg::Command(CommandId::StartStop));
    }
    d.until("running", |s| {
        s.daemon()
            .is_some_and(|x| x.measurements.iter().any(|y| y.id == m.id && y.running))
    })?;
    // The level typed before stays: arm and fire.
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.tf_frames(m.id, 240)?;
    d.send(Msg::Command(CommandId::Slot1));
    d.until("the capture in slot 1", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.slot == Some(1)))
    })?;
    let cap =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.edit.slot == Some(1)))
            .map(|t| t.id)
            .ok_or("capture")?;
    d.until("the capture's data", |s| s.traces.contains_key(&cap))?;
    d.until("the run's data", |s| s.traces.contains_key(&run))?;
    let live = TraceKey::Live(m.id);
    let before = drawn_phase(&d.st, 240);
    let live_before = drawn_curve(&d.st, live);
    let legend = |v: &[(TraceKey, f64, String)], k| {
        v.iter()
            .find(|x| x.0 == k)
            .map_or(String::new(), |x| x.2.clone())
    };
    assert!(legend(&before, live).contains("· ref"), "{before:?}");
    let applied = |s: &AppState| {
        s.daemon()
            .and_then(|x| x.measurements.iter().find(|y| y.id == m.id))
            .and_then(|m| m.delay.as_ref())
            .map(|d| d.applied_samples)
    };
    let a0 = applied(&d.st).ok_or("delay")?;
    for _ in 0..10 {
        d.key("Ctrl+.");
    }
    let want = a0 + 10.0;
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Tf,
    };
    let rate = f64::from(d.st.open_session().ok_or("session")?.sample_rate_hz);
    d.until("a frame at the new delay", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => (tf.meta.delay.0 - want / rate).abs() < 1e-9,
                _ => false,
            })
        })
    })?;
    let after = drawn_phase(&d.st, 240);
    let live_after = drawn_curve(&d.st, live);
    let wrap = |x: f64| (x + 180.0).rem_euclid(360.0) - 180.0;
    // Ten samples later the curve leads by 360°·f·10/fs at every frequency (75° at 1 kHz).
    let off = rotation_residual(&live_before, &live_after, 10.0, rate);
    assert!(
        off.abs() < 2.0,
        "live moved {off:+.1}° off its expected rotation — before {before:?} after {after:?}"
    );
    for k in [TraceKey::Stored(cap), TraceKey::Stored(run)] {
        assert_eq!(
            phase_of(&after, k).to_bits(),
            phase_of(&before, k).to_bits(),
            "{k:?} moved: before {before:?} after {after:?}"
        );
    }
    assert!(legend(&after, live).contains("· ref"), "{after:?}");
    let tag = ac2_scene::format::from_arrival(10.0 / rate).ok_or("an offset")?;
    assert!(legend(&after, live).contains(&tag), "{tag}: {after:?}");
    // A capture now carries the steps as its display nudge and is drawn where the live
    // curve is.
    d.send(Msg::Command(CommandId::Slot2));
    d.until("the capture in slot 2", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.slot == Some(2)))
    })?;
    let cap2 =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.edit.slot == Some(2)))
            .ok_or("capture 2")?
            .clone();
    assert!(
        (cap2.edit.delay_nudge.0 - 10.0 / rate).abs() < 1e-12,
        "{:?}",
        cap2.edit
    );
    d.until("the second capture's data", |s| {
        s.traces.contains_key(&cap2.id)
    })?;
    let with2 = drawn_phase(&d.st, 240);
    let off = wrap(phase_of(&with2, TraceKey::Stored(cap2.id)) - phase_of(&with2, live));
    assert!(
        off.abs() < 3.0,
        "capture {off:.1}° off the live curve: {with2:?}"
    );
    // The same direction as `.` on the stored capture: a 0.1 ms step leads by 36°.
    for _ in 0..6 {
        if d.st.selected_trace == Some(cap) {
            break;
        }
        d.send(Msg::Command(CommandId::NextTrace));
    }
    assert_eq!(d.st.selected_trace, Some(cap));
    let pre = drawn_phase(&d.st, 240);
    let n0 =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.id == cap))
            .map(|t| t.edit.delay_nudge.0)
            .ok_or("capture")?;
    d.key(".");
    d.until("the capture nudged", |s| {
        s.daemon().is_some_and(|x| {
            x.traces
                .iter()
                .any(|t| t.id == cap && t.edit.delay_nudge.0 > n0 + 0.000_05)
        })
    })?;
    let post = drawn_phase(&d.st, 240);
    let nudged =
        wrap(phase_of(&post, TraceKey::Stored(cap)) - phase_of(&pre, TraceKey::Stored(cap)));
    // Ctrl+. made the live curve lead (checked above); `.` must turn the capture the same way.
    assert!(
        nudged > 0.0 && (nudged - 36.0).abs() < 1.0,
        "`.` turned the capture {nudged:.1}°, the way Ctrl+. turned the live curve (leading)"
    );

    // The sweep run as the reference: the steps move the live curve against it, alone.
    for _ in 0..8 {
        if d.st.selected_trace == Some(run) {
            break;
        }
        d.send(Msg::Command(CommandId::NextTrace));
    }
    assert_eq!(d.st.selected_trace, Some(run));
    d.send(Msg::Command(CommandId::PhaseReference));
    let pre = drawn_phase(&d.st, 240);
    let live_pre = drawn_curve(&d.st, live);
    assert!(
        legend(&pre, TraceKey::Stored(run)).contains("· ref"),
        "{pre:?}"
    );
    for _ in 0..10 {
        d.key("Ctrl+,");
    }
    d.until("a frame back at the first delay", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => (tf.meta.delay.0 - a0 / rate).abs() < 1e-9,
                _ => false,
            })
        })
    })?;
    let post = drawn_phase(&d.st, 240);
    let live_post = drawn_curve(&d.st, live);
    let off = rotation_residual(&live_pre, &live_post, -10.0, rate);
    assert!(
        off.abs() < 2.0,
        "live moved {off:+.1}° off its expected rotation back — before {pre:?} after {post:?}"
    );
    for (k, _, _) in &pre {
        if *k != live {
            assert_eq!(
                phase_of(&post, *k).to_bits(),
                phase_of(&pre, *k).to_bits(),
                "{k:?} moved: before {pre:?} after {post:?}"
            );
        }
    }
    d.stop()?;

    // Stopped: no live curve to move, so the keys change nothing and say why.
    d.send(Msg::Command(CommandId::StartStop));
    d.until("stopped", |s| {
        s.daemon()
            .is_some_and(|x| x.measurements.iter().any(|y| y.id == m.id && !y.running))
    })?;
    let a1 = applied(&d.st).ok_or("delay")?;
    d.key("Ctrl+.");
    let toast =
        d.st.toasts
            .last()
            .map(|t| t.text.clone())
            .unwrap_or_default();
    assert!(toast.contains("is stopped"), "{toast}");
    d.synced()?;
    assert_eq!(applied(&d.st), Some(a1));
    drop(d);
    drop(daemon);
    Ok(())
}

/// From an empty daemon: a delay typed in the `D` dialog moves the running measurement's
/// live curve the way the same number of Ctrl+. steps does, and nothing else on the pane.
/// The arrival stays where it was; the typed value's distance from it is its offset from
/// the arrival. Plain `.` then steps the same delay by 0.1 ms.
#[test]
fn a_typed_delay_moves_only_its_live_curve() -> R {
    use ac2_scene::trace::TraceKey;
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    let m = d.st.selected_meas().cloned().ok_or("measurement")?;
    d.send(Msg::Command(CommandId::FocusTransfer));
    let running = |s: &AppState| {
        s.daemon()
            .is_some_and(|x| x.measurements.iter().any(|y| y.id == m.id && y.running))
    };
    if !running(&d.st) {
        d.send(Msg::Command(CommandId::StartStop));
    }
    d.until("running", running)?;
    d.key("Space");
    d.until("armed", |s| s.stimulus.phase == StimPhase::Armed)?;
    d.key("Enter");
    d.until("firing", |s| s.daemon().is_some_and(|x| x.generator.firing))?;
    d.tf_frames(m.id, 240)?;
    d.send(Msg::Command(CommandId::Slot1));
    d.until("the capture in slot 1", |s| {
        s.daemon()
            .is_some_and(|x| x.traces.iter().any(|t| t.edit.slot == Some(1)))
    })?;
    let cap =
        d.st.daemon()
            .and_then(|x| x.traces.iter().find(|t| t.edit.slot == Some(1)))
            .map(|t| t.id)
            .ok_or("capture")?;
    d.until("the capture's data", |s| s.traces.contains_key(&cap))?;
    let live = TraceKey::Live(m.id);
    let delay = |s: &AppState| {
        s.daemon()
            .and_then(|x| x.measurements.iter().find(|y| y.id == m.id))
            .and_then(|m| m.delay.clone())
    };
    let d0 = delay(&d.st).ok_or("delay")?;
    let arrival = d0.applied_samples - d0.nudged_samples;
    let rate = f64::from(d.st.open_session().ok_or("session")?.sample_rate_hz);
    let before = drawn_phase(&d.st, 240);
    let live_before = drawn_curve(&d.st, live);

    let want = d0.applied_samples + 10.0;
    d.send(Msg::Command(CommandId::TypeDelay));
    assert!(
        matches!(d.st.overlay, Overlay::Prompt(_)),
        "{:?}",
        d.st.overlay
    );
    while matches!(&d.st.overlay, Overlay::Prompt(p) if !p.text.is_empty()) {
        d.send(Msg::Backspace);
    }
    d.send(Msg::Text(format!("{:.9}", want / rate * 1000.0)));
    d.key("Enter");
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Tf,
    };
    d.until("a frame at the typed delay", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => (tf.meta.delay.0 - want / rate).abs() < 1e-9,
                _ => false,
            })
        })
    })?;
    let st = delay(&d.st).ok_or("delay")?;
    assert!(
        (st.applied_samples - st.nudged_samples - arrival).abs() < 1e-6,
        "the arrival moved: {arrival} → {st:?}"
    );
    assert!(
        (st.nudged_samples - (d0.nudged_samples + 10.0)).abs() < 1e-6,
        "{st:?}"
    );
    let after = drawn_phase(&d.st, 240);
    let live_after = drawn_curve(&d.st, live);
    // Ten samples later the curve leads by 360°·f·10/fs, as ten Ctrl+. steps make it.
    let off = rotation_residual(&live_before, &live_after, 10.0, rate);
    assert!(
        off.abs() < 2.0,
        "live moved {off:+.1}° off its expected rotation — before {before:?} after {after:?}"
    );
    assert_eq!(
        phase_of(&after, TraceKey::Stored(cap)).to_bits(),
        phase_of(&before, TraceKey::Stored(cap)).to_bits(),
        "the capture moved: before {before:?} after {after:?}"
    );

    // Plain `.` on the measurement steps the same one delay by 0.1 ms, through the daemon:
    // the live curve alone moves, by 0.1 ms worth of samples.
    d.send(Msg::Command(CommandId::SelectLive));
    assert_eq!(d.st.selected_trace, None);
    let step = 0.000_1 * rate;
    let want = st.applied_samples + step;
    d.key(".");
    d.until("a frame 0.1 ms later", |s| {
        s.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Tf(tf) => (tf.meta.delay.0 - want / rate).abs() < 1e-9,
                _ => false,
            })
        })
    })?;
    let stepped = delay(&d.st).ok_or("delay")?;
    assert!(
        (stepped.applied_samples - stepped.nudged_samples - arrival).abs() < 1e-6,
        "the arrival moved: {arrival} → {stepped:?}"
    );
    let after_dot = drawn_phase(&d.st, 240);
    let live_dot = drawn_curve(&d.st, live);
    let off = rotation_residual(&live_after, &live_dot, step, rate);
    assert!(
        off.abs() < 2.0,
        "`.` moved the live curve {off:+.1}° off 0.1 ms — before {after:?} after {after_dot:?}"
    );
    assert_eq!(
        phase_of(&after_dot, TraceKey::Stored(cap)).to_bits(),
        phase_of(&before, TraceKey::Stored(cap)).to_bits(),
        "the capture moved: before {before:?} after {after_dot:?}"
    );
    d.stop()?;
    drop(d);
    drop(daemon);
    Ok(())
}
