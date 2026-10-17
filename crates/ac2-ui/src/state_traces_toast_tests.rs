//! Reducer tests of the palette, navigation and cursor keys, toasts and the notification log.

use super::*;

#[test]
fn palette_runs_commands() {
    let mut t = T::new();
    t.key("Ctrl+K");
    t.text("unwrapped");
    t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(matches!(t.st.view.tf.phase, PhaseView::Unwrapped { .. }));
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
    t.st.update(Msg::Command(CommandId::ToggleCursor), &t.keys);
    let hz = t.st.view.cursor_hz.expect("cursor");
    assert!((hz - (20.0f64 * 20_000.0).sqrt()).abs() < 1e-9);
    t.key("Shift+Right");
    let up = t.st.view.cursor_hz.expect("cursor");
    assert!((up / hz - 2f64.powf(1.0 / 12.0)).abs() < 1e-12);
    t.st.update(Msg::Command(CommandId::ToggleCursor), &t.keys);
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
fn toast_time_follows_the_text_and_the_severity() {
    let mut t = T::new();
    t.st.toast("t13 selected");
    t.st.warn("no shown stored traces (1 … 9 show a slot; Alt+V reaches hidden traces)");
    t.st.fault("sweep failed: the audio stopped");
    let until: Vec<f64> = t.st.toasts.iter().map(|x| x.until_s).collect();
    let sev: Vec<Severity> = t.st.toasts.iter().map(|x| x.severity).collect();
    assert_eq!(sev, [Severity::Info, Severity::Warning, Severity::Fault]);
    for x in &t.st.toasts {
        assert_eq!(x.until_s, ac2_scene::toast::duration_s(x.severity, &x.text));
    }
    assert!(until[0] < until[1] && until[1] < until[2], "{until:?}");
    // The info goes first, the error stays longest.
    t.tick(until[0] + 0.1);
    assert_eq!(t.st.toasts.len(), 2);
    t.tick(until[1] + 0.1);
    assert_eq!(t.last_toast(), "sweep failed: the audio stopped");
}

#[test]
fn hovering_holds_the_toasts_and_a_click_dismisses_one() {
    let mut t = T::new();
    t.st.toast("first");
    t.st.toast("second");
    let until = t.st.toasts[0].until_s;
    t.tick(1.0);
    let keys = t.keys.clone();
    t.st.update(Msg::ToastsHeld(true), &keys);
    // Held far past their time: still there, with the time they had left.
    t.tick(until + 30.0);
    assert_eq!(t.st.toasts.len(), 2);
    t.st.update(Msg::ToastsHeld(false), &keys);
    // They had `until - 1` left when the pointer came.
    let gone = until + 30.0 + (until - 1.0);
    t.tick(gone - 0.1);
    assert_eq!(t.st.toasts.len(), 2, "the time held does not count");
    t.tick(gone + 0.1);
    assert!(t.st.toasts.is_empty());
    // A click takes exactly that one away.
    t.st.toast("a");
    t.st.toast("b");
    let a = t.st.toasts[0].id;
    t.st.update(Msg::DismissToast(a), &keys);
    assert_eq!(t.st.toasts.len(), 1);
    assert_eq!(t.last_toast(), "b");
}

#[test]
fn the_same_message_replaces_the_one_up_and_counts_in_the_log() {
    let mut t = T::new();
    t.st.warn("not connected");
    t.st.toast("t13 selected");
    t.tick(2.0);
    t.st.warn("not connected");
    let texts: Vec<&str> = t.st.toasts.iter().map(|x| x.text.as_str()).collect();
    assert_eq!(texts, ["t13 selected", "not connected"]);
    t.st.warn("not connected");
    let n = t.st.notices.back().cloned();
    assert_eq!(n.as_ref().map(|n| n.count), Some(2));
    assert_eq!(t.st.notices.len(), 3);
}

#[test]
fn the_notification_log_keeps_the_last_ones_and_opens_from_the_palette() {
    let mut t = T::new();
    for i in 0..(ac2_scene::toast::LOG_LEN + 10) {
        t.st.toast(format!("message {i}"));
    }
    assert_eq!(t.st.notices.len(), ac2_scene::toast::LOG_LEN);
    assert_eq!(
        t.st.notices.front().map(|n| n.text.as_str()),
        Some("message 10")
    );
    assert!(t.st.toasts.len() <= MAX_TOASTS);
    // Expired toasts are still in the log.
    t.tick(1000.0);
    assert!(t.st.toasts.is_empty());
    assert_eq!(t.st.notices.len(), ac2_scene::toast::LOG_LEN);
    // The palette finds it by name and opens the window; the keys scroll it, Esc closes.
    let keys = t.keys.clone();
    let scope = t.focus_kind().scope();
    let mut p = crate::palette::Palette::default();
    p.type_text("recent notif");
    assert_eq!(
        p.entries(&keys, scope).first().map(|e| e.command),
        Some(CommandId::Notifications)
    );
    t.st.update(Msg::Command(CommandId::Notifications), &keys);
    assert_eq!(t.st.overlay, Overlay::Notifications);
    t.key("Down");
    assert_eq!(t.st.help_scroll, crate::state::HELP_LINE);
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
}

/// Warning toasts off: a warning goes only to the log, an error and information still pop
/// up; on again, warnings pop up again. The palette and its toast name it the same way.
#[test]
fn warning_toasts_off_keeps_warnings_in_the_log_only() {
    let mut t = T::new();
    assert!(t.st.prefs.warning_toasts);
    let keys = t.keys.clone();
    let scope = t.focus_kind().scope();
    let mut p = crate::palette::Palette::default();
    p.type_text("warning toasts");
    assert_eq!(
        p.entries(&keys, scope).first().map(|e| e.command),
        Some(CommandId::WarningToasts)
    );
    t.st.prefs_dirty = false;
    t.st.update(Msg::Command(CommandId::WarningToasts), &keys);
    assert!(!t.st.prefs.warning_toasts);
    assert!(t.st.prefs_dirty);
    assert_eq!(
        t.last_toast(),
        "warning toasts off: warnings and Leq limit alarms go only to the notification log"
    );
    let toasts = t.st.toasts.len();
    t.st.warn("no stimulus level yet");
    assert_eq!(t.st.toasts.len(), toasts, "a warning does not pop up");
    let n = t.st.notices.back().expect("logged");
    assert_eq!(n.text, "no stimulus level yet");
    assert_eq!(n.severity, Severity::Warning);
    t.st.fault("the link went down");
    assert_eq!(t.last_toast(), "the link went down");
    t.st.toast("saved");
    assert_eq!(t.last_toast(), "saved");
    t.st.update(Msg::Command(CommandId::WarningToasts), &keys);
    assert!(t.st.prefs.warning_toasts);
    t.st.warn("no stimulus level yet");
    assert_eq!(t.last_toast(), "no stimulus level yet");
}

#[test]
fn a_software_rasteriser_is_said_once() {
    let mut t = T::new();
    let keys = t.keys.clone();
    t.st.update(Msg::Rasterizer(ac2_plot::Rasterizer::Gpu), &keys);
    assert!(t.st.toasts.is_empty(), "a GPU is not news");
    t.st.update(Msg::Rasterizer(ac2_plot::Rasterizer::Software), &keys);
    assert_eq!(t.last_toast(), ac2_scene::toast::SOFTWARE_RENDERING);
    assert_eq!(t.last_toast(), "software rendering: reduced frame rate");
    assert_eq!(t.st.rasterizer, ac2_plot::Rasterizer::Software);
    t.st.toasts.clear();
    t.st.update(Msg::Rasterizer(ac2_plot::Rasterizer::Software), &keys);
    assert!(t.st.toasts.is_empty(), "said once");
}

/// Toasts name the keys of the keymap in effect: a rebind moves them, an unbound command
/// is named by its palette entry, and that names the palette's own key as bound.
#[test]
fn toasts_name_the_keys_the_keymap_binds() {
    let mut t = T::new();
    assert!(t.key("5").is_empty());
    assert_eq!(t.last_toast(), "slot 5 is empty (Ctrl+5 captures into it)");
    t.key("Shift+L");
    assert_eq!(
        t.last_toast(),
        "no SPL meter: make one first (New SPL meter… in Ctrl+K)"
    );
    t.keys =
        Keymap::from_toml("[global]\nslot_5 = \"F7\"\nmeas_new_spl = \"F8\"\n").expect("valid");
    t.key("5");
    assert_eq!(t.last_toast(), "slot 5 is empty (F7 captures into it)");
    t.key("Shift+L");
    assert_eq!(t.last_toast(), "no SPL meter: make one first (F8)");
    t.keys = Keymap::from_toml("[global]\nslot_5 = []\npalette = \"F9\"\n").expect("valid");
    t.key("5");
    assert_eq!(
        t.last_toast(),
        "slot 5 is empty (Capture selected measurement to slot 5 in F9)"
    );
}
