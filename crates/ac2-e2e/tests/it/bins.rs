//! A transfer at `bins` from the app: a pane zoomed into the top octave on a wide plot
//! receives the full-rate stage's own bins and draws its curve through them.

use super::harness::{R, measuring_kept};
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
    let (_daemon, mut d) = measuring_kept()?;
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

/// A math over two live transfers at `bins` digs down as a transfer does: shown alone on a
/// pane zoomed into the top octave on a wide plot, its `tf` states the operands' layout, the
/// app subscribes to its full-rate stage and draws its curve through those bins.
#[test]
fn a_math_of_live_bins_draws_its_bins() -> R {
    use super::harness::Cli;
    use ac2_proto::model::{MathConfig, MathDomain, Operand, TraceOwner};
    use ac2_proto::{Command, ReplyBody};
    use ac2_scene::trace::TraceKey;
    let (daemon, mut d) = measuring_kept()?;
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
    let a = d.st.pane_meas(pane).ok_or("the pane's measurement")?;
    let (a, mut second) = (a.id, a.config.clone());
    second.name = "Seat B".into();

    let cli = Cli::connect(daemon.endpoints())?;
    let made = |r: ReplyBody| match r {
        ReplyBody::Measurement(m) => Ok(m.id),
        other => Err(format!("{other:?}")),
    };
    let b = made(cli.call(Command::MeasCreate { config: second })?)?;
    cli.call(Command::MeasStart { meas: b })?;
    let mut math =
        d.st.pane_meas(pane)
            .ok_or("the measurement")?
            .config
            .clone();
    math.name = "Average".into();
    math.kind = MeasKind::Math {
        config: MathConfig::power_average(
            TraceOwner::Meas { meas: a },
            MathDomain::Transfer,
            vec![Operand::Meas { meas: a }, Operand::Meas { meas: b }],
        ),
    };
    let m = made(cli.call(Command::MeasCreate { config: math })?)?;
    cli.call(Command::MeasStart { meas: m })?;
    d.until("the math in the app", |s| {
        s.measurements().iter().any(|x| x.id == m && x.running)
    })?;
    d.drop_meas(pane, m);
    d.until("the math on the pane", |s| {
        s.pane_meas(pane).is_some_and(|x| x.id == m)
    })?;

    d.st.view.freq = ac2_scene::view::FreqRange {
        lo: 10_000.0,
        hi: 20_000.0,
    };
    d.st.nav = ac2_ui::anim::FreqNav::new(d.st.view.freq);
    d.send(Msg::PlotWidth { pane, px: 1900.0 });
    let stage0 = Topic::Bins { meas: m, stage: 0 };
    d.until("the math's stage 0 wanted", |s| {
        s.wanted_topics().contains(&stage0)
    })?;
    d.until("its stage 0 averaged", |s| {
        s.rig.data.as_ref().is_some_and(|x| {
            x.latest
                .get(&stage0)
                .is_some_and(|f| matches!(&f.frame.data, FrameData::Bins(b) if b.meta.blocks > 0))
        })
    })?;
    let size = ac2_scene::primitives::Viewport::new(2000.0, 600.0);
    let now = ac2_ui::scenes::Now {
        instant: std::time::Instant::now(),
        wall: ac2_proto::units::WallNs(0),
    };
    d.until("the math's bins drawn", |s| {
        ac2_ui::scenes::transfer(s, pane, &Theme::dark(), size, now)
            .bins
            .is_some_and(|sp| {
                sp.traces.iter().zip(&sp.spans).any(|(t, spans)| {
                    t.key == TraceKey::Live(m) && spans.iter().any(|x| x.stage == 0)
                })
            })
    })?;
    Ok(())
}
