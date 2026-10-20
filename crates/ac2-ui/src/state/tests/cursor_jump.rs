//! Reducer tests of the cursor's peak / notch jumps and its mark.

use super::*;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

/// Octave columns from 31.25 Hz to 16 kHz: flat at 0 dB but for a 6 dB peak at 1 kHz and
/// a 6 dB notch at 4 kHz.
fn bumpy(meta: &TraceMeta) -> ConnEvent {
    let mut mag = vec![0.0f32; 10];
    mag[5] = 6.0;
    mag[7] = -6.0;
    ConnEvent::Trace(
        Arc::new(TraceData {
            meta: meta.clone(),
            mag_db: mag,
            phase_deg: None,
            coherence: None,
            sweep: None,
            ir: None,
        }),
        Arc::new(GridDef::Log {
            ppo: 1,
            k_min: -5,
            k_max: 4,
        }),
    )
}

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    }
}

const SIZE: Viewport = Viewport {
    width: 900.0,
    height: 500.0,
};

fn cursor(t: &T) -> f64 {
    t.st.view.cursor_hz.expect("cursor")
}

#[test]
fn jumps_mark_and_delta_on_the_transfer_pane() {
    let mut t = T::new();
    let tf = captured(13, Some(1), 2);
    t.conn(with_traces(vec![tf.clone()]));
    t.conn(bumpy(&tf));
    t.go(PaneKind::Transfer);
    assert_eq!(t.st.view.cursor_hz, None);
    // A jump turns the cursor on, on the peak.
    t.key("Alt+P");
    assert_eq!(cursor(&t), 1000.0);
    // Nothing higher stands out: the cursor stays, and a toast says why.
    t.key("Alt+P");
    assert_eq!(cursor(&t), 1000.0);
    assert_eq!(t.last_toast(), "no higher peak in view stands out by 3 dB");
    t.key("Alt+N");
    assert_eq!(cursor(&t), 4000.0);
    t.key("Alt+D");
    assert_eq!(t.st.view.cursor_mark_hz, Some(4000.0));
    t.key("Alt+Shift+P");
    assert_eq!(cursor(&t), 1000.0);
    let theme = Theme::dark();
    let s = crate::scenes::transfer(&t.st, t.pane(PaneKind::Transfer), &theme, SIZE, now());
    let m = s.mark.expect("mark readout");
    // A stored magnitude without phase reads no phase difference.
    assert_eq!(
        m.lines,
        [
            "mark 4.00 kHz",
            "Δ −3.00 kHz · +12.0 dB · —",
            "comb Δf 3.00 kHz · 0.33 ms · 0.11 m @ 20 °C",
        ]
    );
    // Alt+Shift+D clears the mark; the cursor stays.
    t.key("Alt+Shift+D");
    assert_eq!(t.st.view.cursor_mark_hz, None);
    assert_eq!(cursor(&t), 1000.0);
    // A mark goes with the cursor when Shift+T turns it off.
    t.key("Alt+D");
    t.key("Shift+T");
    assert_eq!(
        (t.st.view.cursor_hz, t.st.view.cursor_mark_hz),
        (None, None)
    );
    // Above the setting's prominence the 6 dB features are passed by.
    t.st.view.jump_prominence_db = 10.0;
    t.key("Alt+Shift+N");
    assert_eq!(t.st.view.cursor_hz, None);
    assert_eq!(t.last_toast(), "no lower notch in view stands out by 10 dB");
}

#[test]
fn jumps_on_the_spectrum_pane_and_outside_the_view() {
    let mut t = T::new();
    let mut sp = TraceMeta {
        kind: TraceKind::Spectrum {
            scale: LevelScale::Dbfs,
            level: SpecLevel::Tone,
        },
        ..stored(16, None, 2)
    };
    sp.edit.owner = TraceOwner::Meas { meas: MeasId(2) };
    t.conn(with_traces(vec![sp.clone()]));
    t.conn(bumpy(&sp));
    t.go(PaneKind::Spectrum);
    t.key("Alt+Shift+N");
    assert_eq!(cursor(&t), 4000.0);
    t.key("Alt+D");
    t.key("Alt+P");
    // Up from the notch: nothing; the peak is below it.
    assert_eq!(cursor(&t), 4000.0);
    t.key("Alt+Shift+P");
    assert_eq!(cursor(&t), 1000.0);
    let s = crate::scenes::spectrum(
        &t.st,
        t.pane(PaneKind::Spectrum),
        &Theme::dark(),
        SIZE,
        now(),
    );
    let m = s.mark.expect("mark readout");
    assert_eq!(m.lines[1], "Δ −3.00 kHz · +12.0 dB");
    // Zoomed to 2–20 kHz the peak is out of view: no jump to it.
    t.st.nav.set_target(ac2_scene::view::FreqRange {
        lo: 2000.0,
        hi: 20_000.0,
    });
    t.key("Alt+Shift+P");
    assert_eq!(cursor(&t), 1000.0);
    assert_eq!(t.last_toast(), "no lower peak in view stands out by 3 dB");
}

/// With neither frequency pane in the layout a jump says where it works.
#[test]
fn a_jump_needs_a_frequency_pane() {
    let mut t = T::new();
    for id in t.st.layout.root.reading_order() {
        if let Some(v) = t.st.layout.views.get_mut(&id) {
            v.kind = PaneKind::Spl;
        }
    }
    t.st.update(Msg::Command(CommandId::NextPeak), &t.keys.clone());
    assert_eq!(t.st.view.cursor_hz, None);
    assert_eq!(t.last_toast(), ac2_scene::cursor_jump::NO_PANE);
}
