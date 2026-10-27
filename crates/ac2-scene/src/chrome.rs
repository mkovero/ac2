//! The window's top bar wording: the link to the daemon and the key hints at its right edge.
//!
//! Plain data in, text out, so the bar reads the same in every state the tests can name
//! without a window.

/// The link to the daemon as the bar knows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Link<'a> {
    /// Connecting to `target`.
    Connecting { target: &'a str },
    /// The last attempt failed; another follows.
    Failed { target: &'a str, error: &'a str },
    /// Connected to `server` at `target`; `responding`: its heartbeat is fresh, `synced`:
    /// the mirrored state has caught up.
    Connected {
        target: &'a str,
        server: &'a str,
        responding: bool,
        synced: bool,
    },
}

/// How the link's dot is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTone {
    Ok,
    Warn,
    Fault,
}

/// The link segment of the bar: in full, and short for a narrow window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkLine {
    pub tone: LinkTone,
    pub full: String,
    pub short: String,
}

/// The link segment for `l`.
pub fn link_line(l: Link<'_>) -> LinkLine {
    let (tone, full, short) = match l {
        Link::Connecting { target } => (
            LinkTone::Warn,
            format!("connecting to {target}…"),
            "connecting…".to_owned(),
        ),
        Link::Failed { target, error } => (
            LinkTone::Fault,
            format!("{target}: {error} · retrying"),
            "link failed · retrying".to_owned(),
        ),
        Link::Connected {
            target,
            server,
            responding,
            synced,
        } => {
            if !responding {
                (
                    LinkTone::Fault,
                    format!("{server} · {target} · not responding"),
                    "not responding".to_owned(),
                )
            } else if !synced {
                (
                    LinkTone::Warn,
                    format!("{server} · {target} · syncing"),
                    "syncing".to_owned(),
                )
            } else {
                (
                    LinkTone::Ok,
                    format!("{server} · {target}"),
                    target.to_owned(),
                )
            }
        }
    };
    LinkLine { tone, full, short }
}

/// The stopped-audio segment's hover: the banner and its detail.
pub fn stopped_hover(banner: &str, detail: &str) -> String {
    format!("{banner} · {detail}")
}

/// The working session's short form while it has unsaved changes.
pub fn unsaved_short(name: &str) -> String {
    format!("{name} ·")
}

/// The key hints at the bar's right edge, in full and short, for the Help and Palette keys
/// as the keymap names them.
pub fn keys_hint(help: &str, palette: &str) -> [String; 2] {
    [
        format!("{help} keys · {palette} commands"),
        format!("{help} keys"),
    ]
}

/// The key hints' hover, for the Help, Palette and key-hints keys.
pub fn keys_tip(help: &str, palette: &str, key_hints: &str) -> String {
    format!(
        "{help} shows every key · {palette} finds every command by name · {key_hints} hides or \
         shows the panes' key hints"
    )
}

/// The rigs drop-down's row for adding a rig.
pub const RIG_CONNECT: &str = "Connect a rig…";

/// Shown when the rig shown cannot change because a window acts on it.
pub const RIG_SWITCH_WINDOW_OPEN: &str = "close the window first: it acts on the rig shown";

/// What a rig whose link lost its daemon is, in the drop-down and on its panes.
pub const RIG_UNREACHABLE: &str = "unreachable · retrying";

/// The rigs drop-down's row closing the rig shown.
pub fn rig_close(name: &str) -> String {
    format!("Close {name}")
}

/// A rig's row in the rigs drop-down: its name, and why it is greyed when its link is down.
pub fn rig_row(name: &str, reachable: bool) -> String {
    if reachable {
        name.to_owned()
    } else {
        format!("{name} · {RIG_UNREACHABLE}")
    }
}

/// The title of a pane showing a rig other than the one shown: whose it is first.
pub fn rig_pane_title(rig: &str, title: &str) -> String {
    format!("{rig} · {title}")
}

/// What a pane of a rig whose link is down shows instead of its content.
pub fn rig_pane_unreachable(rig: &str) -> String {
    format!("{rig} {RIG_UNREACHABLE}")
}

/// What a pane of a closed rig shows: it stays, saying which rig it was.
pub fn rig_pane_closed(rig: &str) -> String {
    format!("{rig} not connected")
}

/// A key or click on another rig's pane would open a window, which acts on the rig shown.
pub fn rig_window_elsewhere(rig: &str) -> String {
    format!("switch to {rig} to open that window")
}

/// The daemon reached is a rig already held.
pub fn rig_already_connected(rig: &str) -> String {
    format!("{rig} is already connected")
}

/// A rig closed.
pub fn rig_closed(rig: &str) -> String {
    format!("{rig} closed")
}

/// The rigs drop-down's tip.
pub const RIG_MENU_TIP: &str = "The rig shown: switch to another, connect a rig, or close this one";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_lines_by_state() {
        let l = link_line(Link::Connecting { target: "pupu" });
        assert_eq!(
            (l.tone, l.full.as_str(), l.short.as_str()),
            (LinkTone::Warn, "connecting to pupu…", "connecting…")
        );
        let l = link_line(Link::Failed {
            target: "pupu",
            error: "refused",
        });
        assert_eq!(
            (l.tone, l.full.as_str(), l.short.as_str()),
            (
                LinkTone::Fault,
                "pupu: refused · retrying",
                "link failed · retrying"
            )
        );
        let up = |responding, synced| {
            link_line(Link::Connected {
                target: "pupu",
                server: "ac2d 0.1",
                responding,
                synced,
            })
        };
        let l = up(false, true);
        assert_eq!(
            (l.tone, l.full.as_str(), l.short.as_str()),
            (
                LinkTone::Fault,
                "ac2d 0.1 · pupu · not responding",
                "not responding"
            )
        );
        let l = up(true, false);
        assert_eq!(
            (l.tone, l.full.as_str(), l.short.as_str()),
            (LinkTone::Warn, "ac2d 0.1 · pupu · syncing", "syncing")
        );
        let l = up(true, true);
        assert_eq!(
            (l.tone, l.full.as_str(), l.short.as_str()),
            (LinkTone::Ok, "ac2d 0.1 · pupu", "pupu")
        );
    }

    #[test]
    fn keys_and_small_parts() {
        assert_eq!(keys_hint("F1", "P"), ["F1 keys · P commands", "F1 keys"]);
        assert_eq!(
            keys_tip("F1", "P", "K"),
            "F1 shows every key · P finds every command by name · K hides or shows the panes' \
             key hints"
        );
        assert_eq!(
            stopped_hover("Audio stopped", "xrun"),
            "Audio stopped · xrun"
        );
        assert_eq!(unsaved_short("Show"), "Show ·");
    }
}
