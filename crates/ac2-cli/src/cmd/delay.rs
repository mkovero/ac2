//! `ac2 delay`: the transfer measurement's delay finder.

use ac2_client::expect_body;
use ac2_proto::model::*;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::meas::meas_call;
use super::{connect, find_meas, rate, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};

fn transfer<'s>(s: &'s State, r: &MeasRef) -> Result<&'s Measurement, CliError> {
    let m = find_meas(s, r)?;
    match m.config.kind {
        MeasKind::Transfer { .. } => Ok(m),
        _ => Err(CliError::Usage(format!(
            "{} is not a transfer measurement",
            m.config.name
        ))),
    }
}

fn pick(p: PickArg) -> DelayPick {
    match p {
        PickArg::First => DelayPick::FirstArrival,
        PickArg::Strongest => DelayPick::Strongest,
        PickArg::Ranked(n) => DelayPick::Ranked {
            index: n.saturating_sub(1),
        },
    }
}

fn band(b: BandArg) -> FinderBand {
    match b {
        BandArg::Full => FinderBand::Full,
        BandArg::Mid => FinderBand::Mid,
        BandArg::Sub => FinderBand::Sub,
        BandArg::Auto => FinderBand::Auto,
        BandArg::Custom(lo, hi) => FinderBand::Custom {
            lo_hz: lo.0,
            hi_hz: hi.0,
        },
    }
}

/// Human form of a finding: outcome, band, the arrivals to choose from, confidence.
fn finding_text(f: &DelayFinding, rate: Option<u32>) -> String {
    use ac2_scene::finding as sf;
    let smp = |a: &DelayArrival| match rate {
        Some(_) => format!(
            " ({} samples)",
            ac2_scene::format::fixed(a.delay_samples, 1)
        ),
        None => String::new(),
    };
    let mut out = format!(
        "{}\nband           {} · {} observed\n",
        sf::outcome_text(&f.outcome),
        sf::band_text(f.band),
        ac2_scene::format::duration(f.observation.0)
    );
    match &f.outcome {
        DelayOutcome::Accepted { first, strongest } => {
            out.push_str(&format!(
                "first arrival  {}{}\nstrongest      {}{}\n",
                ac2_scene::format::delay(first.delay.0),
                smp(first),
                ac2_scene::format::delay(strongest.delay.0),
                smp(strongest)
            ));
        }
        DelayOutcome::Ambiguous {
            ranked, strongest, ..
        } => {
            let keys: Vec<String> = (1..=ranked.len().min(3)).map(|i| i.to_string()).collect();
            out.push_str(&format!(
                "strongest      {}{}\n",
                ac2_scene::format::delay(strongest.delay.0),
                smp(strongest)
            ));
            if let Some(note) = sf::ambiguity_note(&f.outcome) {
                out.push_str(&format!("{note}\n"));
            }
            out.push_str(&format!(
                "pick one: ac2 delay insert <meas> --pick {} (1 = the rule's pick)\n",
                keys.join("|")
            ));
            let mut t = output::table(&["#", "delay", "level", "phase", "σ"]);
            for (i, a) in ranked.iter().enumerate() {
                t.add_row(vec![
                    (i + 1).to_string(),
                    ac2_scene::format::delay(a.delay.0),
                    ac2_scene::format::db_readout(a.level.0),
                    ac2_scene::format::phase_readout(a.phase.0),
                    format!("{} smp", ac2_scene::format::fixed(a.uncertainty_samples, 2)),
                ]);
            }
            out.push_str(&t.to_string());
            out.push('\n');
        }
        DelayOutcome::NoEstimate { .. } => {}
    }
    out.push_str(&format!(
        "confidence     {}",
        sf::confidence_text(&f.confidence)
    ));
    if !matches!(f.outcome, DelayOutcome::Ambiguous { .. }) && !f.candidates.is_empty() {
        let mut t = output::table(&["candidate", "delay", "level"]);
        for (i, a) in f.candidates.iter().enumerate() {
            t.add_row(vec![
                i.to_string(),
                ac2_scene::format::delay(a.delay.0),
                ac2_scene::format::db_readout(a.level.0),
            ]);
        }
        out.push('\n');
        out.push_str(&t.to_string());
    }
    out
}

pub(crate) async fn delay(cli: &Cli, cmd: &DelayCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let s = state(&c).await?;
    match cmd {
        DelayCmd::Find {
            meas,
            band: b,
            observation,
            insert,
        } => {
            let id = transfer(&s, meas)?.id;
            let r = c
                .call(Command::DelayFind {
                    meas: id,
                    band: band(*b),
                    observation: observation.map(|t| t.0),
                })
                .await?;
            let f = expect_body!("delay.find", r, ReplyBody::DelayFinding(f) => f)?;
            // A refusal inserts nothing; the finding (with its reasons) is still printed.
            let inserted = match insert.filter(|_| f.no_estimate().is_none()) {
                Some(p) => Some(
                    meas_call(
                        &c,
                        Command::DelayInsert {
                            meas: id,
                            pick: pick(p),
                        },
                    )
                    .await?,
                ),
                None => None,
            };
            let rate = rate(&s);
            out.emit(&json!({ "finding": f, "inserted": inserted }), || {
                let mut t = finding_text(&f, rate);
                if let Some(m) = &inserted {
                    t.push_str(&format!("\ninserted: {}", output::measurement(m)));
                }
                t
            })?;
        }
        DelayCmd::Insert { meas, pick: p } => {
            let id = transfer(&s, meas)?.id;
            let m = meas_call(
                &c,
                Command::DelayInsert {
                    meas: id,
                    pick: pick(*p),
                },
            )
            .await?;
            out.emit(&m, || output::measurement(&m))?;
        }
        DelayCmd::Set { meas, delay, temp } => {
            let id = transfer(&s, meas)?.id;
            let d = delay
                .seconds(rate(&s), *temp)
                .map_err(|e| CliError::Usage(e.0))?;
            let m = meas_call(&c, Command::DelaySet { meas: id, delay: d }).await?;
            out.emit(&m, || output::measurement(&m))?;
        }
        DelayCmd::Nudge { meas, by, temp } => {
            let id = transfer(&s, meas)?.id;
            let by = by
                .seconds(rate(&s), *temp)
                .map_err(|e| CliError::Usage(e.0))?;
            let m = meas_call(&c, Command::DelayNudge { meas: id, by }).await?;
            out.emit(&m, || output::measurement(&m))?;
        }
        DelayCmd::Track { meas, state: on } => {
            let id = transfer(&s, meas)?.id;
            let m = meas_call(
                &c,
                Command::DelayTrack {
                    meas: id,
                    enabled: *on == Switch::On,
                },
            )
            .await?;
            out.emit(&m, || output::measurement(&m))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A merged lobe lists one candidate: the text explains why and offers only `--pick 1`.
    #[test]
    fn merged_lobe_finding_offers_one_pick() {
        use ac2_proto::units::{Db, Degrees, Seconds, WallNs};
        let peak = DelayArrival {
            delay: Seconds(3.346e-3),
            delay_samples: 160.6,
            level: Db(0.0),
            phase: Degrees(160.0),
            uncertainty_samples: 0.1,
            misfit: 0.17,
            refined: true,
        };
        let f = DelayFinding {
            outcome: DelayOutcome::Ambiguous {
                reasons: vec![AmbiguityReason::MergedLobe],
                ranked: vec![peak],
                strongest: peak,
            },
            confidence: DelayConfidence {
                psr_db: None,
                psr_acq_db: None,
                band_snr_db: None,
                excited_fraction: None,
                uncertainty_samples: None,
                pulse_width_samples: None,
                period: None,
            },
            band: DelayBand::Full,
            observation: Seconds(0.25),
            candidates: vec![peak],
            found_at: WallNs(0),
        };
        let t = finding_text(&f, Some(48_000));
        assert!(
            t.starts_with("AMBIGUOUS · arrivals merged into one peak\n"),
            "{t}"
        );
        assert!(t.contains("One peak only: two arrivals"), "{t}");
        assert!(t.contains("--pick 1 (1 = the rule's pick)"), "{t}");
        assert!(!t.contains("1|2"), "{t}");
    }
}
