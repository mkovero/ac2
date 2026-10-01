//! Reducer tests: keyboard flows without a window or a daemon.

use std::sync::Arc;
use std::time::Instant;

use ac2_client::fake::empty_state;
use ac2_client::{MirrorView, Phase};
use ac2_proto::Command;
use ac2_proto::model::*;
use ac2_proto::units::*;

use super::*;
use crate::conn::{ConnEvent, Request, StimEvent};
use crate::keys::{Chord, Keymap};

fn meas(id: u32, name: &str, kind: MeasKind) -> Measurement {
    let delay = matches!(kind, MeasKind::Transfer { .. }).then(|| DelayState {
        applied: Seconds(0.0125),
        applied_samples: Samples(600),
        tracking: false,
        last_finding: None,
    });
    Measurement {
        id: MeasId(id),
        config: MeasConfig {
            name: name.into(),
            kind,
        },
        config_rev: Rev(1),
        running: true,
        frozen: false,
        delay,
        grid_id: None,
    }
}

fn transfer() -> MeasKind {
    MeasKind::Transfer {
        config: TransferConfig {
            reference_input: 0,
            measurement_input: 1,
            averaging: TfAveraging::Exponential {
                time_constant: Seconds(1.0),
            },
            grid: LogGridSpec {
                ppo: 48,
                k_min: -240,
                k_max: 216,
            },
            smoothing: None,
        },
    }
}

fn spectrum() -> MeasKind {
    MeasKind::Spectrum {
        config: SpectrumConfig {
            input: 1,
            fft_len: 8192,
            window: Window::Hann,
            averaging: SpecAveraging::Off,
        },
    }
}

fn daemon_state() -> State {
    let mut s = empty_state();
    s.measurements = vec![meas(2, "Sub", spectrum()), meas(1, "Main L", transfer())];
    s.session.open = Some(OpenSession {
        config: SessionConfig {
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: vec![0, 1],
            output_channels: 2,
            sample_rate_hz: Some(48_000),
            buffer_frames: Some(256),
            loopback: None,
        },
        input_device: DeviceId("fake:loop".into()),
        output_device: DeviceId("fake:loop".into()),
        sample_rate_hz: 48_000,
        buffer_frames: 256,
        clock: ClockRelation::SingleCallback,
        opened_at: WallNs(0),
    });
    s
}

fn mirror(state: State) -> ConnEvent {
    ConnEvent::Mirror(Arc::new(MirrorView {
        phase: Phase::Live,
        incarnation: Some(DaemonIncarnation(1)),
        session_epoch: Some(SessionEpoch(2)),
        rev: Rev(10),
        state: Some(Arc::new(state)),
        last_ka: Some(Instant::now()),
        generator: None,
        timing: None,
        ka_rev: Some(Rev(10)),
        clock_offset_ns: Some(0),
        snapshots: 1,
        since_requests: 0,
        incarnation_changes: 0,
    }))
}

struct T {
    st: AppState,
    keys: Keymap,
}

impl T {
    fn new() -> Self {
        let mut t = Self {
            st: AppState::default(),
            keys: Keymap::default(),
        };
        t.conn(ConnEvent::Connected {
            target: "local daemon".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c1".into()),
        });
        t.conn(mirror(daemon_state()));
        t
    }

    fn disconnected() -> Self {
        Self {
            st: AppState::default(),
            keys: Keymap::default(),
        }
    }

    fn conn(&mut self, e: ConnEvent) -> Vec<Request> {
        self.st.update(Msg::Conn(Box::new(e)), &self.keys)
    }

    fn key(&mut self, s: &str) -> Vec<Request> {
        let c = Chord::parse(s).expect(s);
        self.st.update(Msg::Key(c), &self.keys)
    }

    /// A key press followed by the text it types, as egui delivers it.
    fn type_key(&mut self, s: &str, text: &str) -> Vec<Request> {
        let mut r = self.key(s);
        r.extend(self.st.update(Msg::Text(text.into()), &self.keys));
        r
    }

    fn text(&mut self, s: &str) {
        self.st.update(Msg::Text(s.into()), &self.keys);
    }

    fn last_toast(&self) -> &str {
        self.st.toasts.last().map_or("", |t| t.text.as_str())
    }
}

fn set_level(r: &Request) -> Option<(f64, bool, bool)> {
    match r {
        Request::StimSet(d) => Some((d.settings.level.0, d.armed, d.firing)),
        _ => None,
    }
}

#[test]
fn selects_the_first_transfer_measurement() {
    let t = T::new();
    assert_eq!(t.st.selected, Some(MeasId(1)));
    let names: Vec<&str> =
        t.st.measurements()
            .iter()
            .map(|m| m.config.name.as_str())
            .collect();
    assert_eq!(names, ["Main L", "Sub"]);
}

#[test]
fn stimulus_needs_a_typed_level() {
    let mut t = T::new();
    // Space without a level opens the level prompt and arms nothing.
    let r = t.type_key("Space", " ");
    assert!(r.is_empty(), "{r:?}");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::StimulusLevel && p.text.is_empty()
    ));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    // Enter in the prompt applies it; it does not fire.
    t.text("-20");
    let r = t.key("Enter");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.stimulus.level, Some(Dbfs(-20.0)));
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
}

#[test]
fn arm_fire_level_stop() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    // Enter before arming does nothing but say so.
    assert!(t.key("Enter").is_empty());
    assert!(t.last_toast().contains("not armed"));

    let r = t.key("Space");
    match r.as_slice() {
        [Request::StimArm { settings, force }] => {
            assert_eq!(settings.level, Dbfs(-20.0));
            assert_eq!(settings.signal, Signal::Pink);
            assert_eq!(settings.outputs, vec![0]);
            assert!(!force);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(t.st.stimulus.phase, StimPhase::Arming);
    // A second Space while arming sends nothing.
    assert!(t.key("Space").is_empty());
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(t.st.stimulus.phase, StimPhase::Armed);

    let r = t.key("Enter");
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-20.0, true, true)]
    );
    assert_eq!(t.st.stimulus.phase, StimPhase::FireRequested);
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);

    // ↑/↓ change the level of the running stimulus; Shift steps 3 dB.
    let r = t.key("Up");
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-19.0, true, true)]
    );
    let r = t.key("Shift+Down");
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-22.0, true, true)]
    );

    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Stopping);
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
}

#[test]
fn level_never_exceeds_the_ceiling() {
    let mut t = T::new();
    // empty_state's ceiling is −6 dBFS.
    assert_eq!(t.st.ceiling(), Some(Dbfs(-6.0)));
    t.key("L");
    t.text("-3");
    assert!(t.key("Enter").is_empty());
    match &t.st.overlay {
        Overlay::Prompt(p) => assert!(p.error.as_deref().unwrap_or("").contains("ceiling")),
        o => panic!("{o:?}"),
    }
    assert_eq!(t.st.stimulus.level, None);
    t.key("Esc");
    t.st.stimulus.level = Some(Dbfs(-7.0));
    t.st.stimulus.phase = StimPhase::Armed;
    let r = t.key("Shift+Up");
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-6.0, true, false)]
    );
    assert!(t.key("Up").is_empty());
    assert!(t.last_toast().contains("ceiling"));
}

#[test]
fn arrows_without_level_send_nothing() {
    let mut t = T::new();
    assert!(t.key("Up").is_empty());
    assert!(t.st.toasts.last().is_some_and(|x| x.error));
    assert_eq!(t.st.stimulus.level, None);
}

#[test]
fn escape_always_stops_and_closes() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    // Esc from inside the palette: closes it and stops.
    t.key("Ctrl+K");
    assert!(matches!(t.st.overlay, Overlay::Palette(_)));
    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]));
    assert_eq!(t.st.overlay, Overlay::None);
    // Another client's running generator: Esc stops it too (gen.stop is universal).
    let mut t = T::new();
    let mut s = daemon_state();
    s.generator.firing = true;
    s.generator.armed = true;
    s.generator.owner = Some(ClientId("other".into()));
    t.conn(mirror(s));
    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]));
    // Nothing live: Esc only closes overlays.
    let mut t = T::new();
    t.key("/");
    assert_eq!(t.st.overlay, Overlay::Help);
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn disconnected_never_arms() {
    let mut t = T::disconnected();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    assert!(t.key("Space").is_empty());
    assert!(t.last_toast().contains("not connected"));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
}

#[test]
fn lost_lease_returns_to_idle() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.conn(ConnEvent::Stimulus(StimEvent::Lost("taken over".into())));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(t.last_toast().contains("taken over"));
    assert!(t.key("Enter").is_empty());
    // A failed arm also returns to idle.
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Failed("lease held".into())));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
}

#[test]
fn opening_key_is_not_typed_into_the_prompt() {
    let mut t = T::new();
    t.type_key("L", "l");
    t.text("-1");
    t.text("8");
    match &t.st.overlay {
        Overlay::Prompt(p) => assert_eq!(p.text, "-18"),
        o => panic!("{o:?}"),
    }
    t.st.update(Msg::Backspace, &t.keys.clone());
    t.text("9");
    t.key("Enter");
    assert_eq!(t.st.stimulus.level, Some(Dbfs(-19.0)));
}

#[test]
fn keys_follow_the_focused_pane() {
    let mut t = T::new();
    assert!(t.st.layout.is_shown(PaneKind::Ir));
    // Transfer pane: H hides the IR pane.
    t.key("H");
    assert!(!t.st.layout.is_shown(PaneKind::Ir));
    t.key("H");
    // Spectrum pane: H is peak hold.
    t.key("2");
    assert_eq!(t.st.scope(), Scope::Spectrum);
    t.key("H");
    assert!(t.st.view.spectrum.peak_hold);
    assert!(t.st.layout.is_shown(PaneKind::Ir));
    // X means nothing there.
    assert!(t.key("X").is_empty());
    // Tab cycles panes; W maximizes.
    t.key("Tab");
    assert_eq!(t.st.layout.focus, PaneKind::Ir);
    t.key("Shift+Tab");
    assert_eq!(t.st.layout.focus, PaneKind::Spectrum);
    t.key("W");
    assert_eq!(t.st.layout.visible(), vec![PaneKind::Spectrum]);
}

#[test]
fn transfer_commands() {
    let mut t = T::new();
    let r = t.key("X");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::DelayInsert {
                meas: MeasId(1),
                pick: DelayPick::FirstArrival
            },
            ..
        }]
    ));
    let r = t.key("Shift+X");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::DelayInsert {
                pick: DelayPick::Strongest,
                ..
            },
            ..
        }]
    ));
    let r = t.key("Y");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::DelayTrack { enabled: true, .. },
            ..
        }]
    ));
    let r = t.key("F");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::MeasFreeze { frozen: true, .. },
            ..
        }]
    ));
    // U / J / , . are display edits, not commands.
    assert!(t.key("U").is_empty());
    assert!(t.st.edit(MeasId(1)).inverted);
    t.type_key("J", "j");
    t.text("+3,5 dB");
    t.key("Enter");
    assert_eq!(t.st.edit(MeasId(1)).offset_db, 3.5);
    t.key(".");
    t.key(".");
    t.key(",");
    assert!((t.st.edit(MeasId(1)).nudge_s - 0.000_1).abs() < 1e-15);
    // Typed delay.
    t.type_key("D", "d");
    match &t.st.overlay {
        Overlay::Prompt(p) => assert_eq!(p.text, "12.50"),
        o => panic!("{o:?}"),
    }
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Call { cmd: Command::DelaySet { delay: Seconds(d), .. }, .. }] if (*d - 0.0125).abs() < 1e-12
    ));
    // View toggles.
    t.key("B");
    assert_eq!(t.st.view.tf.coherence.blank_below, Some(0.3));
    t.key("Shift+P");
    assert!(matches!(t.st.view.tf.phase, PhaseView::GroupDelay { .. }));
    // P leaves group delay for wrapped phase, then toggles wrapped / unwrapped.
    t.key("P");
    assert_eq!(t.st.view.tf.phase, PhaseView::Wrapped);
    t.key("P");
    assert!(matches!(t.st.view.tf.phase, PhaseView::Unwrapped { .. }));
    t.key("Shift+C");
    assert_eq!(
        t.st.view.tf.coherence_placement,
        CoherencePlacement::OverlayOnMagnitude
    );
    t.key("E");
    assert_eq!(
        t.st.view.tf.phase_reference,
        Some(TraceKey::Live(MeasId(1)))
    );
    // Stubs say so and do nothing else.
    assert!(t.key("Z").is_empty());
    assert!(t.last_toast().contains("not available"));
    assert!(t.key("M").is_empty());
}

#[test]
fn transfer_commands_need_a_transfer_measurement() {
    let mut t = T::new();
    t.key("N");
    assert_eq!(t.st.selected, Some(MeasId(2)));
    assert!(t.key("X").is_empty());
    assert!(t.last_toast().contains("transfer-function"));
    t.key("Shift+N");
    assert_eq!(t.st.selected, Some(MeasId(1)));
}

#[test]
fn slots_capture_and_replace() {
    let mut t = T::new();
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            meas: MeasId(1),
            slot: 3,
            replace: None
        }]
    ));
    let trace = TraceMeta {
        id: TraceId(9),
        edit: TraceEdit {
            name: "slot 3".into(),
            color: Rgb { r: 1, g: 2, b: 3 },
            visible: true,
            locked: false,
            order: 9,
            offset: Db(0.0),
            polarity: Polarity::Normal,
            delay_nudge: Seconds(0.0),
        },
        source: TraceSource::Captured {
            meas: MeasId(1),
            epoch: SessionEpoch(2),
            at_sample: SampleIndex(0),
        },
        grid_id: ac2_proto::GridId(1),
        delay: Seconds(0.0),
        smoothing: None,
        cal: CalState::Uncalibrated,
        mic: None,
        created_at: WallNs(0),
    };
    t.conn(ConnEvent::Captured {
        slot: 3,
        trace: trace.clone(),
    });
    assert_eq!(t.st.slots[2], Some(TraceId(9)));
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            slot: 3,
            replace: Some(TraceId(9)),
            ..
        }]
    ));
    // A trace deleted daemon-side frees its slot.
    t.conn(mirror(daemon_state()));
    assert_eq!(t.st.slots[2], None);
}

#[test]
fn palette_runs_commands() {
    let mut t = T::new();
    t.key("Ctrl+K");
    t.text("group");
    t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(matches!(t.st.view.tf.phase, PhaseView::GroupDelay { .. }));
    // Keys do not act while the palette is open; arrows move the highlight.
    t.key("Ctrl+K");
    let r = t.type_key("X", "x");
    assert!(r.is_empty());
    t.key("Down");
    match &t.st.overlay {
        Overlay::Palette(p) => {
            assert_eq!(p.query, "x");
            assert_eq!(p.selected, 1);
        }
        o => panic!("{o:?}"),
    }
    // Ctrl+K again closes.
    t.key("Ctrl+K");
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn navigation_animates_values_do_not() {
    let mut t = T::new();
    let before = t.st.view.freq;
    t.key("I");
    // The target moves at once; the drawn range follows over a few frames.
    assert_ne!(t.st.nav.target, before);
    assert_eq!(t.st.view.freq, before);
    t.st.update(
        Msg::Tick {
            now_s: 0.016,
            dt_s: 0.016,
        },
        &t.keys.clone(),
    );
    let mid = t.st.view.freq;
    assert!(mid != before && mid != t.st.nav.target);
    assert!(t.st.animating());
    for i in 0..100 {
        let now = 0.016 * f64::from(i + 2);
        t.st.update(
            Msg::Tick {
                now_s: now,
                dt_s: 0.016,
            },
            &t.keys.clone(),
        );
    }
    assert!(!t.st.animating());
    assert_eq!(t.st.view.freq, t.st.nav.target);
    // Drag pans directly.
    t.st.update(Msg::Pan { octaves: 1.0 }, &t.keys.clone());
    assert!(!t.st.animating());
    t.key("Home");
    assert_eq!(t.st.nav.target, ac2_scene::view::FreqRange::default());
}

#[test]
fn cursor_keys() {
    let mut t = T::new();
    t.key("C");
    let hz = t.st.view.cursor_hz.expect("cursor");
    assert!((hz - (20.0f64 * 20_000.0).sqrt()).abs() < 1e-9);
    t.key("Shift+Right");
    let up = t.st.view.cursor_hz.expect("cursor");
    assert!((up / hz - 2f64.powf(1.0 / 12.0)).abs() < 1e-12);
    t.key("C");
    assert_eq!(t.st.view.cursor_hz, None);
}

#[test]
fn toasts_expire() {
    let mut t = T::new();
    t.key("Z");
    assert!(!t.st.toasts.is_empty());
    t.st.update(
        Msg::Tick {
            now_s: 100.0,
            dt_s: 0.016,
        },
        &t.keys.clone(),
    );
    assert!(t.st.toasts.is_empty());
}

#[test]
fn link_failure_clears_live_state() {
    let mut t = T::new();
    t.st.stimulus.phase = StimPhase::Armed;
    t.conn(ConnEvent::Failed {
        target: "local daemon".into(),
        error: "not responding".into(),
        retry_in: std::time::Duration::from_secs(2),
    });
    assert!(t.st.mirror.is_none());
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(!t.st.connected());
}

#[test]
fn parsers() {
    assert_eq!(parse_number(" −12,5 dBFS", &["dbfs", "db"]), Ok(-12.5));
    assert_eq!(parse_number("3ms", &["ms"]), Ok(3.0));
    assert!(parse_number("loud", &[]).is_err());
    assert!(parse_number("inf", &[]).is_err());
    assert_eq!(parse_outputs("1, 2 2"), Ok(vec![0, 1]));
    assert!(parse_outputs("0").is_err());
    assert!(parse_outputs("").is_err());
}

#[test]
fn level_change_during_arming_is_sent_once_armed() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    // ↓ while the arm is in flight: nothing to send yet.
    assert!(t.key("Down").is_empty());
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert_eq!(
        r.iter().filter_map(set_level).collect::<Vec<_>>(),
        [(-21.0, true, false)]
    );
    // Unchanged settings: the confirmation sends nothing.
    t.key("Esc");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    t.key("Space");
    assert!(t.conn(ConnEvent::Stimulus(StimEvent::Armed)).is_empty());
}
