//! `ac2 levels [set|reset]`: the programme meters and loudness of the open session, their
//! settings and their holds.
//!
//! In `--json`, −∞ is the string `"-inf"` (as `spl_log` writes it): JSON has no infinity,
//! and `null` keeps each field's own meaning (not yet, gated, none), so silence and
//! *waiting for programme* stay distinct.

use std::time::{Duration, Instant};

use ac2_client::{Client, expect_body};
use ac2_proto::cal;
use ac2_proto::frame::{ClipFlags, FrameData, LoudnessSource, SessionLevelsFrame, SourceLoudness};
use ac2_proto::model::{Ballistic, LevelsSettings, PairSide, Rig, State};
use ac2_proto::topic::Topic;
use ac2_proto::units::WallNs;
use ac2_proto::{Command, ReplyBody, Subscription};
use serde_json::json;

use super::osc::rig_edit;
use super::{connect, state};
use crate::CliError;
use crate::args::*;
use crate::output::{Out, kind_word, local_offset_s, table};

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
                loudness_target: None,
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
                    "max holds, overs and loudness cleared on every input".to_owned()
                } else {
                    format!(
                        "max holds, overs and loudness cleared on input {}",
                        ac2_scene::unit_text::channels_text(&channels)
                    )
                }
            })?;
            Ok(())
        }
    }
}

/// One meter frame of the open session as tables: a row per input, a row per pair, a row
/// per loudness source.
async fn show(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    if st.session.open.is_none() {
        return Err(CliError::Usage(
            "no session is open: ac2 session open meters the inputs".into(),
        ));
    }
    let (f, captured_at) = one_frame(&c).await?;
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
                "ppm": level_json(f.ppm[i]),
                "true_peak": level_json(f.true_peak[i]),
                "max": level_json(f.max[i]),
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
    let loudness: Vec<serde_json::Value> = f.meta.loudness.iter().map(loudness_json).collect();
    out.emit(
        &json!({
            "settings": st.rig.levels,
            "inputs": rows,
            "pairs": pairs,
            "loudness": loudness,
        }),
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
            if !f.meta.loudness.is_empty() {
                s.push('\n');
                s.push_str(&loudness_table(&f.meta.loudness, captured_at));
            }
            s
        },
    )?;
    Ok(())
}

/// The ballistic input `ch` reads: the rig's, or the true peak where VU is chosen but not
/// offered (not a line input with a line calibration) and under a loudness ballistic (the
/// loudness is the source's, in the loudness table; an input's own column is its peak).
fn ballistic_of(st: &State, f: &SessionLevelsFrame, ch: u16) -> Ballistic {
    let device = st.session.open.as_ref().map(|o| &o.input_device);
    match (f.meta.ballistic, device) {
        (b, _) if b.is_loudness() => Ballistic::TruePeak,
        (Ballistic::Vu, Some(d)) if !cal::offers_vu(&st.inputs, &st.line_calibrations, d, ch) => {
            Ballistic::TruePeak
        }
        (b, _) => b,
    }
}

/// The loudness table: one row per source, the readouts as the Programme pane words them.
fn loudness_table(sources: &[SourceLoudness], captured_at: WallNs) -> String {
    let mut t = table(&[
        "source",
        "Mom",
        "Short",
        "Int",
        "LRA",
        "max Mom",
        "max Short",
        "TP max",
        "since",
    ]);
    for l in sources {
        let lufs = |v: f64| level_text(v, "LUFS");
        t.add_row([
            source_name(&l.source),
            lufs(l.momentary.0),
            lufs(l.short_term.0),
            l.integrated
                .map_or_else(|| WAITING.to_owned(), |v| lufs(v.0)),
            range_text(l),
            lufs(l.max_momentary.0),
            lufs(l.max_short_term.0),
            level_text(l.max_true_peak.0, "dBTP"),
            ac2_scene::time::local_clock(l.since, captured_at, local_offset_s),
        ]);
    }
    t.to_string()
}

/// What an integrated loudness or a range reads before a block passes its gate.
const WAITING: &str = "— (waiting for programme)";

/// Tech 3342 asks for 60 s of programme before a range is more than a first impression.
const RANGE_SETTLED_S: f64 = 60.0;

fn range_text(l: &SourceLoudness) -> String {
    match l.range {
        None => WAITING.to_owned(),
        Some(r) if l.duration.0 < RANGE_SETTLED_S => format!("{:.1} LU (settling)", r.0),
        Some(r) => format!("{:.1} LU", r.0),
    }
}

/// `1+2` for a pair, `3 mono` for one input (1-based, as the input rows).
fn source_name(s: &LoudnessSource) -> String {
    match *s {
        LoudnessSource::Pair { left, right } => {
            format!("{}+{}", u32::from(left) + 1, u32::from(right) + 1)
        }
        LoudnessSource::Input { channel } => format!("{} mono", u32::from(channel) + 1),
    }
}

fn loudness_json(l: &SourceLoudness) -> serde_json::Value {
    let source = match l.source {
        LoudnessSource::Pair { left, right } => json!({
            "type": "pair",
            "left": u32::from(left) + 1,
            "right": u32::from(right) + 1,
        }),
        LoudnessSource::Input { channel } => json!({
            "type": "input",
            "input": u32::from(channel) + 1,
        }),
    };
    json!({
        "source": source,
        "momentary": level_json(l.momentary.0),
        "short_term": level_json(l.short_term.0),
        "integrated": l.integrated.map(|v| level_json(v.0)),
        "range": l.range.map(|v| level_json(v.0)),
        "max_momentary": level_json(l.max_momentary.0),
        "max_short_term": level_json(l.max_short_term.0),
        "max_true_peak": level_json(l.max_true_peak.0),
        "duration": l.duration.0,
        "since": l.since,
    })
}

/// A level as JSON: a number, or the strings `"-inf"` / `"inf"` / `"nan"`, which JSON
/// numbers cannot carry.
pub(crate) fn level_json<T: Into<f64> + serde::Serialize + Copy>(v: T) -> serde_json::Value {
    let x: f64 = v.into();
    if x.is_finite() {
        json!(v)
    } else if x == f64::NEG_INFINITY {
        json!("-inf")
    } else if x.is_nan() {
        json!("nan")
    } else {
        json!("inf")
    }
}

/// The newest session meter frame and its capture time (daemon clock).
async fn one_frame(c: &Client) -> Result<(SessionLevelsFrame, WallNs), CliError> {
    c.subscribe(Subscription::InputMeters)?;
    let end = Instant::now() + WAIT;
    let mut got = None;
    while Instant::now() < end {
        let latest = c.latest()?;
        if let Some(l) = latest.get(&Topic::SessionLevels)
            && let FrameData::SessionLevels(f) = &l.frame.data
        {
            got = Some((f.clone(), l.frame.stamp.capture_wall_ns));
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

fn level_text(v: impl Into<f64>, unit: &str) -> String {
    let v: f64 = v.into();
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
