//! A transfer at `bins` from the app: a pane zoomed into the top octave on a wide plot
//! receives the full-rate stage's own bins and draws its curve through them.

use super::harness::{R, measuring};
use ac2_proto::FrameData;
use ac2_proto::model::{MeasKind, Measurement, Resolution, TransferResolution};
use ac2_proto::topic::Topic;
use ac2_scene::theme::Theme;
use ac2_ui::state::{AppState, Msg, PaneKind};

fn resolution(s: &AppState) -> Option<TransferResolution> {
    s.measurements()
        .iter()
        .find_map(|m: &&Measurement| match &m.config.kind {
            MeasKind::Transfer { config } => Some(config.resolution),
            _ => None,
        })
}

#[test]
fn a_zoom_into_the_top_octave_draws_the_full_rate_bins() -> R {
    let (_daemon, mut d) = measuring()?;
    // The simulated rig's noise: a reference to average on.
    d.arm()?;
    d.play()?;
    let pane = d.st.lead(PaneKind::Transfer).ok_or("a transfer pane")?;
    d.send(Msg::FocusPane(pane));
    d.key("Alt+Shift+K");
    d.until("1/96", |s| {
        resolution(s) == Some(Resolution::NinetySixth.into())
    })?;
    d.key("Alt+Shift+K");
    d.until("bins", |s| resolution(s) == Some(TransferResolution::Bins))?;
    d.st.view.freq = ac2_scene::view::FreqRange {
        lo: 10_000.0,
        hi: 20_000.0,
    };
    d.st.nav = ac2_ui::anim::FreqNav::new(d.st.view.freq);
    d.send(Msg::PlotWidth { pane, px: 1900.0 });
    let meas =
        d.st.pane_meas(pane)
            .map(|m| m.id)
            .ok_or("the pane's measurement")?;
    let stage0 = Topic::Bins { meas, stage: 0 };
    // Once a `tf` at bins states its layout.
    d.until("stage 0 wanted", |s| s.wanted_topics().contains(&stage0))?;
    assert!(
        !d.st
            .wanted_topics()
            .contains(&Topic::Bins { meas, stage: 1 })
    );
    d.until("stage 0 averaged", |s| {
        s.rig.data.as_ref().is_some_and(|x| {
            x.latest
                .get(&stage0)
                .is_some_and(|f| matches!(&f.frame.data, FrameData::Bins(b) if b.meta.blocks > 0))
        })
    })?;
    // The scene's plot at this viewport is the reported 1900 px or wider.
    let size = ac2_scene::primitives::Viewport::new(2000.0, 600.0);
    let now = ac2_ui::scenes::Now {
        instant: std::time::Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    d.until("the bins drawn", |s| {
        ac2_ui::scenes::transfer(s, pane, &Theme::dark(), size, now)
            .bins
            .is_some_and(|b| b.spans.iter().flatten().any(|sp| sp.stage == 0))
    })?;
    Ok(())
}
