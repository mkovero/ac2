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
use crate::session_dialog::{InputRole, RoleKey, Row, SessionDialog};

fn meas(id: u32, name: &str, kind: MeasKind) -> Measurement {
    let delay = matches!(kind, MeasKind::Transfer { .. }).then(|| DelayState {
        applied: Seconds(0.0125),
        applied_samples: Samples(600),
        tracking: false,
        awaiting_pick: false,
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
            depth: ac2_proto::model::DepthPolicy::EqualConfidence,
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
            backend: None,
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: vec![0, 1],
            output_channels: 2,
            sample_rate_hz: Some(48_000),
            buffer_frames: Some(256),
            loopback: None,
        },
        backend: BackendKind::Cpal,
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
    mirror_of(state, 1, Some("c1"))
}

fn mirror_of(state: State, incarnation: u64, me: Option<&str>) -> ConnEvent {
    ConnEvent::Mirror(Arc::new(MirrorView {
        client_id: me.map(|m| ClientId(m.into())),
        phase: Phase::Live,
        incarnation: Some(DaemonIncarnation(incarnation)),
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
    t.key("Alt+2");
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
        [Request::FindDelay {
            meas: MeasId(1),
            pick: DelayPick::FirstArrival,
            band: FinderBand::Auto,
            observation: None,
        }]
    ));
    let r = t.key("Shift+X");
    assert!(matches!(
        r.as_slice(),
        [Request::FindDelay {
            meas: MeasId(1),
            pick: DelayPick::Strongest,
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
    // Z asks for a target file; M with no stored traces says what it needs.
    assert!(t.key("Z").is_empty());
    assert!(
        matches!(&t.st.overlay, Overlay::Prompt(p) if p.kind == PromptKind::ImportFile(ImportRole::Target))
    );
    t.key("Esc");
    assert!(t.key("M").is_empty());
    assert!(t.last_toast().contains("at least two"));
}

#[test]
fn transfer_commands_need_a_transfer_measurement() {
    let mut t = T::new();
    // The spectrum picked in the list while the transfer pane has the keys.
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(2)));
    assert!(t.key("X").is_empty());
    assert!(t.last_toast().contains("transfer-function"));
    // N in the transfer pane only goes through transfer measurements.
    t.key("Shift+N");
    assert_eq!(t.st.selected, Some(MeasId(1)));
}

fn rta() -> MeasKind {
    MeasKind::Rta {
        config: RtaConfig::on_input(1, BandFraction::Third),
    }
}

/// Two transfer measurements, a spectrum and an RTA.
fn four() -> State {
    let mut s = daemon_state();
    s.measurements.push(meas(3, "Delay tower", transfer()));
    s.measurements.push(meas(4, "Room", rta()));
    s
}

#[test]
fn panes_show_and_select_their_measurement() {
    let mut t = T::new();
    t.conn(mirror(four()));
    let shown = |t: &T, p: PaneKind| t.st.pane_meas(p).map(|m| m.id.0);
    assert_eq!(t.st.selected, Some(MeasId(1)));
    assert_eq!(shown(&t, PaneKind::Transfer), Some(1));
    assert_eq!(shown(&t, PaneKind::Ir), Some(1));
    assert_eq!(shown(&t, PaneKind::Spectrum), Some(2));
    assert_eq!(shown(&t, PaneKind::Spl), None);

    // A click in a pane focuses it and selects what it shows.
    t.st.update(Msg::FocusPane(PaneKind::Spectrum), &t.keys);
    assert_eq!(t.st.layout.focus, PaneKind::Spectrum);
    assert_eq!(t.st.selected, Some(MeasId(2)));
    // N / Shift+N go through the focused pane's kind only: spectrum and RTA here.
    t.key("N");
    assert_eq!(t.st.selected, Some(MeasId(4)));
    assert_eq!(shown(&t, PaneKind::Spectrum), Some(4));
    t.key("N");
    assert_eq!(t.st.selected, Some(MeasId(2)));
    // The transfer pane kept its own measurement.
    t.st.update(Msg::FocusPane(PaneKind::Transfer), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(1)));
    t.key("N");
    assert_eq!(t.st.selected, Some(MeasId(3)));
    assert_eq!(shown(&t, PaneKind::Transfer), Some(3));
    assert_eq!(
        shown(&t, PaneKind::Ir),
        Some(3),
        "IR follows the transfer pane"
    );
    // Focus by key selects too.
    t.key("Alt+2");
    assert_eq!(t.st.selected, Some(MeasId(2)));
    t.key("Alt+3");
    assert_eq!(t.st.selected, Some(MeasId(3)));

    // A list selection updates the pane that shows that kind, not the focused one.
    t.st.update(Msg::SelectMeas(MeasId(4)), &t.keys);
    assert_eq!(shown(&t, PaneKind::Spectrum), Some(4));
    assert_eq!(shown(&t, PaneKind::Transfer), Some(3));

    // The title chip: the pane's list with what it shows highlighted; Down Enter shows the
    // next one.
    t.st.update(Msg::FocusPane(PaneKind::Transfer), &t.keys);
    t.st.update(Msg::PaneMenu(PaneKind::Transfer), &t.keys);
    assert_eq!(
        t.st.overlay,
        Overlay::PaneMenu(PaneMenu {
            pane: PaneKind::Transfer,
            index: 1
        })
    );
    // Keys other than the list's do nothing while it is open.
    assert!(t.key("X").is_empty());
    t.key("Down");
    t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.selected, Some(MeasId(1)));
    assert_eq!(shown(&t, PaneKind::Transfer), Some(1));
    // A click on the chip again closes the list; a pick by mouse shows it.
    t.st.update(Msg::PaneMenu(PaneKind::Spectrum), &t.keys);
    assert!(
        matches!(t.st.overlay, Overlay::PaneMenu(m) if m.pane == PaneKind::Spectrum && m.index == 1)
    );
    t.st.update(Msg::PaneMenu(PaneKind::Spectrum), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None);
    t.st.update(Msg::PaneMenu(PaneKind::Spectrum), &t.keys);
    t.st.update(Msg::PaneShow(PaneKind::Spectrum, MeasId(2)), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.layout.focus, PaneKind::Spectrum);
    assert_eq!(t.st.selected, Some(MeasId(2)));
    // A measurement the pane cannot show is ignored.
    t.st.update(Msg::PaneShow(PaneKind::Spectrum, MeasId(3)), &t.keys);
    assert_eq!(shown(&t, PaneKind::Spectrum), Some(2));
    // Palette entry for the keyboard: the focused pane's list.
    t.st.update(Msg::Command(CommandId::PaneMeasurement), &t.keys);
    assert!(matches!(t.st.overlay, Overlay::PaneMenu(m) if m.pane == PaneKind::Spectrum));
    t.key("Esc");
    assert_eq!(t.st.overlay, Overlay::None);
    // Nothing to list: said, nothing opens.
    t.st.update(Msg::PaneMenu(PaneKind::Spl), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(t.last_toast().contains("no SPL measurements"));
    t.key("Alt+4");
    t.key("N");
    assert!(t.last_toast().contains("no SPL measurements"));

    // A deleted measurement leaves its pane showing the next one that fits.
    t.st.update(Msg::FocusPane(PaneKind::Transfer), &t.keys);
    t.key("N");
    assert_eq!(shown(&t, PaneKind::Transfer), Some(3));
    let mut s = four();
    s.measurements.retain(|m| m.id != MeasId(3));
    t.conn(mirror(s));
    assert_eq!(shown(&t, PaneKind::Transfer), Some(1));
}

fn smoothing(f: SmoothingFraction, mode: SmoothingMode) -> Option<Smoothing> {
    Some(Smoothing { fraction: f, mode })
}

/// The smoothing a request sets, and on what.
fn smoothing_set(r: &[Request]) -> (String, Option<Smoothing>) {
    match r {
        [
            Request::Call {
                cmd: Command::MeasUpdate { meas, config },
                ..
            },
        ] => match &config.kind {
            MeasKind::Transfer { config } => (format!("m{}", meas.0), config.smoothing),
            other => panic!("{other:?}"),
        },
        [
            Request::Call {
                cmd: Command::TraceUpdate { trace, edit },
                ..
            },
        ] => (format!("t{}", trace.0), edit.smoothing),
        other => panic!("{other:?}"),
    }
}

#[test]
fn smoothing_keys_step_the_pane_measurement() {
    let mut t = T::new();
    assert_eq!(t.st.smoothing_caption().as_deref(), Some("smoothing off"));
    // K coarser: off → 1/48 (power), with the measurement's config otherwise unchanged.
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "m1".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Power)
        )
    );
    match r.as_slice() {
        [
            Request::Call {
                cmd: Command::MeasUpdate { config, .. },
                what,
            },
        ] => {
            assert_eq!(what, "Main L: smoothing 1/48 oct");
            let MeasKind::Transfer { config } = &config.kind else {
                unreachable!()
            };
            let MeasKind::Transfer { config: was } = transfer() else {
                unreachable!()
            };
            assert_eq!(config.averaging, was.averaging);
            assert_eq!(config.grid, was.grid);
        }
        other => panic!("{other:?}"),
    }
    // Finer than off: nothing sent, said.
    assert!(t.key("Shift+K").is_empty());
    assert!(t.last_toast().contains("finest"), "{}", t.last_toast());

    // The mirror says 1/6 complex: K → 1/3 (mode kept), Shift+K → 1/12, at 1/3 K stops.
    let mut s = daemon_state();
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Sixth, SmoothingMode::Complex);
    }
    t.conn(mirror(s.clone()));
    assert_eq!(
        t.st.smoothing_caption().as_deref(),
        Some("smoothing 1/6 oct complex")
    );
    assert_eq!(
        smoothing_set(&t.key("K")).1,
        smoothing(SmoothingFraction::Third, SmoothingMode::Complex)
    );
    assert_eq!(
        smoothing_set(&t.key("Shift+K")).1,
        smoothing(SmoothingFraction::Twelfth, SmoothingMode::Complex)
    );
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Third, SmoothingMode::Power);
    }
    t.conn(mirror(s));
    assert!(t.key("K").is_empty());
    assert!(t.last_toast().contains("widest"), "{}", t.last_toast());
    // Palette entries set a step directly.
    let r = t.st.update(Msg::Command(CommandId::SmoothOff), &t.keys);
    assert_eq!(smoothing_set(&r).1, None);
    let r = t.st.update(Msg::Command(CommandId::Smooth24), &t.keys);
    assert_eq!(
        smoothing_set(&r).1,
        smoothing(SmoothingFraction::TwentyFourth, SmoothingMode::Power)
    );
    // K is a transfer-pane key.
    t.key("Alt+2");
    assert!(t.key("K").is_empty());
}

#[test]
fn smoothing_keys_change_a_selected_slot() {
    let mut t = T::new();
    let mut spec = stored(11, Some(4), 2);
    spec.kind = TraceKind::Spectrum {
        scale: LevelScale::Dbfs,
    };
    let mut locked = stored(12, Some(5), 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![stored(10, Some(3), 2), spec, locked]));
    // A click on slot 3 selects it: K changes its trace, the caption says so.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    assert_eq!(
        t.st.smoothing_caption().as_deref(),
        Some("slot 3 (t10): smoothing off")
    );
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "t10".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Power)
        )
    );
    match r.as_slice() {
        [
            Request::Call {
                cmd: Command::TraceUpdate { edit, .. },
                what,
            },
        ] => {
            assert_eq!(what, "slot 3 (t10): smoothing 1/48 oct");
            // Its other edits are sent unchanged.
            let mut want = stored(10, Some(3), 2).edit;
            want.smoothing = edit.smoothing;
            assert_eq!(*edit, want);
        }
        other => panic!("{other:?}"),
    }
    // Spectra cannot be smoothed; a locked slot says so; neither sends anything.
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    assert!(t.key("K").is_empty());
    assert!(t.last_toast().contains("transfer traces only"));
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key("K").is_empty());
    assert!(t.last_toast().contains("locked"));
    // A second click deselects; the keys act on the pane's measurement again.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert_eq!(t.st.selected_trace, None);
    assert_eq!(smoothing_set(&t.key("K")).0, "m1");
    // Selecting a measurement (list, pane click, N) deselects the slot.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.st.update(Msg::FocusPane(PaneKind::Transfer), &t.keys);
    assert_eq!(t.st.selected_trace, None);
    // A selected trace that goes away is forgotten.
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    t.conn(with_traces(vec![]));
    assert_eq!(t.st.selected_trace, None);
}

#[test]
fn resmoothed_trace_data_keeps_its_smoothing_until_refetched() {
    let mut t = T::new();
    let meta = stored(10, Some(3), 2);
    t.conn(with_traces(vec![meta.clone()]));
    let data = Arc::new(TraceData {
        meta: meta.clone(),
        mag_db: vec![0.0; 4],
        phase_deg: None,
        coherence: None,
    });
    let grid = Arc::new(GridDef::Log {
        ppo: 1,
        k_min: 0,
        k_max: 3,
    });
    t.conn(ConnEvent::Trace(data, grid.clone()));
    // The mirror says 1/6 now; the columns held were served unsmoothed, so the drawn
    // trace keeps saying so (other edits follow at once) until the new data arrives.
    let mut m2 = meta.clone();
    m2.edit.smoothing = smoothing(SmoothingFraction::Sixth, SmoothingMode::Power);
    m2.edit.name = "renamed".into();
    t.conn(with_traces(vec![m2.clone()]));
    let held = &t.st.traces[&TraceId(10)].0.meta;
    assert_eq!(held.edit.smoothing, None);
    assert_eq!(held.edit.name, "renamed");
    let fresh = Arc::new(TraceData {
        meta: m2.clone(),
        mag_db: vec![1.0; 4],
        phase_deg: None,
        coherence: None,
    });
    t.conn(ConnEvent::Trace(fresh, grid));
    assert_eq!(t.st.traces[&TraceId(10)].0.meta, m2);
}

fn stored(id: u32, slot: Option<u8>, epoch: u32) -> TraceMeta {
    TraceMeta {
        id: TraceId(id),
        edit: TraceEdit {
            name: format!("t{id}"),
            color: Rgb { r: 1, g: 2, b: 3 },
            visible: true,
            locked: false,
            order: id,
            offset: Db(0.0),
            polarity: Polarity::Normal,
            delay_nudge: Seconds(0.0),
            slot,
            smoothing: None,
        },
        kind: TraceKind::Transfer,
        source: TraceSource::Captured {
            meas: MeasId(1),
            meas_name: "Main L".into(),
            epoch: SessionEpoch(epoch),
            at_sample: SampleIndex(0),
        },
        grid_id: ac2_proto::GridId(1),
        delay: Seconds(0.0),
        depth: Some(DepthPolicy::EqualConfidence),
        cal: CalState::Uncalibrated,
        mic: None,
        created_at: WallNs(0),
    }
}

fn with_traces(traces: Vec<TraceMeta>) -> ConnEvent {
    let mut s = daemon_state();
    s.traces = traces;
    mirror(s)
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
            replace: None,
            name,
        }] if name == "Main L S3"
    ));
    // Slots are the daemon's: a mirrored trace in slot 3 is what Ctrl+3 replaces.
    t.conn(with_traces(vec![stored(9, Some(3), 2)]));
    assert_eq!(t.st.slots()[2].map(|t| t.id), Some(TraceId(9)));
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            slot: 3,
            replace: Some(TraceId(9)),
            ..
        }]
    ));
    // A locked trace is not deleted; it only gives up the slot.
    let mut locked = stored(9, Some(3), 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![locked]));
    let r = t.key("Ctrl+3");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            slot: 3,
            replace: None,
            ..
        }]
    ));
    // Spectrum measurements capture too.
    t.st.selected = Some(MeasId(2));
    let r = t.key("Ctrl+1");
    assert!(matches!(
        r.as_slice(),
        [Request::Capture {
            meas: MeasId(2),
            slot: 1,
            ..
        }]
    ));
    // A trace deleted daemon-side frees its slot.
    t.conn(mirror(daemon_state()));
    assert!(t.st.slots()[2].is_none());
}

#[test]
fn digits_show_and_hide_slots_alt_digits_focus_panes() {
    let mut t = T::new();
    t.conn(with_traces(vec![stored(9, Some(3), 2)]));
    let r = t.key("3");
    let [
        Request::Call {
            cmd: Command::TraceUpdate { trace, edit },
            what,
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    assert_eq!(*trace, TraceId(9));
    assert!(!edit.visible);
    assert_eq!(edit.slot, Some(3));
    assert_eq!(what, "slot 3 hidden");
    // The pane did not change focus.
    assert_eq!(t.st.layout.focus, PaneKind::Transfer);
    // An empty slot says how to fill it.
    assert!(t.key("5").is_empty());
    assert!(t.last_toast().contains("slot 5 is empty"));
    t.key("Alt+2");
    assert_eq!(t.st.layout.focus, PaneKind::Spectrum);
    t.key("Alt+1");
    assert_eq!(t.st.layout.focus, PaneKind::Transfer);
}

#[test]
fn m_averages_the_shown_stored_traces() {
    let mut t = T::new();
    // One shown trace is not enough.
    t.conn(with_traces(vec![stored(4, Some(1), 2)]));
    assert!(t.key("M").is_empty());
    assert!(t.st.toasts.last().is_some_and(|x| x.error));
    let mut hidden = stored(6, Some(3), 2);
    hidden.edit.visible = false;
    let mut target = stored(7, None, 2);
    target.kind = TraceKind::Target;
    t.conn(with_traces(vec![
        stored(4, Some(2), 2),
        stored(5, Some(1), 2),
        hidden,
        target,
    ]));
    let r = t.key("M");
    let [
        Request::Call {
            cmd:
                Command::TraceAverage {
                    traces,
                    method,
                    reference,
                    name,
                },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    // Slot order, hidden and target traces left out, reference = the first.
    assert_eq!(traces, &[TraceId(5), TraceId(4)]);
    assert_eq!(*method, AverageMethod::Power);
    assert_eq!(*reference, DelayReference::Trace { trace: TraceId(5) });
    assert_eq!(name, "avg S1+S2");
    // The phase reference (decision 8b) wins when it is one of them.
    t.st.view.tf.phase_reference = Some(TraceKey::Stored(TraceId(4)));
    let r = t.key("M");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::TraceAverage {
                reference: DelayReference::Trace { trace: TraceId(4) },
                ..
            },
            ..
        }]
    ));
    // Complex averaging from the palette.
    t.key("Ctrl+K");
    t.text("average complex");
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::TraceAverage {
                method: AverageMethod::Complex,
                ..
            },
            ..
        }]
    ));
}

#[test]
fn a_minus_b_from_the_palette() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(4, Some(2), 2),
        stored(5, Some(7), 2),
    ]));
    t.key("Ctrl+K");
    t.text("A − B dB difference");
    let r = t.key("Enter");
    assert!(
        matches!(
            r.as_slice(),
            [Request::Call {
                cmd: Command::TraceMath {
                    a: TraceId(4),
                    b: TraceId(5),
                    op: MathOp::MagnitudeDifference,
                    ..
                },
                ..
            }]
        ),
        "{r:?}"
    );
    t.key("Ctrl+K");
    t.text("complex division");
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::TraceMath {
                op: MathOp::ComplexDivision,
                ..
            },
            ..
        }]
    ));
}

#[test]
fn z_loads_a_target_curve_file() {
    let mut t = T::new();
    t.type_key("Z", "z");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::ImportFile(ImportRole::Target) && p.text.is_empty()
    ));
    t.text("/home/foh/house.txt");
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Import { path, role: ImportRole::Target }] if path == std::path::Path::new("/home/foh/house.txt")
    ));
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn sessions_from_the_palette() {
    let mut t = T::new();
    t.key("Ctrl+K");
    t.text("session save");
    t.key("Enter");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.kind == PromptKind::SessionSave));
    t.text("friday show");
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Call {
            cmd: Command::FileSave {
                session: SessionRef::Name { name }
            },
            ..
        }] if name == "friday show"
    ));
    t.key("Ctrl+K");
    t.text("session load");
    t.key("Enter");
    t.text("./shows/fri");
    let r = t.key("Enter");
    let [
        Request::Call {
            cmd:
                Command::FileLoad {
                    session: SessionRef::Path { path },
                },
            ..
        },
    ] = r.as_slice()
    else {
        panic!("{r:?}")
    };
    assert!(std::path::Path::new(path).is_absolute());
    assert!(path.ends_with("fri"));
}

#[test]
fn stored_trace_metadata_follows_the_mirror() {
    let mut t = T::new();
    let meta = stored(9, Some(1), 2);
    t.conn(with_traces(vec![meta.clone()]));
    let data = TraceData {
        meta: meta.clone(),
        mag_db: vec![0.0; 3],
        phase_deg: None,
        coherence: None,
    };
    t.conn(ConnEvent::Trace(
        Arc::new(data),
        Arc::new(ac2_proto::GridDef::Log {
            ppo: 1,
            k_min: 0,
            k_max: 2,
        }),
    ));
    let mut hidden = meta.clone();
    hidden.edit.visible = false;
    hidden.edit.slot = None;
    t.conn(with_traces(vec![hidden.clone()]));
    assert_eq!(t.st.traces[&TraceId(9)].0.meta, hidden);
    t.conn(with_traces(vec![]));
    assert!(t.st.traces.is_empty());
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
    t.key("M");
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

fn arrival(ms: f64, level: f64) -> DelayArrival {
    DelayArrival {
        delay: Seconds(ms / 1000.0),
        delay_samples: ms * 48.0,
        level: Db(level),
        phase: Degrees(0.0),
        uncertainty_samples: 0.2,
        misfit: 0.0,
        refined: true,
    }
}

fn finding(outcome: DelayOutcome) -> Box<DelayFinding> {
    Box::new(DelayFinding {
        outcome,
        confidence: DelayConfidence {
            psr_db: Some(Db(20.0)),
            psr_acq_db: None,
            band_snr_db: Some(Db(25.0)),
            excited_fraction: Some(1.0),
            uncertainty_samples: Some(0.2),
            pulse_width_samples: None,
            period: None,
        },
        band: DelayBand::Full,
        observation: Seconds(0.25),
        candidates: vec![],
        found_at: WallNs(0),
    })
}

fn ambiguous() -> Box<DelayFinding> {
    finding(DelayOutcome::Ambiguous {
        reasons: vec![AmbiguityReason::BorderlineLevel],
        ranked: vec![
            arrival(12.5, -11.5),
            arrival(12.7, 0.0),
            arrival(13.4, -6.0),
        ],
        strongest: arrival(12.7, 0.0),
    })
}

fn found(t: &mut T, pick: DelayPick, f: Box<DelayFinding>) -> Vec<Request> {
    t.conn(ConnEvent::DelayFound {
        meas: MeasId(1),
        pick,
        finding: f,
    })
}

fn inserted(r: &[Request]) -> Option<DelayPick> {
    match r {
        [
            Request::Call {
                cmd:
                    Command::DelayInsert {
                        meas: MeasId(1),
                        pick,
                    },
                ..
            },
        ] => Some(*pick),
        _ => None,
    }
}

#[test]
fn accepted_finding_inserts_what_was_asked() {
    let mut t = T::new();
    let f = || {
        finding(DelayOutcome::Accepted {
            first: arrival(12.5, -3.0),
            strongest: arrival(12.7, 0.0),
        })
    };
    let r = found(&mut t, DelayPick::FirstArrival, f());
    assert_eq!(inserted(&r), Some(DelayPick::FirstArrival));
    let r = found(&mut t, DelayPick::Strongest, f());
    assert_eq!(inserted(&r), Some(DelayPick::Strongest));
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn ambiguous_first_arrival_opens_the_candidate_list() {
    let mut t = T::new();
    t.key("X");
    let r = found(&mut t, DelayPick::FirstArrival, ambiguous());
    assert!(r.is_empty(), "{r:?}");
    let Overlay::DelayPick(c) = &t.st.overlay else {
        panic!("no candidate list: {:?}", t.st.overlay);
    };
    let rows = c.rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].text, "12.50 ms  −11.5 dB · rule pick");
    // Keys 1–3 pick (and do not focus panes while the list is up).
    let r = t.key("2");
    assert_eq!(inserted(&r), Some(DelayPick::Ranked { index: 1 }));
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.layout.focus, PaneKind::Transfer);
    // 1 is the rule's pre-selection.
    found(&mut t, DelayPick::FirstArrival, ambiguous());
    let r = t.key("1");
    assert_eq!(inserted(&r), Some(DelayPick::Ranked { index: 0 }));
    // Other keys keep working with the list up; Esc closes it.
    found(&mut t, DelayPick::FirstArrival, ambiguous());
    t.key("Alt+4");
    assert_eq!(t.st.layout.focus, PaneKind::Spl);
    assert!(matches!(t.st.overlay, Overlay::DelayPick(_)));
    t.key("Esc");
    assert_eq!(t.st.overlay, Overlay::None);
    // Shift+X on an ambiguous finding: the strongest is well defined, insert it.
    let r = found(&mut t, DelayPick::Strongest, ambiguous());
    assert_eq!(inserted(&r), Some(DelayPick::Strongest));
}

#[test]
fn refusal_inserts_nothing_and_says_why() {
    let mut t = T::new();
    let r = found(
        &mut t,
        DelayPick::FirstArrival,
        finding(DelayOutcome::NoEstimate {
            reasons: vec![NoEstimateReason::LowPsr],
        }),
    );
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(t.st.toasts.last().is_some_and(|x| x.error));
    assert_eq!(t.last_toast(), "Main L: no delay estimate (no clear peak)");
    // The banner over the transfer pane carries the reason too.
    let mut s = daemon_state();
    if let Some(d) = s.measurements[1].delay.as_mut() {
        d.last_finding = Some(*finding(DelayOutcome::NoEstimate {
            reasons: vec![NoEstimateReason::LowPsr, NoEstimateReason::LowBandSnr],
        }));
    }
    t.conn(mirror(s));
    let m = t.st.meas(MeasId(1)).cloned().expect("meas");
    let why = ac2_scene::banner::no_delay_estimate(&m);
    let b = ac2_scene::banner::banners(&ac2_scene::banner::Status {
        no_delay_estimate: why,
        ..Default::default()
    });
    assert_eq!(
        b[0].detail.as_deref(),
        Some("finder: no clear peak, too noisy in band")
    );
}

#[test]
fn own_client_id_follows_the_daemon_incarnation() {
    let mut t = T::new();
    assert_eq!(t.st.my_client_id(), Some(&ClientId("c1".into())));
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("c1".into()));
    s.generator.armed = true;
    // The daemon restarted: the mirror shows the new incarnation, and until its welcome
    // arrives this client has no identity there; the old id is never "me".
    t.conn(mirror_of(s.clone(), 2, None));
    assert_eq!(t.st.my_client_id(), None);
    // The new welcome binds another id; an owner "c1" (another client now) is not us.
    t.conn(mirror_of(s, 2, Some("c9")));
    assert_eq!(t.st.my_client_id(), Some(&ClientId("c9".into())));
}

fn inputs_call(r: &[Request]) -> Option<Vec<InputSetup>> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::SessionInputs { inputs },
            ..
        } => Some(inputs.clone()),
        _ => None,
    })
}

#[test]
fn input_mics_prompt_sets_the_input_setup() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("M30".into()),
        mic_curve: false,
    }];
    t.conn(mirror(s));
    let r = t.st.update(Msg::Command(CommandId::InputMics), &t.keys);
    assert!(r.is_empty());
    match &t.st.overlay {
        Overlay::Prompt(p) => {
            assert_eq!(p.kind, PromptKind::InputMics);
            assert_eq!(p.text, "2=M30");
        }
        o => panic!("{o:?}"),
    }
    for _ in 0..3 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text(", 3= ECM 8000 ");
    let r = t.key("Enter");
    assert_eq!(
        inputs_call(&r),
        Some(vec![
            InputSetup {
                channel: 1,
                mic: None,
                mic_curve: false,
            },
            InputSetup {
                channel: 2,
                mic: Some("ECM 8000".into()),
                mic_curve: true,
            },
        ]),
        "{r:?}"
    );
    // Bad text keeps the prompt open with the reason.
    t.st.update(Msg::Command(CommandId::InputMics), &t.keys);
    for _ in 0..5 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("M30");
    let r = t.key("Enter");
    assert!(inputs_call(&r).is_none());
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
}

#[test]
fn mic_curve_toggles_the_selected_measurements_input() {
    let mut t = T::new();
    // Main L (transfer): its measurement input is 1 (shown as 2); curve on by default.
    let r = t.st.update(Msg::Command(CommandId::MicCurve), &t.keys);
    assert_eq!(
        inputs_call(&r),
        Some(vec![InputSetup {
            channel: 1,
            mic: None,
            mic_curve: false,
        }])
    );
    assert!(t.last_toast().contains("no mic name"), "{}", t.last_toast());
    let mut s = daemon_state();
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("M30".into()),
        mic_curve: false,
    }];
    t.conn(mirror(s));
    let r = t.st.update(Msg::Command(CommandId::MicCurve), &t.keys);
    assert_eq!(
        inputs_call(&r),
        Some(vec![InputSetup {
            channel: 1,
            mic: Some("M30".into()),
            mic_curve: true,
        }])
    );
}

#[test]
fn mic_text_round_trips() {
    let rows = vec![
        InputSetup {
            channel: 0,
            mic: Some("M30 #1".into()),
            mic_curve: true,
        },
        InputSetup {
            channel: 3,
            mic: Some("ECM".into()),
            mic_curve: false,
        },
    ];
    assert_eq!(mics_text(&rows), "1=M30 #1, 4=ECM");
    assert_eq!(
        parse_mics(&mics_text(&rows)),
        Ok(vec![(0, Some("M30 #1".into())), (3, Some("ECM".into()))])
    );
    for bad in ["", "M30", "0=M30", "x=M30", "1=a, 1=b"] {
        assert!(parse_mics(bad).is_err(), "{bad:?}");
    }
}

fn with_output_device(dev: &str) -> State {
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.output_device = DeviceId(dev.into());
    }
    s
}

fn prompt_text(t: &mut T, c: CommandId, text: &str) -> Vec<Request> {
    t.st.update(Msg::Command(c), &t.keys);
    if let Overlay::Prompt(p) = &mut t.st.overlay {
        p.text.clear();
    }
    t.text(text);
    t.key("Enter")
}

#[test]
fn stimulus_outputs_are_remembered_per_device() {
    // First run: output 1, nothing to save.
    let mut t = T::new();
    assert_eq!(t.st.stimulus.outputs, vec![0]);
    assert!(t.st.stimulus.describe().ends_with("→ out 1"));
    assert!(!t.st.prefs_dirty);
    // Choosing outputs remembers them for the session's output device.
    prompt_text(&mut t, CommandId::StimulusOutputs, "2, 3");
    assert_eq!(t.st.stimulus.outputs, vec![1, 2]);
    assert!(t.st.prefs_dirty);
    assert_eq!(t.st.prefs.outputs_for("fake:loop"), Some(&[1u16, 2][..]));

    // Another device: never used, so output 1; back on the first, its outputs return.
    t.conn(mirror(with_output_device("hw:UMC1820")));
    assert_eq!(t.st.stimulus.outputs, vec![0]);
    t.conn(mirror(with_output_device("fake:loop")));
    assert_eq!(t.st.stimulus.outputs, vec![1, 2]);

    // A later run starts from the saved preferences.
    let mut t2 = T::disconnected();
    t2.st.prefs = t.st.prefs.clone();
    t2.conn(ConnEvent::Connected {
        target: "local daemon".into(),
        server: "ac2d test".into(),
        client_id: ClientId("c1".into()),
    });
    t2.conn(mirror(daemon_state()));
    assert_eq!(t2.st.stimulus.outputs, vec![1, 2]);
}

#[test]
fn outputs_never_change_under_a_held_stimulus() {
    let mut t = T::new();
    t.st.prefs.outputs.insert("hw:UMC1820".into(), vec![3]);
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.conn(mirror(with_output_device("hw:UMC1820")));
    assert_eq!(t.st.stimulus.outputs, vec![0]);
    // Once stopped, the device's remembered outputs apply.
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    t.conn(mirror(with_output_device("hw:UMC1820")));
    assert_eq!(t.st.stimulus.outputs, vec![3]);
}

fn cal_delete(r: &[Request]) -> Option<(CalKey, CalPart)> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::CalDelete { key, part },
            ..
        } => Some((key.clone(), *part)),
        _ => None,
    })
}

#[test]
fn calibrations_are_deleted_from_the_palette() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("M30".into()),
        mic_curve: true,
    }];
    t.conn(mirror(s));
    // Prefilled with the selected measurement's input and its mic.
    t.st.update(Msg::Command(CommandId::CalDeleteCurve), &t.keys);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::CalDelete(CalPart::MicCurve) && p.text == "2=M30"
    ));
    let r = t.key("Enter");
    let key = CalKey {
        device: DeviceId("fake:loop".into()),
        channel: 1,
        mic: "M30".into(),
    };
    assert_eq!(cal_delete(&r), Some((key.clone(), CalPart::MicCurve)));
    let r = prompt_text(&mut t, CommandId::CalDelete, "2=M30");
    assert_eq!(cal_delete(&r), Some((key.clone(), CalPart::All)));
    let r = prompt_text(&mut t, CommandId::CalDeleteSensitivity, "4=ECM 8000");
    assert_eq!(
        cal_delete(&r),
        Some((
            CalKey {
                channel: 3,
                mic: "ECM 8000".into(),
                ..key
            },
            CalPart::Sensitivity
        ))
    );
    // A mic name is required; the prompt stays with the reason.
    let r = prompt_text(&mut t, CommandId::CalDelete, "2=");
    assert!(cal_delete(&r).is_none());
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    // Without a session there is no device to name.
    let mut s = daemon_state();
    s.session.open = None;
    t.conn(mirror(s));
    let r = prompt_text(&mut t, CommandId::CalDelete, "2=M30");
    assert!(cal_delete(&r).is_none());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.contains("cal rm"))
    ));
}

fn find_request(r: &[Request]) -> Option<(FinderBand, Option<Seconds>)> {
    r.iter().find_map(|r| match r {
        Request::FindDelay {
            band, observation, ..
        } => Some((*band, *observation)),
        _ => None,
    })
}

#[test]
fn finder_band_and_observation_are_the_operators_choice() {
    let mut t = T::new();
    t.key("Alt+1");
    assert_eq!(find_request(&t.key("X")), Some((FinderBand::Auto, None)));
    t.st.update(Msg::Command(CommandId::FinderSub), &t.keys);
    assert!(t.last_toast().contains("sub band · auto observation"));
    assert_eq!(find_request(&t.key("X")), Some((FinderBand::Sub, None)));
    // The sub band observes 2, 4 or 8 s.
    prompt_text(&mut t, CommandId::FinderObservation, "3");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    t.key("Escape");
    prompt_text(&mut t, CommandId::FinderObservation, "8 s");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(
        t.last_toast().contains("sub band · 8 s"),
        "{}",
        t.last_toast()
    );
    assert_eq!(
        find_request(&t.key("Shift+X")),
        Some((FinderBand::Sub, Some(Seconds(8.0))))
    );
    // Another band starts from its automatic observation.
    t.st.update(Msg::Command(CommandId::FinderMid), &t.keys);
    assert_eq!(find_request(&t.key("X")), Some((FinderBand::Mid, None)));
    prompt_text(&mut t, CommandId::FinderObservation, "0,5");
    assert_eq!(
        find_request(&t.key("X")),
        Some((FinderBand::Mid, Some(Seconds(0.5))))
    );
    // Empty: automatic again; above 8 s is refused.
    prompt_text(&mut t, CommandId::FinderObservation, "");
    assert_eq!(t.st.finder.observation, None);
    prompt_text(&mut t, CommandId::FinderObservation, "9");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    t.key("Escape");
    // Custom edges.
    prompt_text(&mut t, CommandId::FinderCustom, "80 – 800 Hz");
    assert_eq!(
        find_request(&t.key("X")),
        Some((
            FinderBand::Custom {
                lo_hz: Hz(80.0),
                hi_hz: Hz(800.0)
            },
            None
        ))
    );
    prompt_text(&mut t, CommandId::FinderCustom, "800-80");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    t.key("Escape");
    // A custom band reaching below 150 Hz is a sub band: 2, 4 or 8 s.
    prompt_text(&mut t, CommandId::FinderObservation, "1");
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
    t.key("Escape");
    t.st.update(Msg::Command(CommandId::FinderAuto), &t.keys);
    assert_eq!(t.st.finder, FinderChoice::default());
}

// ----- audio session and measurement dialogs ----------------------------------------------

fn no_session_state() -> State {
    let mut s = empty_state();
    s.session.open = None;
    s
}

fn backends(fake_names: bool) -> Vec<BackendInfo> {
    let dir = |ch, names: Option<Vec<&str>>| DirectionInfo {
        max_channels: ch,
        rates_hz: vec![RangeU32 {
            min: 48_000,
            max: 48_000,
        }],
        buffer_frames: Some(RangeU32 { min: 256, max: 256 }),
        default_rate_hz: Some(48_000),
        default_buffer_frames: Some(256),
        channel_names: names.map(|n| n.into_iter().map(String::from).collect()),
    };
    let dev = |backend, id: &str, names: bool| DeviceInfo {
        backend,
        host: "test".into(),
        id: DeviceId(id.into()),
        name: id.into(),
        input: Some(dir(
            4,
            names.then(|| vec!["Loop return", "Room mic", "Line 3", "Line 4"]),
        )),
        output: Some(dir(2, None)),
        duplex_clock: ClockRelation::SingleCallback,
        index: IndexExactness::Exact,
        notes: vec![],
    };
    vec![
        BackendInfo {
            kind: BackendKind::Fake,
            description: "Simulated rig".into(),
            availability: Availability::Available,
            devices: vec![dev(BackendKind::Fake, "fake:loop", fake_names)],
        },
        BackendInfo {
            kind: BackendKind::Jack,
            description: "JACK".into(),
            availability: Availability::Unavailable {
                reason: "JACK server not running".into(),
            },
            devices: vec![],
        },
    ]
}

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

fn dialog(t: &T) -> &SessionDialog {
    match &t.st.overlay {
        Overlay::Session(d) => d,
        other => panic!("no session dialog: {other:?}"),
    }
}

fn form(t: &T) -> &crate::forms::Form {
    match &t.st.overlay {
        Overlay::Form(f) => f,
        other => panic!("no dialog open: {other:?}"),
    }
}

fn created(r: &[Request]) -> Option<&MeasConfig> {
    r.iter().find_map(|r| match r {
        Request::CreateMeas { config } => Some(config),
        _ => None,
    })
}

fn opened(r: &[Request]) -> Option<(&SessionConfig, &[InputSetup], &[MeasConfig])> {
    r.iter().find_map(|r| match r {
        Request::OpenSession {
            config,
            inputs,
            transfers,
            ..
        } => Some((config, inputs.as_slice(), transfers.as_slice())),
        _ => None,
    })
}

fn previewed(r: &[Request]) -> Option<&DeviceId> {
    r.iter().find_map(|r| match r {
        Request::Preview { device, .. } => Some(device),
        _ => None,
    })
}

/// Focuses the session dialog row `row` with ↓ from the top.
fn focus(t: &mut T, row: Row) {
    for _ in 0..64 {
        if dialog(t).focus == row {
            return;
        }
        t.key("Down");
    }
    panic!("row {row:?} not reached");
}

/// Shift+O on a daemon without a session, the simulated rig listed.
fn open_dialog(t: &mut T, list: Vec<BackendInfo>) -> Vec<Request> {
    t.conn(mirror(no_session_state()));
    let mut r = t.type_key("Shift+O", "O");
    r.extend(t.conn(ConnEvent::Devices(Ok(list))));
    r
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

#[test]
fn empty_hints_guide_to_a_session_then_a_measurement() {
    let mut t = T::disconnected();
    assert_eq!(t.st.empty_hint(&t.keys), None);
    connected_to(&mut t, "local daemon");
    // Connected but not synced: nothing to say yet.
    assert_eq!(t.st.empty_hint(&t.keys), None);
    t.conn(mirror(no_session_state()));
    assert_eq!(
        t.st.empty_hint(&t.keys).as_deref(),
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
    let hint = t.st.empty_hint(&t.keys).unwrap_or_default();
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
        t.st.empty_hint(&keys).as_deref(),
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
                mic_curve: true
            },
            InputSetup {
                channel: 1,
                mic: Some("M30 FOH".into()),
                mic_curve: true
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
fn esc_closes_the_dialog_stops_the_stimulus_and_the_preview() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    t.st.stimulus.phase = StimPhase::Firing;
    let r = t.key("Escape");
    assert!(r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");
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
        latest.frames.insert(f.topic.to_string(), f);
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
    let r =
        t.st.update(Msg::Command(CommandId::DeleteMeasurement), &t.keys);
    assert!(
        matches!(r.as_slice(), [Request::Call { cmd: Command::MeasDelete { meas: MeasId(1) }, what }]
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
        t.st.update(Msg::Command(CommandId::DeleteMeasurement), &t.keys)
            .is_empty()
    );
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
            MeasKind::Transfer { .. } => ("tf", 99),
        };
        assert_eq!(kind, (want, 1));
    }
    // Inputs are picked by name among the captured ones: ←/→ walks them.
    t.st.update(Msg::Command(CommandId::NewSpl), &t.keys);
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(1));
    assert_eq!(form(&t).fields[0].display(), "2 · Input 2");
    t.key("Right");
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

    // A daemon that already has a session: no dialog.
    let mut t = T::disconnected();
    t.st.open_session_when_empty = true;
    connected_to(&mut t, "embedded daemon (cpal)");
    assert!(t.conn(mirror(daemon_state())).is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(!t.st.open_session_when_empty);
}
