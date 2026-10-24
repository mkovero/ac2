//! `ac2 levels [set|reset]`: the programme meters of the open session, their settings and
//! their holds.

use std::time::{Duration, Instant};

use ac2_client::{Client, expect_body};
use ac2_proto::cal;
use ac2_proto::frame::{ClipFlags, FrameData, SessionLevelsFrame};
use ac2_proto::model::{Ballistic, LevelsSettings, PairSide, Rig, State};
use ac2_proto::topic::Topic;
use ac2_proto::{Command, ReplyBody, Subscription};
use serde_json::json;

use super::osc::rig_edit;
use super::{connect, state};
use crate::CliError;
use crate::args::*;
use crate::output::{Out, kind_word, table};

/// How long one meter frame may take to arrive (they come thirty times a second).
const WAIT: Duration = Duration::from_secs(3);

pub(crate) async fn run(
    cli: &Cli,
    cmd: Option<&LevelsCmd>,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    match cmd {
        None => show(cli, out).await,
        Some(LevelsCmd::Set {
            ballistic,
            window,
            over_threshold,
            gate,
        }) => {
            let c = connect(cli, false).await?;
            let (ballistic, window, over, gate) = (
                ballistic.map(Ballistic::from),
                window.map(|w| w.0),
                over_threshold.map(|t| t.0),
                gate.map(|g| g.0),
            );
            let rig = rig_edit(&c, |base| Command::LevelsSet {
                base,
                ballistic,
                correlation_window: window,
                over_threshold: over,
                correlation_gate: gate,
            })
            .await?;
            out.emit(&rig, || settings_text(&rig.levels))?;
            Ok(())
        }
        Some(LevelsCmd::Reset { inputs }) => {
            let c = connect(cli, false).await?;
            let channels: Vec<u16> = inputs.iter().map(|i| i.0).collect();
            let r = c
                .call(Command::LevelsReset {
                    channels: channels.clone(),
                })
                .await?;
            let rev = expect_body!("levels.reset", r, ReplyBody::Ack { rev } => rev)?;
            out.emit(&json!({ "rev": rev, "channels": channels }), || {
                if channels.is_empty() {
                    "max holds and overs cleared on every input".to_owned()
                } else {
                    format!(
                        "max holds and overs cleared on input {}",
                        ac2_scene::unit_text::channels_text(&channels)
                    )
                }
            })?;
            Ok(())
        }
    }
}

/// One meter frame of the open session as a table: a row per input, a row per pair.
async fn show(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    if st.session.open.is_none() {
        return Err(CliError::Usage(
            "no session is open: ac2 session open meters the inputs".into(),
        ));
    }
    let f = one_frame(&c).await?;
    let rows: Vec<serde_json::Value> = f
        .meta
        .channels
        .iter()
        .enumerate()
        .map(|(i, &ch)| {
            let b = ballistic_of(&st, &f, ch);
            json!({
                "input": u32::from(ch) + 1,
                "kind": kind_word(&cal::input_kind(&st.inputs, ch)),
                "ballistic": b,
                "ppm": f.ppm[i],
                "true_peak": f.true_peak[i],
                "max": f.max[i],
                "overs": f.overs[i],
                "clip": f.clip[i].contains(ClipFlags::CLIP) || f.clip[i].contains(ClipFlags::HELD),
            })
        })
        .collect();
    let pairs: Vec<serde_json::Value> = f
        .meta
        .pair_levels
        .iter()
        .map(|p| {
            json!({
                "left": u32::from(p.left) + 1,
                "right": u32::from(p.right) + 1,
                "correlation": p.correlation,
                "quiet": p.quiet,
                "window": st.rig.levels.correlation_window,
            })
        })
        .collect();
    out.emit(
        &json!({ "settings": st.rig.levels, "inputs": rows, "pairs": pairs }),
        || {
            let mut t = table(&[
                "in",
                "kind",
                "ballistic",
                "ppm",
                "true peak",
                "max",
                "overs",
                "CLIP",
            ]);
            for (i, &ch) in f.meta.channels.iter().enumerate() {
                let b = ballistic_of(&st, &f, ch);
                let clip =
                    f.clip[i].contains(ClipFlags::CLIP) || f.clip[i].contains(ClipFlags::HELD);
                t.add_row([
                    (u32::from(ch) + 1).to_string(),
                    kind_word(&cal::input_kind(&st.inputs, ch)).to_owned(),
                    b.word().to_owned(),
                    level_text(f.ppm[i], b.unit()),
                    level_text(f.true_peak[i], "dBTP"),
                    level_text(f.max[i], b.unit()),
                    f.overs[i].to_string(),
                    if clip { "CLIP" } else { "" }.to_owned(),
                ]);
            }
            let mut s = t.to_string();
            if !f.meta.pair_levels.is_empty() {
                let mut p = table(&["pair", "correlation", "window"]);
                for pl in &f.meta.pair_levels {
                    p.add_row([
                        format!("{} · {}", u32::from(pl.left) + 1, u32::from(pl.right) + 1),
                        correlation_text(pl.correlation, pl.quiet),
                        st.rig.levels.correlation_window.text().to_owned(),
                    ]);
                }
                s.push('\n');
                s.push_str(&p.to_string());
            }
            s
        },
    )?;
    Ok(())
}

/// The ballistic input `ch` reads: the rig's, or the true peak where VU is chosen but not
/// offered (not a line input with a line calibration).
fn ballistic_of(st: &State, f: &SessionLevelsFrame, ch: u16) -> Ballistic {
    let device = st.session.open.as_ref().map(|o| &o.input_device);
    match (f.meta.ballistic, device) {
        (Ballistic::Vu, Some(d)) if !cal::offers_vu(&st.inputs, &st.line_calibrations, d, ch) => {
            Ballistic::TruePeak
        }
        (b, _) => b,
    }
}

async fn one_frame(c: &Client) -> Result<SessionLevelsFrame, CliError> {
    c.subscribe(Subscription::InputMeters)?;
    let end = Instant::now() + WAIT;
    let mut got = None;
    while Instant::now() < end {
        let latest = c.latest()?;
        if let Some(FrameData::SessionLevels(f)) =
            latest.get(&Topic::SessionLevels).map(|f| &f.frame.data)
        {
            got = Some(f.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = c.unsubscribe(Subscription::InputMeters);
    got.ok_or_else(|| {
        CliError::Usage(format!(
            "no meter frame in {} s: is the audio running? (ac2 session status)",
            WAIT.as_secs()
        ))
    })
}

fn level_text(v: f32, unit: &str) -> String {
    if v.is_finite() {
        format!("{v:.1} {unit}")
    } else {
        "—".to_owned()
    }
}

/// `+0.87`, or which side is quiet.
pub(crate) fn correlation_text(r: Option<f32>, quiet: Option<PairSide>) -> String {
    match (r, quiet) {
        (Some(r), _) => format!("{r:+.2}"),
        (None, Some(PairSide::Left)) => "left quiet".to_owned(),
        (None, Some(PairSide::Right)) => "right quiet".to_owned(),
        (None, None) => "—".to_owned(),
    }
}

/// The settings as `levels set` and `rig status` say them.
pub(crate) fn settings_text(l: &LevelsSettings) -> String {
    format!(
        "programme meters: {}; correlation over {}; over above {} dBTP; gate {} dBFS",
        l.ballistic.word(),
        l.correlation_window.text(),
        l.over_threshold.0,
        l.correlation_gate.0
    )
}

/// The programme-meter line of `rig status`.
pub(crate) fn status_text(rig: &Rig) -> String {
    format!("{}\n", settings_text(&rig.levels))
}
