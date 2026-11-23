//! The reducer's side of Settings › Connection: the link, the `ac2d` rows, and in network
//! mode the authorized and refused clients.

use ac2_proto::Command;
use eframe::egui::Key;

use super::{AppState, Overlay, typed_char};
use crate::conn::Request;
use crate::keys::Chord;
use crate::settings::ConnAction;

impl AppState {
    pub(super) fn connection_key(
        &mut self,
        chord: Chord,
        swallow: Option<char>,
        out: &mut Vec<Request>,
    ) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let plain = !(chord.command || chord.alt);
        let p = &mut s.connection;
        let action = match chord.key {
            Key::ArrowUp => {
                p.move_focus(-1);
                None
            }
            Key::Tab if chord.shift => {
                p.move_focus(-1);
                None
            }
            Key::ArrowDown | Key::Tab => {
                p.move_focus(1);
                None
            }
            Key::Enter => p.enter(),
            Key::ArrowLeft if !p.typing() => p.toggle(-1),
            Key::ArrowRight if !p.typing() => p.toggle(1),
            _ if p.typing() => {
                self.swallow_text = swallow;
                None
            }
            Key::A if plain => {
                if p.start_authorize() {
                    self.swallow_text = typed_char(&chord);
                }
                None
            }
            Key::Delete | Key::Backspace => p.delete(),
            _ => None,
        };
        if let Some(a) = action {
            self.connection_action(a, out);
        }
    }

    pub(super) fn connection_action(&mut self, a: ConnAction, out: &mut Vec<Request>) {
        match a {
            ConnAction::Reconnect => out.push(Request::Reconnect),
            ConnAction::ConnectOther => self.want_connect_dialog = true,
            ConnAction::ToggleStartLocal => {
                self.prefs.daemon.start_local = !self.prefs.daemon.start_local;
                self.prefs_dirty = true;
                self.toast(crate::settings::start_local_text(
                    self.prefs.daemon.start_local,
                ));
            }
            ConnAction::ToggleStopOnQuit if crate::spawn::stop_forced() => {
                self.toast(crate::settings::stop_on_quit_text(true, true));
            }
            ConnAction::ToggleStopOnQuit => {
                self.prefs.daemon.stop_on_quit = !self.prefs.daemon.stop_on_quit;
                self.prefs_dirty = true;
                self.toast(crate::settings::stop_on_quit_text(
                    self.prefs.daemon.stop_on_quit,
                    false,
                ));
            }
            ConnAction::Authorize { name, key } => out.push(Request::ServerCall {
                what: format!("client {name} authorized"),
                failed: format!("cannot authorize {name}"),
                cmd: Command::ServerAuthorize { name, key },
            }),
            ConnAction::Revoke(name) => out.push(Request::ServerCall {
                what: format!("client {name} revoked"),
                failed: format!("cannot revoke {name}"),
                cmd: Command::ServerRevoke { name },
            }),
            ConnAction::Network {
                enabled,
                bind,
                port,
                change,
            } => out.push(Request::ServerCall {
                what: ac2_scene::network::change_done(change).into(),
                failed: ac2_scene::network::change_failed(change).into(),
                cmd: Command::ServerNetwork {
                    enabled,
                    bind,
                    port,
                },
            }),
            // As `ac2 session link-offset`: the open session reopened with it, everything
            // else as it was.
            ConnAction::LinkOffset(offset) => {
                let Some(open) = self.open_session() else {
                    return;
                };
                let mut config = open.config.clone();
                config.link_offset = offset.map(ac2_proto::units::Seconds);
                out.push(Request::RemoteSession {
                    peers: None,
                    config,
                    what: ac2_scene::cross_rig::link_offset_toast(offset),
                });
            }
            ConnAction::PairPick => self.open_pair_pick(out),
            ConnAction::Pair(rig) => self.pair_with(rig, out),
            ConnAction::Unpair(rig) => self.unpair(rig, out),
        }
    }
}
