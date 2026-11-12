//! Peaks by P, end to end: a click on a curve's legend row selects it and marks nothing;
//! P to tracking lists the pane's own curve's peaks; T to traces only hides them.

use super::new_spectrum;
use crate::harness::{R, measuring};
use ac2_proto::FrameData;
use ac2_proto::model::PeakMode;
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use ac2_scene::trace::TraceKey;
use ac2_ui::state::{AppState, Msg, Overlay, PaneId};
use std::time::Instant;

const SIZE: Viewport = Viewport::new(2400.0, 900.0);

fn now() -> ac2_ui::scenes::Now {
    ac2_ui::scenes::Now {
        instant: Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    }
}

/// The peak list pane `p` draws.
fn list(s: &AppState, p: PaneId) -> Option<String> {
    ac2_ui::scenes::spectrum(s, p, &Theme::dark(), SIZE, now()).peak_list
}

/// From an empty daemon, two spectra in one pane, pink noise fired: a click on the second's
/// legend row selects it and lists no peaks; P twice (hold, then track) lists the pane's
/// own curve's peaks, and T twice (to traces only) hides them.
#[test]
fn a_legend_click_selects_and_p_tracks_from_an_empty_daemon() -> R {
    let (_daemon, mut d) = measuring()?;
    let s1 = new_spectrum(&mut d, false)?;
    let s2 = new_spectrum(&mut d, true)?;
    d.key("Alt+2");
    let p = d.st.layout.focus;
    d.drop_meas(p, s1);
    d.key("C");
    let Overlay::Choose(c) = &mut d.st.overlay else {
        return Err(format!("no tick list: {:?}", d.st.overlay).into());
    };
    c.index = 1;
    d.key("Enter");
    d.key("Escape");
    let key = TraceKey::Live(s2);
    d.until("both curves in the legend", |s| {
        let sc = ac2_ui::scenes::spectrum(s, p, &Theme::dark(), SIZE, now());
        sc.legend_hits.iter().any(|h| h.key == key) && sc.legend_hits.len() == 2
    })?;
    // Pink noise through the simulated room (no real audio), the level range fitted to it
    // so the peaks stand on the plot.
    // The level is set already (measure_from_empty): arm and fire.
    d.arm()?;
    d.play()?;
    let topic = Topic::Data {
        meas: s2,
        stream: Stream::Spec,
    };
    d.until("the noise in Spectrum 2", |s| {
        s.rig.data.as_ref().is_some_and(|x| {
            x.latest.get(&topic).is_some_and(|f| match &f.frame.data {
                FrameData::Spec(f) => f.level.iter().any(|l| *l > -80.0),
                _ => false,
            })
        })
    })?;
    d.key("Shift+Home");
    d.send(Msg::LegendPick(p, key));
    assert_eq!(d.st.rig.selected, Some(s2));
    assert_eq!(list(&d.st, p), None, "a legend click marks no peaks");
    // Each P edits the measurement on the daemon: the second steps from the first's reply.
    let mode = |s: &AppState| s.pane_meas(p).map(|m| s.display_of(m).peak);
    d.key("P");
    d.until("peaks held", |s| mode(s) == Some(PeakMode::Hold))?;
    d.key("P");
    d.until("the tracked peaks listed", |s| list(s, p).is_some())?;
    let l = list(&d.st, p).unwrap_or_default();
    assert!(
        l.lines()
            .all(|r| r.contains("Hz") || !r.starts_with(char::is_numeric)),
        "{l}"
    );
    d.key("T");
    assert!(list(&d.st, p).is_some(), "no grid keeps them");
    d.key("T");
    assert_eq!(list(&d.st, p), None, "traces only");
    Ok(())
}
