//! Chords: a key with modifiers, how one is parsed and how it is written on screen.

use std::fmt;

use eframe::egui::{Key, Modifiers};

/// A key with modifiers. `command` is Ctrl on Linux / Windows and ⌘ on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub command: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Punctuation keys: which one arrives already depends on Shift on many layouts (`/` is
/// Shift+7 on Nordic keyboards), so Shift is ignored for them and never part of a chord.
pub(super) fn is_symbol(k: Key) -> bool {
    matches!(
        k,
        Key::Colon
            | Key::Comma
            | Key::Backslash
            | Key::Slash
            | Key::Pipe
            | Key::Questionmark
            | Key::Exclamationmark
            | Key::OpenBracket
            | Key::CloseBracket
            | Key::OpenCurlyBracket
            | Key::CloseCurlyBracket
            | Key::Backtick
            | Key::Minus
            | Key::Period
            | Key::Plus
            | Key::Equals
            | Key::Semicolon
            | Key::Quote
    )
}

/// Keys never bound by default: unreachable without AltGr / Shift or dead on common
/// European layouts (Nordic `[ ] { } \ |` need AltGr; `+ - =` move around; `` ` `` and `´`
/// are dead keys).
pub fn layout_unsafe(k: Key) -> bool {
    matches!(
        k,
        Key::OpenBracket
            | Key::CloseBracket
            | Key::OpenCurlyBracket
            | Key::CloseCurlyBracket
            | Key::Plus
            | Key::Minus
            | Key::Equals
            | Key::Backtick
            | Key::Quote
            | Key::Backslash
            | Key::Pipe
            | Key::Colon
            | Key::Semicolon
    )
}

impl Chord {
    pub const fn key(key: Key) -> Self {
        Self {
            key,
            command: false,
            alt: false,
            shift: false,
        }
    }

    pub const fn shift(key: Key) -> Self {
        Self {
            shift: true,
            ..Self::key(key)
        }
    }

    pub const fn command(key: Key) -> Self {
        Self {
            command: true,
            ..Self::key(key)
        }
    }

    pub const fn alt(key: Key) -> Self {
        Self {
            alt: true,
            ..Self::key(key)
        }
    }

    /// The chord of a key event.
    pub fn from_event(key: Key, m: Modifiers) -> Self {
        Self {
            key,
            // ⌘Tab is the macOS app switcher and never reaches the app, so there Control
            // stands in for ⌘ on Tab (elsewhere Ctrl is the command key anyway).
            command: m.command || (key == Key::Tab && m.ctrl),
            alt: m.alt,
            shift: m.shift && !is_symbol(key),
        }
    }

    /// Parses `Ctrl+K`, `Cmd+K`, `Shift+P`, `Alt+Left`, `Space`, `/`, `Up`, `F1`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut c = Chord::key(Key::Space);
        // macOS labels (`⌘⇧P`) parse too, so every label reads back as its chord.
        let mut rest = s.trim();
        loop {
            // `⌃` too: the macOS label of Ctrl+Tab, as `Ctrl` always parses as the command key.
            if let Some(r) = rest.strip_prefix('⌘').or_else(|| rest.strip_prefix('⌃')) {
                c.command = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix('⌥') {
                c.alt = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix('⇧') {
                c.shift = true;
                rest = r;
            } else {
                break;
            }
        }
        let parts: Vec<&str> = rest.split('+').map(str::trim).collect();
        let Some((key, mods)) = parts.split_last() else {
            return Err(format!("empty key {s:?}"));
        };
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "command" => c.command = true,
                "alt" | "option" => c.alt = true,
                "shift" => c.shift = true,
                other => return Err(format!("unknown modifier {other:?} in {s:?}")),
            }
        }
        c.key = match key.to_ascii_lowercase().as_str() {
            "esc" | "escape" => Key::Escape,
            "enter" | "return" => Key::Enter,
            "space" => Key::Space,
            "up" | "↑" => Key::ArrowUp,
            "down" | "↓" => Key::ArrowDown,
            "left" | "←" => Key::ArrowLeft,
            "right" | "→" => Key::ArrowRight,
            "tab" => Key::Tab,
            _ => {
                let k = if key.len() == 1 {
                    key.to_ascii_uppercase()
                } else {
                    (*key).to_owned()
                };
                Key::from_name(&k).ok_or_else(|| format!("unknown key {key:?} in {s:?}"))?
            }
        };
        if c.shift && is_symbol(c.key) {
            return Err(format!(
                "{s:?}: Shift cannot be combined with a punctuation key (layouts differ)"
            ));
        }
        Ok(c)
    }

    /// How the chord is shown: `Ctrl+K` (`⌘K` on macOS), `Shift+P`, `Space`, `↑`.
    pub fn label(&self) -> String {
        self.label_in(label_style())
    }

    /// The label in an explicit style (help, palette and top bar use [`label_style`]).
    pub fn label_in(&self, style: LabelStyle) -> String {
        let key = match self.key {
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            Key::ArrowLeft => "←",
            Key::ArrowRight => "→",
            Key::Escape => "Esc",
            Key::Enter => "Enter",
            Key::Space => "Space",
            k if is_symbol(k) => k.symbol_or_name(),
            k => k.name(),
        };
        let mac = style == LabelStyle::Mac;
        let mut s = String::new();
        if self.command {
            // On macOS the chord is pressed with Control on Tab ([`Chord::from_event`]).
            s.push_str(match (mac, self.key) {
                (true, Key::Tab) => "⌃",
                (true, _) => "⌘",
                (false, _) => "Ctrl+",
            });
        }
        if self.alt {
            s.push_str(if mac { "⌥" } else { "Alt+" });
        }
        if self.shift {
            s.push_str(if mac { "⇧" } else { "Shift+" });
        }
        s.push_str(key);
        s
    }
}

/// How chords are written on screen: macOS glyphs (`⌘⇧K`) or PC words (`Ctrl+Shift+K`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    /// `Ctrl+`, `Alt+`, `Shift+`.
    Pc,
    /// `⌘`, `⌥`, `⇧`.
    Mac,
}

impl LabelStyle {
    /// The host platform's convention.
    pub fn platform() -> Self {
        if cfg!(target_os = "macos") {
            LabelStyle::Mac
        } else {
            LabelStyle::Pc
        }
    }
}

/// 0 = platform default, 1 = PC, 2 = Mac.
static LABEL_STYLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Overrides the label style for the whole process (snapshot tests pin `Pc` so screenshots
/// match on every OS).
pub fn set_label_style(style: LabelStyle) {
    let v = match style {
        LabelStyle::Pc => 1,
        LabelStyle::Mac => 2,
    };
    LABEL_STYLE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// The label style in effect.
pub fn label_style() -> LabelStyle {
    match LABEL_STYLE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => LabelStyle::Pc,
        2 => LabelStyle::Mac,
        _ => LabelStyle::platform(),
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}
