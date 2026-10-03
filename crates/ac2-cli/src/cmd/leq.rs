//! `spl leq …`: rolling Leq windows of an SPL meter (`docs/design/leq.md`).

use ac2_client::{Client, expect_body};
use ac2_proto::model::{
    LeqConfig, LeqPreset, LeqWindow, MeasConfig, MeasKind, Measurement, SplConfig, SplLogPage,
    SplLogRow, SplLogWhich, State, TimeWeighting, Weighting,
};
use ac2_proto::units::{Db, Seconds};
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::{Cli, LeqCmd, LeqExport, LeqNew, LeqSet, LeqWatch, MeterRef, PresetArg};
use crate::output::{self, Out};
use crate::watch;

pub(crate) async fn run(cli: &Cli, cmd: &LeqCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        LeqCmd::Watch(w) => watch_cmd(cli, w, out).await,
        LeqCmd::Set(s) => set(cli, s, out).await,
        LeqCmd::Export(e) => export(cli, e, out).await,
        LeqCmd::New(n) => new_log(cli, n, out).await,
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
        PresetArg::France => LeqPreset::France,
        PresetArg::FranceChildren => LeqPreset::FranceChildren,
        PresetArg::Flanders85 => LeqPreset::Flanders85,
        PresetArg::Flanders95 => LeqPreset::Flanders95,
        PresetArg::Flanders100 => LeqPreset::Flanders100,
        PresetArg::Brussels85 => LeqPreset::Brussels85,
        PresetArg::Brussels95 => LeqPreset::Brussels95,
        PresetArg::Brussels100 => LeqPreset::Brussels100,
        PresetArg::NlCovenant => LeqPreset::NetherlandsCovenant,
        PresetArg::NlCovenant16To17 => LeqPreset::NetherlandsCovenant16To17,
        PresetArg::NlCovenant14To15 => LeqPreset::NetherlandsCovenant14To15,
        PresetArg::NlCovenantTo13 => LeqPreset::NetherlandsCovenantTo13,
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
        preset(*p)
            .apply(&mut cfg.windows)
            .map_err(CliError::Usage)?;
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
            format!(
                "{} — {}",
                ac2_scene::leq_preset::summary(p),
                ac2_scene::leq_preset::source(p)
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

/// Rows of one of the meter's logs from `from`, at most `max`.
async fn log_page(
    c: &Client,
    meas: ac2_proto::units::MeasId,
    log: SplLogWhich,
    from: u64,
    max: u32,
) -> Result<SplLogPage, CliError> {
    let r = c
        .call(Command::SplLogGet {
            meas,
            log,
            from,
            max,
        })
        .await?;
    Ok(expect_body!("spl.log_get", r, ReplyBody::SplLogPage(p) => p)?)
}

/// Every row of one of the meter's logs, paged.
pub(crate) async fn all_rows(
    c: &Client,
    meas: ac2_proto::units::MeasId,
    log: SplLogWhich,
) -> Result<Vec<SplLogRow>, CliError> {
    let mut rows = Vec::new();
    let mut from = 0;
    loop {
        let r = c
            .call(Command::SplLogGet {
                meas,
                log,
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

/// The CSV of `rows` of meter `m` (the session file's format).
fn log_csv(st: &State, m: &Measurement, rows: &[SplLogRow]) -> Result<String, CliError> {
    let cfg = spl_config(m)?;
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
    Ok(ac2_traces::spl_log::export_csv(&info, rows))
}

/// `spl leq new`: without `--yes`, says what would end and refuses; with it, ends the log
/// and, with `--export`, writes the ended log (read back as the daemon's previous log, so
/// no second logged in between is lost).
async fn new_log(cli: &Cli, n: &LeqNew, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = meter(&st, &n.meter)?
        .ok_or_else(|| CliError::Usage("no SPL meter on that input".into()))?
        .clone();
    spl_config(&m)?;
    let head = log_page(&c, m.id, SplLogWhich::Current, 0, 1).await?;
    let now = crate::watch::now_wall().0;
    let ended = match head.rows.first() {
        Some(r) => format!(
            "{} seconds logged over {} since {}",
            head.total,
            ac2_scene::leq::clock(now.saturating_sub(r.start.0) as f64 / 1e9),
            ac2_traces::spl_log::utc_iso(r.start.0)
        ),
        None => "nothing logged yet".to_owned(),
    };
    if !n.yes {
        return Err(CliError::Usage(format!(
            "this ends {}'s SPL log ({ended}): its Leq windows and their states, the alarms, \
             the run clock and the total start over (windows and limits are kept). Pass --yes \
             to go ahead; --export FILE writes the ended log first",
            m.config.name
        )));
    }
    c.call(Command::SplLogNew { meas: m.id }).await?;
    let file = match &n.export {
        Some(path) => {
            let rows = all_rows(&c, m.id, SplLogWhich::Previous).await?;
            let csv = log_csv(&st, &m, &rows)?;
            std::fs::write(path, csv.as_bytes()).map_err(|e| {
                CliError::Usage(format!(
                    "the new log started, but {} could not be written ({e}); the ended log is \
                     still in the daemon: ac2 spl leq export --previous -o FILE",
                    path.display()
                ))
            })?;
            Some((path.clone(), rows.len()))
        }
        None => None,
    };
    out.emit(
        &json!({
            "meas": m.id.0,
            "ended_rows": head.total,
            "ended_started_at": head.rows.first().map(|r| ac2_traces::spl_log::utc_iso(r.start.0)),
            "file": file.as_ref().map(|(p, _)| p.to_string_lossy()),
            "exported_rows": file.as_ref().map(|(_, k)| *k),
        }),
        || {
            let mut t = format!(
                "{}: new SPL log started; the ended log had {ended}",
                m.config.name
            );
            match &file {
                Some((p, k)) => t.push_str(&format!("; {k} seconds written to {}", p.display())),
                None => t.push_str(
                    "; until the next new log or a daemon restart: ac2 spl leq export --previous",
                ),
            }
            t
        },
    )?;
    Ok(())
}

async fn export(cli: &Cli, e: &LeqExport, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = meter(&st, &e.meter)?
        .ok_or_else(|| CliError::Usage("no SPL meter on that input".into()))?;
    let log = if e.previous {
        SplLogWhich::Previous
    } else {
        SplLogWhich::Current
    };
    let rows = all_rows(&c, m.id, log).await?;
    let csv = log_csv(&st, m, &rows)?;
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
        // France sets two windows, A and C, added in order of length.
        let s = set_args(&["--preset", "france"]);
        let f = apply(&cur, &s).expect("valid");
        let names: Vec<String> = f.windows.iter().map(ac2_scene::leq::window_name).collect();
        assert_eq!(
            names,
            [
                "LAeq 1 min",
                "LAeq 5 min",
                "LAeq 10 min",
                "LAeq 15 min",
                "LCeq 15 min",
                "LAeq 30 min",
                "LAeq 60 min"
            ]
        );
        assert_eq!(f.windows[3].limit.map(|l| l.0), Some(102.0));
        assert_eq!(f.windows[4].limit.map(|l| l.0), Some(118.0));
        // A window the meter lacks and that would pass the most windows: refused, named.
        let s = set_args(&["--preset", "france", "--preset", "brussels-100"]);
        let g = apply(&cur, &s).expect("eight windows");
        assert_eq!(g.windows.len(), LeqConfig::MAX_WINDOWS);
        let s = set_args(&[
            "--preset",
            "who",
            "--windows",
            "5s,10s,30s,1min,5min,10min,30min",
        ]);
        let full = apply(&cur, &s).expect("eight windows");
        let s = set_args(&["--preset", "france"]);
        assert!(
            matches!(apply(&full, &s), Err(CliError::Usage(m)) if m.contains("France R1336-1 needs 1 more window")),
        );
        // A limit on a window the meter does not have.
        let s = set_args(&["--limit", "15min=100db"]);
        assert!(matches!(apply(&cur, &s), Err(CliError::Usage(m)) if m.contains("no LAeq 15 min")));
    }

    /// Every preset has a `--preset` name, in the app's order.
    #[test]
    fn every_preset_parses() {
        use clap::ValueEnum;
        let names = [
            "din15905",
            "swiss93",
            "swiss96",
            "swiss100",
            "who",
            "france",
            "france-children",
            "flanders-85",
            "flanders-95",
            "flanders-100",
            "brussels-85",
            "brussels-95",
            "brussels-100",
            "nl-covenant",
            "nl-covenant-16-17",
            "nl-covenant-14-15",
            "nl-covenant-13",
        ];
        let parsed: Vec<LeqPreset> = names
            .iter()
            .map(|n| preset(set_args(&["--preset", n]).preset[0]))
            .collect();
        assert_eq!(parsed, LeqPreset::ALL);
        assert_eq!(PresetArg::value_variants().len(), LeqPreset::ALL.len());
    }
}
