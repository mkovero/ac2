//! `spl leq …`: rolling Leq windows of an SPL meter (`docs/design/leq.md`).

use ac2_client::{Client, expect_body};
use ac2_proto::model::{
    LeqConfig, LeqPreset, LeqWindow, MeasConfig, MeasKind, Measurement, SplConfig, SplLogPage,
    SplLogRow, State, TimeWeighting, Weighting,
};
use ac2_proto::units::{Db, Seconds};
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::{Cli, LeqCmd, LeqExport, LeqSet, LeqWatch, MeterRef, PresetArg};
use crate::output::{self, Out};
use crate::watch;

pub(crate) async fn run(cli: &Cli, cmd: &LeqCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        LeqCmd::Watch(w) => watch_cmd(cli, w, out).await,
        LeqCmd::Set(s) => set(cli, s, out).await,
        LeqCmd::Export(e) => export(cli, e, out).await,
    }
}

/// The SPL meter `r` names: `--meas`, the one on `--input`, or the only one.
fn meter<'s>(s: &'s State, r: &MeterRef) -> Result<Option<&'s Measurement>, CliError> {
    let is_spl = |m: &&Measurement| matches!(m.config.kind, MeasKind::Spl { .. });
    if let Some(m) = &r.meas {
        let m = find_meas(s, m)?;
        if !is_spl(&m) {
            return Err(CliError::Usage(format!(
                "{} is not an SPL meter",
                m.config.name
            )));
        }
        return Ok(Some(m));
    }
    let meters: Vec<&Measurement> = s.measurements.iter().filter(is_spl).collect();
    if let Some(input) = r.input {
        let on: Vec<&Measurement> = meters
            .into_iter()
            .filter(
                |m| matches!(&m.config.kind, MeasKind::Spl { config } if config.input == input.0),
            )
            .collect();
        return match on.as_slice() {
            [] => Ok(None),
            [m] => Ok(Some(m)),
            more => Err(CliError::Usage(format!(
                "{} SPL meters on input {input} ({}); pick one with --meas",
                more.len(),
                names(more)
            ))),
        };
    }
    match meters.as_slice() {
        [] => Err(CliError::Usage(
            "no SPL meter: make one with `ac2 meas new spl --input N --start`, or give --input"
                .into(),
        )),
        [m] => Ok(Some(m)),
        more => Err(CliError::Usage(format!(
            "{} SPL meters ({}); pick one with --meas or --input",
            more.len(),
            names(more)
        ))),
    }
}

fn names(ms: &[&Measurement]) -> String {
    ms.iter()
        .map(|m| format!("{} {:?}", m.id.0, m.config.name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn spl_config(m: &Measurement) -> Result<&SplConfig, CliError> {
    match &m.config.kind {
        MeasKind::Spl { config } => Ok(config),
        _ => Err(CliError::Usage(format!(
            "{} is not an SPL meter",
            m.config.name
        ))),
    }
}

/// `spl leq watch`: an existing meter, or on `--input` without one a meter made (and kept:
/// its windows and log are meant to run on) for it.
async fn watch_cmd(cli: &Cli, w: &LeqWatch, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, true).await?;
    let s = state(&c).await?;
    let m = match meter(&s, &w.meter)? {
        Some(m) => m.clone(),
        None => {
            let input = w
                .meter
                .input
                .ok_or_else(|| CliError::Usage("give --input".into()))?;
            let r = c
                .call(Command::MeasCreate {
                    config: MeasConfig {
                        name: format!("SPL in {input}"),
                        kind: MeasKind::Spl {
                            config: SplConfig::on_input(input.0, Weighting::A, TimeWeighting::Fast),
                        },
                    },
                })
                .await?;
            let m = expect_body!("meas.create", r, ReplyBody::Measurement(m) => m)?;
            if !out.json {
                eprintln!(
                    "made SPL meter {} ({:?}) on input {input}; it keeps logging (`ac2 meas rm {}` removes it)",
                    m.id.0, m.config.name, m.id.0
                );
            }
            m
        }
    };
    if !m.running {
        c.call(Command::MeasStart { meas: m.id }).await?;
    }
    let until = w
        .duration
        .map(|t| std::time::Instant::now() + std::time::Duration::from_secs_f64(t.0.0.max(0.0)));
    watch::leq(&c, m.id, until, out).await
}

fn preset(p: PresetArg) -> LeqPreset {
    match p {
        PresetArg::Din15905 => LeqPreset::Din15905,
        PresetArg::Swiss93 => LeqPreset::Swiss93,
        PresetArg::Swiss96 => LeqPreset::Swiss96,
        PresetArg::Swiss100 => LeqPreset::Swiss100,
        PresetArg::Who => LeqPreset::Who,
    }
}

/// The windows `s` asks for, from the meter's `cur`.
pub(crate) fn apply(cur: &LeqConfig, s: &LeqSet) -> Result<LeqConfig, CliError> {
    let mut cfg = cur.clone();
    if let Some(ws) = &s.windows {
        cfg.windows = ws
            .iter()
            .map(|w| {
                let d = Seconds(f64::from(w.seconds));
                cur.windows
                    .iter()
                    .find(|o| o.duration == d && o.weighting == w.weighting())
                    .copied()
                    .unwrap_or(LeqWindow {
                        duration: d,
                        weighting: w.weighting(),
                        limit: None,
                        warn_margin: Db(LeqWindow::DEFAULT_WARN_MARGIN_DB),
                    })
            })
            .collect();
    }
    for p in &s.preset {
        preset(*p).apply(&mut cfg.windows);
    }
    for l in &s.limits {
        let d = Seconds(f64::from(l.window.seconds));
        let wt = l.window.weighting();
        let Some(w) = cfg
            .windows
            .iter_mut()
            .find(|w| w.duration == d && w.weighting == wt)
        else {
            let name = ac2_scene::leq::window_name(&LeqWindow {
                duration: d,
                weighting: wt,
                limit: None,
                warn_margin: Db(0.0),
            });
            return Err(CliError::Usage(format!(
                "the meter has no {name} window; add it with --windows"
            )));
        };
        w.limit = l.limit;
    }
    if let Some(m) = s.warn {
        for w in &mut cfg.windows {
            w.warn_margin = m.0;
        }
    }
    if let Some(h) = s.horizon {
        cfg.horizon = Seconds(f64::from(h.seconds));
    }
    cfg.check().map_err(CliError::Usage)?;
    Ok(cfg)
}

/// The windows as a table: name, limit, warn margin.
fn windows_table(cfg: &LeqConfig) -> String {
    let mut t = output::table(&["window", "limit", "warn within"]);
    for w in &cfg.windows {
        t.add_row(vec![
            ac2_scene::leq::window_name(w),
            w.limit.map_or_else(
                || "none".to_owned(),
                |l| format!("{} dB", ac2_scene::format::level(l.0)),
            ),
            format!("{} dB", ac2_scene::format::level(w.warn_margin.0)),
        ]);
    }
    format!(
        "{t}\nheadroom over the next {}",
        ac2_scene::leq::length(cfg.horizon.0)
    )
}

async fn set(cli: &Cli, s: &LeqSet, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = meter(&st, &s.meter)?.ok_or_else(|| {
        CliError::Usage("no SPL meter on that input: make one with `ac2 meas new spl`".into())
    })?;
    let cfg = spl_config(m)?;
    let leq = apply(&cfg.leq, s)?;
    let r = c
        .call(Command::MeasUpdate {
            meas: m.id,
            config: MeasConfig {
                name: m.config.name.clone(),
                kind: MeasKind::Spl {
                    config: SplConfig {
                        leq: leq.clone(),
                        ..cfg.clone()
                    },
                },
            },
        })
        .await?;
    let m = expect_body!("meas.update", r, ReplyBody::Measurement(m) => m)?;
    let presets: Vec<String> = s
        .preset
        .iter()
        .map(|p| {
            let p = preset(*p);
            let w = p.window();
            format!(
                "{}: {} ≤ {} dB — {} (informational, not legal advice)",
                p.name(),
                ac2_scene::leq::window_name(&w),
                ac2_scene::format::level(w.limit.map_or(f64::NAN, |l| l.0)),
                p.source()
            )
        })
        .collect();
    out.emit(&m, || {
        let mut text = format!(
            "{}: Leq windows set\n{}",
            m.config.name,
            windows_table(&leq)
        );
        for p in &presets {
            text.push('\n');
            text.push_str(p);
        }
        text
    })?;
    Ok(())
}

/// Every row of the meter's log, paged.
pub(crate) async fn all_rows(
    c: &Client,
    meas: ac2_proto::units::MeasId,
) -> Result<Vec<SplLogRow>, CliError> {
    let mut rows = Vec::new();
    let mut from = 0;
    loop {
        let r = c
            .call(Command::SplLogGet {
                meas,
                from,
                max: SplLogPage::MAX_ROWS,
            })
            .await?;
        let p = expect_body!("spl.log_get", r, ReplyBody::SplLogPage(p) => p)?;
        let n = p.rows.len() as u64;
        rows.extend(p.rows);
        from = p.from + n;
        if n == 0 || from >= p.total {
            return Ok(rows);
        }
    }
}

async fn export(cli: &Cli, e: &LeqExport, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = meter(&st, &e.meter)?
        .ok_or_else(|| CliError::Usage("no SPL meter on that input".into()))?;
    let cfg = spl_config(m)?;
    let rows = all_rows(&c, m.id).await?;
    let info = ac2_traces::spl_log::SplLogInfo {
        meas: m.id,
        name: m.config.name.clone(),
        input: cfg.input,
        mic: st
            .inputs
            .iter()
            .find(|i| i.channel == cfg.input)
            .and_then(|i| i.mic.clone()),
    };
    let csv = ac2_traces::spl_log::export_csv(&info, &rows);
    match &e.out {
        Some(path) => {
            std::fs::write(path, csv.as_bytes())?;
            let first = rows
                .first()
                .map(|r| ac2_traces::spl_log::utc_iso(r.start.0));
            let last = rows.last().map(|r| ac2_traces::spl_log::utc_iso(r.start.0));
            out.emit(
                &json!({
                    "meas": m.id.0,
                    "file": path.to_string_lossy(),
                    "rows": rows.len(),
                    "first": first,
                    "last": last,
                }),
                || {
                    format!(
                        "{} seconds of {} written to {}{}",
                        rows.len(),
                        m.config.name,
                        path.display(),
                        match (&first, &last) {
                            (Some(a), Some(b)) => format!(" ({a} … {b})"),
                            _ => String::new(),
                        }
                    )
                },
            )?;
        }
        None => {
            out.w.write_all(csv.as_bytes())?;
            out.w.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::LeqSet;
    use clap::Parser;

    fn set_args(args: &[&str]) -> LeqSet {
        let argv: Vec<&str> = ["ac2", "spl", "leq", "set"]
            .into_iter()
            .chain(args.iter().copied())
            .collect();
        match crate::Cli::try_parse_from(argv).map(|c| c.cmd) {
            Ok(crate::args::Cmd::Spl {
                cmd:
                    crate::args::SplCmd::Leq {
                        cmd: LeqCmd::Set(s),
                    },
            }) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn big_numbers_are_three_rows_of_blocks() {
        let r = crate::watch::big_number("94.0");
        assert_eq!(r[0], "█▀█ █ █   █▀█ ");
        assert_eq!(r[1], "▀▀█ ▀▀█   █ █ ");
        assert_eq!(r[2], "▀▀▀   ▀ ▄ ▀▀▀ ");
        let r = crate::watch::big_number("—");
        assert_eq!(r[1], "▀▀▀ ");
    }

    #[test]
    fn set_windows_presets_and_limits() {
        let cur = LeqConfig::default_windows();
        let s = set_args(&[
            "--preset",
            "din15905",
            "--limit",
            "1min=102db",
            "--warn",
            "2db",
        ]);
        let c = apply(&cur, &s).expect("valid");
        assert_eq!(c.windows.len(), 5);
        assert_eq!(c.windows[0].limit.map(|l| l.0), Some(102.0));
        assert_eq!(c.windows[3].limit.map(|l| l.0), Some(99.0));
        assert!(c.windows.iter().all(|w| w.warn_margin == Db(2.0)));
        // New windows; ones kept keep their limits.
        let s = set_args(&["--windows", "5s,c:10s,1min", "--horizon", "2s"]);
        let d = apply(&c, &s).expect("valid");
        let names: Vec<String> = d.windows.iter().map(ac2_scene::leq::window_name).collect();
        assert_eq!(names, ["LAeq 5 s", "LCeq 10 s", "LAeq 1 min"]);
        assert_eq!(d.windows[2].limit.map(|l| l.0), Some(102.0));
        assert_eq!(d.horizon, Seconds(2.0));
        // WHO adds its 15 min window in order of length.
        let s = set_args(&["--preset", "who", "--limit", "60min=none"]);
        let e = apply(&cur, &s).expect("valid");
        assert_eq!(e.windows.len(), 6);
        assert_eq!(e.windows[3].duration, Seconds(900.0));
        // A limit on a window the meter does not have.
        let s = set_args(&["--limit", "15min=100db"]);
        assert!(matches!(apply(&cur, &s), Err(CliError::Usage(m)) if m.contains("no LAeq 15 min")));
    }
}
