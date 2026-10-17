//! The effective binding table: defaults plus the overrides from `keys.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui::Key;
use serde::Deserialize;

use super::*;

/// The effective binding table.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: defaults(),
        }
    }
}

/// `keys.toml`: `[scope] command = "Key"` or `command = ["Key", "Key"]`; `[]` unbinds.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ChordList {
    One(String),
    Many(Vec<String>),
}

/// Where user overrides live: `keys.toml` in the ac2 config directory
/// (`ac2_paths::config_dir`; `~/.config/ac2` on Linux).
pub fn config_path() -> PathBuf {
    ac2_paths::keymap()
}

impl Keymap {
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Defaults with the overrides in `toml` applied, validated.
    pub fn from_toml(toml: &str) -> Result<Self, String> {
        let table: BTreeMap<String, BTreeMap<String, ChordList>> =
            toml::from_str(toml).map_err(|e| format!("keys.toml: {e}"))?;
        let mut bindings = defaults();
        for (scope_name, cmds) in table {
            let scope = Scope::parse(&scope_name)
                .ok_or_else(|| format!("keys.toml: unknown scope [{scope_name}]"))?;
            for (cmd_name, list) in cmds {
                let command = CommandId::from_name(&cmd_name).ok_or_else(|| {
                    format!("keys.toml: unknown command {cmd_name:?} in [{scope_name}]")
                })?;
                if !command.scopes().contains(&scope) {
                    return Err(format!(
                        "keys.toml: {cmd_name} does nothing in [{scope_name}] (valid: {})",
                        command
                            .scopes()
                            .iter()
                            .map(|s| s.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                let chords = match list {
                    ChordList::One(s) => vec![s],
                    ChordList::Many(v) => v,
                };
                bindings.retain(|b| !(b.command == command && b.scope == scope));
                for s in chords {
                    let chord = Chord::parse(&s).map_err(|e| format!("keys.toml: {e}"))?;
                    bindings.push(Binding {
                        command,
                        scope,
                        chord,
                    });
                }
            }
        }
        let map = Self { bindings };
        map.validate()?;
        Ok(map)
    }

    /// Loads `path`; a missing file means defaults. On error the defaults are returned with
    /// the message, so a typo never leaves the operator without keys.
    pub fn load(path: Option<&Path>) -> (Self, Option<String>) {
        let Some(path) = path else {
            return (Self::default(), None);
        };
        match std::fs::read_to_string(path) {
            Ok(s) => match Self::from_toml(&s) {
                Ok(m) => (m, None),
                Err(e) => (Self::default(), Some(format!("{e}; using default keys"))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!("{}: {e}; using default keys", path.display())),
            ),
        }
    }

    /// Conflicts, reserved chords and scope rules.
    pub fn validate(&self) -> Result<(), String> {
        for b in &self.bindings {
            if !b.command.scopes().contains(&b.scope) {
                return Err(format!(
                    "{} bound in {} where it does nothing",
                    b.command.name(),
                    b.scope.name()
                ));
            }
            if let Some((_, owner)) = RESERVED.iter().find(|(c, _)| *c == b.chord)
                && *owner != b.command
            {
                return Err(format!(
                    "{} is reserved for {}; cannot bind it to {}",
                    b.chord,
                    owner.name(),
                    b.command.name()
                ));
            }
        }
        let esc = Chord::key(Key::Escape);
        if !self
            .bindings
            .iter()
            .any(|b| b.chord == esc && b.command == CommandId::StimulusStop)
        {
            return Err("Esc must stay bound to stimulus_stop".into());
        }
        let anywhere: Vec<Chord> = self
            .bindings
            .iter()
            .filter(|b| b.command == CommandId::StopAnywhere)
            .map(|b| b.chord)
            .collect();
        if anywhere != [STOP_ANYWHERE] {
            return Err(format!(
                "stimulus_stop_anywhere is fixed to {STOP_ANYWHERE}: it works inside dialogs, \
                 so it may not be a key they use"
            ));
        }
        for scope in Scope::ALL {
            let mut seen: std::collections::HashMap<Chord, CommandId> =
                std::collections::HashMap::new();
            for b in self
                .bindings
                .iter()
                .filter(|b| b.scope == Scope::Global || b.scope == scope)
            {
                if let Some(prev) = seen.insert(b.chord, b.command)
                    && prev != b.command
                {
                    return Err(format!(
                        "{} is bound to both {} and {} in {}",
                        b.chord,
                        prev.name(),
                        b.command.name(),
                        scope.name()
                    ));
                }
            }
        }
        Ok(())
    }

    /// The command `chord` runs with `scope` active.
    pub fn lookup(&self, scope: Scope, chord: Chord) -> Option<CommandId> {
        let find = |s: Scope| {
            self.bindings
                .iter()
                .find(|b| b.scope == s && b.chord == chord)
                .map(|b| b.command)
        };
        find(scope).or_else(|| find(Scope::Global))
    }

    /// Chords of `command` in `scope`.
    pub fn chords(&self, command: CommandId, scope: Scope) -> Vec<Chord> {
        self.bindings
            .iter()
            .filter(|b| b.command == command && b.scope == scope)
            .map(|b| b.chord)
            .collect()
    }

    /// The first chord that runs `command` with `scope` active: the scope's own, else the
    /// global one.
    pub fn first_chord(&self, command: CommandId, scope: Scope) -> Option<Chord> {
        [scope, Scope::Global]
            .into_iter()
            .find_map(|s| self.chords(command, s).first().copied())
    }

    /// The label, in `style`, of the chord that runs `command` with `scope` active; `None`
    /// when no key does.
    pub fn label_in(&self, command: CommandId, scope: Scope, style: LabelStyle) -> Option<String> {
        self.first_chord(command, scope).map(|c| c.label_in(style))
    }

    /// [`Keymap::label_in`] in the current label style.
    pub fn label(&self, command: CommandId, scope: Scope) -> Option<String> {
        self.label_in(command, scope, label_style())
    }

    /// Where message text sends the operator for palette entry `entry`: `New SPL meter… in
    /// Ctrl+K`, or `… in the command palette` when no key opens it.
    pub fn palette_entry_in(&self, entry: &str, style: LabelStyle) -> String {
        match self.label_in(CommandId::Palette, Scope::Global, style) {
            Some(p) => format!("{entry} in {p}"),
            None => format!("{entry} in the command palette"),
        }
    }

    /// [`Keymap::palette_entry_in`] in the current label style.
    pub fn palette_entry(&self, entry: &str) -> String {
        self.palette_entry_in(entry, label_style())
    }

    /// How message text names global `command`: its chord, else its palette entry (its
    /// title, so the message and the palette use the same words).
    pub fn reach_in(&self, command: CommandId, style: LabelStyle) -> String {
        self.label_in(command, Scope::Global, style)
            .unwrap_or_else(|| self.palette_entry_in(command.title(), style))
    }

    /// [`Keymap::reach_in`] in the current label style.
    pub fn reach(&self, command: CommandId) -> String {
        self.reach_in(command, label_style())
    }

    /// Key text for the palette: the chord in `active` scope (or global), else the chord in
    /// the command's own scope with that scope named.
    pub fn key_hint(&self, command: CommandId, active: Scope) -> Option<String> {
        let join = |v: Vec<Chord>| v.iter().map(Chord::label).collect::<Vec<_>>().join(" · ");
        for s in [active, Scope::Global] {
            let v = self.chords(command, s);
            if !v.is_empty() {
                return Some(join(v));
            }
        }
        command.scopes().iter().find_map(|s| {
            let v = self.chords(command, *s);
            (!v.is_empty()).then(|| format!("{} ({})", join(v), s.name()))
        })
    }
}
