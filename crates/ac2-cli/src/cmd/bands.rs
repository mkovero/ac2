//! `spl bands …`: the band meter of an SPL meter (`docs/design/band-leq.md`).

use ac2_client::expect_body;
use ac2_proto::model::{
    BAND_NOMINAL_HZ, BandCorrection, BandLeqConfig, BandLeqPreset, BandLevelSource, BandMicPlace,
    BandTransferBand, BandTransferSet, ImpulseCorrection, MeasConfig, MeasKind, Measurement,
    SplBandLog, SplConfig, State, TonalCorrection, TransferOrigin,
};
use ac2_proto::units::{Db, DbSpl, Seconds, WallNs};
use ac2_proto::{Command, ReplyBody};
use ac2_scene::band_leq;

use super::leq::{meter, spl_config};
use super::{connect, find_meas, state};
use crate::CliError;
use crate::args::{
    BandMicArg, BandPresetArg, BandsCmd, BandsEstimate, BandsLog, BandsSet, BandsTransfer, Cli,
    ImpulseArg, LeqWatch, MeasRef, TonalArg,
};
use crate::output::Out;
use crate::units::BandSourceArg;
use crate::watch;

pub(crate) async fn run(cli: &Cli, cmd: &BandsCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        BandsCmd::Watch(w) => watch_cmd(cli, w, out).await,
        BandsCmd::Set(s) => set(cli, s, out).await,
        BandsCmd::Transfer(t) => transfer(cli, t, out).await,
        BandsCmd::Log(l) => log(cli, l, out).await,
        BandsCmd::Estimate(e) => estimate(cli, e, out).await,
    }
}

pub(crate) fn preset(p: BandPresetArg) -> BandLeqPreset {
    match p {
        BandPresetArg::Finland545Lf => BandLeqPreset::Finland545Lf,
        BandPresetArg::Finland545LivingRoom => BandLeqPreset::Finland545LivingRoom,
    }
}

/// The meter `r` names, with a band meter or not.
fn the_meter<'s>(st: &'s State, r: &crate::args::MeterRef) -> Result<&'s Measurement, CliError> {
    meter(st, r)?.ok_or_else(|| {
        CliError::Usage("no SPL meter on that input: make one with `ac2 meas new spl`".into())
    })
}

/// The band meter configuration `s` asks for, from the meter's `cur` (`None`: off). A
/// preset keeps a measured transfer; turning on needs a preset.
pub(crate) fn apply(
    cur: Option<&BandLeqConfig>,
    s: &BandsSet,
) -> Result<Option<BandLeqConfig>, CliError> {
    if s.off {
        return Ok(None);
    }
    let mut c = match (s.preset, cur) {
        (Some(p), cur) => preset(p).config(cur.and_then(|c| c.transfer)),
        (None, Some(c)) => c.clone(),
        (None, None) => {
            return Err(CliError::Usage(
                "the band meter is off: turn it on with --preset finland-545-lf (or \
                 finland-545-living-room)"
                    .into(),
            ));
        }
    };
    if let Some(d) = s.duration {
        if d.weighting.is_some() {
            return Err(CliError::Usage(
                "the bands are unweighted: give the length alone, e.g. --duration 1h".into(),
            ));
        }
        c.duration = Seconds(f64::from(d.seconds));
    }
    // A preset sets no correction: one typed now stays, else the meter's carries on.
    let keep = cur.map(|c| c.correction).unwrap_or_default();
    c.correction = BandCorrection {
        impulse: match s.impulse {
            Some(ImpulseArg::None) => ImpulseCorrection::None,
            Some(ImpulseArg::Plus5) => ImpulseCorrection::Plus5,
            Some(ImpulseArg::Plus10) => ImpulseCorrection::Plus10,
            None => keep.impulse,
        },
        tonal: match s.tonal {
            Some(TonalArg::None) => TonalCorrection::None,
            Some(TonalArg::Plus3) => TonalCorrection::Plus3,
            Some(TonalArg::Plus6) => TonalCorrection::Plus6,
            None => keep.tonal,
        },
    };
    if let Some(w) = s.warn {
        c.warn_margin = Db(w.0.0);
    }
    // A preset puts the mic at FOH: the place the meter had stays unless given.
    c.mic = match s.mic {
        Some(BandMicArg::Foh) => BandMicPlace::Foh,
        Some(BandMicArg::Bedroom) => BandMicPlace::Dwelling,
        None => cur.map_or(BandMicPlace::Foh, |c| c.mic),
    };
    c.check().map_err(CliError::Usage)?;
    Ok(Some(c))
}

/// The band meter in words: preset-like summary of its limits, window, correction, transfer.
fn describe(c: Option<&BandLeqConfig>) -> String {
    let Some(c) = c else {
        return "band meter off".to_owned();
    };
    let preset = BandLeqPreset::ALL.into_iter().find(|p| {
        let pc = p.config(c.transfer);
        pc.day == c.day && pc.night == c.night && pc.predicted == c.predicted
    });
    let mut lines = vec![format!(
        "{} · warn {} dB · §13 correction {} dB · mic {}",
        band_leq::meter_name(c.duration.0),
        c.warn_margin.0,
        c.correction.db(),
        band_leq::mic_place_text(c.mic)
    )];
    if let Some(p) = preset {
        lines.push(band_leq::preset_summary(p));
        lines.push(band_leq::preset_source(p));
    }
    lines.push(match c.mic {
        BandMicPlace::Foh => band_leq::transfer_summary(c.transfer.as_ref()),
        BandMicPlace::Dwelling => band_leq::mic_place_note(c.mic).to_owned(),
    });
    lines.join("\n")
}

async fn set(cli: &Cli, s: &BandsSet, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = the_meter(&st, &s.meter)?;
    let cfg = spl_config(m)?;
    let bands = apply(cfg.bands.as_deref(), s)?;
    let r = c
        .call(Command::MeasUpdate {
            meas: m.id,
            config: MeasConfig {
                name: m.config.name.clone(),
                kind: MeasKind::Spl {
                    config: SplConfig {
                        bands: bands.clone().map(Box::new),
                        ..cfg.clone()
                    },
                },
            },
        })
        .await?;
    let m = expect_body!("meas.update", r, ReplyBody::Measurement(m) => m)?;
    out.emit(&m, || {
        format!("{}: {}", m.config.name, describe(bands.as_ref()))
    })?;
    Ok(())
}

async fn watch_cmd(cli: &Cli, w: &LeqWatch, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, true).await?;
    let s = state(&c).await?;
    let m = the_meter(&s, &w.meter)?;
    if spl_config(m)?.bands.is_none() {
        return Err(CliError::Usage(format!(
            "{} has no band meter: turn it on with `ac2 spl bands set --preset finland-545-lf`",
            m.config.name
        )));
    }
    if !m.running {
        c.call(Command::MeasStart { meas: m.id }).await?;
    }
    let until = w
        .duration
        .map(|t| std::time::Instant::now() + std::time::Duration::from_secs_f64(t.0.0.max(0.0)));
    watch::bands(&c, m.id, until, out).await
}

/// The protocol source for `a`: a file read and parsed here, a span resolved against this
/// host's clock and the daemon's meters.
pub(crate) fn source(
    st: &State,
    a: &BandSourceArg,
    now_ns: u64,
) -> Result<BandLevelSource, CliError> {
    match a {
        BandSourceArg::File(p) => levels_file(p),
        BandSourceArg::Span { meter, from, until } => {
            let m = find_meas(st, &MeasRef(meter.clone()))?;
            let bad = |e: crate::units::UnitError| CliError::Usage(e.0);
            let (from, until) = (
                from.resolve(now_ns).map_err(bad)?,
                until.resolve(now_ns).map_err(bad)?,
            );
            if until <= from {
                return Err(CliError::Usage(format!(
                    "{meter}: the span ends before it starts"
                )));
            }
            Ok(BandLevelSource::Log {
                meas: m.id,
                from: WallNs(from),
                until: WallNs(until),
            })
        }
    }
}

/// Typed levels from a file of `<Hz> <dB>` lines.
fn levels_file(p: &std::path::Path) -> Result<BandLevelSource, CliError> {
    let text =
        std::fs::read_to_string(p).map_err(|e| CliError::Usage(format!("{}: {e}", p.display())))?;
    let levels = ac2_traces::band_levels::parse(&text)
        .map_err(|e| CliError::Usage(format!("{}: {e}", p.display())))?;
    Ok(BandLevelSource::Levels {
        levels: levels.iter().map(|l| l.map(DbSpl)).collect(),
    })
}

async fn transfer(cli: &Cli, t: &BandsTransfer, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = the_meter(&st, &t.meter)?;
    if spl_config(m)?.bands.is_none() {
        return Err(CliError::Usage(format!(
            "{} has no band meter: turn it on first (`ac2 spl bands set --preset finland-545-lf`)",
            m.config.name
        )));
    }
    let now = watch::now_wall().0;
    let foh = source(&st, &t.foh, now)?;
    let dwelling = source(&st, &t.dwelling, now)?;
    let background = t
        .background
        .as_ref()
        .map(|b| source(&st, b, now))
        .transpose()?;
    let r = c
        .call(Command::SplBandTransfer {
            meas: m.id,
            foh,
            dwelling,
            background,
        })
        .await?;
    let m = expect_body!("spl.band_transfer", r, ReplyBody::Measurement(m) => m)?;
    let set = match &m.config.kind {
        MeasKind::Spl { config } => config.bands.as_ref().and_then(|b| b.transfer),
        _ => None,
    };
    out.emit(&m, || {
        let mut text = format!(
            "{}: {}",
            m.config.name,
            band_leq::transfer_summary(set.as_ref())
        );
        if let Some(set) = &set {
            for (i, b) in set.bands[..ac2_proto::model::LF_BAND_COUNT]
                .iter()
                .enumerate()
            {
                text.push_str("\n  ");
                text.push_str(&band_leq::transfer_band_text(BAND_NOMINAL_HZ[i], b));
            }
        }
        text
    })?;
    Ok(())
}

/// The local UTC offset (s) in force at `t`.
fn offset_s(t: WallNs) -> i32 {
    use chrono::{Local, Offset, TimeZone};
    Local
        .timestamp_nanos(i64::try_from(t.0).unwrap_or(i64::MAX))
        .offset()
        .fix()
        .local_minus_utc()
}

/// The text of a band log span: the meter and span, the coverage, each band's average,
/// then the rows asked for.
pub(crate) fn log_text(name: &str, l: &SplBandLog, offset_s: impl Fn(WallNs) -> i32) -> String {
    use ac2_scene::band_transfer as bt;
    let mut t = format!(
        "{name} band log {}: {}",
        bt::span_time_text(
            bt::SpanState::Marked {
                from: l.from,
                until: l.until
            },
            l.until,
            offset_s(l.from)
        ),
        bt::coverage_text(&l.average, l.from, l.until)
    );
    if let Some(levels) = &l.average.levels {
        t.push_str("\n  average, dB SPL:");
        for (hz, v) in BAND_NOMINAL_HZ.iter().zip(levels) {
            t.push_str(&format!(
                "\n  {:>7} Hz  {}",
                band_leq::band_label(*hz),
                v.map_or_else(|| "—".into(), |v| format!("{:.1}", v.0))
            ));
        }
    }
    for r in &l.rows {
        let unit = if r.sensitivity.is_some() {
            "dB SPL"
        } else {
            "dBFS"
        };
        let levels: Vec<String> = r
            .levels
            .iter()
            .map(|v| v.map_or_else(|| "—".into(), |v| format!("{v:.1}")))
            .collect();
        t.push_str(&format!(
            "\n  {} {:.2} s {unit}: {}",
            bt::time_of_day(r.start, offset_s(r.start)),
            r.measured.0,
            levels.join(" ")
        ));
    }
    t
}

async fn log(cli: &Cli, a: &BandsLog, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = the_meter(&st, &a.meter)?;
    let now = watch::now_wall().0;
    let bad = |e: crate::units::UnitError| CliError::Usage(e.0);
    let (from, until) = (
        a.from.resolve(now).map_err(bad)?,
        a.until.resolve(now).map_err(bad)?,
    );
    if until <= from {
        return Err(CliError::Usage("the span ends before it starts".into()));
    }
    let r = c
        .call(Command::SplBandLogGet {
            meas: m.id,
            from: WallNs(from),
            until: WallNs(until),
            step: a.step,
        })
        .await?;
    let l = expect_body!("spl.band_log_get", r, ReplyBody::SplBandLog(l) => l)?;
    if let Some(p) = &a.levels_out {
        let text = levels_file_text(&m.config.name, &l, offset_s)?;
        std::fs::write(p, text).map_err(CliError::Io)?;
    }
    out.emit(&l, || log_text(&m.config.name, &l, offset_s))?;
    Ok(())
}

/// A span's averages as a `<Hz> <dB>` file, under a comment naming the meter and span; an
/// error when the span has no dB SPL average.
pub(crate) fn levels_file_text(
    name: &str,
    l: &SplBandLog,
    offset_s: impl Fn(WallNs) -> i32,
) -> Result<String, CliError> {
    use ac2_scene::band_transfer as bt;
    let span = bt::span_time_text(
        bt::SpanState::Marked {
            from: l.from,
            until: l.until,
        },
        l.until,
        offset_s(l.from),
    );
    let Some(levels) = &l.average.levels else {
        return Err(CliError::Usage(format!(
            "{name} {span}: no dB SPL average to write ({})",
            bt::coverage_text(&l.average, l.from, l.until)
        )));
    };
    let levels = levels.map(|v| v.map(|v| v.0));
    Ok(ac2_traces::band_levels::format(
        &format!(
            "{name} band log {span}, {} UTC: energy average per band, dB SPL",
            ac2_traces::spl_log::utc_iso(l.from.0)
        ),
        &levels,
    ))
}

/// `spl bands estimate`: the operator's attenuation per band as an estimated transfer.
async fn estimate(cli: &Cli, e: &BandsEstimate, out: &mut Out<'_>) -> Result<(), CliError> {
    let text = std::fs::read_to_string(&e.attenuation)
        .map_err(|err| CliError::Usage(format!("{}: {err}", e.attenuation.display())))?;
    let typed = ac2_traces::band_levels::parse(&text)
        .map_err(|err| CliError::Usage(format!("{}: {err}", e.attenuation.display())))?;
    let set = estimated_set(&typed, watch::now_wall())
        .map_err(|err| CliError::Usage(format!("{}: {err}", e.attenuation.display())))?;
    let c = connect(cli, false).await?;
    let st = state(&c).await?;
    let m = the_meter(&st, &e.meter)?;
    let cfg = spl_config(m)?;
    let Some(bands) = cfg.bands.as_deref() else {
        return Err(CliError::Usage(format!(
            "{} has no band meter: turn it on first (`ac2 spl bands set --preset finland-545-lf`)",
            m.config.name
        )));
    };
    let bands = BandLeqConfig {
        transfer: Some(set),
        ..bands.clone()
    };
    let r = c
        .call(Command::MeasUpdate {
            meas: m.id,
            config: MeasConfig {
                name: m.config.name.clone(),
                kind: MeasKind::Spl {
                    config: SplConfig {
                        bands: Some(Box::new(bands.clone())),
                        ..cfg.clone()
                    },
                },
            },
        })
        .await?;
    let m = expect_body!("meas.update", r, ReplyBody::Measurement(m) => m)?;
    out.emit(&m, || {
        let mut text = format!(
            "{}: {}",
            m.config.name,
            band_leq::transfer_summary(Some(&set))
        );
        for (i, b) in set.bands[..ac2_proto::model::LF_BAND_COUNT]
            .iter()
            .enumerate()
        {
            text.push_str("\n  ");
            text.push_str(&band_leq::transfer_band_text(BAND_NOMINAL_HZ[i], b));
        }
        text
    })?;
    Ok(())
}

/// Typed attenuations as an estimated transfer: each band unchecked at its attenuation, a
/// band without one missing (no limit at FOH).
pub(crate) fn estimated_set(
    typed: &[Option<f64>; ac2_proto::model::BAND_COUNT],
    now: WallNs,
) -> Result<BandTransferSet, String> {
    if let Some((i, a)) = typed
        .iter()
        .enumerate()
        .find_map(|(i, a)| a.filter(|a| *a < 0.0).map(|a| (i, a)))
    {
        return Err(format!(
            "{} Hz: attenuation {a} dB; the bedroom is not louder than FOH (≥ 0 dB)",
            band_leq::band_label(BAND_NOMINAL_HZ[i])
        ));
    }
    Ok(BandTransferSet {
        measured_at: now,
        origin: TransferOrigin::Estimated,
        bands: typed.map(|a| match a {
            Some(a) => BandTransferBand::Unchecked { attenuation: Db(a) },
            None => BandTransferBand::Missing,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::TimeRef;
    use clap::Parser;

    fn parse(args: &[&str]) -> BandsCmd {
        let argv: Vec<&str> = ["ac2", "spl", "bands"]
            .into_iter()
            .chain(args.iter().copied())
            .collect();
        match crate::Cli::try_parse_from(argv).map(|c| c.cmd) {
            Ok(crate::args::Cmd::Spl {
                cmd: crate::args::SplCmd::Bands { cmd },
            }) => cmd,
            other => panic!("{other:?}"),
        }
    }

    fn set_args(args: &[&str]) -> BandsSet {
        let mut a = vec!["set"];
        a.extend_from_slice(args);
        match parse(&a) {
            BandsCmd::Set(s) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_preset_turns_the_meter_on_and_keeps_a_transfer() {
        let c = apply(None, &set_args(&["--preset", "finland-545-lf"]))
            .expect("test value")
            .expect("test value");
        assert_eq!(c, BandLeqPreset::Finland545Lf.config(None));
        let measured = ac2_proto::samples::band_leq_config();
        let c = apply(
            Some(&measured),
            &set_args(&[
                "--preset",
                "finland-545-living-room",
                "--duration",
                "15min",
                "--impulse",
                "10",
                "--tonal",
                "3",
            ]),
        )
        .expect("test value")
        .expect("test value");
        assert_eq!(c.transfer, measured.transfer);
        assert_eq!(c.predicted.night, Some(DbSpl(30.0)));
        assert_eq!(c.duration, Seconds(900.0));
        assert_eq!(c.correction.db(), 13.0);
        // The correction alone: the rest carries on.
        let d = apply(Some(&c), &set_args(&["--impulse", "none"]))
            .expect("test value")
            .expect("test value");
        assert_eq!(d.correction.db(), 3.0);
        assert_eq!(d.duration, Seconds(900.0));
        assert_eq!(
            apply(Some(&c), &set_args(&["--off"])).expect("test value"),
            None
        );
    }

    #[test]
    fn set_refuses_what_cannot_run() {
        let e = apply(None, &set_args(&["--impulse", "5"])).expect_err("refused");
        assert!(e.to_string().contains("--preset finland-545-lf"), "{e}");
        let e = apply(
            None,
            &set_args(&["--preset", "finland-545-lf", "--duration", "c:1h"]),
        )
        .expect_err("refused");
        assert!(e.to_string().contains("unweighted"), "{e}");
        let argv = ["ac2", "spl", "bands", "set", "--off", "--impulse", "5"];
        assert!(crate::Cli::try_parse_from(argv).is_err());
        assert!(crate::Cli::try_parse_from(["ac2", "spl", "bands", "set"]).is_err());
        assert!(
            crate::Cli::try_parse_from(["ac2", "spl", "bands", "set", "--impulse", "7"]).is_err()
        );
    }

    #[test]
    fn transfer_sources_parse() {
        let BandsCmd::Transfer(t) = parse(&[
            "transfer",
            "--foh",
            "FOH SPL@21:00..21:00:30",
            "--dwelling",
            "bedroom.txt",
            "--background",
            "Bedroom@-2min..now",
        ]) else {
            panic!("transfer");
        };
        assert_eq!(
            t.foh,
            BandSourceArg::Span {
                meter: "FOH SPL".into(),
                from: TimeRef::Local {
                    date: None,
                    seconds: 21 * 3600
                },
                until: TimeRef::Local {
                    date: None,
                    seconds: 21 * 3600 + 30
                },
            }
        );
        assert_eq!(t.dwelling, BandSourceArg::File("bedroom.txt".into()));
        assert_eq!(
            t.background,
            Some(BandSourceArg::Span {
                meter: "Bedroom".into(),
                from: TimeRef::Ago(120.0),
                until: TimeRef::Now,
            })
        );
        let utc: TimeRef = "2026-10-08T21:00:30Z".parse().expect("test value");
        assert_eq!(
            utc.resolve(0).expect("test value"),
            1_791_493_230 * 1_000_000_000
        );
        assert_eq!(
            TimeRef::Ago(30.0)
                .resolve(100_000_000_000)
                .expect("test value"),
            70_000_000_000
        );
        for bad in ["25:00", "2026-13-01T00:00", "-5", "21:00Z", "1"] {
            assert!(bad.parse::<TimeRef>().is_err(), "{bad}");
        }
        assert!("x@1..2".parse::<BandSourceArg>().is_err());
        assert!("@now..now".parse::<BandSourceArg>().is_err());
    }

    #[test]
    fn log_parses_and_reads() {
        let BandsCmd::Log(l) = parse(&["log", "--meas", "FOH SPL", "--from", "21:00"]) else {
            panic!("log");
        };
        assert_eq!(l.until, TimeRef::Now);
        assert_eq!(l.step, None);
        assert_eq!(
            l.from,
            TimeRef::Local {
                date: None,
                seconds: 21 * 3600
            }
        );
        let BandsCmd::Log(l) =
            parse(&["log", "--from", "-2min", "--until", "-1min", "--step", "10"])
        else {
            panic!("log");
        };
        assert_eq!(
            (l.from, l.until, l.step),
            (TimeRef::Ago(120.0), TimeRef::Ago(60.0), Some(10))
        );
        let argv = ["ac2", "spl", "bands", "log"];
        assert!(
            crate::Cli::try_parse_from(argv).is_err(),
            "--from is needed"
        );

        let mut l = ac2_proto::samples::replies()
            .into_iter()
            .find_map(|r| match r {
                Ok(ReplyBody::SplBandLog(l)) => Some(*l),
                _ => None,
            })
            .expect("a sample");
        l.until = WallNs(l.from.0 + 30_000_000_000);
        let t = log_text("FOH SPL", &l, |_| 3 * 3600);
        let mut lines = t.lines();
        assert_eq!(
            lines.next(),
            Some("FOH SPL band log 22:20:00–22:20:30 · 0:30: 30 of 30 s logged")
        );
        assert_eq!(lines.next(), Some("  average, dB SPL:"));
        assert_eq!(lines.next(), Some("       20 Hz  70.8"));
        assert!(t.contains("\n       63 Hz  80.0\n"), "{t}");
        assert!(t.contains("\n    10000 Hz  —\n"), "{t}");
        assert!(
            t.ends_with("dB SPL: 71.2 — — — — 80.5 — — — — — — — — — — — — — — — — — — — — — —"),
            "{t}"
        );
    }

    #[test]
    fn a_span_writes_a_levels_file_and_a_guess_is_an_estimate() {
        let mut l = ac2_proto::samples::replies()
            .into_iter()
            .find_map(|r| match r {
                Ok(ReplyBody::SplBandLog(l)) => Some(*l),
                _ => None,
            })
            .expect("a sample");
        let text = levels_file_text("Bedroom", &l, |_| 3 * 3600).expect("levels");
        assert!(
            text.starts_with("# Bedroom band log 22:20:00–22:20:30 · 0:30, "),
            "{text}"
        );
        let back = ac2_traces::band_levels::parse(&text).expect("parse");
        assert_eq!(back[0], Some(70.75));
        assert_eq!(back[5], Some(80.0));
        assert_eq!(back[1], None);
        l.average.levels = None;
        l.average.uncalibrated = 30;
        let e = levels_file_text("Bedroom", &l, |_| 0).expect_err("uncalibrated");
        assert!(e.to_string().contains("no dB SPL average to write"), "{e}");

        let BandsCmd::Estimate(e) = parse(&["estimate", "--attenuation", "guess.txt"]) else {
            panic!("estimate");
        };
        assert_eq!(e.attenuation, std::path::PathBuf::from("guess.txt"));
        let mut typed = [None; ac2_proto::model::BAND_COUNT];
        typed[5] = Some(25.0);
        let set = estimated_set(&typed, WallNs(1)).expect("set");
        assert_eq!(set.origin, TransferOrigin::Estimated);
        assert_eq!(
            set.bands[5],
            BandTransferBand::Unchecked {
                attenuation: Db(25.0)
            }
        );
        assert_eq!(set.bands[0], BandTransferBand::Missing);
        typed[6] = Some(-3.0);
        let e = estimated_set(&typed, WallNs(1)).expect_err("negative");
        assert!(e.starts_with("80 Hz: attenuation -3 dB"), "{e}");
    }

    #[test]
    fn a_levels_file_becomes_typed_levels() {
        let dir = tempfile::tempdir().expect("test value");
        let p = dir.path().join("dwelling.txt");
        std::fs::write(&p, "# bedroom\n63 41.5\n100 38\n").expect("test value");
        let BandLevelSource::Levels { levels } = levels_file(&p).expect("test value") else {
            panic!("levels");
        };
        assert_eq!(levels.len(), ac2_proto::model::BAND_COUNT);
        assert_eq!(levels[5], Some(DbSpl(41.5)));
        assert_eq!(levels[7], Some(DbSpl(38.0)));
        assert_eq!(levels[0], None);
        std::fs::write(&p, "63 loud\n").expect("test value");
        assert!(levels_file(&p).is_err());
    }
}
