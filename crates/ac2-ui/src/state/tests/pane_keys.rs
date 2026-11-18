//! Reducer tests of where a key goes: the focused pane's scope with a picked Inputs row
//! laid over it, the IR view's share of the transfer keys, and the one target rule (a pane
//! key acts on what the focused pane shows).

use super::*;
use crate::keys::{LabelStyle, Scope};

fn hint_texts(t: &T) -> Vec<String> {
    t.st.pane_hints(&t.keys, t.st.layout.focus, LabelStyle::Pc)
        .iter()
        .map(crate::hints::KeyHint::text)
        .collect()
}

/// A picked Inputs row takes R / Shift+R for its meters and nothing else: the focused
/// pane's keys keep working, and its hint line opens with the row's.
#[test]
fn a_picked_inputs_row_leaves_the_pane_its_keys() {
    let mut t = T::new();
    t.key("Alt+2");
    let pane = t.st.layout.focus;
    t.st.apply(Msg::InputPick(0), &t.keys);
    assert_eq!(t.st.rig.input_pick, Some(0));
    assert_eq!(t.st.scope(), Scope::Spectrum);
    assert_eq!(t.st.overlay_scope(), Some(Scope::Inputs));
    // G steps the spectrum pane's view; the row stays picked.
    let before = t.st.pane_modes(pane).spectrum;
    t.key("G");
    assert_ne!(t.st.pane_modes(pane).spectrum, before);
    t.key("Shift+G");
    assert_eq!(t.st.pane_modes(pane).spectrum, before);
    assert_eq!(t.st.rig.input_pick, Some(0));
    // S is the pane's start/stop, K its smoothing.
    let r = t.key("S");
    assert!(
        matches!(
            calls(&r).first(),
            Some((Command::MeasStop { meas: MeasId(2) }, _))
        ),
        "{r:?}"
    );
    assert!(matches!(
        calls(&t.key("K")).first(),
        Some((
            Command::MeasUpdate {
                meas: MeasId(2),
                ..
            },
            _
        ))
    ));
    // R is the row's: its meters reset, the spectrum's averaging does not.
    let r = t.key("R");
    assert!(
        matches!(
            calls(&r).as_slice(),
            [(Command::LevelsReset { channels }, _)] if !channels.is_empty()
        ),
        "{r:?}"
    );
    let r = t.key("Shift+R");
    assert!(
        matches!(
            calls(&r).as_slice(),
            [(Command::LevelsReset { channels }, _)] if channels.is_empty()
        ),
        "{r:?}"
    );
    // The hint line: the row's keys, then the pane's.
    let h = hint_texts(&t);
    assert_eq!(h[..2], ["R reset", "Shift+R reset all"], "{h:?}");
    assert!(h.contains(&"G/Shift+G view".to_owned()), "{h:?}");
    assert!(h.contains(&"K smoothing".to_owned()), "{h:?}");
    assert_eq!(h.last().map(String::as_str), Some("H all keys"));
    // Another pane's title lists its own keys alone.
    let other = t.pane(PaneKind::Transfer);
    let o = t.st.pane_hints(&t.keys, other, LabelStyle::Pc);
    assert!(o.iter().all(|h| h.command != CommandId::LevelsReset));
    // The row let go: R is the pane's reset again.
    t.st.apply(Msg::InputPick(0), &t.keys);
    assert_eq!(t.st.overlay_scope(), None);
    let r = t.key("R");
    assert!(
        matches!(
            calls(&r).first(),
            Some((Command::MeasReset { meas: MeasId(2) }, _))
        ),
        "{r:?}"
    );
    assert!(!hint_texts(&t).contains(&"Shift+R reset all".to_owned()));
}

/// The IR view shows the transfer pane's measurement: reset, the delay finder, the delay
/// keys and the typed delay act on it there as on the frequency views. The keys that read
/// the frequency plot do nothing.
#[test]
fn the_ir_view_has_the_transfer_measurement_keys() {
    let mut t = T::new();
    t.put_ir();
    assert_eq!(t.st.scope(), Scope::Ir);
    let r = t.key("R");
    assert!(
        matches!(
            calls(&r).as_slice(),
            [(Command::MeasReset { meas: MeasId(1) }, _)]
        ),
        "{r:?}"
    );
    let r = t.key("X");
    assert!(
        matches!(
            r.as_slice(),
            [Request::FindDelay {
                meas: MeasId(1),
                pick: DelayPick::FirstArrival,
                ..
            }]
        ),
        "{r:?}"
    );
    let r = t.key("Shift+X");
    assert!(
        matches!(
            r.as_slice(),
            [Request::FindDelay {
                meas: MeasId(1),
                pick: DelayPick::Strongest,
                ..
            }]
        ),
        "{r:?}"
    );
    for key in [".", ",", "Ctrl+.", "Ctrl+,", "Alt+.", "Alt+,"] {
        let r = t.key(key);
        assert!(
            matches!(
                calls(&r).as_slice(),
                [(
                    Command::DelayNudge {
                        meas: MeasId(1),
                        ..
                    },
                    _
                )]
            ),
            "{key}: {r:?}"
        );
    }
    let r = t.key("Y");
    assert!(
        matches!(
            calls(&r).as_slice(),
            [(
                Command::DelayTrack {
                    meas: MeasId(1),
                    ..
                },
                _
            )]
        ),
        "{r:?}"
    );
    t.type_key("D", "d");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::Delay(MeasId(1))
    ));
    t.key("Esc");
    // Smoothing, min / max and the cursor jumps belong to the frequency plot.
    let extremes = t.st.view.tf.extremes;
    for key in ["K", "Shift+K", "P", "Alt+P", "Alt+N", "B"] {
        assert!(t.key(key).is_empty(), "{key}");
    }
    assert_eq!(t.st.view.tf.extremes, extremes);
    assert_eq!(t.st.view.cursor_hz, None);
    // The view is still the IR's, and its hint line names its keys.
    assert_eq!(t.st.scope(), Scope::Ir);
    let h = hint_texts(&t);
    for want in ["X find delay", "R reset", "S start/stop", ",/. delay"] {
        assert!(h.contains(&want.to_owned()), "{want}: {h:?}");
    }
}

/// A jump reads the focused pane only: a transfer pane on its phase view has no magnitude
/// to jump along, and the spectrum pane beside it does not stand in.
#[test]
fn a_cursor_jump_reads_the_focused_pane_only() {
    let mut t = T::new();
    t.go(PaneKind::Transfer);
    let pane = t.st.layout.focus;
    t.key("G");
    assert_ne!(t.st.pane_modes(pane).transfer, TransferView::Magnitude);
    assert_eq!(t.st.scope(), Scope::Transfer);
    assert!(t.st.layout.lead(PaneKind::Spectrum).is_some());
    t.key("Alt+P");
    assert_eq!(t.st.view.cursor_hz, None);
    assert_eq!(t.last_toast(), ac2_scene::cursor_jump::NO_PANE);
}

/// Start/stop and reset act on the focused pane's measurement, whatever the list has
/// selected.
#[test]
fn start_stop_and_reset_stay_with_the_focused_pane() {
    let mut t = T::new();
    t.go(PaneKind::Transfer);
    t.st.rig.selected = Some(MeasId(2));
    let r = t.key("S");
    assert!(
        matches!(
            calls(&r).first(),
            Some((Command::MeasStop { meas: MeasId(1) }, _))
        ),
        "{r:?}"
    );
    let r = t.key("R");
    assert!(
        matches!(
            calls(&r).first(),
            Some((Command::MeasReset { meas: MeasId(1) }, _))
        ),
        "{r:?}"
    );
}

/// M averages the traces the focused transfer pane shows; from another pane the palette's
/// command says where it works.
#[test]
fn average_needs_the_transfer_pane_in_focus() {
    let mut t = T::new();
    t.conn(with_traces(vec![
        captured(10, Some(1), 2),
        captured(11, Some(2), 2),
    ]));
    t.key("Alt+4");
    let r =
        t.st.apply(Msg::Command(CommandId::Average), &t.keys.clone());
    assert!(r.is_empty(), "{r:?}");
    assert!(
        t.last_toast().contains("focus a Transfer pane first"),
        "{}",
        t.last_toast()
    );
    t.key("Alt+1");
    let r = t.key("M");
    assert!(
        matches!(
            calls(&r).as_slice(),
            [(Command::TraceAverage { traces, .. }, _)] if traces.len() == 2
        ),
        "{r:?}"
    );
}
