//! `spl leq …`: rolling Leq windows of an SPL meter (`docs/design/leq.md`).

use ac2_client::{Client, expect_body};
use ac2_proto::model::{
    LeqConfig, LeqPreset, LeqWindow, MeasConfig, MeasKind, Measurement, PeakLimit, PeakQuantity,
    PositionCorrection, SplConfig, SplLogPage, SplLogRow, SplLogWhich, State, TimeWeighting,
    Weighting,
};
use ac2_proto::units::{Db, Seconds};
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::{Cli, LeqCmd, LeqExport, LeqNew, LeqSet, LeqWatch, MeterRef, PresetArg, SplSet};
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
        PresetArg::Finland545 => LeqPreset::Finland545,
    }
}

/// The measuring-position correction `s` asks for, from the meter's `cur`: `--position`
/// sets both differences (or removes the correction), `--position-peak` then the peaks'.
pub(crate) fn apply_position(
    cur: Option<PositionCorrection>,
    s: &LeqSet,
) -> Result<Option<PositionCorrection>, CliError> {
    let mut p = match s.position {
        Some(a) => a.0.map(|d| PositionCorrection::both(d.0)),
        None => cur,
    };
    if let Some(peak) = s.position_peak {
        match &mut p {
            Some(p) => p.peak = peak.0,
            None => {
                return Err(CliError::Usage(
                    "--position-peak needs a correction of the levels: give --position too".into(),
                ));
            }
        }
    }
    if p.is_some_and(|p| !p.is_valid()) {
        return Err(CliError::Usage(format!(
            "a position correction is at most ±{} dB",
            PositionCorrection::MAX_DB
        )));
    }
    Ok(p)
}

/// The windows `s` asks for, from the meter's `cur`. Presets replace the windows and peak
/// limits with theirs (several: the union, a shared window or quantity at the lower limit);
/// `--windows` with them adds windows to the preset's, without limits; then `--limit`,
/// `--peak-limit`, `--warn`, `--horizon`.
pub(crate) fn apply(cur: &LeqConfig, s: &LeqSet) -> Result<LeqConfig, CliError> {
    let mut cfg = cur.clone();
    let blank = |w: &crate::units::LeqWindowArg| LeqWindow {
        duration: Seconds(f64::from(w.seconds)),
        weighting: w.weighting(),
        limit: None,
        warn_margin: Db(LeqWindow::DEFAULT_WARN_MARGIN_DB),
    };
    if !s.preset.is_empty() {
        let presets: Vec<LeqPreset> = s.preset.iter().map(|p| preset(*p)).collect();
        let mut ws = LeqPreset::windows_of(&presets);
        for w in s.windows.iter().flatten().map(blank) {
            if !ws
                .iter()
                .any(|p| p.duration == w.duration && p.weighting == w.weighting)
            {
                ws.push(w);
            }
        }
        LeqWindow::sort(&mut ws);
        cfg.windows = ws;
        cfg.peaks = LeqPreset::peaks_of(&presets);
    } else if let Some(ws) = &s.windows {
        cfg.windows = ws
            .iter()
            .map(|w| {
                let b = blank(w);
                cur.windows
                    .iter()
                    .find(|o| o.duration == b.duration && o.weighting == b.weighting)
                    .copied()
                    .unwrap_or(b)
            })
            .collect();
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
    for p in &s.peak_limits {
        let margin = cfg
            .peaks
            .get(p.quantity)
            .map_or(Db(LeqWindow::DEFAULT_WARN_MARGIN_DB), |l| l.warn_margin);
        *cfg.peaks.get_mut(p.quantity) = p.limit.map(|limit| PeakLimit {
            limit,
            warn_margin: margin,
        });
    }
    if let Some(m) = s.warn {
        for w in &mut cfg.windows {
            w.warn_margin = m.0;
        }
        for q in PeakQuantity::ALL {
            if let Some(l) = cfg.peaks.get_mut(q) {
                l.warn_margin = m.0;
            }
        }
    }
    if let Some(h) = s.horizon {
        cfg.horizon = Seconds(f64::from(h.seconds));
    }
    cfg.check().map_err(CliError::Usage)?;
    Ok(cfg)
}

/// The windows and peak limits as a table (name, limit, warn margin), the horizon and the
/// measuring-position correction.
fn windows_table(cfg: &LeqConfig, position: Option<PositionCorrection>) -> String {
    let mut t = output::table(&["window", "limit", "warn within"]);
    let db = |v: f64| format!("{} dB", ac2_scene::format::level(v));
    for w in &cfg.windows {
        t.add_row(vec![
            ac2_scene::leq::window_name(w),
            w.limit.map_or_else(|| "none".to_owned(), |l| db(l.0)),
            db(w.warn_margin.0),
        ]);
    }
    for q in PeakQuantity::ALL {
        if let Some(l) = cfg.peaks.get(q) {
            t.add_row(vec![
                format!("{} (highest second of 10 s)", ac2_scene::leq::peak_name(q)),
                db(l.limit.0),
                db(l.warn_margin.0),
            ]);
        }
    }
    let position = match position {
        Some(p) => format!(
            "\nposition: {} (the log keeps what was measured)",
            ac2_scene::leq::position_text(&p)
        ),
        None => "\nposition: as measured (no correction)".to_owned(),
    };
    format!(
        "{t}\nheadroom over the next {}{position}",
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
    let position = apply_position(cfg.position, s)?;
    let r = c
        .call(Command::MeasUpdate {
            meas: m.id,
            config: MeasConfig {
                name: m.config.name.clone(),
                kind: MeasKind::Spl {
                    config: SplConfig {
                        leq: leq.clone(),
                        position,
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
            windows_table(&leq, position)
        );
        for p in &presets {
            text.push('\n');
            text.push_str(p);
        }
        text
    })?;
    Ok(())
}

/// `spl set`: the meter's weightings, changed in place.
pub(crate) async fn set_weightings(
    cli: &Cli,
    s: &SplSet,
    weighting: Option<Weighting>,
    time_weighting: Option<TimeWeighting>,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = meter(&st, &s.meter)?.ok_or_else(|| {
        CliError::Usage("no SPL meter on that input: make one with `ac2 meas new spl`".into())
    })?;
    let cfg = spl_config(m)?;
    let config = SplConfig {
        weighting: weighting.unwrap_or(cfg.weighting),
        time_weighting: time_weighting.unwrap_or(cfg.time_weighting),
        ..cfg.clone()
    };
    let r = c
        .call(Command::MeasUpdate {
            meas: m.id,
            config: MeasConfig {
                name: m.config.name.clone(),
                kind: MeasKind::Spl {
                    config: config.clone(),
                },
            },
        })
        .await?;
    let m = expect_body!("meas.update", r, ReplyBody::Measurement(m) => m)?;
    out.emit(&m, || {
        format!(
            "{}: {}",
            m.config.name,
            ac2_scene::spl::metric_name(config.weighting, config.time_weighting)
        )
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

    fn names(c: &LeqConfig) -> Vec<String> {
        c.windows
            .iter()
            .map(|w| {
                let l = w.limit.map_or_else(String::new, |l| format!(" ≤ {}", l.0));
                format!("{}{l}", ac2_scene::leq::window_name(w))
            })
            .collect()
    }

    #[test]
    fn set_windows_and_limits() {
        let cur = LeqConfig::default_windows();
        let s = set_args(&["--limit", "1min=102db", "--warn", "2db"]);
        let c = apply(&cur, &s).expect("valid");
        assert_eq!(c.windows.len(), 5);
        assert_eq!(c.windows[0].limit.map(|l| l.0), Some(102.0));
        assert!(c.windows.iter().all(|w| w.warn_margin == Db(2.0)));
        // New windows; ones kept keep their limits.
        let s = set_args(&["--windows", "5s,c:10s,1min", "--horizon", "2s"]);
        let d = apply(&c, &s).expect("valid");
        assert_eq!(names(&d), ["LAeq 5 s", "LCeq 10 s", "LAeq 1 min ≤ 102"]);
        assert_eq!(d.horizon, Seconds(2.0));
        // A limit on a window the meter does not have.
        let s = set_args(&["--limit", "15min=100db"]);
        assert!(matches!(apply(&cur, &s), Err(CliError::Usage(m)) if m.contains("no LAeq 15 min")));
    }

    /// A preset replaces the windows with exactly its own; the meter's limits on others go.
    #[test]
    fn a_preset_replaces_the_windows() {
        let mut cur = LeqConfig::default_windows();
        cur.windows[0].limit = Some(ac2_proto::units::DbSpl(110.0));
        let c = apply(&cur, &set_args(&["--preset", "din15905"])).expect("valid");
        assert_eq!(names(&c), ["LAeq 30 min ≤ 99"]);
        assert_eq!(c.peaks.lcpeak.map(|l| l.limit.0), Some(135.0));
        assert_eq!(c.peaks.lafmax, None);
        let c = apply(&cur, &set_args(&["--preset", "swiss100"])).expect("valid");
        assert_eq!(c.peaks.lafmax.map(|l| l.limit.0), Some(125.0));
        assert_eq!(c.peaks.lcpeak, None, "a preset's peaks replace the meter's");
        assert_eq!(c.horizon, cur.horizon);
        let c = apply(&cur, &set_args(&["--preset", "france"])).expect("valid");
        assert_eq!(names(&c), ["LAeq 15 min ≤ 102", "LCeq 15 min ≤ 118"]);
        let c = apply(&cur, &set_args(&["--preset", "flanders-100"])).expect("valid");
        assert_eq!(names(&c), ["LAeq 15 min", "LAeq 60 min ≤ 100"]);
        // Then a limit and the warn margin on its windows.
        let s = set_args(&["--preset", "who", "--limit", "15min=98db", "--warn", "2db"]);
        let c = apply(&cur, &s).expect("valid");
        assert_eq!(names(&c), ["LAeq 15 min ≤ 98"]);
        assert_eq!(c.windows[0].warn_margin, Db(2.0));
        // A window the preset lacks: refused, named.
        let s = set_args(&["--preset", "who", "--limit", "60min=none"]);
        assert!(matches!(apply(&cur, &s), Err(CliError::Usage(m)) if m.contains("no LAeq 60 min")));
    }

    /// Several presets: the windows of all, a shared window at the lower limit; `--windows`
    /// adds windows to them, without limits, shortest first.
    #[test]
    fn presets_together_and_extra_windows() {
        let cur = LeqConfig::default_windows();
        let s = set_args(&[
            "--preset", "france", "--preset", "who", "--preset", "swiss96",
        ]);
        let c = apply(&cur, &s).expect("valid");
        assert_eq!(
            names(&c),
            ["LAeq 15 min ≤ 100", "LCeq 15 min ≤ 118", "LAeq 60 min ≤ 96"]
        );
        let s = set_args(&["--preset", "din15905", "--windows", "60min,1min,30min"]);
        let c = apply(&cur, &s).expect("valid");
        assert_eq!(names(&c), ["LAeq 1 min", "LAeq 30 min ≤ 99", "LAeq 60 min"]);
    }

    /// `--peak-limit` sets and removes a peak limit (its margin kept, `--warn` sets it too);
    /// `--position` and `--position-peak` the correction.
    #[test]
    fn peak_limits_and_the_position() {
        let cur = LeqConfig::default_windows();
        let s = set_args(&[
            "--peak-limit",
            "lcpeak=135db",
            "--peak-limit",
            "lafmax=125db",
        ]);
        let c = apply(&cur, &s).expect("valid");
        assert_eq!(c.peaks.lcpeak.map(|l| l.limit.0), Some(135.0));
        assert_eq!(c.peaks.lafmax.map(|l| l.warn_margin), Some(Db(3.0)));
        let d = apply(
            &c,
            &set_args(&["--peak-limit", "lcpeak=none", "--warn", "2db"]),
        )
        .expect("valid");
        assert_eq!(d.peaks.lcpeak, None);
        assert_eq!(d.peaks.lafmax.map(|l| l.warn_margin), Some(Db(2.0)));
        let table = windows_table(&c, Some(PositionCorrection::both(4.0)));
        assert!(table.contains("LCpeak (highest second of 10 s)"), "{table}");
        assert!(table.contains("position: corrected +4.0 dB"), "{table}");
        let p = apply_position(None, &set_args(&["--position", "4db"])).expect("valid");
        assert_eq!(p, Some(PositionCorrection::both(4.0)));
        let p = apply_position(p, &set_args(&["--position-peak", "2db"])).expect("valid");
        assert_eq!(p.map(|p| (p.level, p.peak)), Some((Db(4.0), Db(2.0))));
        assert_eq!(
            apply_position(p, &set_args(&["--windows", "1min"])).expect("kept"),
            p
        );
        assert_eq!(
            apply_position(p, &set_args(&["--position", "none"])).expect("removed"),
            None
        );
        assert!(apply_position(None, &set_args(&["--position-peak", "2db"])).is_err());
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
            "finland-545",
        ];
        let parsed: Vec<LeqPreset> = names
            .iter()
            .map(|n| preset(set_args(&["--preset", n]).preset[0]))
            .collect();
        assert_eq!(parsed, LeqPreset::ALL);
        assert_eq!(PresetArg::value_variants().len(), LeqPreset::ALL.len());
    }
}
