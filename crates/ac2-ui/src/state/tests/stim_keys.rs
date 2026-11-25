//! Reducer tests of Space and Enter as stop keys, of the transfers this app's stop takes
//! with it, and of this app's stimulus rows in Settings.

use super::*;
use ac2_scene::stim_rows::StimRow;

/// Arms the generator at −20 dBFS and, with `fire`, plays it.
fn play(t: &mut T, fire: bool) {
    t.pink_at(-20.0);
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    if fire {
        t.key("Enter");
        t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    }
}

fn only_stop(r: &[Request]) -> bool {
    matches!(r, [Request::StimStop])
}

/// Space and Enter in every phase of this app's stimulus, with no window open.
#[test]
fn space_and_enter_in_every_phase() {
    // Idle: Space arms, Enter only says so.
    let mut t = T::new();
    t.pink_at(-20.0);
    assert!(t.key("Enter").is_empty());
    assert!(t.last_toast().contains("not armed"));
    assert!(matches!(
        t.key("Space").as_slice(),
        [Request::StimArm { .. }]
    ));
    // Arming: a second Space and Enter do nothing; Esc still stops.
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Arming);
    assert!(t.key("Space").is_empty());
    assert!(t.key("Enter").is_empty());
    t.conn(ConnEvent::Stimulus(StimEvent::Armed));
    // Armed: Enter fires.
    let r = t.key("Enter");
    assert!(
        matches!(r.as_slice(), [Request::StimSet(d)] if d.firing),
        "{r:?}"
    );
    // Fire pending: Space or Enter stop.
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::FireRequested);
    assert!(only_stop(&t.key("Enter")));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Stopping);
    // Stopping: Enter does nothing more.
    assert!(t.key("Enter").is_empty());
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.last_toast(), "stimulus stopped");

    // Armed: Space disarms, as Esc does.
    let mut t = T::new();
    play(&mut t, false);
    assert!(only_stop(&t.key("Space")));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Stopping);

    // Firing: Space stops, and so does Enter.
    for key in ["Space", "Enter"] {
        let mut t = T::new();
        play(&mut t, true);
        assert_eq!(t.st.rig.stimulus.phase, StimPhase::Firing);
        assert!(only_stop(&t.key(key)), "{key}");
        assert_eq!(t.st.rig.stimulus.phase, StimPhase::Stopping);
        t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
        assert_eq!(t.last_toast(), "stimulus stopped", "{key}");
        assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    }
}

/// While a stop is on its way Space and Enter do nothing: a second press of the key that
/// stopped the sound queues no arm, and nothing arms or fires once the stop lands.
#[test]
fn a_stop_in_flight_takes_no_key() {
    let no_start = |r: &[Request]| {
        !r.iter()
            .any(|x| matches!(x, Request::StimArm { .. } | Request::StimSet(_)))
    };
    // Firing: Space stops, Space again does nothing; once stopped, Enter only says so.
    let mut t = T::new();
    play(&mut t, true);
    assert!(only_stop(&t.key("Space")));
    assert!(t.key("Space").is_empty());
    assert!(t.key("Enter").is_empty());
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(no_start(&r), "{r:?}");
    let r = t.key("Enter");
    assert!(no_start(&r), "{r:?}");
    assert!(t.last_toast().contains("not armed"));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
    // Armed: Space disarms, Space again does nothing; it stays disarmed.
    let mut t = T::new();
    play(&mut t, false);
    assert!(only_stop(&t.key("Space")));
    assert!(t.key("Space").is_empty());
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(no_start(&r), "{r:?}");
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
}

/// Presses that arrive as fresh ones (a platform that does not flag its auto-repeat):
/// exactly one stop, never an arm.
#[test]
fn fresh_repeats_of_a_stop_key_stop_once() {
    for key in ["Space", "Enter"] {
        let mut t = T::new();
        play(&mut t, true);
        let r: Vec<Request> = (0..6).flat_map(|_| t.key(key)).collect();
        assert!(only_stop(&r), "{key}: {r:?}");
    }
}

/// The daemon's generator as this client's lease shows it.
fn mirror_mine(t: &mut T, armed: bool, firing: bool) {
    t.remirror(|s| {
        s.generator.owner = Some(ClientId("c1".into()));
        s.generator.armed = armed;
        s.generator.firing = firing;
    });
}

/// The bar says `STOPPING…` (no keys) while this app's stop is on its way, though the
/// daemon still says it plays or is armed.
#[test]
fn the_bar_says_stopping_while_the_stop_is_in_flight() {
    use ac2_scene::stim_item::StimState;
    for fire in [true, false] {
        let mut t = T::new();
        play(&mut t, fire);
        mirror_mine(&mut t, true, fire);
        let live = if fire {
            StimState::Firing
        } else {
            StimState::Armed
        };
        assert_eq!(t.st.stim_state(), Some(live));
        t.key("Space");
        assert_eq!(t.st.stim_state(), Some(StimState::Stopping), "fire {fire}");
        assert_eq!(t.st.own_stim(), crate::state::OwnStim::Stopping);
    }
}

/// A lease the mirror shows under this client's name that this run of the app did not arm
/// (an earlier connection's): this connection holds none, so Enter cannot fire it and says
/// so, Space stops it, and the bar offers Space alone.
#[test]
fn a_lease_from_elsewhere_is_stopped_never_fired() {
    use ac2_scene::stim_item::{ARMED_ELSEWHERE, StimState};
    for firing in [false, true] {
        let mut t = T::new();
        t.pink_at(-20.0);
        mirror_mine(&mut t, true, firing);
        assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
        assert_eq!(t.st.own_stim(), crate::state::OwnStim::Elsewhere);
        assert!(!t.st.holds_stimulus());
        let shown = if firing {
            StimState::Firing
        } else {
            StimState::Armed
        };
        assert_eq!(t.st.stim_state(), Some(shown));
        assert!(t.key("Enter").is_empty(), "firing {firing}");
        assert_eq!(t.last_toast(), ARMED_ELSEWHERE);
        assert_eq!(t.st.rig.stimulus.phase, StimPhase::Idle);
        assert!(only_stop(&t.key("Space")), "firing {firing}");
    }
}

/// A fire the daemon refuses leaves the stimulus armed as this app armed it.
#[test]
fn a_refused_fire_returns_to_armed() {
    let mut t = T::new();
    play(&mut t, false);
    t.key("Enter");
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::FireRequested);
    t.conn(ConnEvent::Stimulus(StimEvent::Failed(
        "no stimulus lease held".into(),
    )));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Armed);
}

/// Armed locally before the mirror says so, the bar says ARMED (never `STIM OFF · Space
/// arms` while Space would disarm). A stop landed before the mirror agrees reads as a
/// stop, and the keys wait for the mirror.
#[test]
fn the_bar_follows_the_local_phase_where_the_mirror_lags() {
    use ac2_scene::stim_item::StimState;
    let mut t = T::new();
    play(&mut t, false);
    assert_eq!(t.st.stim_state(), Some(StimState::Armed));
    t.key("Enter");
    assert_eq!(t.st.stim_state(), Some(StimState::FirePending));
    t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: true }));
    mirror_mine(&mut t, true, true);
    t.key("Space");
    t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(t.st.stim_state(), Some(StimState::Stopping));
    assert!(t.key("Space").is_empty());
    assert!(t.key("Enter").is_empty());
    mirror_mine(&mut t, false, false);
    assert_eq!(t.st.stim_state(), Some(StimState::Off));
    assert!(matches!(
        t.key("Space").as_slice(),
        [Request::StimArm { .. }]
    ));
}

/// What the item says plays: this app's own as it set it; another client's as the daemon
/// holds it, whatever level this app has typed.
#[test]
fn what_plays_is_the_daemons_for_a_stimulus_not_armed_here() {
    let mut t = T::new();
    t.pink_at(-20.0);
    t.remirror(|s| {
        s.generator.owner = Some(ClientId("c2".into()));
        s.generator.armed = true;
        s.generator.settings = Some(ac2_proto::model::GeneratorSettings {
            signal: ac2_proto::model::Signal::White,
            level: Dbfs(-30.0),
            band: None,
            outputs: vec![1],
        });
    });
    assert!(
        t.st.stim_plays().starts_with("white −30.0 dBFS → "),
        "{}",
        t.st.stim_plays()
    );
    let mut t = T::new();
    play(&mut t, false);
    assert!(
        t.st.stim_plays().starts_with("−20.0 dBFS → "),
        "{}",
        t.st.stim_plays()
    );
}

/// The take-over acts only on another client's stimulus: on this app's own, playing or
/// stopping, it arms nothing.
#[test]
fn the_take_over_needs_another_clients_stimulus() {
    let take = |t: &mut T| {
        t.st.apply(Msg::Command(CommandId::StimulusTakeOver), &t.keys)
    };
    let mut t = T::new();
    play(&mut t, true);
    mirror_mine(&mut t, true, true);
    assert!(take(&mut t).is_empty());
    t.key("Space");
    assert!(take(&mut t).is_empty());
    let mut t = T::new();
    t.pink_at(-20.0);
    t.remirror(|s| {
        s.generator.owner = Some(ClientId("c2".into()));
        s.generator.armed = true;
    });
    let r = take(&mut t);
    assert!(
        matches!(r.as_slice(), [Request::StimArm { force: true, .. }]),
        "{r:?}"
    );
}

/// The prompt opens on the level set: typing starts from an empty line.
fn clear_prompt(t: &mut T) {
    if let Overlay::Prompt(p) = &mut t.st.overlay {
        p.text.clear();
    }
}

/// An open prompt keeps Enter and Space: Enter applies it, Space is typed text, and the
/// stimulus plays on.
#[test]
fn an_open_prompt_keeps_enter_and_space() {
    let mut t = T::new();
    play(&mut t, true);
    t.key("L");
    assert!(matches!(t.st.overlay, Overlay::Prompt(_)));
    t.key("Space");
    clear_prompt(&mut t);
    t.text("-25");
    let r = t.key("Enter");
    assert!(!r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");
    assert_eq!(t.st.rig.stimulus.level, Some(Dbfs(-25.0)));
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Firing);
    // Armed, the same.
    let mut t = T::new();
    play(&mut t, false);
    t.key("L");
    clear_prompt(&mut t);
    t.text("-25");
    let r = t.key("Enter");
    assert!(!r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Armed);
}

/// Auto-repeat never reaches the stimulus keys: a held Space that stopped never arms.
#[test]
fn held_stimulus_keys_do_not_repeat() {
    use eframe::egui::{Key, Modifiers};
    for key in [Key::Space, Key::Enter, Key::Escape, Key::ArrowUp] {
        for open in [false, true] {
            let repeats = crate::app::repeats(key, Modifiers::NONE, open);
            assert_eq!(repeats, key == Key::ArrowUp && open, "{key:?} open {open}");
        }
    }
    assert!(!crate::app::repeats(Key::Space, Modifiers::SHIFT, false));
}

/// The rig's loopback returns on input 0; Main L (1) and TF 2 (5) measure against it, the
/// mic-to-mic transfer (6) against input 1, and TF 3 (7) against it but stopped.
fn with_transfers(t: &mut T) {
    t.remirror(|s| {
        if let Some(o) = &mut s.session.open {
            o.config.loopback = Some(ac2_proto::model::LoopbackRoute {
                output: 1,
                input: 0,
            });
        }
        let mut mic = transfer();
        if let MeasKind::Transfer { config } = &mut mic {
            config.reference_input = 1;
            config.measurement_input = 0;
        }
        let mut idle = meas(7, "TF 3", transfer());
        idle.running = false;
        s.measurements.extend([
            meas(5, "TF 2", transfer()),
            meas(6, "Mic to mic", mic),
            idle,
        ]);
    });
}

/// The measurement stops in `r`.
fn meas_stops(r: &[Request]) -> Vec<MeasId> {
    r.iter()
        .filter_map(|x| match x {
            Request::Act {
                cmd: Command::MeasStop { meas },
                ..
            } => Some(*meas),
            _ => None,
        })
        .collect()
}

#[test]
fn an_own_stop_while_playing_stops_the_loopback_transfers() {
    for key in ["Escape", "Shift+Escape", "Space", "Enter"] {
        let mut t = T::new();
        with_transfers(&mut t);
        play(&mut t, true);
        assert!(
            t.key(key).iter().any(|x| matches!(x, Request::StimStop)),
            "{key}"
        );
        let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
        assert_eq!(meas_stops(&r), [MeasId(1), MeasId(5)], "{key}");
        assert_eq!(
            t.last_toast(),
            "stimulus stopped · Main L, TF 2 stopped",
            "{key}"
        );
        // Their replies add nothing on success: the toast said it.
        assert!(
            r.iter()
                .all(|x| !matches!(x, Request::Act { done: Some(_), .. })),
            "{key}"
        );
    }
    // The palette's stop too.
    let mut t = T::new();
    with_transfers(&mut t);
    play(&mut t, true);
    t.st.apply(Msg::Command(CommandId::StimulusStop), &t.keys);
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(meas_stops(&r).len(), 2);
}

#[test]
fn the_setting_off_stops_no_transfer() {
    let mut t = T::new();
    with_transfers(&mut t);
    t.st.prefs.stop_transfers_with_stimulus = false;
    play(&mut t, true);
    t.key("Space");
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(meas_stops(&r).is_empty(), "{r:?}");
    assert_eq!(t.last_toast(), "stimulus stopped");
}

#[test]
fn a_disarm_stops_no_transfer() {
    let mut t = T::new();
    with_transfers(&mut t);
    play(&mut t, false);
    assert!(only_stop(&t.key("Space")));
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(meas_stops(&r).is_empty(), "{r:?}");
}

/// Another client stopping the generator (the mirror goes quiet) or the lease running out
/// stops nothing of the transfers.
#[test]
fn another_clients_stop_or_a_lost_lease_stops_no_transfer() {
    let mut t = T::new();
    with_transfers(&mut t);
    play(&mut t, true);
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Lost("expired".into())));
    assert!(meas_stops(&r).is_empty(), "{r:?}");
    let mut t = T::new();
    with_transfers(&mut t);
    play(&mut t, true);
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Set { firing: false }));
    assert!(meas_stops(&r).is_empty(), "{r:?}");
    assert!(!t.st.rig.stimulus.stop_transfers);
}

/// S on the last running transfer stops the stimulus; that stop comes back to stop no
/// transfer, so the two rules never feed each other. S on one of two running transfers
/// stops only that one.
#[test]
fn s_and_the_stop_rule_do_not_ping_pong() {
    let mut t = T::new();
    with_transfers(&mut t);
    play(&mut t, true);
    t.put(PaneKind::Transfer);
    t.st.rig.selected = Some(MeasId(1));
    // Three transfers run (1, 5, 6): S stops Main L alone.
    let r = t.key("S");
    assert!(!r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");
    assert_eq!(t.st.rig.stimulus.phase, StimPhase::Firing);
    // The last one running: S stops it and the stimulus; the stop, landing, stops no
    // transfer.
    t.remirror(|s| {
        for m in &mut s.measurements {
            if m.id != MeasId(1) {
                m.running = false;
            }
        }
    });
    let r = t.key("S");
    assert!(r.iter().any(|x| matches!(x, Request::StimStop)), "{r:?}");
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(meas_stops(&r).is_empty(), "{r:?}");
}

/// Another client's stimulus: Esc and Shift+Esc stop it, but none of this app's transfers.
#[test]
fn another_clients_stimulus_stops_but_takes_no_transfer() {
    for key in ["Escape", "Shift+Escape"] {
        let mut t = T::new();
        with_transfers(&mut t);
        t.remirror(|s| {
            s.generator.owner = Some(ClientId("c2".into()));
            s.generator.armed = true;
            s.generator.firing = true;
        });
        let r = t.key(key);
        assert!(only_stop(&r), "{key}: {r:?}");
        let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
        assert!(meas_stops(&r).is_empty(), "{key}: {r:?}");
    }
}

/// Esc after Space is the same stop: it is sent (the panic stop is never held back), and
/// the transfers stop once, in one toast.
#[test]
fn a_second_stop_keeps_what_the_first_asked() {
    let mut t = T::new();
    with_transfers(&mut t);
    play(&mut t, true);
    assert!(only_stop(&t.key("Space")));
    assert!(only_stop(&t.key("Escape")));
    let n = t.st.toasts.list.len();
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert_eq!(meas_stops(&r), [MeasId(1), MeasId(5)]);
    assert_eq!(t.st.toasts.list.len(), n + 1);
    assert_eq!(t.last_toast(), "stimulus stopped · Main L, TF 2 stopped");
}

/// A stimulus under this name from elsewhere, stopped here, stops no transfer: this run of
/// the app did not start it.
#[test]
fn a_lease_from_elsewhere_stops_no_transfer() {
    let mut t = T::new();
    with_transfers(&mut t);
    mirror_mine(&mut t, true, true);
    assert!(only_stop(&t.key("Space")));
    let r = t.conn(ConnEvent::Stimulus(StimEvent::Stopped));
    assert!(meas_stops(&r).is_empty(), "{r:?}");
}

fn settings_of(t: &T) -> &crate::settings::Settings {
    t.st.overlay.settings().expect("Settings open")
}

/// The level row sets the rig shown's level as L does, and the transfer row turns the
/// preference over.
#[test]
fn the_settings_rows_set_the_level_and_the_rule() {
    let mut t = T::new();
    t.st.apply(Msg::Command(CommandId::StimulusSignal), &t.keys);
    t.key("ArrowDown");
    assert_eq!(settings_of(&t).signal.app, Some(StimRow::StopTransfers));
    t.key("ArrowRight");
    assert!(!t.st.prefs.stop_transfers_with_stimulus);
    assert!(t.st.prefs_dirty);
    t.key("Space");
    assert!(t.st.prefs.stop_transfers_with_stimulus);
    t.key("ArrowDown");
    assert_eq!(settings_of(&t).signal.app, Some(StimRow::Level));
    assert!(settings_of(&t).typing());
    // Empty, Enter says what to type; above the max level it is refused in the prompt's
    // words.
    t.key("Enter");
    assert_eq!(
        settings_of(&t).signal.error.as_deref(),
        Some("type a level in dBFS (e.g. -30), then Enter")
    );
    t.text("-3");
    t.key("Enter");
    assert_eq!(
        settings_of(&t).signal.error.as_deref(),
        Some("above the max level −10.0 dBFS")
    );
    assert_eq!(t.st.rig.stimulus.level, None);
    for _ in 0..2 {
        t.st.apply(Msg::Backspace, &t.keys);
    }
    t.text("-30");
    t.key("Enter");
    assert_eq!(t.st.rig.stimulus.level, Some(Dbfs(-30.0)));
    assert_eq!(settings_of(&t).signal.text, "");
    assert_eq!(settings_of(&t).signal.error, None);
    // A click on the transfer row's ‹ turns it over too.
    t.st.apply(
        Msg::Settings(SettingsMsg::Stim(StimRow::StopTransfers, -1)),
        &t.keys,
    );
    assert!(!t.st.prefs.stop_transfers_with_stimulus);
    assert_eq!(settings_of(&t).signal.app, Some(StimRow::StopTransfers));
}

/// The level prompt names the max level it may reach.
#[test]
fn the_level_prompt_states_the_max() {
    let t = T::new();
    assert_eq!(
        t.st.prompt_title(PromptKind::StimulusLevel),
        "Stimulus level (dBFS, max −10.0)"
    );
}
