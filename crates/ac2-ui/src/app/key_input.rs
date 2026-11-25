//! Keyboard and text events into the reducer, and which held keys repeat.

use eframe::egui::{self, Event, Key};

use super::App;
use crate::keys::Chord;
use crate::state::{Msg, Overlay};

impl App {
    /// Keyboard and text events → reducer. Handled events are removed so egui widgets never
    /// also act on them (Space must not click a focused button, Tab must not move focus).
    pub(super) fn input(&mut self, ctx: &egui::Context) {
        let text_overlay = self.state.overlay.takes_text();
        let events = ctx.input_mut(|i| {
            let (mine, rest): (Vec<Event>, Vec<Event>) =
                std::mem::take(&mut i.events).into_iter().partition(|e| {
                    matches!(e, Event::Key { .. }) || (matches!(e, Event::Text(_)) && text_overlay)
                });
            i.events = rest;
            mine
        });
        for e in events {
            match e {
                Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } => {
                    let open = self.state.overlay != Overlay::None;
                    let overlay = open && !self.state.overlay.is_reading();
                    if overlay && key == Key::Backspace {
                        // Held, it keeps erasing typed text; where Backspace is Delete (the
                        // delete confirmation), one held press must not open and answer it.
                        if !repeat || text_overlay {
                            self.dispatch(Msg::Backspace);
                        }
                        continue;
                    }
                    if repeat && !repeats(key, modifiers, open) {
                        continue;
                    }
                    self.dispatch(Msg::Key(Chord::from_event(key, modifiers)));
                }
                Event::Text(t) => self.dispatch(Msg::Text(t)),
                _ => {}
            }
        }
    }
}

/// Whether a held `key` acts again on its auto-repeat: only navigation does, never the
/// stimulus or a toggle, so a held key cannot ramp the level or flicker a mode (the
/// stimulus keys are also inert while a stop is on its way, for a repeat no platform
/// flags). In an open window (`open`) ↑/↓ and the
/// page keys only move or scroll it (the window owns them). Ctrl+↑/↓ move the level axis
/// (never the stimulus level, which is ↑/↓ without Ctrl).
pub(crate) fn repeats(key: Key, modifiers: egui::Modifiers, open: bool) -> bool {
    matches!(key, Key::ArrowLeft | Key::ArrowRight)
        || (open && matches!(key, Key::PageUp | Key::PageDown))
        || (matches!(key, Key::ArrowUp | Key::ArrowDown)
            && (open || (modifiers.command && !modifiers.alt)))
}
