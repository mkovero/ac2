//! Request/response commands and the live views built on them.

use ac2_client::{Client, expect_body};
use ac2_proto::model::*;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::{connect, find_meas, rate, state};
use crate::CliError;
use crate::args::*;
use crate::output::{self, Out};
use crate::watch;

pub(crate) async fn devices(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let r = c.call(Command::SessionDevices).await?;
    let d = expect_body!("session.devices", r, ReplyBody::Devices(d) => d)?;
    out.emit(&d, || output::devices(&d))?;
    Ok(())
}

fn backend(b: BackendArg) -> BackendKind {
    match b {
        BackendArg::Jack => BackendKind::Jack,
        BackendArg::Cpal => BackendKind::Cpal,
        BackendArg::Fake => BackendKind::Fake,
    }
}

pub(crate) async fn session(
    cli: &Cli,
    cmd: &SessionCmd,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    match cmd {
        SessionCmd::Open(o) => {
            let r = c.call(Command::SessionDevices).await?;
            let devs = expect_body!("session.devices", r, ReplyBody::Devices(d) => d)?;
            let want = backend(o.backend);
            let of_backend: Vec<&DeviceInfo> = devs.iter().filter(|d| d.backend == want).collect();
            let dev = match &o.device {
                Some(sel) => of_backend
                    .iter()
                    .find(|d| &d.id.0 == sel || &d.name == sel)
                    .copied()
                    .ok_or_else(|| {
                        CliError::Usage(format!("no {want:?} device {sel:?} (see `ac2 devices`)"))
                    })?,
                None => match of_backend.as_slice() {
                    [d] => *d,
                    [] => {
                        return Err(CliError::Usage(format!(
                            "the daemon lists no {want:?} device"
                        )));
                    }
                    _ => {
                        return Err(CliError::Usage(format!(
                            "{} {want:?} devices; choose one with --device (see `ac2 devices`)",
                            of_backend.len()
                        )));
                    }
                },
            };
            let rate = match o.rate {
                None => None,
                Some(f) if f.0.0.fract() == 0.0 && f.0.0 <= f64::from(u32::MAX) => {
                    Some(f.0.0 as u32)
                }
                Some(f) => {
                    return Err(CliError::Usage(format!(
                        "sample rate {} Hz is not a whole number",
                        f.0.0
                    )));
                }
            };
            let buffer = match o.buffer {
                None => None,
                Some(n) => Some(
                    u32::try_from(n.0)
                        .map_err(|_| CliError::Usage(format!("buffer of {} samples", n.0)))?,
                ),
            };
            let loopback = match (o.loopback_out, o.loopback_in) {
                (Some(out_ch), Some(in_ch)) => Some(LoopbackRoute {
                    output: out_ch.0,
                    input: in_ch.0,
                }),
                _ => None,
            };
            let sel = DeviceSelector::Id { id: dev.id.clone() };
            let config = SessionConfig {
                input_device: sel.clone(),
                output_device: sel,
                input_channels: o.inputs.0.clone(),
                output_channels: o.outputs,
                sample_rate_hz: rate,
                buffer_frames: buffer,
                loopback,
            };
            let r = c.call(Command::SessionOpen { config }).await?;
            let s = expect_body!("session.open", r, ReplyBody::Session(s) => s)?;
            let inputs = if o.mics.is_empty() {
                None
            } else {
                Some(super::cal::set_inputs(&c, &o.mics, &[]).await?)
            };
            out.emit(&s, || match &inputs {
                None => output::session(&s),
                Some(i) => format!("{}\n{}", output::session(&s), output::inputs(i)),
            })?;
        }
        SessionCmd::Close => {
            let r = c.call(Command::SessionClose).await?;
            let rev = expect_body!("session.close", r, ReplyBody::Ack { rev } => rev)?;
            out.emit(&json!({ "rev": rev }), || "session closed".to_owned())?;
        }
        SessionCmd::Status => {
            let r = c.call(Command::SessionStatus).await?;
            let s = expect_body!("session.status", r, ReplyBody::Session(s) => s)?;
            out.emit(&s, || output::session(&s))?;
        }
        SessionCmd::Save { session } => {
            super::traces::save_or_load(cli, &c, session, false, out).await?;
        }
        SessionCmd::Load { session } => {
            super::traces::save_or_load(cli, &c, session, true, out).await?;
        }
        SessionCmd::List => super::traces::list(&c, out).await?,
        SessionCmd::Inputs(a) => return super::cal::session_inputs(cli, a, out).await,
    }
    Ok(())
}

fn weighting(w: WeightArg) -> Weighting {
    match w {
        WeightArg::A => Weighting::A,
        WeightArg::C => Weighting::C,
        WeightArg::Z => Weighting::Z,
    }
}

fn time_weighting(t: TimeWeightArg) -> TimeWeighting {
    match t {
        TimeWeightArg::Fast => TimeWeighting::Fast,
        TimeWeightArg::Slow => TimeWeighting::Slow,
        TimeWeightArg::Impulse => TimeWeighting::Impulse,
    }
}

fn smoothing(f: FractionArg) -> Result<SmoothingFraction, CliError> {
    Ok(match f {
        FractionArg::F3 => SmoothingFraction::Third,
        FractionArg::F6 => SmoothingFraction::Sixth,
        FractionArg::F12 => SmoothingFraction::Twelfth,
        FractionArg::F24 => SmoothingFraction::TwentyFourth,
        FractionArg::F48 => SmoothingFraction::FortyEighth,
        FractionArg::F1 => return Err(CliError::Usage("smoothing is 1/3 … 1/48 octave".into())),
    })
}

fn band_fraction(f: FractionArg) -> Result<BandFraction, CliError> {
    Ok(match f {
        FractionArg::F1 => BandFraction::Octave,
        FractionArg::F3 => BandFraction::Third,
        FractionArg::F6 => BandFraction::Sixth,
        FractionArg::F12 => BandFraction::Twelfth,
        FractionArg::F24 => BandFraction::TwentyFourth,
        FractionArg::F48 => return Err(CliError::Usage("RTA bands are 1/1 … 1/24 octave".into())),
    })
}

fn window(w: WindowArg) -> Window {
    match w {
        WindowArg::Hann => Window::Hann,
        WindowArg::Bh4 => Window::BlackmanHarris4,
        WindowArg::Flattop => Window::FlatTop,
        WindowArg::Rect => Window::Rectangular,
    }
}

/// Builds the measurement configuration of `meas new`, checking the kind's required inputs.
pub fn meas_config(n: &MeasNew) -> Result<MeasConfig, CliError> {
    let need = |c: Option<crate::units::Channel>, flag: &str| {
        c.map(|c| c.0)
            .ok_or_else(|| CliError::Usage(format!("{:?} needs --{flag}", n.kind)))
    };
    let refuse = |c: Option<crate::units::Channel>, flag: &str| match c {
        Some(_) => Err(CliError::Usage(format!(
            "--{flag} does not apply to {:?}",
            n.kind
        ))),
        None => Ok(()),
    };
    let kind = match n.kind {
        MeasKindArg::Tf => {
            refuse(n.input, "input")?;
            if n.ppo == 0 || n.ppo > 96 {
                return Err(CliError::Usage("--ppo must be 1 … 96".into()));
            }
            if n.blocks == 0 {
                return Err(CliError::Usage("--blocks must be at least 1".into()));
            }
            MeasKind::Transfer {
                config: TransferConfig {
                    reference_input: need(n.reference, "ref")?,
                    measurement_input: need(n.measurement, "meas")?,
                    averaging: TfAveraging::Fifo { blocks: n.blocks },
                    grid: LogGridSpec::ten_octaves(n.ppo),
                    smoothing: n
                        .smooth
                        .map(|f| {
                            smoothing(f).map(|fraction| Smoothing {
                                fraction,
                                mode: SmoothingMode::Power,
                            })
                        })
                        .transpose()?,
                    depth: match n.fast_lf {
                        None => DepthPolicy::EqualConfidence,
                        Some(t) if t.0.0 > 0.0 => DepthPolicy::FastLf { max_settle_s: t.0 },
                        Some(_) => {
                            return Err(CliError::Usage("--fast-lf must be longer than 0".into()));
                        }
                    },
                },
            }
        }
        k => {
            refuse(n.reference, "ref")?;
            refuse(n.measurement, "meas")?;
            let input = need(n.input, "input")?;
            match k {
                MeasKindArg::Spectrum => {
                    let len = n.fft.0;
                    if !(64..=1 << 20).contains(&len) || (len & (len - 1)) != 0 {
                        return Err(CliError::Usage(
                            "--fft must be a power of two, 64 … 1048576 samples".into(),
                        ));
                    }
                    MeasKind::Spectrum {
                        config: SpectrumConfig {
                            fft_len: len as u32,
                            window: window(n.window),
                            ..SpectrumConfig::on_input(input)
                        },
                    }
                }
                MeasKindArg::Rta => {
                    if n.from.0.0 >= n.to.0.0 {
                        return Err(CliError::Usage("--from must be below --to".into()));
                    }
                    MeasKind::Rta {
                        config: RtaConfig {
                            f_lo: n.from.0,
                            f_hi: n.to.0,
                            weighting: weighting(n.weight),
                            ..RtaConfig::on_input(input, band_fraction(n.fraction)?)
                        },
                    }
                }
                _ => MeasKind::Spl {
                    config: SplConfig::on_input(input, weighting(n.weight), time_weighting(n.time)),
                },
            }
        }
    };
    if n.name.trim().is_empty() {
        return Err(CliError::Usage("--name must not be empty".into()));
    }
    Ok(MeasConfig {
        name: n.name.clone(),
        kind,
    })
}

async fn meas_call(c: &Client, cmd: Command) -> Result<Measurement, CliError> {
    let op = cmd.name();
    let r = c.call(cmd).await?;
    Ok(expect_body!(op, r, ReplyBody::Measurement(m) => m)?)
}

pub(crate) async fn meas(cli: &Cli, cmd: &MeasCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    if let MeasCmd::List { watch: true } = cmd {
        let c = connect(cli, true).await?;
        return watch::meas_list(&c, out).await;
    }
    let c = connect(cli, false).await?;
    match cmd {
        MeasCmd::New(n) => {
            let config = meas_config(n)?;
            let mut m = meas_call(&c, Command::MeasCreate { config }).await?;
            if n.start {
                m = meas_call(&c, Command::MeasStart { meas: m.id }).await?;
            }
            out.emit(&m, || output::measurement(&m))?;
        }
        MeasCmd::List { .. } => {
            let s = state(&c).await?;
            out.emit(&s.measurements, || output::measurements(&s.measurements))?;
        }
        MeasCmd::Start { meas } | MeasCmd::Stop { meas } => {
            let s = state(&c).await?;
            let id = find_meas(&s, meas)?.id;
            let cmd = if matches!(cmd, MeasCmd::Start { .. }) {
                Command::MeasStart { meas: id }
            } else {
                Command::MeasStop { meas: id }
            };
            let m = meas_call(&c, cmd).await?;
            out.emit(&m, || output::measurement(&m))?;
        }
        MeasCmd::Rm { meas } => {
            let snap = c.snapshot().await?;
            let m = find_meas(&snap.state, meas)?;
            // Guarded by the rev the name was resolved at: if the measurements changed in
            // between, the daemon refuses instead of deleting the wrong one.
            let r = c
                .call_expect(Command::MeasDelete { meas: m.id }, snap.rev)
                .await?;
            let rev = expect_body!("meas.delete", r, ReplyBody::Ack { rev } => rev)?;
            let name = m.config.name.clone();
            out.emit(&json!({ "deleted": m.id, "rev": rev }), || {
                format!("deleted measurement {} {name}", m.id)
            })?;
        }
    }
    Ok(())
}

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
                output::ms(first.delay.0),
                smp(first),
                output::ms(strongest.delay.0),
                smp(strongest)
            ));
        }
        DelayOutcome::Ambiguous {
            ranked, strongest, ..
        } => {
            out.push_str(&format!(
                "strongest      {}{}\npick one: ac2 delay insert <meas> --pick 1|2|3 (1 = the rule's pick)\n",
                output::ms(strongest.delay.0),
                smp(strongest)
            ));
            let mut t = output::table(&["#", "delay", "level", "phase", "σ"]);
            for (i, a) in ranked.iter().enumerate() {
                t.add_row(vec![
                    (i + 1).to_string(),
                    output::ms(a.delay.0),
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
                output::ms(a.delay.0),
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

pub(crate) async fn spl(cli: &Cli, cmd: &SplCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        SplCmd::Cal(a) => super::cal::cal_spl(cli, a, out).await,
        SplCmd::Watch(w) => {
            let c = connect(cli, true).await?;
            let s = state(&c).await?;
            let (id, created) = match (&w.meas, w.input) {
                (Some(r), _) => {
                    let m = find_meas(&s, r)?;
                    if !matches!(m.config.kind, MeasKind::Spl { .. }) {
                        return Err(CliError::Usage(format!(
                            "{} is not an SPL measurement",
                            m.config.name
                        )));
                    }
                    (m.id, false)
                }
                (None, Some(input)) => {
                    let want = SplConfig {
                        input: input.0,
                        weighting: weighting(w.weight),
                        time_weighting: time_weighting(w.time),
                        peak_weighting: PeakWeighting::C,
                    };
                    let existing = s.measurements.iter().find(
                        |m| matches!(&m.config.kind, MeasKind::Spl { config } if *config == want),
                    );
                    match existing {
                        Some(m) => (m.id, false),
                        None => {
                            let m = meas_call(
                                &c,
                                Command::MeasCreate {
                                    config: MeasConfig {
                                        name: format!("spl-in{input}"),
                                        kind: MeasKind::Spl { config: want },
                                    },
                                },
                            )
                            .await?;
                            (m.id, true)
                        }
                    }
                }
                (None, None) => return Err(CliError::Usage("give --meas or --input".into())),
            };
            let running = s
                .measurements
                .iter()
                .find(|m| m.id == id)
                .is_some_and(|m| m.running);
            if !running {
                meas_call(&c, Command::MeasStart { meas: id }).await?;
            }
            let result = watch::spl(&c, id, out).await;
            if created {
                let _ = c.call(Command::MeasDelete { meas: id }).await;
            } else if !running {
                let _ = c.call(Command::MeasStop { meas: id }).await;
            }
            result
        }
    }
}

pub(crate) async fn timing(cli: &Cli, live: bool, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, live).await?;
    if live {
        return watch::timing(&c, out).await;
    }
    let s = state(&c).await?;
    out.emit(&s.timing, || output::timing(&s.timing, rate(&s)))?;
    Ok(())
}

async fn dump(c: &Client, out: &mut Out<'_>) -> Result<(), CliError> {
    let snap = c.snapshot().await?;
    // A dump is JSON in both modes.
    let s = serde_json::to_string_pretty(&snap).map_err(std::io::Error::other)?;
    writeln!(out.w, "{s}")?;
    Ok(())
}

pub(crate) async fn state_dump(
    cli: &Cli,
    cmd: &StateCmd,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    match cmd {
        StateCmd::Dump => {
            let c = connect(cli, false).await?;
            dump(&c, out).await
        }
    }
}
