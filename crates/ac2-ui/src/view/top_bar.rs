//! What the top bar says, from the app's state: the inputs of [`ac2_scene::top_bar::bar`]
//! and the items fixed at the bar's right end. No drawing here.

use std::time::Instant;

use ac2_scene::autosave::AutosaveTone;
use ac2_scene::recording::RecordingTone;
use ac2_scene::stim_item::{StimInputs, StimItem};
use ac2_scene::top_bar::{Bar, BarAudio, BarInputs, BarLink, BarSession, Note, Place, Tone};

use crate::app::App;
use crate::keys::{CommandId, STOP_ANYWHERE, Scope};
use crate::state::ConnState;

/// The first key of a global command, as shown in the bar.
pub(super) fn key_hint(app: &App, c: CommandId) -> String {
    app.keymap
        .chords(c, Scope::Global)
        .first()
        .map_or_else(|| "—".into(), |k| k.label())
}

/// The stop shown at the bar's right end while anything is armed or playing on a rig this
/// app holds, with a chip per rig not shown that is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarStop {
    pub label: String,
    pub tip: String,
    /// `(text, playing)` per rig not shown, armed or playing.
    pub elsewhere: Vec<(String, bool)>,
}

impl App {
    /// The stimulus of the rig shown, fixed at the bar's right end: what it is doing, what
    /// plays, what the stimulus keys do next. `None` before the daemon's state is mirrored.
    pub fn bar_stim(&self) -> Option<StimItem> {
        let st = &self.state;
        let generator = &st.daemon()?.generator;
        let mine = generator
            .owner
            .as_ref()
            .zip(st.my_client_id())
            .is_some_and(|(o, me)| o == me);
        let held_by = generator.owner.as_ref().filter(|_| !mine).map(|o| {
            ac2_scene::stimulus::holder(o, generator.owner_local, &st.rig_name(st.active()))
        });
        let state = st.stim_state()?;
        let plays = st.stim_plays();
        let tip = ac2_scene::stimulus::level_tip(&key_hint(self, CommandId::StimulusLevel));
        let next = self.stimulus_next_hint();
        let rig = st.many_rigs().then(|| st.rig_name(st.active()));
        Some(ac2_scene::stim_item::stim_item(&StimInputs {
            state,
            plays: &plays,
            held_by: held_by.as_deref(),
            off_keys: next.as_ref().map(|(_, short)| short.as_str()),
            keys: st.stim_keys(),
            rig: rig.as_deref(),
            next_long: next.as_ref().map(|(long, _)| long.as_str()),
            tip: &tip,
        }))
    }

    /// What Space (or the level key) starts next on the focused view, long and short. With
    /// a window open the window has Space and Enter; only the missing level is said then.
    /// Nothing while a sweep or dynamics run runs: its progress says it.
    fn stimulus_next_hint(&self) -> Option<(String, String)> {
        let st = &self.state;
        // A run between its last step and its result still holds Space inert.
        let running = st.daemon().is_some_and(|s| {
            s.sweep.as_ref().is_some_and(|r| r.active())
                || s.dynamics.as_ref().is_some_and(|r| r.active())
        }) || st.own_stim() == crate::state::OwnStim::Run;
        let window = st.overlay != crate::state::Overlay::None;
        st.stimulus_next()
            .filter(|_| !running)
            .filter(|(next, what)| {
                !window
                    || matches!(
                        (next, what),
                        (
                            ac2_scene::stimulus::Next::Space,
                            ac2_scene::stimulus::Stimulus::Generator { level: None, .. }
                        )
                    )
            })
            .map(|(next, what)| {
                ac2_scene::stimulus::hint(next, &what, &key_hint(self, CommandId::StimulusLevel))
            })
    }

    /// What the line says after the session: an autosave, a recording, a replay.
    fn bar_notes(&self) -> Vec<Note> {
        let st = &self.state;
        let wall = super::now().wall;
        let mut notes = Vec::new();
        if let Some(l) = st.autosave_label(wall) {
            notes.push(Note {
                text: l.text,
                tone: match l.tone {
                    AutosaveTone::Quiet | AutosaveTone::Busy => Tone::Dim,
                    AutosaveTone::Warning => Tone::Warn,
                },
                hover: Some(l.detail),
            });
        }
        if let Some(l) = st.recording_label() {
            notes.push(Note {
                text: l.text,
                tone: match l.tone {
                    RecordingTone::Recording => Tone::Fault,
                    RecordingTone::Quiet => Tone::Dim,
                    RecordingTone::Warning => Tone::Warn,
                },
                hover: Some(l.detail),
            });
        }
        if let Some(t) = st.replay_label() {
            notes.push(Note {
                text: t,
                tone: Tone::Armed,
                hover: Some(ac2_scene::recording::REPLAY_DETAIL.to_owned()),
            });
        }
        notes
    }

    /// The top bar as of `now`: the rig shown and the line after it.
    pub fn top_bar(&self, now: Instant) -> Bar {
        let st = &self.state;
        let wall = super::now().wall;
        let rig = st.rig_name(st.active());
        let host = st.rig_host(&st.rig);
        let place = match &host {
            Some(host) => Place::Remote { host },
            None => Place::Local,
        };
        let (target, server, link) = match &st.rig.conn {
            ConnState::Connecting { target } => (target, None, BarLink::Connecting),
            ConnState::Failed {
                target,
                error,
                down,
            } => (target, None, BarLink::Failed { error, down: *down }),
            ConnState::Connected { target, server, .. } => (
                target,
                Some(server.as_str()),
                BarLink::Connected {
                    responding: st.rig.mirror.as_ref().is_some_and(|m| m.responding(now)),
                    synced: st.rig.mirror.as_ref().is_some_and(|m| m.synced()),
                },
            ),
        };
        let stopped = crate::scenes::audio_stopped(st, wall);
        let stopped_hover = stopped
            .as_ref()
            .map(|t| ac2_scene::chrome::stopped_hover(&t.banner, &t.detail));
        let open = st.daemon().and_then(|s| s.session.open.as_ref());
        let session_line = open.map(ac2_scene::audio::session_line);
        let no_audio = st.no_audio_line(&self.keymap);
        let audio = match (open, &session_line, &stopped, &stopped_hover) {
            (Some(_), _, Some(t), Some(hover)) => Some(BarAudio::Stopped {
                text: &t.bar,
                hover,
            }),
            (Some(o), Some(line), ..) => Some(BarAudio::Open {
                device: &o.input_device.0,
                line,
            }),
            _ if st.daemon().is_some() => Some(BarAudio::None { text: &no_audio }),
            _ => None,
        };
        let banners = st.cross_rig_banners();
        // A daemon that stopped answering has no current figures.
        let daemon = st
            .rig
            .mirror
            .as_ref()
            .filter(|m| m.responding(now))
            .and_then(|m| m.health.as_ref());
        let wanted_fps = 1.0 / st.display_period().as_secs_f64();
        let health = ac2_scene::health::health_line(&self.health.inputs(
            daemon,
            st.rig.daemon_addr.as_deref(),
            wanted_fps,
            now,
        ));
        let working = st.daemon().map(|_| st.working_label(wall));
        ac2_scene::top_bar::bar(&BarInputs {
            rig: &rig,
            place,
            target,
            server,
            link,
            audio,
            banners: &banners,
            health,
            session: working.as_ref().map(|l| BarSession {
                text: &l.text,
                unsaved: l.unsaved,
                detail: &l.detail,
            }),
            notes: self.bar_notes(),
        })
    }

    /// The stop at the bar's right end; `None` while nothing is armed or playing.
    pub fn bar_stop(&self) -> Option<BarStop> {
        let st = &self.state;
        // A run between its last step and its result still holds Space inert.
        let running = st.daemon().is_some_and(|s| {
            s.sweep.as_ref().is_some_and(|r| r.active())
                || s.dynamics.as_ref().is_some_and(|r| r.active())
        }) || st.own_stim() == crate::state::OwnStim::Run;
        let elsewhere = st.live_elsewhere();
        if !(running || st.stimulus_live() || !elsewhere.is_empty()) {
            return None;
        }
        let stop = STOP_ANYWHERE.label();
        Some(BarStop {
            label: ac2_scene::chrome::stop_label(&stop),
            tip: if st.many_rigs() {
                ac2_scene::stimulus::stop_every_rig_tip(&stop)
            } else {
                ac2_scene::chrome::stop_tip(&stop)
            },
            elsewhere: elsewhere
                .iter()
                .map(|e| (ac2_scene::stimulus::elsewhere_chip(e), e.firing))
                .collect(),
        })
    }
}
