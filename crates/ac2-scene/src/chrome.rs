//! The window's top bar wording beside its line ([`crate::top_bar`]): the rigs drop-down, the
//! key hints and the stop at its right edge, and what a rig whose link is down reads as.
//!
//! Plain data in, text out, so the bar reads the same in every state the tests can name
//! without a window.

/// How the link's dot is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTone {
    Ok,
    Warn,
    Fault,
}

/// The stopped-audio segment's hover: the banner and its detail.
pub fn stopped_hover(banner: &str, detail: &str) -> String {
    format!("{banner} · {detail}")
}

/// The key hints at the bar's right edge, for the Help and Palette keys as the keymap
/// names them.
pub fn keys_hint(help: &str, palette: &str) -> String {
    format!("{help} keys · {palette} commands")
}

/// The bar's stop while anything is armed or playing: the key that stops from anywhere,
/// with what it stops, since the bar's other items are measurements that stop with S.
pub fn stop_label(stop: &str) -> String {
    format!("■ Stop stimulus: {stop}")
}

/// The stop's hover with one rig.
pub fn stop_tip(stop: &str) -> String {
    format!(
        "{stop} stops and disarms the stimulus (the generator, a sweep or a dynamics run) from \
         anywhere, also with a window open; with no window open Esc does too, and Space or \
         Enter while it plays"
    )
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

/// What a rig whose daemon refuses this app's key is, in the drop-down and on its panes.
pub const RIG_NOT_AUTHORIZED: &str = "has not authorized this app";

/// Why a rig's link is down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigDown {
    /// No answer yet: the daemon is not running or not reachable from here; the app
    /// retries.
    Unreachable,
    /// The daemon answers but its authorized clients do not list this app's key: retrying
    /// cannot help until the key is added on that host.
    NotAuthorized,
    /// The rig said it turned its network off before the link went: on purpose, not a
    /// fault. The app keeps trying, and connects once it is back on.
    NetworkOff,
}

/// The rigs drop-down's row closing the rig shown.
pub fn rig_close(name: &str) -> String {
    format!("Close {name}")
}

/// A rig's row in the rigs drop-down: its name, and why it is greyed when its link is down.
pub fn rig_row(name: &str, down: Option<RigDown>) -> String {
    match down {
        None => name.to_owned(),
        Some(RigDown::Unreachable) => format!("{name} · {RIG_UNREACHABLE}"),
        Some(RigDown::NotAuthorized) => format!("{name} · {RIG_NOT_AUTHORIZED}"),
        Some(RigDown::NetworkOff) => format!("{name} · {}", crate::network::RIG_NETWORK_OFF),
    }
}

/// The title of a pane showing a rig other than the one shown: whose it is first.
pub fn rig_pane_title(rig: &str, title: &str) -> String {
    format!("{rig} · {title}")
}

/// What a pane of a rig whose link is down shows instead of its content. A refused key
/// points where the line to add is spelled out.
pub fn rig_pane_down(rig: &str, down: RigDown) -> String {
    match down {
        RigDown::Unreachable => format!("{rig} {RIG_UNREACHABLE}"),
        RigDown::NotAuthorized => {
            format!("{rig} {RIG_NOT_AUTHORIZED} · Settings › Connection says what to add")
        }
        RigDown::NetworkOff => format!("{rig} {}", crate::network::RIG_NETWORK_OFF),
    }
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
pub const RIG_MENU_TIP: &str = "The rig shown, of every rig this app holds: switch to another, connect a rig, or \
     close this one. The measurement list shows this rig and the rigs paired with it";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rig_says_why_it_is_down() {
        assert_eq!(rig_row("pupu", None), "pupu");
        assert_eq!(
            rig_row("pupu", Some(RigDown::Unreachable)),
            "pupu · unreachable · retrying"
        );
        assert_eq!(
            rig_row("kiosk", Some(RigDown::NotAuthorized)),
            "kiosk · has not authorized this app"
        );
        assert_eq!(
            rig_pane_down("kiosk", RigDown::NotAuthorized),
            "kiosk has not authorized this app · Settings › Connection says what to add"
        );
    }

    #[test]
    fn keys_and_small_parts() {
        assert_eq!(keys_hint("F1", "P"), "F1 keys · P commands");
        assert_eq!(stop_label("Shift+Esc"), "■ Stop stimulus: Shift+Esc");
        assert_eq!(
            keys_tip("F1", "P", "K"),
            "F1 shows every key · P finds every command by name · K hides or shows the panes' \
             key hints"
        );
        assert_eq!(
            stopped_hover("Audio stopped", "xrun"),
            "Audio stopped · xrun"
        );
    }
}
