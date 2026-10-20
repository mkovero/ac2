//! `ac2 align new|show`: two transfer curves aligned in their overlap. The daemon fits
//! (`docs/design/alignment.md`); this names the operands, waits for a fit and prints it as
//! the app's alignment view words it.

use std::time::{Duration, Instant};

use ac2_client::Client;
use ac2_proto::model::{
    AlignOutcome, AlignState, MathConfig, MathDomain, MathExpr, MeasConfig, MeasKind, MeasScope,
    Measurement, TraceOwner,
};
use ac2_proto::{Command, FrameData, Stream, Subscription, Topic};
use serde_json::json;

use super::math::{default_owner, find_operand, operand_name};
use super::meas::meas_call;
use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::{AlignCmd, Cli};
use crate::output::Out;

/// How long a fit is waited for: an operand's first averages take a few frames.
const WAIT: Duration = Duration::from_secs(10);

pub(crate) async fn run(cli: &Cli, cmd: &AlignCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    let (m, temp) = match cmd {
        AlignCmd::New(a) => {
            let (oa, ob) = (find_operand(&s, &a.a)?, find_operand(&s, &a.b)?);
            let owner = match (&a.under, a.imported) {
                (_, true) => TraceOwner::Imported,
                (Some(u), false) => TraceOwner::Meas {
                    meas: find_meas(&s, u)?.id,
                },
                (None, false) => default_owner(&s, oa),
            };
            let expr = MathExpr::Align {
                a: oa,
                b: ob,
                band: a.band.0,
                max_delay: a.max_delay.0,
            };
            let name = a
                .name
                .clone()
                .unwrap_or_else(|| ac2_scene::math::default_name(&expr, |o| operand_name(&s, o)));
            let config = MeasConfig {
                scope: MeasScope::Session,
                name,
                kind: MeasKind::Math {
                    config: MathConfig::of(owner, MathDomain::Transfer, expr),
                },
            };
            let m = meas_call(&c, Command::MeasCreate { config }).await?;
            let m = meas_call(&c, Command::MeasStart { meas: m.id }).await?;
            (m, a.temp)
        }
        AlignCmd::Show { alignment, temp } => (find_meas(&s, alignment)?.clone(), *temp),
    };
    let MeasKind::Math { config } = &m.config.kind else {
        return Err(not_alignment(&m));
    };
    let MathExpr::Align { a, b, band, .. } = config.expr else {
        return Err(not_alignment(&m));
    };
    let fit = newest_fit(&c, &m).await?;
    let (na, nb) = (operand_name(&s, a), operand_name(&s, b));
    let r = ac2_scene::align::readout(&fit, &na, &nb, band, temp.0);
    let doc = json!({
        "meas": m.id,
        "name": m.config.name,
        "a": na,
        "b": nb,
        "band": band,
        "temperature_c": temp.0,
        "align": fit,
        "readout": { "headline": r.headline, "lines": r.lines },
    });
    out.emit(&doc, || {
        let mut t = format!("{}: {}\n", m.config.name, r.headline);
        for l in &r.lines {
            t.push_str(&format!("  {l}\n"));
        }
        t
    })?;
    Ok(())
}

fn not_alignment(m: &Measurement) -> CliError {
    CliError::Usage(format!(
        "{} is not an alignment (ac2 align new makes one)",
        m.config.name
    ))
}

/// The alignment's newest fit: the first frame with a fit, else after [`WAIT`] the newest
/// state (why there is none).
async fn newest_fit(c: &Client, m: &Measurement) -> Result<AlignState, CliError> {
    let topic = Topic::Data {
        meas: m.id,
        stream: Stream::Tf,
    };
    c.subscribe(Subscription::Topic(topic))?;
    let end = Instant::now() + WAIT;
    let mut newest = None;
    while Instant::now() < end {
        let latest = c.latest()?;
        if let Some(FrameData::Tf(f)) = latest.get(&topic).map(|f| &f.frame.data)
            && let Some(st) = f.meta.math.as_ref().and_then(|x| x.align.as_deref())
        {
            if matches!(st.outcome, AlignOutcome::Fit { .. }) {
                newest = Some(*st);
                break;
            }
            newest = Some(*st);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = c.unsubscribe(Subscription::Topic(topic));
    newest.ok_or_else(|| {
        CliError::Usage(format!(
            "no frame from {} in {} s: is it running? (ac2 meas start)",
            m.config.name,
            WAIT.as_secs()
        ))
    })
}
