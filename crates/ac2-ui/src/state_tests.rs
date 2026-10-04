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
            smoothing: None,
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
    t.key("H");
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
    // Transfer pane: Shift+I hides the IR pane.
    t.key("Shift+I");
    assert!(!t.st.layout.is_shown(PaneKind::Ir));
    t.key("Shift+I");
    // Spectrum pane: P is peak hold.
    t.key("Alt+2");
    assert_eq!(t.st.scope(), Scope::Spectrum);
    t.key("P");
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
    // The spectrum picked in the list: the spectrum pane takes the keys, and a transfer
    // command from the palette says what it needs.
    t.st.update(Msg::SelectMeas(MeasId(2)), &t.keys);
    assert_eq!(t.st.selected, Some(MeasId(2)));
    assert_eq!(t.st.layout.focus, PaneKind::Spectrum);
    assert!(t.key("X").is_empty());
    assert!(
        t.st.update(Msg::Command(CommandId::InsertDelay), &t.keys)
            .is_empty()
    );
    assert!(t.last_toast().contains("transfer-function"));
    // N in the transfer pane only goes through transfer measurements.
    t.st.layout.focus = PaneKind::Transfer;
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
            MeasKind::Spectrum { config } => (
                format!("m{}", meas.0),
                config.smoothing.map(|fraction| Smoothing {
                    fraction,
                    mode: SmoothingMode::Magnitude,
                }),
            ),
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
    assert_eq!(
        t.st.smoothing_caption(PaneKind::Transfer).as_deref(),
        Some("smoothing off")
    );
    // K coarser: off → 1/48 of magnitude and phase, with the measurement's config
    // otherwise unchanged.
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "m1".into(),
            smoothing(
                SmoothingFraction::FortyEighth,
                SmoothingMode::MagnitudePhase
            )
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

    // The mirror says 1/6 magnitude only (set by a script): K → 1/3 (mode kept),
    // Shift+K → 1/12, at 1/3 K stops.
    let mut s = daemon_state();
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Sixth, SmoothingMode::Magnitude);
    }
    t.conn(mirror(s.clone()));
    assert_eq!(
        t.st.smoothing_caption(PaneKind::Transfer).as_deref(),
        Some("smoothing 1/6 oct mag only")
    );
    assert_eq!(
        smoothing_set(&t.key("K")).1,
        smoothing(SmoothingFraction::Third, SmoothingMode::Magnitude)
    );
    assert_eq!(
        smoothing_set(&t.key("Shift+K")).1,
        smoothing(SmoothingFraction::Twelfth, SmoothingMode::Magnitude)
    );
    if let MeasKind::Transfer { config } = &mut s.measurements[1].config.kind {
        config.smoothing = smoothing(SmoothingFraction::Third, SmoothingMode::Magnitude);
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
        smoothing(SmoothingFraction::TwentyFourth, SmoothingMode::Magnitude)
    );
    // In the spectrum pane K acts on its spectrum: power smoothing, no phase to name.
    t.key("Alt+2");
    assert_eq!(
        t.st.smoothing_caption(PaneKind::Spectrum).as_deref(),
        Some("smoothing off")
    );
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "m2".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Magnitude)
        )
    );
    match r.as_slice() {
        [Request::Call { what, .. }] => assert_eq!(what, "Sub: smoothing 1/48 oct"),
        other => panic!("{other:?}"),
    }
    // The transfer pane's caption still speaks of its own measurement.
    assert_eq!(
        t.st.smoothing_caption(PaneKind::Transfer).as_deref(),
        Some("smoothing 1/3 oct mag only")
    );
}

/// RTA bands are fractional-octave already: K in the spectrum pane on an RTA says so and
/// sends nothing.
#[test]
fn smoothing_keys_explain_rta() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements[0].config.kind = MeasKind::Rta {
        config: ac2_proto::model::RtaConfig::on_input(1, ac2_proto::model::BandFraction::Third),
    };
    t.conn(mirror(s));
    t.key("Alt+2");
    assert_eq!(t.st.smoothing_caption(PaneKind::Spectrum), None);
    assert!(t.key("K").is_empty());
    assert!(
        t.last_toast().contains("already are fractional-octave"),
        "{}",
        t.last_toast()
    );
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
        t.st.smoothing_caption(PaneKind::Transfer).as_deref(),
        Some("slot 3 (t10): smoothing off")
    );
    let r = t.key("K");
    assert_eq!(
        smoothing_set(&r),
        (
            "t10".into(),
            smoothing(
                SmoothingFraction::FortyEighth,
                SmoothingMode::MagnitudePhase
            )
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
    // A spectrum slot is power-smoothed; its caption goes to the spectrum pane.
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    assert_eq!(
        t.st.smoothing_caption(PaneKind::Spectrum).as_deref(),
        Some("slot 4 (t11): smoothing off")
    );
    assert_eq!(
        t.st.smoothing_caption(PaneKind::Transfer).as_deref(),
        Some("smoothing off")
    );
    assert_eq!(
        smoothing_set(&t.key("K")),
        (
            "t11".into(),
            smoothing(SmoothingFraction::FortyEighth, SmoothingMode::Magnitude)
        )
    );
    // A locked slot says so and sends nothing.
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
fn traces_are_selected_from_the_keyboard() {
    let mut t = T::new();
    // No shown trace: V says so.
    assert!(t.key("V").is_empty());
    assert!(t.last_toast().contains("no shown stored traces"));
    let mut hidden = stored(11, Some(2), 2);
    hidden.edit.visible = false;
    let mut hidden_sweep = sweep_meta(14);
    hidden_sweep.edit.visible = false;
    t.conn(with_traces(vec![
        stored(12, Some(5), 2),
        hidden,
        stored(10, Some(1), 2),
        stored(13, None, 2),
        hidden_sweep,
    ]));
    // The list's order: slotted by slot, then the rest oldest first.
    let order: Vec<u32> = t.st.trace_list().iter().map(|x| x.id.0).collect();
    assert_eq!(order, [10, 11, 12, 13, 14]);
    // V steps through the shown traces in that order, slotted or not (hidden skipped), then
    // back to the live measurement; it sends nothing.
    assert!(t.key("V").is_empty());
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    assert!(t.last_toast().contains("slot 1 (t10) selected"));
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(12)));
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    assert_eq!(t.last_toast(), "t13 selected");
    t.key("V");
    assert_eq!(t.st.selected_trace, None);
    assert!(t.last_toast().contains("live measurement"));
    // Shift+V goes the other way, from live to the last shown trace.
    t.key("Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    // K then changes the selected trace, though it has no slot.
    assert_eq!(smoothing_set(&t.key("K")).0, "t13");
    t.key("Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(12)));
    // Alt+V reaches the hidden ones too.
    t.key("Alt+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    t.key("Alt+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(14)));
    assert_eq!(t.last_toast(), "t14 (hidden) selected");
    t.key("Alt+Shift+V");
    t.key("Alt+Shift+V");
    t.key("Alt+Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(11)));
    // From a hidden trace, V and Shift+V go to the shown ones beside it.
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(12)));
    t.st.update(Msg::SelectTrace(TraceId(11)), &t.keys);
    t.key("Shift+V");
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    // The palette entry deselects.
    t.st.update(Msg::Command(CommandId::SelectLive), &t.keys);
    assert_eq!(t.st.selected_trace, None);
    // Esc with a dialog open only closes it (and stops); with nothing open it also hands
    // the keys back to the live measurement.
    t.key("V");
    t.key("H");
    assert!(t.key("Esc").is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    t.key("Esc");
    assert_eq!(t.st.selected_trace, None);
    assert_eq!(smoothing_set(&t.key("K")).0, "m1");
    // N (another measurement) deselects too.
    t.key("V");
    t.key("N");
    assert_eq!(t.st.selected_trace, None);
}

/// The TraceUpdate a reply carries, with its toast text.
fn trace_update(r: &[Request]) -> (TraceId, TraceEdit, String) {
    match r {
        [
            Request::Call {
                cmd: Command::TraceUpdate { trace, edit },
                what,
            },
        ] => (*trace, edit.clone(), what.clone()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_shows_and_hides_the_selected_trace_and_the_eye_any_trace() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        stored(13, None, 2),
    ]));
    // Nothing selected: A says how to select.
    assert!(t.key("A").is_empty());
    assert!(t.last_toast().contains("select a stored trace first"));
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    let (id, edit, what) = trace_update(&t.key("A"));
    assert_eq!(id, TraceId(13));
    assert!(!edit.visible);
    assert_eq!(edit.slot, None);
    assert_eq!(what, "t13 hidden");
    // The eye in the list toggles any trace, selected or not, and selects nothing.
    let (id, edit, what) = trace_update(&t.st.update(Msg::ToggleShown(TraceId(10)), &t.keys));
    assert_eq!(id, TraceId(10));
    assert!(!edit.visible);
    assert_eq!(what, "slot 1 (t10) hidden");
    assert_eq!(t.st.selected_trace, Some(TraceId(13)));
    // The list says what each one is and which is selected.
    let rows = t.st.trace_rows();
    assert_eq!(
        rows.iter()
            .map(|r| (r.name.as_str(), r.details[0].as_str(), r.selected))
            .collect::<Vec<_>>(),
        [
            ("t10", "capture · slot 1 · no data yet", false),
            ("t13", "capture · no data yet", true),
        ]
    );
}

#[test]
fn the_selected_trace_moves_to_a_slot() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        stored(13, None, 2),
    ]));
    t.st.update(Msg::Command(CommandId::TraceSlot), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None, "nothing selected");
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    t.st.update(Msg::Command(CommandId::TraceSlot), &t.keys);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceSlot(TraceId(13)) && p.text.is_empty()
    ));
    t.text("1");
    // The daemon takes slot 1 from its holder.
    let (id, edit, what) = trace_update(&t.key("Enter"));
    assert_eq!((id, edit.slot, edit.visible), (TraceId(13), Some(1), true));
    assert_eq!(what, "t13 in slot 1");
    let r = prompt_text(&mut t, CommandId::TraceSlot, "none");
    let (_, edit, what) = trace_update(&r);
    assert_eq!(edit.slot, None);
    assert_eq!(what, "t13: slot freed");
    let r = prompt_text(&mut t, CommandId::TraceSlot, "10");
    assert!(r.is_empty());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.contains("1 … 9"))
    ));
    assert_eq!(parse_slot("slot 3"), Ok(Some(3)));
    assert_eq!(parse_slot(" "), Ok(None));
    assert!(parse_slot("0").is_err());
}

/// F2 (or the palette) renames the selected trace; a double click on a row in the list
/// selects it and asks for the name in one go.
#[test]
fn the_selected_trace_is_renamed() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        stored(13, None, 2),
    ]));
    t.st.update(Msg::Command(CommandId::TraceRename), &t.keys);
    assert_eq!(t.st.overlay, Overlay::None, "nothing selected");
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    t.key("F2");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceRename(TraceId(13)) && p.text == "t13"
    ));
    let r = prompt_text(&mut t, CommandId::TraceRename, "  1083 on axis ");
    let (id, edit, what) = trace_update(&r);
    assert_eq!((id, edit.name.as_str()), (TraceId(13), "1083 on axis"));
    assert_eq!(what, "t13 renamed to 1083 on axis");
    // An empty name is refused in the prompt.
    let r = prompt_text(&mut t, CommandId::TraceRename, "   ");
    assert!(r.is_empty());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref() == Some("type a name")
    ));
    t.key("Escape");
    // Double click on another row: selected, and its name asked for.
    t.st.update(Msg::RenameTrace(TraceId(10)), &t.keys);
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceRename(TraceId(10)) && p.text == "t10"
    ));
}

#[test]
fn trace_keys_act_on_the_selected_trace() {
    let mut t = T::new();
    let mut locked = stored(12, None, 2);
    locked.edit.locked = true;
    let mut target = stored(15, None, 2);
    target.kind = TraceKind::Target;
    let mut spec = stored(16, None, 2);
    spec.kind = TraceKind::Spectrum {
        scale: LevelScale::Dbfs,
    };
    t.conn(with_traces(vec![
        stored(13, None, 2),
        locked,
        target,
        spec,
        sweep_meta(14),
    ]));
    // An unslotted capture: U, J, , . and E change it, not the live measurement.
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    let (id, edit, what) = trace_update(&t.key("U"));
    assert_eq!((id, edit.polarity), (TraceId(13), Polarity::Inverted));
    assert_eq!(what, "t13: polarity inverted");
    t.type_key("J", "j");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceOffset(TraceId(13)) && p.text.is_empty()
    ));
    t.text("-3,5 dB");
    let (_, edit, what) = trace_update(&t.key("Enter"));
    assert_eq!(edit.offset, Db(-3.5));
    assert_eq!(what, "t13: offset −3.5 dB");
    let (_, edit, _) = trace_update(&t.key("."));
    assert!((edit.delay_nudge.0 - 0.000_1).abs() < 1e-15);
    let (_, edit, _) = trace_update(&t.key(","));
    assert!((edit.delay_nudge.0 + 0.000_1).abs() < 1e-15);
    assert!(t.key("E").is_empty());
    assert_eq!(
        t.st.view.tf.phase_reference,
        Some(TraceKey::Stored(TraceId(13)))
    );
    assert_eq!(t.last_toast(), "phase reference: t13");
    assert_eq!(t.st.edit(MeasId(1)), LiveEdit::default(), "live untouched");
    // A sweep result too.
    t.st.update(Msg::SelectTrace(TraceId(14)), &t.keys);
    assert_eq!(trace_update(&t.key("U")).0, TraceId(14));
    // A locked trace says so and sends nothing.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key("U").is_empty());
    assert!(t.last_toast().contains("t12 is locked"));
    // A target curve has an offset but no phase.
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    assert!(t.key("U").is_empty());
    assert!(t.last_toast().contains("no phase"));
    assert!(t.key("E").is_empty());
    assert!(t.last_toast().contains("no phase"));
    t.type_key("J", "j");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceOffset(TraceId(15))
    ));
    t.key("Esc");
    // The caption names a selected target.
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    assert_eq!(
        t.st.pane_caption(PaneKind::Transfer).as_deref(),
        Some("t15")
    );
    // A spectrum trace is not on the transfer pane: the keys act on the live measurement.
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    assert!(t.key("U").is_empty());
    assert!(t.st.edit(MeasId(1)).inverted);
}

/// One selection: a sweep selected in the transfer pane is what the sweep pane shows, and
/// N on the sweep pane selects the sweep it steps to.
#[test]
fn the_sweep_pane_follows_the_selection_and_selects() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        stored(10, Some(1), 2),
        sweep_meta(14),
        sweep_meta(15),
    ]));
    for id in [14, 15] {
        let (d, g) = sweep_data(id);
        t.conn(ConnEvent::Trace(d, g));
    }
    let shown = |t: &T| t.st.shown_sweep().map(|(d, _)| d.meta.id.0);
    // Nothing chosen: the newest.
    assert_eq!(shown(&t), Some(15));
    // V in the transfer pane: slot 1, then the first sweep, which the sweep pane shows.
    t.key("V");
    assert_eq!(
        shown(&t),
        Some(15),
        "a capture selected: the sweep pane keeps its sweep"
    );
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(14)));
    assert_eq!(shown(&t), Some(14));
    assert_eq!(
        t.st.pane_caption(PaneKind::Transfer).as_deref(),
        Some("t14: smoothing off")
    );
    // Back to live: the sweep pane keeps the sweep selected last.
    t.key("Esc");
    assert_eq!(t.st.selected_trace, None);
    assert_eq!(shown(&t), Some(14));
    // N on the sweep pane steps the sweeps and selects them for the transfer pane.
    t.key("Alt+5");
    assert_eq!(t.st.layout.focus, PaneKind::Distortion);
    t.key("N");
    assert_eq!(shown(&t), Some(15));
    assert_eq!(t.st.selected_trace, Some(TraceId(15)));
    assert_eq!(t.last_toast(), "t15 selected");
    t.key("Shift+N");
    assert_eq!(t.st.selected_trace, Some(TraceId(14)));
    // U on the sweep pane is its unit; on the transfer pane it inverts the selected sweep.
    assert!(t.key("U").is_empty());
    t.key("Alt+1");
    assert_eq!(trace_update(&t.key("U")).0, TraceId(14));
    // A click on a sweep in the list does the same.
    t.st.update(Msg::SelectTrace(TraceId(15)), &t.keys);
    assert_eq!(shown(&t), Some(15));
}

#[test]
fn escape_stops_the_stimulus_with_a_slot_selected() {
    let mut t = T::new();
    t.conn(with_traces(vec![stored(10, Some(1), 2)]));
    t.st.stimulus.level = Some(Dbfs(-20.0));
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.key("V");
    assert_eq!(t.st.selected_trace, Some(TraceId(10)));
    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
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
        sweep: None,
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
    m2.edit.smoothing = smoothing(SmoothingFraction::Sixth, SmoothingMode::Magnitude);
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
        sweep: None,
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
        mic_curve: None,
        created_at: WallNs(0),
    }
}

fn curve_ref(label: &str) -> MicCurveRef {
    MicCurveRef {
        label: label.into(),
        file_name: format!("{label}.txt"),
        content_hash: "0".into(),
        points: 2,
        f_lo: Hz(20.0),
        f_hi: Hz(20_000.0),
        imported_at: WallNs(0),
        stated_sensitivity: Some(15.0),
    }
}

/// MM1 34804 with its 0° and 90° curves.
fn mm1() -> Mic {
    Mic {
        name: "MM1 34804".into(),
        curves: vec![curve_ref("0°"), curve_ref("90°")],
    }
}

#[test]
fn mic_curve_goes_on_the_selected_trace_from_the_palette() {
    let mut t = T::new();
    let mut a = stored(10, Some(3), 2);
    a.mic = Some(MicState {
        name: "MM1 34804".into(),
        curve: None,
    });
    let mut s = daemon_state();
    s.traces = vec![a];
    s.mics = vec![mm1()];
    t.conn(mirror(s));
    // Nothing selected: refused with the way to select.
    t.st.update(Msg::Command(CommandId::TraceMicCurve), &t.keys);
    assert!(!matches!(&t.st.overlay, Overlay::Prompt(_)));
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    // Prefilled with the mic the trace was captured with.
    t.st.update(Msg::Command(CommandId::TraceMicCurve), &t.keys);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceMicCurve(TraceId(10))
            && p.text == "MM1 34804 "
    ));
    let call = |r: &[Request]| {
        r.iter().find_map(|r| match r {
            Request::Call {
                cmd: Command::TraceMicCurve { trace, curve },
                ..
            } => Some((*trace, curve.clone())),
            _ => None,
        })
    };
    // Two curves: the label is required, and the prompt says which there are.
    let r = t.key("Enter");
    assert!(call(&r).is_none());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.contains("0°, 90°"))
    ));
    t.text("90°");
    let r = t.key("Enter");
    assert_eq!(
        call(&r),
        Some((
            TraceId(10),
            Some(MicCurveId {
                mic: "MM1 34804".into(),
                label: "90°".into()
            })
        ))
    );
    let r = prompt_text(&mut t, CommandId::TraceMicCurve, "MM1 34804 45°");
    assert!(call(&r).is_none());
    let r = prompt_text(&mut t, CommandId::TraceMicCurve, "none");
    assert_eq!(call(&r), Some((TraceId(10), None)));
    let r = prompt_text(&mut t, CommandId::TraceMicCurve, " ");
    assert!(call(&r).is_none());
    assert!(matches!(&t.st.overlay, Overlay::Prompt(p) if p.error.is_some()));
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
    assert_eq!(what, "slot 3 (t9) hidden");
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
        sweep: None,
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
        curve: CurveChoice::Off,
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
            // Another mic (here none): the old choice is dropped.
            InputSetup {
                channel: 1,
                mic: None,
                curve: CurveChoice::NotChosen,
            },
            InputSetup {
                channel: 2,
                mic: Some("ECM 8000".into()),
                curve: CurveChoice::NotChosen,
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
fn mic_curve_steps_through_the_selected_measurements_input() {
    let mut t = T::new();
    // Main L (transfer): its measurement input is 1 (shown as 2). No mic name: nothing to
    // choose from.
    let r = t.st.update(Msg::Command(CommandId::MicCurve), &t.keys);
    assert_eq!(inputs_call(&r), None);
    assert!(t.last_toast().contains("no mic name"), "{}", t.last_toast());
    let mut s = daemon_state();
    s.mics = vec![mm1()];
    let row = |curve: CurveChoice| InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve,
    };
    let label = |l: &str| CurveChoice::Curve { label: l.into() };
    // off → 0° → 90° → off, each from what the daemon holds.
    for (from, to) in [
        (CurveChoice::Off, label("0°")),
        (label("0°"), label("90°")),
        (label("90°"), CurveChoice::Off),
        (CurveChoice::NotChosen, label("0°")),
    ] {
        s.inputs = vec![row(from.clone())];
        t.conn(mirror(s.clone()));
        let r = t.st.update(Msg::Command(CommandId::MicCurve), &t.keys);
        assert_eq!(inputs_call(&r), Some(vec![row(to.clone())]), "{from:?}");
    }
    // The palette's "Mic curve on input N…": a label of the mic, or off.
    let r = prompt_text(&mut t, CommandId::MicCurveInput, "2=90°");
    assert_eq!(inputs_call(&r), Some(vec![row(label("90°"))]));
    let r = prompt_text(&mut t, CommandId::MicCurveInput, "2=off");
    assert_eq!(inputs_call(&r), Some(vec![row(CurveChoice::Off)]));
    let r = prompt_text(&mut t, CommandId::MicCurveInput, "2=45°");
    assert_eq!(inputs_call(&r), None);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.contains("stored: 0°, 90°"))
    ));
}

#[test]
fn mic_text_round_trips() {
    let rows = vec![
        InputSetup {
            channel: 0,
            mic: Some("M30 #1".into()),
            curve: CurveChoice::NotChosen,
        },
        InputSetup {
            channel: 3,
            mic: Some("ECM".into()),
            curve: CurveChoice::Off,
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
    // First run: output 1, no outputs to remember (the layout is, with the measurements
    // the panes show).
    let mut t = T::new();
    assert_eq!(t.st.stimulus.outputs, vec![0]);
    assert!(t.st.stimulus.describe().ends_with("→ out 1"));
    assert!(t.st.prefs.outputs.is_empty());
    t.st.prefs_dirty = false;
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

fn cal_delete(r: &[Request]) -> Option<CalKey> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::CalDelete { key },
            ..
        } => Some(key.clone()),
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
        curve: CurveChoice::NotChosen,
    }];
    t.conn(mirror(s));
    // Prefilled with the selected measurement's input and its mic.
    t.st.update(Msg::Command(CommandId::CalDelete), &t.keys);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::CalDelete && p.text == "2=M30"
    ));
    let r = t.key("Enter");
    let key = CalKey {
        device: DeviceId("fake:loop".into()),
        channel: 1,
        mic: "M30".into(),
    };
    assert_eq!(cal_delete(&r), Some(key.clone()));
    let r = prompt_text(&mut t, CommandId::CalDelete, "4=ECM 8000");
    assert_eq!(
        cal_delete(&r),
        Some(CalKey {
            channel: 3,
            mic: "ECM 8000".into(),
            ..key
        })
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

/// Stored curves on the transfer pane move the hint out of the plot into the title strip;
/// hidden ones (or no data yet) leave it centred.
#[test]
fn empty_hint_yields_to_stored_traces() {
    let mut t = T::disconnected();
    connected_to(&mut t, "local daemon");
    let meta = stored(10, Some(1), 2);
    let mut s = no_session_state();
    s.traces = vec![meta.clone()];
    t.conn(mirror(s));
    let place = |t: &T| t.st.empty_hint(&t.keys).map(|h| h.place);
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
        })
    };
    t.conn(ConnEvent::Trace(data(&meta), grid.clone()));
    assert!(t.st.transfer_shows_stored());
    let hint = t.st.empty_hint(&t.keys).expect("hint");
    assert_eq!(hint.place, HintPlace::Title);
    assert!(
        hint.text.starts_with("No audio session — "),
        "{}",
        hint.text
    );
    // Hidden: the plot is empty again and the hint goes back to its centre.
    let mut hidden = meta.clone();
    hidden.edit.visible = false;
    let mut s = no_session_state();
    s.traces = vec![hidden.clone()];
    t.conn(mirror(s));
    t.conn(ConnEvent::Trace(data(&hidden), grid));
    assert_eq!(place(&t), Some(HintPlace::Centre));
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
        t.st.empty_hint(&t.keys).map(|h| h.text).as_deref(),
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
    let hint = t.st.empty_hint(&t.keys).map(|h| h.text).unwrap_or_default();
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
        t.st.empty_hint(&keys).map(|h| h.text).as_deref(),
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
                curve: CurveChoice::NotChosen,
            },
            InputSetup {
                channel: 1,
                mic: Some("M30 FOH".into()),
                curve: CurveChoice::NotChosen,
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

/// The sidebar's always-on meters: every captured input by name and role, its reading,
/// and what the running sweep (else the selected measurement) uses it as.
#[test]
fn sidebar_meters_name_every_session_input_by_role() {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{ClipFlags, SessionLevelsFrame};
    use ac2_proto::{Frame, FrameData};
    use ac2_scene::meter::{InputUse, MeterState};
    let mut t = T::new();
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
        o.config.input_channels = vec![0, 1, 2];
        o.config.loopback = Some(LoopbackRoute {
            output: 0,
            input: 0,
        });
    }
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve: CurveChoice::Curve {
            label: "90°".into(),
        },
    }];
    s.mics = vec![mm1()];
    t.conn(mirror(s.clone()));
    let labels = |t: &T| -> Vec<(String, Option<InputUse>)> {
        t.st.session_inputs()
            .into_iter()
            .map(|r| (r.label, r.used))
            .collect()
    };
    // Before the device list: the mic's name, else `Input N`, with the role.
    assert_eq!(
        labels(&t),
        vec![
            ("Input 1 · reference".into(), Some(InputUse::Reference)),
            (
                "MM1 34804 · 90° · mic (in 2)".into(),
                Some(InputUse::Measurement)
            ),
            ("Input 3".into(), None),
        ]
    );
    // The device's channel names once listed.
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    assert_eq!(
        labels(&t),
        vec![
            (
                "Loop return · reference (in 1)".into(),
                Some(InputUse::Reference)
            ),
            (
                "MM1 34804 · 90° · mic (in 2)".into(),
                Some(InputUse::Measurement)
            ),
            ("Line 3 (in 3)".into(), None),
        ]
    );
    // Readings from the session's meters; an input without one reads as no data.
    let mut latest = Latest::default();
    let data = FrameData::SessionLevels(SessionLevelsFrame {
        meta: ac2_proto::frame::LevelsMeta {
            channels: vec![0, 1],
        },
        peak: vec![-10.0, -1.0],
        rms: vec![-20.0, -12.0],
        clip: vec![ClipFlags::NONE, ClipFlags::HELD],
    });
    let f = TopicFrame {
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
    latest.frames.insert(f.topic.to_string(), f);
    t.st.data = Some(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }));
    let rows = t.st.session_inputs();
    assert_eq!(rows[0].reading.with_unit(), "\u{2212}20.0 dBFS");
    assert_eq!(rows[0].reading.state, MeterState::Signal);
    assert_eq!(rows[1].reading.state, MeterState::Clip);
    assert_eq!(rows[2].reading.state, MeterState::NoData);

    // A running sweep's inputs are the marked ones while it runs.
    let mut run = sweep_run(SweepStatus::Playing { repeat: 1 });
    run.reference_input = 2;
    s.sweep = Some(run);
    t.conn(mirror(s.clone()));
    let used: Vec<_> = labels(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(
        used,
        vec![None, Some(InputUse::Measurement), Some(InputUse::Reference)]
    );
}

/// The meters stay subscribed while a session is open, whatever dialog opens and closes,
/// and the subscription ends with the session.
#[test]
fn session_meters_stay_subscribed_while_a_session_is_open() {
    let mut t = T::disconnected();
    connected_to(&mut t, "local daemon");
    let r = t.conn(mirror(daemon_state()));
    assert!(
        r.iter().any(|x| matches!(x, Request::Meters(true))),
        "{r:?}"
    );
    let r = t.st.update(Msg::Command(CommandId::NewTransfer), &t.keys);
    assert!(!r.iter().any(|x| matches!(x, Request::Meters(_))), "{r:?}");
    let r = t.key("Escape");
    assert!(!r.iter().any(|x| matches!(x, Request::Meters(_))), "{r:?}");
    let r = t.conn(mirror(no_session_state()));
    assert!(
        r.iter().any(|x| matches!(x, Request::Meters(false))),
        "{r:?}"
    );
}

/// A set of sweeps on the progress strip: which one of how many, the bar, the time left
/// counted from when each step was seen, analysing, and gone once it ends.
#[test]
fn sweep_progress_strip_counts_steps_and_time_left() {
    let mut t = T::new();
    let tick = |t: &mut T, now_s: f64| {
        t.st.update(Msg::Tick { now_s, dt_s: 0.02 }, &t.keys);
    };
    tick(&mut t, 10.0);
    assert_eq!(t.st.operation(), None);
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("c1".into()));
    s.generator.armed = true;
    s.generator.firing = true;
    let mut run = sweep_run(SweepStatus::Playing { repeat: 1 });
    run.repeats = 2;
    // Each step: 3.1 s of sweep and 1 s of silence.
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.title, "sweep \"Sweep 1\"");
    assert_eq!(p.step, "sweep 1 of 2");
    assert_eq!(p.fraction, 0.0);
    assert_eq!(p.remaining.as_deref(), Some("about 9 s left"));
    tick(&mut t, 12.05);
    let p = t.st.operation().expect("progress");
    assert!((p.fraction - 0.25).abs() < 1e-3, "{}", p.fraction);
    assert_eq!(p.remaining.as_deref(), Some("about 7 s left"));

    tick(&mut t, 14.0);
    run.status = SweepStatus::Playing { repeat: 2 };
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.step, "sweep 2 of 2");
    assert!((p.fraction - 0.5).abs() < 1e-3, "{}", p.fraction);
    assert_eq!(p.remaining.as_deref(), Some("about 5 s left"));

    // Stop (the strip's button sends the same command as Esc).
    let r = t.st.update(Msg::Command(CommandId::StimulusStop), &t.keys);
    assert!(r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");

    run.status = SweepStatus::Analysing;
    s.sweep = Some(run.clone());
    t.conn(mirror(s.clone()));
    let p = t.st.operation().expect("progress");
    assert_eq!(p.step, "analysing…");
    assert_eq!(p.remaining, None);

    run.status = SweepStatus::Failed {
        reason: SweepFailure::Stopped,
        msg: "stopped".into(),
    };
    s.sweep = Some(run);
    t.conn(mirror(s));
    assert_eq!(t.st.operation(), None);
    assert_eq!(t.st.sweep.step_seen, None);
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
    // Inputs are picked by name among the captured ones: ←/→ walks them, stopping at the
    // ends.
    t.st.update(Msg::Command(CommandId::NewSpl), &t.keys);
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(1));
    assert_eq!(form(&t).fields[0].display(), "2 · Input 2");
    t.key("Right");
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(1));
    t.key("Left");
    assert_eq!(form(&t).channel(crate::forms::FieldId::Input), Some(0));
    t.key("Left");
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

    // A daemon that already has a session: no dialog; its input meters and the channel
    // names for them.
    let mut t = T::disconnected();
    t.st.open_session_when_empty = true;
    connected_to(&mut t, "embedded daemon (cpal)");
    let r = t.conn(mirror(daemon_state()));
    assert!(
        matches!(r.as_slice(), [Request::Meters(true), Request::Devices]),
        "{r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(!t.st.open_session_when_empty);
}

fn sweep_run(status: SweepStatus) -> SweepRun {
    SweepRun {
        id: SweepId(4),
        owner: ClientId("c1".into()),
        name: "Sweep 1".into(),
        reference_input: 0,
        measurement_input: 1,
        outputs: vec![0],
        level: Dbfs(-50.0),
        sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
        sweep_duration: Seconds(3.1),
        post_roll: Seconds(1.0),
        repeats: 1,
        gate: None,
        status,
        started_at: WallNs(0),
    }
}

fn sweep_meta(id: u32) -> TraceMeta {
    TraceMeta {
        kind: TraceKind::Sweep,
        source: TraceSource::IrCapture {
            run: SweepId(4),
            epoch: SessionEpoch(2),
            sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
            level: Dbfs(-50.0),
            repeats: 1,
            reference_input: 0,
            measurement_input: 1,
        },
        depth: None,
        ..stored(id, None, 2)
    }
}

fn sweep_data(id: u32) -> (Arc<TraceData>, Arc<GridDef>) {
    let grid = GridDef::Log {
        ppo: 12,
        k_min: -60,
        k_max: 50,
    };
    let n = ac2_scene::grid::column_frequencies(&grid).len();
    let curve = |l: f32| DistortionCurve {
        level_db: vec![l; n],
        floor_db: vec![-80.0; n],
    };
    let data = TraceData {
        meta: sweep_meta(id),
        mag_db: vec![-6.0; n],
        phase_deg: Some(vec![0.0; n]),
        coherence: None,
        sweep: Some(SweepData {
            harmonics: vec![HarmonicCurve {
                order: 2,
                curve: curve(-40.0),
            }],
            thd: curve(-40.0),
            ir: SweepIr {
                t0: Seconds(-0.1),
                dt: Seconds(0.001),
                linear: vec![0.0; 200],
                etc_db: vec![-90.0; 200],
            },
            info: SweepInfo {
                sample_rate: Hz(48_000.0),
                rate: Seconds(0.45),
                duration: Seconds(3.1),
                repeats: 1,
                arrival: Seconds(0.003),
                reference_level: Db(0.0),
                window_pre: Seconds(0.008),
                window_post: Seconds(0.09),
                gate_pre: Seconds(0.03),
                gate: Seconds(0.9),
                floor_margin: Db(6.0),
                clipped: false,
            },
        }),
    };
    (Arc::new(data), Arc::new(grid))
}

/// The sweep from the palette's dialog to the distortion pane, keyboard only: the dialog
/// arms the sweep, Enter plays it, the stored result opens the pane, Esc ends sweep mode.
#[test]
fn sweep_from_the_dialog_to_the_distortion_pane() {
    let mut t = T::new();
    assert!(
        !t.st.layout.is_shown(PaneKind::Distortion),
        "hidden until a sweep"
    );
    t.type_key("Shift+S", "S");
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("no dialog: {:?}", t.st.overlay);
    };
    assert_eq!(f.kind, FormKind::Sweep);
    // Inputs by name, the mic as the measurement, the speaker output. The session declares
    // no loopback: the reference is not guessed.
    assert_eq!(f.channel(crate::forms::FieldId::Reference), None);
    assert_eq!(f.channel(crate::forms::FieldId::Measurement), Some(1));
    assert_eq!(f.channel(crate::forms::FieldId::Output), Some(0));
    assert_eq!(f.text(crate::forms::FieldId::Name), "Sweep 1");
    // No reference, no sweep: the dialog asks for it and stays.
    assert!(t.key("Enter").is_empty());
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("dialog closed");
    };
    assert!(
        f.error
            .as_deref()
            .is_some_and(|e| e.contains("choose the reference")),
        "{:?}",
        f.error
    );
    // The reference field has the focus: → picks the first input.
    t.key("Right");
    // No level, no sweep: the dialog says so and stays.
    assert!(t.key("Enter").is_empty());
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("dialog closed");
    };
    assert!(
        f.error.as_deref().is_some_and(|e| e.contains("level")),
        "{:?}",
        f.error
    );
    let at = f
        .fields
        .iter()
        .position(|x| x.id == crate::forms::FieldId::Level)
        .expect("level field");
    for _ in 0..at {
        t.key("Down");
    }
    t.text("-50");
    let r = t.key("Enter");
    let settings = r
        .iter()
        .find_map(|x| match x {
            Request::StimArm { settings, force } => {
                assert!(!force);
                Some(settings.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no arm: {r:?}"));
    assert_eq!(settings.level, Dbfs(-50.0));
    assert_eq!(settings.outputs, vec![0]);
    assert!(matches!(settings.signal, Signal::Ess { sweep } if sweep.start == Hz(20.0)));
    assert_eq!(t.st.overlay, Overlay::None);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert!(
        t.last_toast().contains("Enter plays the sweep"),
        "{}",
        t.last_toast()
    );

    // Enter plays it: `ir.capture` under the held lease.
    let r = t.key("Enter");
    match r.as_slice() {
        [Request::Sweep { request, name }] => {
            assert_eq!(name, "Sweep 1");
            assert_eq!(request.level, Some(Dbfs(-50.0)));
            assert_eq!(
                request.inputs,
                SweepInputs::Channels {
                    reference: 0,
                    measurement: 1
                }
            );
            assert_eq!(request.repeats, 1);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(t.st.stimulus.phase, StimPhase::FireRequested);
    t.conn(ConnEvent::Stimulus(StimEvent::SweepStarted(Box::new(
        sweep_run(SweepStatus::Playing { repeat: 1 }),
    ))));
    assert_eq!(t.st.stimulus.phase, StimPhase::Firing);

    // Recorded: the daemon has disarmed; the stimulus is off while the analysis runs.
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("c1".into()));
    s.sweep = Some(sweep_run(SweepStatus::Analysing));
    let r = t.conn(mirror(s.clone()));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(!t.st.stimulus_live(), "STIM OFF while analysing");
    assert!(
        !r.iter().any(|x| matches!(x, Request::StimStop)),
        "a stop now would abort the analysis: {r:?}"
    );
    // Stored: the pane appears with it, focused. Nothing is armed again: the lease is given
    // back quietly and sweep mode ends; Shift+S sets up the next one.
    s.traces = vec![sweep_meta(7)];
    s.sweep = Some(sweep_run(SweepStatus::Done { trace: TraceId(7) }));
    let r = t.conn(mirror(s));
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    assert!(t.st.layout.is_shown(PaneKind::Distortion));
    assert_eq!(t.st.layout.focus, PaneKind::Distortion);
    assert!(t.st.sweep.plan.is_none());
    assert_eq!(t.st.stimulus.signal, Signal::Pink);
    assert!(
        t.last_toast().contains("Shift+S sweeps again"),
        "{}",
        t.last_toast()
    );
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(!t.st.stimulus_live(), "STIM OFF after the sweep");
    assert!(
        t.last_toast().contains("sweep stored"),
        "{}",
        t.last_toast()
    );
    // Enter does not play anything: nothing is armed.
    let r = t.key("Enter");
    assert!(
        !r.iter()
            .any(|x| matches!(x, Request::Sweep { .. } | Request::StimSet(_))),
        "{r:?}"
    );
    assert_eq!(t.st.sweep.shown, Some(TraceId(7)));
    let (d, g) = sweep_data(7);
    t.conn(ConnEvent::Trace(d, g));
    assert_eq!(t.st.shown_sweep().map(|(d, _)| d.meta.id), Some(TraceId(7)));
    // The sweep is drawn like a transfer function in the transfer pane too.
    assert_eq!(t.st.view.distortion.unit, DistortionUnit::Db);
    t.key("U");
    assert_eq!(t.st.view.distortion.unit, DistortionUnit::Percent);
    // The pane's dB | % toggle: a click on either sets that unit (again: stays).
    for unit in [
        DistortionUnit::Db,
        DistortionUnit::Db,
        DistortionUnit::Percent,
    ] {
        let r = t.st.update(Msg::DistortionUnit(unit), &t.keys);
        assert!(r.is_empty(), "display only: {r:?}");
        assert_eq!(t.st.view.distortion.unit, unit);
    }
    t.key("Shift+I");
    assert!(t.st.view.distortion.show_ir);
    t.key("G");
    assert_eq!(t.st.view.ir.mode, IrMode::Log);

    // Shift+W hides the pane again.
    t.key("Shift+W");
    assert!(!t.st.layout.is_shown(PaneKind::Distortion));
    assert_eq!(t.st.layout.focus, PaneKind::Transfer);
}

/// A sweep submitted right after Esc, while that stop is still on its way, arms once the
/// stop has landed instead of being dropped with it.
#[test]
fn a_sweep_submitted_during_a_stop_arms_after_it() {
    let mut t = T::new();
    t.st.stimulus.level = Some(Dbfs(-30.0));
    let r = t.key("Space");
    assert!(matches!(r.as_slice(), [Request::StimArm { .. }]), "{r:?}");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    let r = t.key("Esc");
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    assert_eq!(t.st.stimulus.phase, StimPhase::Stopping);

    t.type_key("Shift+S", "S");
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!("no dialog");
    };
    f.set_text(crate::forms::FieldId::Level, "-50");
    assert!(f.set_channel(crate::forms::FieldId::Reference, 0));
    let r = t.key("Enter");
    assert!(
        !r.iter().any(|x| matches!(x, Request::StimArm { .. })),
        "nothing armed into the lease being released: {r:?}"
    );
    assert!(t.st.sweep.plan.is_some());
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    let settings = r
        .iter()
        .find_map(|x| match x {
            Request::StimArm { settings, .. } => Some(settings.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no arm after the stop: {r:?}"));
    assert!(matches!(settings.signal, Signal::Ess { .. }));
    assert_eq!(settings.level, Dbfs(-50.0));
    assert_eq!(t.st.stimulus.phase, StimPhase::Arming);
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    assert!(
        t.last_toast().contains("Enter plays the sweep"),
        "{}",
        t.last_toast()
    );
}

#[test]
fn a_failed_sweep_says_why_and_disarms() {
    let mut t = T::new();
    t.type_key("Shift+S", "S");
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!("no dialog");
    };
    f.set_text(crate::forms::FieldId::Level, "-50");
    assert!(f.set_channel(crate::forms::FieldId::Reference, 0));
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    t.key("Enter");
    t.conn(ConnEvent::Stimulus(StimEvent::SweepStarted(Box::new(
        sweep_run(SweepStatus::Playing { repeat: 1 }),
    ))));
    let mut s = daemon_state();
    s.generator.owner = Some(ClientId("c1".into()));
    s.sweep = Some(sweep_run(SweepStatus::Failed {
        reason: SweepFailure::NoReference,
        msg: "the reference input carries no sweep".into(),
    }));
    let r = t.conn(mirror(s));
    assert!(matches!(r.as_slice(), [Request::StimStop]), "{r:?}");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(
        t.last_toast().contains("carries no sweep"),
        "{}",
        t.last_toast()
    );
    assert_eq!(t.st.stimulus.phase, StimPhase::Idle);
    assert!(!t.st.stimulus_live());
    assert!(!t.st.layout.is_shown(PaneKind::Distortion));
}

/// The top bar's autosave indicator follows the daemon's `autosave` entity: nothing when the
/// daemon does not autosave, its age once saved, `saving…` while a change waits, the
/// failure as a warning.
#[test]
fn autosave_indicator_follows_the_daemon() {
    use ac2_scene::autosave::AutosaveTone;
    const S: u64 = 1_000_000_000;
    let now = WallNs(1_000 * S);
    assert_eq!(T::disconnected().st.autosave_label(now), None);
    let mut t = T::new();
    assert_eq!(
        t.st.autosave_label(now),
        None,
        "the daemon does not autosave"
    );

    let mut st = daemon_state();
    st.autosave = Autosave {
        state: AutosaveState::Saved,
        saved_at: Some(WallNs(1_000 * S - 3 * S)),
    };
    t.conn(mirror(st.clone()));
    let l = t.st.autosave_label(now).expect("label");
    assert_eq!(
        (l.text.as_str(), l.tone),
        ("autosaved just now", AutosaveTone::Quiet)
    );

    st.autosave.state = AutosaveState::Pending;
    t.conn(mirror(st.clone()));
    assert_eq!(t.st.autosave_label(now).expect("label").text, "saving…");

    st.autosave.state = AutosaveState::Failed {
        reason: "disk full".into(),
    };
    t.conn(mirror(st));
    let l = t.st.autosave_label(now).expect("label");
    assert_eq!(
        (l.text.as_str(), l.tone),
        ("autosave failed: disk full", AutosaveTone::Warning)
    );
}

/// The daemon state with MM1 34804 (0° / 90°) on input 2 choosing `curve`, a sensitivity
/// calibration of it, the session on the simulated rig.
fn mm1_state(curve: CurveChoice) -> State {
    let mut s = daemon_state();
    if let Some(o) = &mut s.session.open {
        o.backend = BackendKind::Fake;
    }
    s.mics = vec![mm1()];
    s.inputs = vec![InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve,
    }];
    s.calibrations = vec![CalEntry {
        key: CalKey {
            device: DeviceId("fake:loop".into()),
            channel: 1,
            mic: "MM1 34804".into(),
        },
        spl: SplCal {
            sensitivity: Db(130.0),
            method: ac2_proto::model::CalMethod::Acoustic {
                calibrator_level: DbSpl(94.0),
            },
            freq: Hz(1000.0),
            measured: Dbfs(-36.0),
            calibrated_at: WallNs(0),
        },
    }];
    s
}

fn cal_view(t: &T) -> &crate::cal_view::CalView {
    match &t.st.overlay {
        Overlay::Calibrations(v) => v,
        other => panic!("no calibrations view: {other:?}"),
    }
}

fn calls(r: &[Request]) -> Vec<&Command> {
    r.iter()
        .filter_map(|r| match r {
            Request::Call { cmd, .. } => Some(cmd),
            _ => None,
        })
        .collect()
}

#[test]
fn input_setup_view_steps_curves_and_manages_the_library() {
    use crate::cal_view::{CalLine, lines};
    let label = |l: &str| CurveChoice::Curve { label: l.into() };
    let mut t = T::new();
    t.conn(mirror(mm1_state(label("0°"))));
    // Main L (transfer, measuring input 2) is selected: Input setup opens on its input.
    t.st.update(Msg::Command(CommandId::InputSetup), &t.keys);
    let st = t.st.daemon().cloned().expect("state");
    assert_eq!(
        cal_view(&t).focused(&st),
        Some(CalLine::Input(1)),
        "{:?}",
        lines(&st)
    );
    // The lines: inputs 1 and 2, the two curves, the calibration.
    assert_eq!(lines(&st).len(), 5);
    let rows = crate::cal_view::line_texts(&st, WallNs(3 * 3_600_000_000_000), Default::default());
    assert_eq!(rows[1].title, "in 2 · MM1 34804");
    assert_eq!(rows[1].detail, "curve 0°");
    assert_eq!(
        rows[1].extra,
        "verified · 94.0 dB SPL at 1.00 kHz · 3 h ago"
    );
    assert_eq!(rows[2].title, "MM1 34804 0°");
    assert_eq!(rows[2].extra, "in use on in 2");
    assert_eq!(rows[3].extra, "not in use");
    assert_eq!(rows[4].title, "MM1 34804 on in 2 of fake:loop");

    // → / ←: one request each, the next / previous curve (off after the last).
    let row = |curve: CurveChoice| InputSetup {
        channel: 1,
        mic: Some("MM1 34804".into()),
        curve,
    };
    let r = t.key("ArrowRight");
    assert_eq!(inputs_call(&r), Some(vec![row(label("90°"))]));
    t.conn(mirror(mm1_state(label("90°"))));
    let r = t.key("ArrowRight");
    assert_eq!(inputs_call(&r), Some(vec![row(CurveChoice::Off)]));
    let r = t.key("ArrowLeft");
    assert_eq!(inputs_call(&r), Some(vec![row(label("0°"))]));

    // I: a curve file for this input's mic, by path.
    let r = t.type_key("I", "i");
    assert!(r.is_empty());
    t.text("/curves/449350_34804_90Grad.txt");
    let r = t.key("Enter");
    assert!(
        r.iter().any(|x| matches!(x,
            Request::ImportCurve { path, mic, input: Some(1) }
                if mic == "MM1 34804" && path.ends_with("449350_34804_90Grad.txt"))),
        "{r:?}"
    );
    assert!(cal_view(&t).edit.is_none());

    // N: another mic name on the input drops the curve choice.
    t.type_key("N", "n");
    for _ in 0.."MM1 34804".len() {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("ECM 8000");
    let r = t.key("Enter");
    assert_eq!(
        inputs_call(&r),
        Some(vec![InputSetup {
            channel: 1,
            mic: Some("ECM 8000".into()),
            curve: CurveChoice::NotChosen,
        }])
    );

    // On the 90° curve: R renames, Delete twice deletes.
    t.key("ArrowDown");
    t.key("ArrowDown");
    assert_eq!(
        cal_view(&t).focused(&st),
        Some(CalLine::Curve(MicCurveId {
            mic: "MM1 34804".into(),
            label: "90°".into()
        }))
    );
    t.type_key("R", "r");
    for _ in 0..3 {
        t.st.update(Msg::Backspace, &t.keys);
    }
    t.text("grazing");
    let r = t.key("Enter");
    assert!(
        matches!(
            calls(&r).as_slice(),
            [Command::CalCurveRename { curve, label }] if curve.label == "90°" && label == "grazing"
        ),
        "{r:?}"
    );
    let r = t.key("Delete");
    assert!(calls(&r).is_empty());
    assert!(
        cal_view(&t)
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("Delete again"))
    );
    let r = t.key("Delete");
    assert!(matches!(
        calls(&r).as_slice(),
        [Command::CalCurveDelete { curve }] if curve.label == "90°"
    ));
    // The sensitivity calibration; Backspace deletes as Delete does.
    t.key("ArrowDown");
    t.st.update(Msg::Backspace, &t.keys);
    let r = t.st.update(Msg::Backspace, &t.keys);
    assert!(matches!(
        calls(&r).as_slice(),
        [Command::CalDelete { key }] if key.mic == "MM1 34804" && key.channel == 1
    ));
    // ←/→ off an input line explain themselves instead of acting.
    let r = t.key("ArrowRight");
    assert!(inputs_call(&r).is_none());
    assert!(cal_view(&t).notice.is_some());
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
}

/// E on an input of the calibrations view: the electrical calibration dialog, prefilled
/// with the data sheet's sensitivity; Enter sends `cal.spl_electrical`, a refusal stays in
/// the dialog, a success closes it with what to do next.
#[test]
fn electrical_calibration_from_the_input_setup_view() {
    use ac2_proto::model::ElectricalConnection;
    use ac2_proto::units::Volts;
    let mut t = T::new();
    let mut s = mm1_state(CurveChoice::Curve {
        label: "0°".into()
    });
    s.calibrations.clear();
    for c in &mut s.mics[0].curves {
        c.stated_sensitivity = Some(15.0);
    }
    t.conn(mirror(s));
    t.st.update(Msg::Command(CommandId::InputSetup), &t.keys);
    let r = t.type_key("E", "e");
    assert!(r.is_empty());
    let d = cal_view(&t).electrical.clone().expect("the dialog");
    assert_eq!(d.title(), "Electrical calibration · in 2 · MM1 34804");
    assert_eq!(d.sensitivity, "15.0 mV/Pa");
    assert_eq!(d.sensitivity_source(), "data sheet (MM1 34804 0°)");
    assert!(d.safety().contains("pins 2 and 3"));
    // The typed letter that opened it is not in the voltage.
    assert_eq!(d.volts, "");
    t.text("15.03 mV");
    let r = t.key("Enter");
    let what = match r.as_slice() {
        [
            Request::Call {
                cmd:
                    Command::CalSplElectrical {
                        input: 1,
                        volts,
                        mic_sensitivity: None,
                        connection: ElectricalConnection::InLine,
                        replace_acoustic: false,
                        ..
                    },
                what,
            },
        ] if *volts == Volts(0.01503) => what.clone(),
        other => panic!("{other:?}"),
    };
    // Backspace edits the field, it does not delete a calibration.
    let r = t.st.update(Msg::Backspace, &t.keys);
    assert!(calls(&r).is_empty());
    t.text("V");
    // A refusal stays in the dialog; Enter again retries.
    t.conn(ConnEvent::Reply {
        what: what.clone(),
        result: Err("the tone level on input 2 is not steady yet".into()),
    });
    let d = cal_view(&t).electrical.clone().expect("still open");
    assert!(d.error.as_deref().is_some_and(|e| e.contains("not steady")));
    let r = t.key("Enter");
    assert_eq!(calls(&r).len(), 1);
    t.conn(ConnEvent::Reply {
        what,
        result: Ok(()),
    });
    assert!(cal_view(&t).electrical.is_none());
    assert!(
        cal_view(&t)
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("stored") && n.contains("unplug the meter")),
        "{:?}",
        cal_view(&t).notice
    );
    // Off an input line E explains itself.
    t.key("ArrowDown");
    t.type_key("E", "e");
    assert!(cal_view(&t).electrical.is_none());
    assert!(cal_view(&t).notice.is_some());
}

#[test]
fn input_setup_names_a_missing_curve_and_the_meter_label_follows() {
    let mut t = T::new();
    let mut s = mm1_state(CurveChoice::Curve {
        label: "45°".into(),
    });
    if let Some(o) = &mut s.session.open {
        o.config.input_channels = vec![0, 1];
    }
    t.conn(mirror(s));
    let label = |t: &T| t.st.session_inputs()[1].label.clone();
    assert_eq!(label(&t), "MM1 34804 · 45° not stored · mic (in 2)");
    assert_eq!(
        t.st.curve_note(1, false).as_deref(),
        Some("mic curve 45° not stored for MM1 34804")
    );
    t.conn(mirror(mm1_state(CurveChoice::NotChosen)));
    assert_eq!(label(&t), "MM1 34804 · curve not chosen · mic (in 2)");
    t.conn(mirror(mm1_state(CurveChoice::Curve {
        label: "90°".into(),
    })));
    assert_eq!(label(&t), "MM1 34804 · 90° · mic (in 2)");
    assert_eq!(
        t.st.curve_note(1, true).as_deref(),
        Some("mic curve: MM1 34804 90°")
    );
}

#[test]
fn session_dialog_steps_the_mic_curve_of_a_named_mic() {
    let mut t = T::new();
    // The session runs on the rig with MM1 34804 on input 2, 0° chosen.
    t.conn(mirror(mm1_state(CurveChoice::Curve {
        label: "0°".into()
    })));
    t.type_key("Shift+O", "O");
    t.conn(ConnEvent::Devices(Ok(backends(true))));
    assert!(dialog(&t).is_open_device());
    focus(&mut t, Row::Input(1));
    let st = t.st.daemon().cloned().expect("state");
    let text = dialog(&t)
        .row_cal_text(1, &st, WallNs(3 * 3_600_000_000_000), Default::default())
        .expect("named mic");
    assert_eq!(
        text,
        (
            "curve 0° · verified · 94.0 dB SPL at 1.00 kHz · 3 h ago".to_owned(),
            false
        )
    );
    // → applies at once: the session captures this mic already.
    let r = t.key("ArrowRight");
    assert_eq!(
        inputs_call(&r),
        Some(vec![InputSetup {
            channel: 1,
            mic: Some("MM1 34804".into()),
            curve: CurveChoice::Curve {
                label: "90°".into()
            },
        }])
    );
    assert_eq!(
        dialog(&t).inputs[1].curve,
        CurveChoice::Curve {
            label: "90°".into()
        }
    );
    // Another name typed: not live yet, the choice starts over and waits for Enter.
    t.type_key("N", "n");
    t.text(" B");
    t.key("Enter");
    assert_eq!(dialog(&t).inputs[1].curve, CurveChoice::NotChosen);
    let r = t.key("ArrowRight");
    assert_eq!(inputs_call(&r), None);
    assert!(
        dialog(&t)
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("when the session opens"))
    );
    // A row without a mic: nothing to choose.
    focus(&mut t, Row::Input(2));
    let r = t.key("ArrowRight");
    assert_eq!(inputs_call(&r), None);
}

fn spl_meter() -> MeasKind {
    MeasKind::Spl {
        config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
    }
}

fn with_spl() -> State {
    let mut s = daemon_state();
    s.measurements.push(meas(4, "FOH SPL", spl_meter()));
    s
}

/// A `leq` frame of meter 4 with `seq`, as the link delivers it.
fn leq_data(seq: u64, at_s: u64, leq: f32, flags: ac2_proto::frame::LeqFlags) -> ConnEvent {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{Frame, FrameData, LeqFrame, LeqMeta, LeqRun};
    let n = 5;
    let data = FrameData::Leq(LeqFrame {
        meas: MeasId(4),
        meta: LeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(1),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0),
                },
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            logged: seq,
            // A log of `seq` seconds up to `at_s`.
            run: Some(LeqRun {
                started_at: WallNs(at_s.saturating_sub(seq) * 1_000_000_000),
                until: WallNs(at_s * 1_000_000_000),
                measured: Seconds(seq as f64),
                gaps: Seconds(0.0),
                trimmed: false,
                laeq: f64::from(leq),
                lceq: f64::from(leq) + 3.0,
                lzeq: f64::from(leq) + 5.0,
            }),
        },
        leq: vec![leq; n],
        elapsed: vec![60.0; n],
        measured: vec![60.0; n],
        allowed: vec![f32::NAN; n],
        recover: vec![f32::NAN; n],
        flags: vec![flags; n],
    });
    let mut stamp = ac2_proto::samples::stamp(None);
    stamp.seq = seq;
    stamp.capture_wall_ns = WallNs(at_s * 1_000_000_000);
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame { stamp, data }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string(), f);
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }))
}

/// A once-a-second Leq frame is a little over 1 s old just before the next one arrives; only
/// the client's per-stream flag makes it stale, so the Leq view does not dim for a moment.
#[test]
fn leq_frame_a_little_over_a_second_old_is_fresh() {
    let ConnEvent::Data(d) = leq_data(1, 100, 80.0, ac2_proto::frame::LeqFlags::NONE) else {
        unreachable!()
    };
    let mut f = d.latest.frames.values().next().expect("frame").clone();
    f.age = Some(1.2);
    f.since_new = std::time::Duration::from_millis(1200);
    assert!(!crate::scenes::freshness(&f).is_stale());
    f.stale = true;
    assert!(crate::scenes::freshness(&f).is_stale());
}

/// Shift+L opens the SPL meter's Leq windows by name; a preset and a typed limit, Enter
/// sends the meter's configuration with the new windows and the SPL pane shows them. G
/// switches the pane between the meter and its windows.
#[test]
fn leq_windows_from_the_keyboard() {
    let mut t = T::new();
    // No meter: the key says what to do.
    t.key("Shift+L");
    assert!(
        t.last_toast().contains("no SPL meter"),
        "{}",
        t.last_toast()
    );
    t.conn(mirror(with_spl()));
    t.conn(leq_data(1, 100, 80.0, ac2_proto::frame::LeqFlags::NONE));
    t.type_key("Shift+L", "L");
    let Overlay::Leq(d) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(d.name, "FOH SPL");
    assert!(d.calibrated);
    assert_eq!(d.rows.len(), 5);
    // → on the preset: DIN 15905-5 limits the 30 min window.
    t.key("ArrowRight");
    // ↓ ↓ to the 1 min window, Tab Tab to its limit, typed.
    t.key("ArrowDown");
    t.key("ArrowDown");
    t.key("Tab");
    t.key("Tab");
    t.text("100");
    let Overlay::Leq(d) = &t.st.overlay else {
        panic!()
    };
    assert_eq!(d.rows[0].limit, "100");
    assert_eq!(d.rows[3].limit, "99");
    let r = t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    let sent = r
        .iter()
        .find_map(|r| match r {
            Request::Call {
                cmd: Command::MeasUpdate { meas, config },
                what,
            } => Some((*meas, config.clone(), what.clone())),
            _ => None,
        })
        .expect("meas.update");
    assert_eq!(sent.0, MeasId(4));
    assert_eq!(sent.2, "FOH SPL: Leq windows set");
    let MeasKind::Spl { config } = sent.1.kind else {
        panic!()
    };
    assert_eq!(config.leq.windows[0].limit, Some(DbSpl(100.0)));
    assert_eq!(config.leq.windows[3].limit, Some(DbSpl(99.0)));
    assert_eq!(config.input, 1);
    assert!(t.st.view.spl.leq);
    assert_eq!(t.st.layout.focus, PaneKind::Spl);
    // G: back to the meter, and again to the windows.
    t.key("G");
    assert!(!t.st.view.spl.leq);
    t.key("G");
    assert!(t.st.view.spl.leq);
    // A refused value keeps the dialog open and says why.
    t.type_key("Shift+L", "L");
    t.key("ArrowDown");
    t.key("ArrowDown");
    t.key("Tab");
    t.key("Tab");
    t.text("loud");
    t.key("Enter");
    let Overlay::Leq(d) = &t.st.overlay else {
        panic!()
    };
    assert!(d.error.as_deref().is_some_and(|e| e.contains("LAeq 1 min")));
}

/// Shift+R in the SPL pane (or "Start a new SPL log…" in Ctrl+K) asks first, naming the
/// run that ends and what starts over; N keeps the log, Enter sends `spl.log_new`.
#[test]
fn new_spl_log_asks_first() {
    let mut t = T::new();
    t.st.local_zone = crate::scenes::LocalZone::Fixed { offset_s: 7200 };
    t.key("Alt+4");
    t.key("Shift+R");
    assert!(
        t.last_toast().contains("no SPL meter"),
        "{}",
        t.last_toast()
    );
    assert_eq!(t.st.overlay, Overlay::None);
    t.conn(mirror(with_spl()));
    // A log of 2:14:05.
    t.conn(leq_data(
        8045,
        1_791_055_000,
        97.84,
        ac2_proto::frame::LeqFlags::NONE,
    ));
    let r = t.key("Shift+R");
    assert!(r.is_empty(), "{r:?}");
    let Overlay::NewLog(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(p.meas, MeasId(4));
    assert_eq!(p.confirm.title, "Start a new SPL log for FOH SPL?");
    // 8045 s up to 19:16:40 UTC+2 on 3 October 2026: since 19:02.
    assert_eq!(
        p.confirm.lines[0],
        "The current log ends: running 2:14:05 since 19:02 · LAeq total 97.8."
    );
    assert!(p.confirm.lines[1].contains("the 5 Leq windows and their states, the alarms"));
    // N keeps the log: nothing sent.
    let r = t.key("N");
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(t.st.overlay, Overlay::None);
    // Esc closes it too (and stops the stimulus, as always).
    t.key("Shift+R");
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
    // From the palette, then Enter: spl.log_new for the pane's meter.
    t.key("Ctrl+K");
    t.text("new spl log");
    t.key("Enter");
    assert!(
        matches!(t.st.overlay, Overlay::NewLog(_)),
        "{:?}",
        t.st.overlay
    );
    let r = t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(
        r.iter().any(|r| matches!(
            r,
            Request::Call { cmd: Command::SplLogNew { meas }, what }
                if *meas == MeasId(4) && what == "FOH SPL: new SPL log started"
        )),
        "{r:?}"
    );
    // The mouse: "Keep the current log" sends nothing, "Start a new log" sends it.
    t.key("Shift+R");
    let r = t.st.update(Msg::NewLog(false), &t.keys);
    assert!(r.is_empty());
    assert_eq!(t.st.overlay, Overlay::None);
    t.key("Shift+R");
    let r = t.st.update(Msg::NewLog(true), &t.keys);
    assert!(r.iter().any(|r| matches!(
        r,
        Request::Call {
            cmd: Command::SplLogNew { .. },
            ..
        }
    )));
}

/// The Leq view starts as columns without the history strip; B switches columns / tiles, H
/// the strip, each showing the windows on the SPL pane and remembered in the preferences,
/// which the next start applies. Full screen with the pane maximised is the stage view,
/// unless a stimulus may be sounding.
#[test]
fn leq_layout_keys_and_prefs() {
    use ac2_scene::view::{LeqLayout, LeqStyle};
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.conn(leq_data(1, 100, 80.0, ac2_proto::frame::LeqFlags::NONE));
    assert_eq!(
        t.st.view.spl.layout,
        LeqLayout {
            style: LeqStyle::Columns,
            history: false
        }
    );
    assert!(!t.st.view.spl.leq);
    t.key("Alt+4");
    assert_eq!(t.st.layout.focus, PaneKind::Spl);
    // B from the meter: the windows, as tiles.
    t.key("B");
    assert!(t.st.view.spl.leq);
    assert_eq!(t.st.view.spl.layout.style, LeqStyle::Tiles);
    assert!(t.st.prefs_dirty);
    assert_eq!(t.st.prefs.leq, t.st.view.spl.layout);
    t.st.prefs_dirty = false;
    t.key("Shift+B");
    assert!(t.st.view.spl.layout.history);
    assert!(t.st.prefs_dirty);
    assert_eq!(
        t.st.prefs.leq,
        LeqLayout {
            style: LeqStyle::Tiles,
            history: true
        }
    );
    // G still flips meter / windows and leaves the layout alone.
    t.key("G");
    assert!(!t.st.view.spl.leq);
    t.key("G");
    assert_eq!(t.st.prefs.leq.style, LeqStyle::Tiles);
    t.key("B");
    t.key("Shift+B");
    assert_eq!(t.st.view.spl.layout, LeqLayout::default());
    // Elsewhere B keeps its meaning (the RTA's bars / line), Shift+B means nothing.
    t.key("Alt+2");
    t.key("B");
    assert!(t.key("Shift+B").is_empty());
    t.key("B");
    assert_eq!(t.st.view.spl.layout, LeqLayout::default());
    // The next start takes the remembered layout.
    let prefs = crate::prefs::UiPrefs {
        leq: LeqLayout {
            style: LeqStyle::Tiles,
            history: true,
        },
        ..Default::default()
    };
    let mut u = T::new();
    u.st.set_prefs(prefs.clone());
    assert_eq!(u.st.view.spl.layout, prefs.leq);
    // The stage view: full screen, the SPL pane maximised on its windows.
    t.key("Alt+4");
    assert!(t.st.view.spl.leq);
    assert!(!t.st.stage_view());
    t.key("W");
    assert!(!t.st.stage_view());
    t.key("F11");
    assert!(t.st.stage_view());
    // Not with the stimulus armed: what drives the speakers stays in view.
    t.st.stimulus.phase = StimPhase::Armed;
    assert!(!t.st.stage_view());
    t.st.stimulus.phase = StimPhase::Idle;
    // The meter is full screen as well; W goes back to the split layout.
    t.key("G");
    assert!(t.st.stage_view());
    t.key("W");
    assert!(!t.st.stage_view());
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
}

/// A resync (no daemon state for a moment) is not "every meter deleted": the history strip
/// keeps what it gathered and goes on from there.
#[test]
fn leq_history_survives_a_resync() {
    use ac2_proto::frame::LeqFlags;
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    let judged = LeqFlags::LIMIT.with(LeqFlags::JUDGED);
    t.conn(leq_data(1, 100, 98.0, judged));
    t.conn(leq_data(2, 101, 98.5, judged));
    t.st.mirror = None;
    t.conn(leq_data(3, 102, 99.0, judged));
    t.conn(mirror(with_spl()));
    t.conn(leq_data(4, 103, 99.2, judged));
    let cfg = LeqConfig::default_windows();
    let h = &t.st.leq_history[&MeasId(4)].1;
    let p = h.points(&cfg.windows[0]).expect("series");
    assert!(p.len() >= 3, "kept through the resync: {} points", p.len());
    assert_eq!(p[0].t, 100.0);
}

/// Each new `leq` frame goes into the meter's history once; a window going over or coming
/// back is a toast, alarms that were there before the app connected are not.
#[test]
fn leq_history_and_alarm_toasts() {
    use ac2_proto::frame::LeqFlags;
    let mut t = T::new();
    let mut s = with_spl();
    let old = LeqAlarm {
        at: WallNs(5),
        duration: Seconds(1800.0),
        weighting: Weighting::A,
        kind: LeqAlarmKind::Over,
        leq: DbSpl(99.4),
        limit: DbSpl(99.0),
    };
    s.spl_logs = vec![SplLog {
        meas: MeasId(4),
        started_at: Some(WallNs(1)),
        windows: vec![],
        alarms: vec![old],
    }];
    t.conn(mirror(s.clone()));
    let toasts = t.st.toasts.len();
    let judged = LeqFlags::LIMIT.with(LeqFlags::JUDGED);
    t.conn(leq_data(1, 100, 98.0, judged));
    t.conn(leq_data(1, 100, 98.0, judged));
    t.conn(leq_data(2, 101, 99.5, judged.with(LeqFlags::OVER)));
    let cfg = LeqConfig::default_windows();
    let h = &t.st.leq_history[&MeasId(4)].1;
    let p = h.points(&cfg.windows[0]).expect("series");
    assert_eq!(p.len(), 2);
    assert!(p[1].over && !p[0].over);
    assert_eq!(t.st.toasts.len(), toasts, "old alarms are history");
    // Over, then recovered: one toast each, the over one an error.
    let over = LeqAlarm {
        at: WallNs(10),
        ..old
    };
    s.spl_logs[0].alarms.push(over);
    t.conn(mirror(s.clone()));
    assert_eq!(
        t.last_toast(),
        "FOH SPL: LAeq 30 min over its limit — 99.4 dB > 99.0 dB"
    );
    assert!(t.st.toasts.last().is_some_and(|x| x.error));
    t.conn(mirror(s.clone()));
    assert_eq!(t.st.toasts.len(), toasts + 1, "the same alarm toasts once");
    s.spl_logs[0].alarms.push(LeqAlarm {
        at: WallNs(20),
        kind: LeqAlarmKind::Recovered,
        leq: DbSpl(98.9),
        ..old
    });
    t.conn(mirror(s));
    assert_eq!(
        t.last_toast(),
        "FOH SPL: LAeq 30 min back within its limit — 98.9 dB"
    );
}

fn hint_texts(t: &T, pane: PaneKind) -> Option<Vec<String>> {
    t.st.key_hint_line(&t.keys, pane, crate::keys::LabelStyle::Pc)
        .map(|v| v.iter().map(crate::hints::KeyHint::text).collect())
}

/// The focused pane, and only it, has a hint line; its hints are the pane's own and follow
/// what the pane shows now; Shift+H turns the lines off and on and the choice is kept.
#[test]
fn key_hints_follow_the_focused_pane() {
    let mut t = T::new();
    assert!(t.st.prefs.key_hints);
    let tf = hint_texts(&t, PaneKind::Transfer).expect("transfer focused");
    assert_eq!(tf.first().map(String::as_str), Some("V select trace"));
    assert_eq!(tf.last().map(String::as_str), Some("H all keys"));
    for p in [PaneKind::Spectrum, PaneKind::Ir, PaneKind::Spl] {
        assert_eq!(hint_texts(&t, p), None, "{p:?} is not focused");
    }
    t.key("Alt+2");
    assert_eq!(hint_texts(&t, PaneKind::Transfer), None);
    let sp = hint_texts(&t, PaneKind::Spectrum).expect("spectrum focused");
    assert!(sp.contains(&"P peak hold".to_owned()), "{sp:?}");
    t.key("Alt+3");
    let ir = hint_texts(&t, PaneKind::Ir).expect("IR focused");
    assert!(ir.contains(&"G linear/log/ETC".to_owned()), "{ir:?}");
    t.key("Alt+4");
    let spl = hint_texts(&t, PaneKind::Spl).expect("SPL focused");
    assert_eq!(spl[..3], ["G meter/Leq", "F F/S/I", "Z A/C/Z"]);
    // The sweep pane names dB / % while it shows distortion, the IR mode while it shows the IR.
    t.key("Alt+5");
    let d = hint_texts(&t, PaneKind::Distortion).expect("sweep pane focused");
    assert!(d.contains(&"U dB/%".to_owned()), "{d:?}");
    assert!(!d.contains(&"G linear/log/ETC".to_owned()), "{d:?}");
    t.key("Shift+I");
    let d = hint_texts(&t, PaneKind::Distortion).expect("sweep pane focused");
    assert!(!d.contains(&"U dB/%".to_owned()), "{d:?}");
    assert!(d.contains(&"G linear/log/ETC".to_owned()), "{d:?}");
    // Mac labels.
    let mac: Vec<String> =
        t.st.key_hint_line(&t.keys, PaneKind::Distortion, crate::keys::LabelStyle::Mac)
            .expect("line")
            .iter()
            .map(crate::hints::KeyHint::text)
            .collect();
    assert_eq!(mac.first().map(String::as_str), Some("⇧S new sweep"));

    // Off: no line anywhere, remembered, and the toast says how to bring it back.
    t.st.prefs_dirty = false;
    t.key("Shift+H");
    assert!(!t.st.prefs.key_hints);
    assert!(t.st.prefs_dirty);
    assert!(t.last_toast().contains("Shift+H"), "{}", t.last_toast());
    for p in PaneKind::ALL {
        assert_eq!(hint_texts(&t, p), None);
    }
    // The title's tooltip still lists the pane's hints.
    assert!(
        !t.st
            .pane_hints(&t.keys, PaneKind::Spl, crate::keys::LabelStyle::Pc)
            .is_empty()
    );
    // The palette entry turns them back on.
    t.st.update(Msg::Command(CommandId::KeyHints), &t.keys);
    assert!(t.st.prefs.key_hints);
    assert!(hint_texts(&t, PaneKind::Distortion).is_some());
    // The next start takes the remembered choice.
    let mut u = T::new();
    u.st.set_prefs(crate::prefs::UiPrefs {
        key_hints: false,
        ..Default::default()
    });
    assert_eq!(hint_texts(&u, PaneKind::Transfer), None);
}

/// A remapped key shows its new chord on the line; the stage view has no line.
#[test]
fn key_hints_use_the_live_keymap_and_never_show_on_stage() {
    let mut t = T::new();
    t.keys = Keymap::from_toml("[global]\nnext_trace = \"Alt+T\"\nhelp = \"F1\"\n").expect("valid");
    let tf = hint_texts(&t, PaneKind::Transfer).expect("line");
    assert_eq!(tf.first().map(String::as_str), Some("Alt+T select trace"));
    assert_eq!(tf.last().map(String::as_str), Some("F1 all keys"));
    // The stage view: full screen, the SPL pane maximised on its Leq windows.
    t.key("Alt+4");
    t.key("G");
    t.key("W");
    t.key("F11");
    assert!(t.st.stage_view());
    assert!(t.st.prefs.key_hints);
    assert!(!t.st.key_hints_shown());
    assert_eq!(hint_texts(&t, PaneKind::Spl), None);
    // Out of it (W: the split layout), the line is back.
    t.key("W");
    assert!(!t.st.stage_view());
    assert!(hint_texts(&t, PaneKind::Spl).is_some());
}

/// A narrow title keeps the selected stored trace's name: the caption's variants shorten
/// from everything to the name alone.
#[test]
fn pane_caption_shortens_to_the_selected_trace() {
    let mut t = T::new();
    let mut a = stored(10, Some(3), 2);
    a.mic = Some(MicState {
        name: "MM1 34804".into(),
        curve: Some(curve_ref("90°")),
    });
    t.conn(with_traces(vec![a]));
    // Nothing selected: the measurement's smoothing.
    let v = t.st.pane_caption_variants(PaneKind::Transfer);
    assert_eq!(v.first(), t.st.pane_caption(PaneKind::Transfer).as_ref());
    assert!(!v.iter().any(|c| c.contains("t10")), "{v:?}");
    t.st.update(Msg::SelectTrace(TraceId(10)), &t.keys);
    let v = t.st.pane_caption_variants(PaneKind::Transfer);
    assert_eq!(
        v,
        [
            "slot 3 (t10): smoothing off · mic curve: MM1 34804 90°",
            "slot 3 (t10): smoothing off",
            "slot 3 (t10)",
        ]
    );
    assert_eq!(t.st.pane_caption(PaneKind::Transfer).as_ref(), v.first());
    // A transfer trace is not the spectrum pane's.
    assert!(
        !t.st
            .pane_caption_variants(PaneKind::Spectrum)
            .iter()
            .any(|c| c.contains("t10"))
    );
}

#[path = "state_display_tests.rs"]
mod display;

/// An `spl` frame of meter 4 at `at_ms` (daemon clock) under `rev`.
fn spl_data(seq: u64, at_ms: u64, level: f64, tw: TimeWeighting, rev: u64) -> ConnEvent {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{Frame, FrameData, SplFrame, SplMeta};
    let data = FrameData::Spl(SplFrame {
        meas: MeasId(4),
        meta: SplMeta {
            scale: LevelScale::DbSpl,
            weighting: Weighting::A,
            time_weighting: tw,
            peak_weighting: PeakWeighting::C,
            level,
            lmax: 100.0,
            lmin: 80.0,
            leq: 90.0,
            lpeak: 110.0,
            duration: Seconds(60.0),
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
        },
    });
    let mut stamp = ac2_proto::samples::stamp(None);
    stamp.seq = seq;
    stamp.capture_wall_ns = WallNs(at_ms * 1_000_000);
    stamp.config_rev = Rev(rev);
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame { stamp, data }),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string(), f);
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    }))
}

fn spl_update(r: &[Request]) -> Option<(SplConfig, String)> {
    r.iter().find_map(|r| match r {
        Request::Call {
            cmd: Command::MeasUpdate { config, .. },
            what,
        } => match &config.kind {
            MeasKind::Spl { config } => Some((config.clone(), what.clone())),
            _ => None,
        },
        _ => None,
    })
}

/// F steps the meter's time weighting F → S → I → F, Z its frequency weighting A → C → Z →
/// A, in place (`meas.update` of the same input, the Leq windows as they were); either shows
/// the meter. The palette names each choice. Without a meter, it says so.
#[test]
fn spl_keys_cycle_the_weightings() {
    let mut t = T::new();
    assert!(
        t.st.update(Msg::Command(CommandId::SplSlow), &t.keys)
            .is_empty()
    );
    assert!(
        t.last_toast().contains("no SPL meter"),
        "{}",
        t.last_toast()
    );
    let mut state = with_spl();
    t.conn(mirror(state.clone()));
    t.key("Alt+4");
    t.key("G");
    assert!(t.st.view.spl.leq);
    let mut seen = Vec::new();
    for key in ["F", "F", "F", "Z", "Z", "Z"] {
        let (cfg, what) = spl_update(&t.key(key)).expect(key);
        assert_eq!(cfg.input, 1);
        assert_eq!(cfg.leq, LeqConfig::default_windows());
        seen.push(what);
        // The daemon applies it.
        state.measurements[2].config.kind = MeasKind::Spl { config: cfg };
        t.conn(mirror(state.clone()));
    }
    assert_eq!(
        seen,
        [
            "FOH SPL: LAS",
            "FOH SPL: LAI",
            "FOH SPL: LAF",
            "FOH SPL: LCF",
            "FOH SPL: LZF",
            "FOH SPL: LAF"
        ]
    );
    assert!(!t.st.view.spl.leq, "the meter shows");
    for (c, want) in [
        (CommandId::SplSlow, "FOH SPL: LAS"),
        (CommandId::SplImpulse, "FOH SPL: LAI"),
        (CommandId::SplC, "FOH SPL: LCF"),
        (CommandId::SplZ, "FOH SPL: LZF"),
        (CommandId::SplFast, "FOH SPL: LAF"),
        (CommandId::SplA, "FOH SPL: LAF"),
    ] {
        t.key("Alt+1");
        let r = t.st.update(Msg::Command(c), &t.keys);
        assert_eq!(spl_update(&r).map(|x| x.1).as_deref(), Some(want));
        assert_eq!(t.st.layout.focus, PaneKind::Spl);
    }
}

/// Frames every 100 ms: the number takes a new reading every 0.5 s with F and every 1 s with
/// S (the reading at that instant, the bar live in between); a new weighting shows at once;
/// the preference overrides the period.
#[test]
fn spl_number_holds_for_the_display_period() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    let theme = ac2_scene::theme::Theme::dark();
    let size = ac2_scene::primitives::Viewport {
        width: 800.0,
        height: 500.0,
    };
    let now = crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    };
    let held = |t: &T| t.st.spl_hold.get(&MeasId(4)).map(|h| h.frame.meta.level);
    let mut changes = Vec::new();
    let mut last = None;
    for k in 0..20u64 {
        t.conn(spl_data(
            k + 1,
            1000 + k * 100,
            90.0 + k as f64,
            TimeWeighting::Fast,
            1,
        ));
        if held(&t) != last {
            last = held(&t);
            changes.push(k);
        }
        // The number is the held reading; the bar follows the newest frame.
        let s = crate::scenes::spl(&t.st, &theme, size, now).expect("scene");
        let texts: Vec<&str> = s.scene.layers[2]
            .labels
            .iter()
            .map(|l| l.text.as_str())
            .collect();
        let want = ac2_scene::format::level(last.expect("held"));
        assert!(texts.contains(&want.as_str()), "{k}: {want} in {texts:?}");
    }
    assert_eq!(changes, [0, 5, 10, 15]);
    // Slow under a new rev: at once, then once a second.
    let mut changes = Vec::new();
    for k in 20..50u64 {
        t.conn(spl_data(
            k + 1,
            1000 + k * 100,
            90.0 + k as f64,
            TimeWeighting::Slow,
            2,
        ));
        if held(&t) != last {
            last = held(&t);
            changes.push(k);
        }
    }
    assert_eq!(changes, [20, 30, 40]);
    // A display period of 200 ms from ui.toml.
    t.st.prefs.spl_hold_ms = Some(200);
    assert_eq!(t.st.spl_display_period_s(TimeWeighting::Slow), 0.2);
    let mut n = 0;
    for k in 50..60u64 {
        t.conn(spl_data(
            k + 1,
            1000 + k * 100,
            90.0 + k as f64,
            TimeWeighting::Slow,
            2,
        ));
        if held(&t) != last {
            last = held(&t);
            n += 1;
        }
    }
    assert_eq!(n, 5);
}

/// W: split → the focused pane alone → full screen (the stage view, on any pane) → split.
/// F11 alone is the window full screen in whatever layout; with one pane, the stage view.
/// The top bar comes back whenever a stimulus is armed.
#[test]
fn w_cycles_split_maximised_full_screen() {
    let mut t = T::new();
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
    t.key("W");
    assert!(t.st.layout.maximized && !t.st.fullscreen);
    assert!(!t.st.stage_view());
    t.key("W");
    assert!(t.st.layout.maximized && t.st.fullscreen);
    assert!(t.st.stage_view(), "the transfer pane full screen");
    assert!(!t.st.key_hints_shown());
    t.st.stimulus.phase = StimPhase::Armed;
    assert!(!t.st.stage_view());
    t.st.stimulus.phase = StimPhase::Idle;
    t.key("W");
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
    // F11 in the split layout: the window full screen, the layout as it was.
    t.key("F11");
    assert!(t.st.fullscreen && !t.st.layout.maximized && !t.st.stage_view());
    t.key("W");
    assert!(t.st.stage_view());
    t.key("F11");
    assert!(t.st.layout.maximized && !t.st.fullscreen && !t.st.stage_view());
    t.key("F11");
    assert!(t.st.stage_view());
    t.key("W");
    assert!(!t.st.layout.maximized && !t.st.fullscreen);
}

/// The layout goes into the preferences whenever it changes, measurements by name; the next
/// start (preferences set before the link) comes back to it, the pane's measurement once
/// the daemon's state is known. One that is gone falls back quietly.
#[test]
fn layout_is_remembered_and_restored() {
    use ac2_scene::view::IrMode;
    let mut state = with_spl();
    state.measurements.push(meas(5, "Stage SPL", spl_meter()));
    let mut t = T::new();
    t.conn(mirror(state.clone()));
    t.st.prefs_dirty = false;
    t.key("Alt+3");
    t.key("G");
    assert_eq!(t.st.view.ir.mode, IrMode::Log);
    t.key("Alt+4");
    t.key("N");
    assert_eq!(t.st.pane_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(5)));
    t.key("G");
    t.key("W");
    t.key("W");
    assert!(t.st.prefs_dirty);
    let l = t.st.prefs.layout.clone();
    assert_eq!(l.focus, PaneKind::Spl);
    assert!(l.maximized && l.fullscreen && l.spl_leq);
    assert_eq!(l.ir_mode, IrMode::Log);
    assert_eq!(
        l.measurements.get(&PaneKind::Spl).map(String::as_str),
        Some("Stage SPL")
    );
    assert_eq!(
        l.measurements.get(&PaneKind::Transfer).map(String::as_str),
        Some("Main L")
    );
    // Ticks change nothing and write nothing.
    t.st.prefs_dirty = false;
    t.st.update(
        Msg::Tick {
            now_s: 5.0,
            dt_s: 0.1,
        },
        &t.keys,
    );
    assert!(!t.st.prefs_dirty);

    // The next start.
    let prefs = t.st.prefs.clone();
    let connected = ConnEvent::Connected {
        target: "local daemon".into(),
        server: "ac2d test".into(),
        client_id: ClientId("c1".into()),
    };
    let mut u = T::disconnected();
    u.st.set_prefs(prefs.clone());
    assert_eq!(u.st.layout.focus, PaneKind::Spl);
    assert!(u.st.layout.maximized && u.st.fullscreen && u.st.view.spl.leq);
    assert_eq!(u.st.view.ir.mode, IrMode::Log);
    assert_eq!(u.st.stimulus.phase, StimPhase::Idle);
    // Before the daemon's state, the remembered names stay as they were.
    assert_eq!(u.st.layout_prefs(), prefs.layout);
    u.conn(connected.clone());
    u.conn(mirror(state.clone()));
    assert_eq!(u.st.pane_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(5)));
    assert!(u.st.stage_view());
    assert_eq!(u.st.layout_prefs(), prefs.layout);

    // The remembered meter is gone: the pane shows its usual choice, nothing is said.
    let mut gone = prefs.clone();
    gone.layout
        .measurements
        .insert(PaneKind::Spl, "Gone SPL".into());
    let mut v = T::disconnected();
    v.st.set_prefs(gone);
    v.conn(connected);
    v.conn(mirror(state));
    assert_eq!(v.st.pane_meas(PaneKind::Spl).map(|m| m.id), Some(MeasId(4)));
    assert!(!v.st.toasts.iter().any(|t| t.error), "{:?}", v.st.toasts);
    assert_eq!(
        v.st.prefs
            .layout
            .measurements
            .get(&PaneKind::Spl)
            .map(String::as_str),
        Some("FOH SPL")
    );
}
