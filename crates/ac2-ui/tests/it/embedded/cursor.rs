//! The comparison cursor's jumps and mark from an empty daemon.

use super::harness::{Driver, NAME, R, measure_from_empty};
use ac2_scene::theme::Theme;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::state::{AppState, PaneKind};
use std::time::Instant;

/// The transfer pane's scene.
fn transfer(s: &AppState) -> ac2_scene::tf::TfScene {
    ac2_ui::scenes::transfer(
        s,
        crate::common::pane(s, PaneKind::Transfer),
        &Theme::dark(),
        ac2_scene::primitives::Viewport {
            width: 1200.0,
            height: 600.0,
        },
        ac2_ui::scenes::Now {
            instant: Instant::now(),
            wall: ac2_proto::units::WallNs(0),
        },
    )
}

/// From an empty daemon, on the measurement made and stopped (its curve holds still): the
/// rig's mic path is flat to the eye but its last average is ragged at the top, where the
/// stimulus stopped feeding it. Alt+P puts the cursor on a peak, a second Alt+P on a higher
/// one and Alt+Shift+P back; Alt+D marks it, Alt+N goes on to a notch above, and the
/// readout reads the cursor against the mark on the measurement's curve, with no comb from
/// a peak to a notch; Alt+P to the next peak reads what the curve says of the comb that
/// spacing would make.
/// Alt+Shift+D clears the mark.
#[test]
fn cursor_jumps_and_mark_from_an_empty_daemon() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    assert_eq!(d.st.layout.focus_kind(), PaneKind::Transfer);
    d.key("S");
    d.until("the measurement stopped", |s| {
        s.selected_meas().is_some_and(|m| !m.running)
    })?;
    assert_eq!(d.st.view.cursor_hz, None);

    d.key("Alt+P");
    let first = d.st.view.cursor_hz.ok_or("a peak")?;
    d.key("Alt+P");
    let second = d.st.view.cursor_hz.ok_or("cursor")?;
    assert!(second > first, "{first} → {second}");
    d.key("Alt+Shift+P");
    assert_eq!(d.st.view.cursor_hz, Some(first));

    d.key("Alt+D");
    assert_eq!(d.st.view.cursor_mark_hz, Some(first));
    d.key("Alt+N");
    let notch = d.st.view.cursor_hz.ok_or("cursor")?;
    assert!(notch > first, "{first} → {notch}");
    let s = transfer(&d.st);
    let m = s.mark.ok_or("mark readout")?;
    assert_eq!(m.freq_hz, first);
    assert_eq!(
        m.lines[0],
        format!("mark {}", ac2_scene::format::freq_readout(first))
    );
    assert!(m.lines[1].starts_with("Δ +"), "{m:?}");
    // A peak to a notch is half a comb's spacing: no comb line.
    assert_eq!(m.lines.len(), 2, "{m:?}");
    // Peak to peak spaces one, if the curve repeats the spacing. This ripple is the
    // average's noise, so whether a third peak falls one spacing beyond is chance from run
    // to run: either verdict, but nothing else (the scene's tests pin which is which).
    d.key("Alt+P");
    let peak = d.st.view.cursor_hz.ok_or("cursor")?;
    assert!(peak > notch, "{notch} → {peak}");
    let m = transfer(&d.st).mark.ok_or("mark readout")?;
    let confirmed = ac2_scene::readout::comb_text(peak - first, 20.0);
    assert!(
        m.lines[2] == confirmed || m.lines[2] == "comb: uneven, no third peak",
        "{m:?}"
    );

    d.key("Alt+Shift+D");
    assert_eq!(d.st.view.cursor_mark_hz, None);
    assert_eq!(transfer(&d.st).mark, None);
    Ok(())
}
