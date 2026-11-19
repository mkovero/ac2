//! The focused pane's key-hint line: its most used commands as `key name` pairs, written
//! from the live keymap (a remapped key shows its new chord), fitted to the pane's width by
//! dropping the least used first. The help key closes the line and always stays.

use crate::keys::{self, CommandId, Keymap, LabelStyle, Scope};

/// One `key name` pair of the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyHint {
    pub command: CommandId,
    /// The chord as bound now: `Ctrl+1`, `⌘1`, `Shift+I`.
    pub keys: String,
    /// What the line calls it: `capture`, `all keys`.
    pub name: &'static str,
    /// Higher stays longer; the help hint has the top.
    pub priority: u8,
}

impl KeyHint {
    /// `Ctrl+1 capture`.
    pub fn text(&self) -> String {
        format!("{} {}", self.keys, self.name)
    }

    /// The command's full title, for the tooltip.
    pub fn title(&self) -> &'static str {
        self.command.title()
    }
}

/// Between two pairs on the line.
pub const SEP: &str = " · ";

/// The help hint's name.
pub const ALL_KEYS: &str = "all keys";

/// The hints of `scope` with a key in `keymap`, in the line's order, then the help key. A
/// command unbound by the operator is left out (no key to show).
pub fn line(keymap: &Keymap, scope: Scope, style: LabelStyle) -> Vec<KeyHint> {
    let mut v: Vec<KeyHint> = keys::hints(scope)
        .iter()
        .filter_map(|h| {
            let chord = keymap.first_chord(h.command, scope)?;
            let mut keys = chord.label_in(style);
            // The view keys are one pair, forward and back: one hint names both. So are the
            // delay nudges and the 3D camera's, where one direction alone reads as a key
            // that only goes one way.
            if let Some(back) = partner(h.command, scope).and_then(|p| keymap.first_chord(p, scope))
            {
                keys = format!("{keys}/{}", back.label_in(style));
            }
            Some(KeyHint {
                command: h.command,
                keys,
                name: h.name,
                priority: h.priority,
            })
        })
        .collect();
    if let Some(c) = keymap.first_chord(CommandId::Help, scope) {
        v.push(KeyHint {
            command: CommandId::Help,
            keys: c.label_in(style),
            name: ALL_KEYS,
            priority: u8::MAX,
        });
    }
    v
}

/// [`line`] with scope `over` laid over `scope` (a picked Inputs row over the focused
/// pane): the overlay's hints first, then the pane's whose keys the overlay has not taken.
pub fn line_over(
    keymap: &Keymap,
    over: Option<Scope>,
    scope: Scope,
    style: LabelStyle,
) -> Vec<KeyHint> {
    let under = line(keymap, scope, style);
    let Some(over) = over else {
        return under;
    };
    let mut v: Vec<KeyHint> = line(keymap, over, style)
        .into_iter()
        .filter(|h| h.command != CommandId::Help)
        .collect();
    let taken: Vec<String> = v.iter().map(|h| h.keys.clone()).collect();
    v.extend(under.into_iter().filter(|h| !taken.contains(&h.keys)));
    v
}

/// The command whose key a hint for `c` names with its own.
fn partner(c: CommandId, scope: Scope) -> Option<CommandId> {
    use CommandId as C;
    match (c, scope) {
        (C::NextView, _) => Some(C::PrevView),
        (C::NudgeEarlier, _) => Some(C::NudgeLater),
        (C::PanLeft, Scope::Spectrograph3d) => Some(C::PanRight),
        (C::TiltUp, Scope::Spectrograph3d) => Some(C::TiltDown),
        (C::ZoomIn, Scope::Spectrograph3d) => Some(C::ZoomOut),
        _ => None,
    }
}

/// Which items stay so that their widths plus a separator between each two fit in
/// `available`: the lowest priority goes first (the later of equals), never the top
/// priority one. `widths[i]` is item `i`'s width; the result is in the same order.
pub fn fit(items: &[(u8, f32)], sep: f32, available: f32) -> Vec<bool> {
    let mut keep = vec![true; items.len()];
    let total = |keep: &[bool]| {
        let (n, w) = items
            .iter()
            .zip(keep)
            .filter(|(_, k)| **k)
            .fold((0usize, 0.0f32), |(n, w), ((_, x), _)| (n + 1, w + x));
        w + sep * n.saturating_sub(1) as f32
    };
    while total(&keep) > available {
        let next = items
            .iter()
            .enumerate()
            .filter(|(i, (p, _))| keep[*i] && *p < u8::MAX)
            .min_by(|(i, (p, _)), (j, (q, _))| p.cmp(q).then(j.cmp(i)))
            .map(|(i, _)| i);
        let Some(i) = next else { break };
        keep[i] = false;
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: &[KeyHint]) -> Vec<String> {
        v.iter().map(KeyHint::text).collect()
    }

    #[test]
    fn each_pane_lists_its_most_used_keys_then_help() {
        let k = Keymap::default();
        let pc = |s| texts(&line(&k, s, LabelStyle::Pc));
        assert_eq!(
            pc(Scope::Transfer),
            [
                "V select trace",
                "G/Shift+G view",
                "A show/hide",
                "Ctrl+1 capture",
                "X find delay",
                "P peaks",
                "K smoothing",
                "H all keys"
            ]
        );
        assert_eq!(
            pc(Scope::Spectrum),
            [
                "S start/stop",
                "P peaks",
                "G/Shift+G view",
                "K smoothing",
                "Shift+Home fit level",
                "Ctrl+1 capture",
                "W maximise",
                "H all keys"
            ]
        );
        assert_eq!(
            pc(Scope::Ir),
            [
                "G/Shift+G view",
                "S start/stop",
                "X find delay",
                ",/. delay",
                "R reset",
                "I zoom time",
                "Shift+Home fit level",
                "H all keys"
            ]
        );
        assert_eq!(
            pc(Scope::Spl),
            [
                "G/Shift+G view",
                "Shift+F F/S/I",
                "Z A/C/Z",
                "Shift+L windows",
                "W maximise",
                "H all keys"
            ]
        );
        assert_eq!(
            pc(Scope::Distortion),
            [
                "Shift+S new sweep",
                "V select trace",
                "G/Shift+G view",
                "E reference",
                "Alt+P next peak",
                "Alt+D mark",
                "W maximise",
                "Q close pane",
                "H all keys"
            ]
        );
        assert_eq!(
            pc(Scope::Spectrograph3d),
            [
                "←/→ orbit",
                "PageUp/PageDown tilt",
                "I/O zoom",
                "Home reset camera",
                "G/Shift+G view",
                "S start/stop",
                "P peaks",
                "W maximise",
                "H all keys"
            ]
        );
        assert!(pc(Scope::Global) == ["H all keys"]);
    }

    /// A picked Inputs row lays its keys over the pane's: its hints open the line, the
    /// pane's follow, and a pane hint on a key the row took is left out.
    #[test]
    fn an_inputs_row_puts_its_keys_before_the_panes() {
        let k = Keymap::default();
        let over = |s| texts(&line_over(&k, Some(Scope::Inputs), s, LabelStyle::Pc));
        assert_eq!(
            over(Scope::Spectrum),
            [
                "R reset",
                "Shift+R reset all",
                "Tab measurements",
                "Shift+Tab back",
                "S start/stop",
                "P peaks",
                "G/Shift+G view",
                "K smoothing",
                "Shift+Home fit level",
                "Ctrl+1 capture",
                "W maximise",
                "H all keys"
            ]
        );
        // The Programme pane's own R / Shift+R are the row's now: named once.
        let p = over(Scope::Programme);
        assert_eq!(p.iter().filter(|s| s.starts_with("R ")).count(), 1, "{p:?}");
        assert_eq!(p.iter().filter(|s| s.starts_with("Shift+R ")).count(), 1);
        assert_eq!(p.last().map(String::as_str), Some("H all keys"));
        assert_eq!(
            line_over(&k, None, Scope::Spl, LabelStyle::Pc),
            line(&k, Scope::Spl, LabelStyle::Pc)
        );
    }

    #[test]
    fn mac_labels_and_remapped_keys() {
        let k = Keymap::default();
        let mac = texts(&line(&k, Scope::Distortion, LabelStyle::Mac));
        assert!(mac.contains(&"⇧S new sweep".to_owned()), "{mac:?}");
        assert!(mac.contains(&"G/⇧G view".to_owned()), "{mac:?}");
        let mac = texts(&line(&k, Scope::Transfer, LabelStyle::Mac));
        assert!(mac.contains(&"⌘1 capture".to_owned()), "{mac:?}");
        // Remapped: the line shows the new chord; unbound: the hint goes.
        let k = Keymap::from_toml(
            "[global]\nnext_trace = \"Alt+T\"\nhelp = \"F1\"\n[transfer]\ninsert_delay = []\n",
        )
        .expect("valid");
        let t = texts(&line(&k, Scope::Transfer, LabelStyle::Pc));
        assert_eq!(t[0], "Alt+T select trace");
        assert!(!t.iter().any(|s| s.contains("find delay")), "{t:?}");
        assert_eq!(t.last().map(String::as_str), Some("F1 all keys"));
        // A pane's own binding wins over the global one.
        let k = Keymap::from_toml("[distortion]\nnext_view = \"Shift+Q\"\n").expect("valid");
        let t = texts(&line(&k, Scope::Distortion, LabelStyle::Pc));
        assert!(t.contains(&"Shift+Q/Shift+G view".to_owned()), "{t:?}");
    }

    #[test]
    fn fit_drops_the_least_used_first_and_keeps_help() {
        let items = [(90, 50.0), (40, 30.0), (70, 40.0), (u8::MAX, 40.0)];
        // 160 + 3 separators of 10.
        assert_eq!(fit(&items, 10.0, 190.0), [true, true, true, true]);
        assert_eq!(fit(&items, 10.0, 189.0), [true, false, true, true]);
        assert_eq!(fit(&items, 10.0, 150.0), [true, false, true, true]);
        assert_eq!(fit(&items, 10.0, 149.0), [true, false, false, true]);
        assert_eq!(fit(&items, 10.0, 100.0), [true, false, false, true]);
        assert_eq!(fit(&items, 10.0, 99.0), [false, false, false, true]);
        // Help stays even when nothing fits.
        assert_eq!(fit(&items, 10.0, 5.0), [false, false, false, true]);
        // Equal priorities: the later goes first.
        let eq = [(50, 10.0), (50, 10.0), (u8::MAX, 10.0)];
        assert_eq!(fit(&eq, 0.0, 20.0), [true, false, true]);
    }
}
