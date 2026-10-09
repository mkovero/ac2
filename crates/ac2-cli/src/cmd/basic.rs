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
    let d = expect_body!("session.devices", r, ReplyBody::Backends(d) => d)?;
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
            let backends = expect_body!("session.devices", r, ReplyBody::Backends(d) => d)?;
            let want = backend(o.backend);
            let Some(b) = backends.iter().find(|b| b.kind == want) else {
                return Err(CliError::Usage(format!(
                    "the daemon offers no {want:?} backend (see `ac2 devices`)"
                )));
            };
            if let Availability::Unavailable { reason } = &b.availability {
                return Err(CliError::Usage(format!(
                    "{want:?} is unavailable: {reason}"
                )));
            }
            let of_backend: Vec<&DeviceInfo> = b.devices.iter().collect();
            let named = |sel: &String| {
                of_backend
                    .iter()
                    .find(|d| &d.id.0 == sel || &d.name == sel)
                    .copied()
                    .ok_or_else(|| {
                        CliError::Usage(format!("no {want:?} device {sel:?} (see `ac2 devices`)"))
                    })
            };
            let dev = match &o.device {
                Some(sel) => named(sel)?,
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
            let out_dev = match &o.out_device {
                Some(sel) => named(sel)?,
                None => default_output(dev, &of_backend).unwrap_or(dev),
            };
            let has_outputs = out_dev.output.as_ref().is_some_and(|d| d.max_channels > 0);
            if o.outputs > 0 && !has_outputs {
                return Err(CliError::Usage(format!(
                    "{} has no outputs: choose the playback device with --out-device, or \
                     --outputs 0 (see `ac2 devices`)",
                    out_dev.name
                )));
            }
            let config = SessionConfig {
                backend: Some(want),
                input_device: DeviceSelector::Id { id: dev.id.clone() },
                output_device: DeviceSelector::Id {
                    id: out_dev.id.clone(),
                },
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
                Some(i) => format!(
                    "{}\n{}",
                    output::session(&s),
                    output::inputs(i, crate::watch::now_wall())
                ),
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
            let st = state(&c).await?;
            let (autosave, recording) = (st.autosave, st.recording);
            let mut j = json!(s);
            j["autosave"] = json!(autosave);
            j["recording"] = json!(recording);
            out.emit(&j, || {
                let mut t = format!(
                    "{}\n{}",
                    output::session(&s),
                    output::autosave(&autosave, crate::watch::now_wall())
                );
                if let Some(r) = &recording {
                    t.push_str(&format!("\nrecording    {}", output::recording_line(r)));
                }
                t
            })?;
        }
        SessionCmd::Save { session } => {
            super::traces::save_or_load(cli, &c, session, false, out).await?;
        }
        SessionCmd::Load { session } => {
            super::traces::save_or_load(cli, &c, session, true, out).await?;
        }
        SessionCmd::List => super::traces::list(&c, out).await?,
        SessionCmd::Replay { recording, fast } => {
            super::rec::replay(cli, &c, recording, *fast, out).await?;
        }
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

pub(crate) fn smoothing(f: FractionArg) -> Result<SmoothingFraction, CliError> {
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
    let sweep_only = [
        ("out", n.outputs.is_some()),
        ("level", n.level.is_some()),
        ("duration", n.duration.is_some()),
        ("repeats", n.repeats.is_some()),
        ("gate", n.gate.is_some()),
        ("tail", n.tail.is_some()),
        ("lf-harmonics", n.lf_harmonics.is_some()),
    ];
    if n.kind != MeasKindArg::Sweep
        && let Some((flag, _)) = sweep_only.iter().find(|(_, given)| *given)
    {
        return Err(CliError::Usage(format!(
            "--{flag} applies to a sweep, not {:?}",
            n.kind
        )));
    }
    if n.average.is_some() && !matches!(n.kind, MeasKindArg::Spectrum | MeasKindArg::Rta) {
        return Err(CliError::Usage(format!(
            "--average applies to spectrum and rta, not {:?}{}",
            n.kind,
            if n.kind == MeasKindArg::Tf {
                " (tf averages over --blocks)"
            } else {
                ""
            }
        )));
    }
    let kind = match n.kind {
        MeasKindArg::Sweep => {
            refuse(n.input, "input")?;
            if n.smooth.is_some() || n.start {
                return Err(CliError::Usage(
                    "a sweep measurement has no smoothing and no job to start: `ac2 sweep run` \
                     plays it"
                        .into(),
                ));
            }
            MeasKind::Sweep {
                config: super::ir::sweep_config(&super::ir::SweepFlags {
                    reference: need(n.reference, "ref")?,
                    mic: need(n.measurement, "meas")?,
                    outputs: n
                        .outputs
                        .as_ref()
                        .map(|o| o.0.clone())
                        .ok_or_else(|| CliError::Usage("sweep needs --out".into()))?,
                    level: n.level.map(|l| l.0).ok_or_else(|| {
                        CliError::Usage("sweep needs --level: a sweep has no default level".into())
                    })?,
                    from: n.from.0,
                    to: n.to.0,
                    duration: n.duration.map_or(
                        ac2_proto::units::Seconds(ac2_proto::model::EssSpec::DEFAULT_DURATION_S),
                        |d| d.0,
                    ),
                    repeats: n.repeats.unwrap_or(1),
                    gate: n.gate.map(|g| g.0),
                    tail: n.tail.map(|t| t.0),
                    lf_harmonics: n.lf_harmonics.unwrap_or_default().into(),
                })?,
            }
        }
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
                                mode: if n.smooth_magnitude_only {
                                    SmoothingMode::Magnitude
                                } else {
                                    SmoothingMode::MagnitudePhase
                                },
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
            if n.smooth_magnitude_only {
                return Err(CliError::Usage(format!(
                    "--smooth-magnitude-only applies to tf, not {:?}",
                    n.kind
                )));
            }
            if n.smooth.is_some() && k != MeasKindArg::Spectrum {
                return Err(CliError::Usage(format!(
                    "--smooth applies to tf and spectrum, not {:?}{}",
                    n.kind,
                    if k == MeasKindArg::Rta {
                        " (RTA bands already are fractional-octave)"
                    } else {
                        ""
                    }
                )));
            }
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
                            smoothing: n.smooth.map(smoothing).transpose()?,
                            averaging: n.average.map_or(SpecAveraging::Off, |a| a.0),
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
                            weighting: weighting(n.weight.unwrap_or(WeightArg::Z)),
                            averaging: n.average.map_or(SpecAveraging::Off, |a| a.0),
                            ..RtaConfig::on_input(input, band_fraction(n.fraction)?)
                        },
                    }
                }
                _ => MeasKind::Spl {
                    config: SplConfig::on_input(
                        input,
                        weighting(n.weight.unwrap_or(WeightArg::A)),
                        time_weighting(n.time),
                    ),
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

/// `meas set`: `config` with the averaging changed.
pub(crate) fn meas_set(
    config: &MeasConfig,
    average: Option<SpecAveraging>,
    blocks: Option<u32>,
) -> Result<MeasConfig, CliError> {
    if average.is_none() && blocks.is_none() {
        return Err(CliError::Usage(
            "say what changes: --average (spectrum, rta) or --blocks (tf)".into(),
        ));
    }
    let mut config = config.clone();
    let name = config.name.clone();
    let kind = match &config.kind {
        MeasKind::Transfer { .. } => "tf",
        MeasKind::Spectrum { .. } => "spectrum",
        MeasKind::Rta { .. } => "rta",
        MeasKind::Spl { .. } => "spl",
        MeasKind::Math { .. } => "math",
        MeasKind::Sweep { .. } => "sweep",
    };
    match (&mut config.kind, average, blocks) {
        (MeasKind::Spectrum { config: c }, Some(a), None) => c.averaging = a,
        (MeasKind::Rta { config: c }, Some(a), None) => c.averaging = a,
        (MeasKind::Transfer { config: c }, None, Some(b)) => {
            if b == 0 {
                return Err(CliError::Usage("--blocks must be at least 1".into()));
            }
            c.averaging = TfAveraging::Fifo { blocks: b };
        }
        (_, Some(_), _) => {
            return Err(CliError::Usage(format!(
                "--average applies to spectrum and rta; {name} is a {kind} measurement"
            )));
        }
        (_, _, _) => {
            return Err(CliError::Usage(format!(
                "--blocks applies to tf; {name} is a {kind} measurement"
            )));
        }
    }
    Ok(config)
}

pub(crate) async fn meas_call(c: &Client, cmd: Command) -> Result<Measurement, CliError> {
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
        MeasCmd::Set {
            meas,
            average,
            blocks,
        } => {
            let s = state(&c).await?;
            let m = find_meas(&s, meas)?;
            let config = meas_set(&m.config, average.map(|a| a.0), *blocks)?;
            let m = meas_call(&c, Command::MeasUpdate { meas: m.id, config }).await?;
            out.emit(&m, || output::measurement(&m))?;
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
        MeasCmd::Rm {
            meas,
            keep_traces,
            delete_traces,
        } => {
            let snap = c.snapshot().await?;
            let m = find_meas(&snap.state, meas)?;
            let owner = TraceOwner::Meas { meas: m.id };
            let traces = snap
                .state
                .traces
                .iter()
                .filter(|t| t.edit.owner == owner)
                .count();
            let maths = snap
                .state
                .measurements
                .iter()
                .filter(|x| matches!(&x.config.kind, MeasKind::Math { config } if config.owner == owner))
                .count();
            // What it owns goes nowhere by default: the operator says which.
            let what = match (keep_traces, delete_traces) {
                (true, _) => OwnedTraces::Keep,
                (_, true) => OwnedTraces::Delete,
                _ if traces + maths == 0 => OwnedTraces::Keep,
                _ => {
                    return Err(CliError::Usage(format!(
                        "{} owns {traces} trace(s) and {maths} math channel(s): add \
                         --keep-traces (moved to the imported group) or --delete-traces",
                        m.config.name
                    )));
                }
            };
            // Guarded by the rev the name was resolved at: if the measurements changed in
            // between, the daemon refuses instead of deleting the wrong one.
            let r = c
                .call_expect(
                    Command::MeasDelete {
                        meas: m.id,
                        traces: what,
                    },
                    snap.rev,
                )
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
            let keys: Vec<String> = (1..=ranked.len().min(3)).map(|i| i.to_string()).collect();
            out.push_str(&format!(
                "strongest      {}{}\n",
                output::ms(strongest.delay.0),
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

pub(crate) async fn spl(cli: &Cli, cmd: &SplCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        SplCmd::Cal(a) => super::cal::cal_spl(cli, a, out).await,
        SplCmd::Leq { cmd } => super::leq::run(cli, cmd, out).await,
        SplCmd::Bands { cmd } => super::bands::run(cli, cmd, out).await,
        SplCmd::Set(a) => {
            super::leq::set_weightings(
                cli,
                a,
                a.weight.map(weighting),
                a.time.map(time_weighting),
                out,
            )
            .await
        }
        SplCmd::Watch(w) => {
            let c = connect(cli, true).await?;
            let s = state(&c).await?;
            let (id, input, created) = match (&w.meas, w.input) {
                (Some(r), _) => {
                    let m = find_meas(&s, r)?;
                    let MeasKind::Spl { config } = &m.config.kind else {
                        return Err(CliError::Usage(format!(
                            "{} is not an SPL measurement",
                            m.config.name
                        )));
                    };
                    (m.id, config.input, false)
                }
                (None, Some(input)) => {
                    // Always a meter of its own: another one on the same input (the UI's, or
                    // one a killed watch left behind) integrates over a span this command
                    // knows nothing of, and its Leq would not describe what it watched.
                    let want =
                        SplConfig::on_input(input.0, weighting(w.weight), time_weighting(w.time));
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
                    (m.id, input.0, true)
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
            let until = w.duration.map(|t| {
                std::time::Instant::now() + std::time::Duration::from_secs_f64(t.0.0.max(0.0))
            });
            let result = watch::spl(&c, id, input, until, out).await;
            if created {
                // A meter made for this view owns nothing.
                let _ = c
                    .call(Command::MeasDelete {
                        meas: id,
                        traces: OwnedTraces::Keep,
                    })
                    .await;
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

/// The playback device when none is named: the capture device itself when it has outputs
/// (one device, one clock where the host allows it), else the system's default output.
fn default_output<'a>(input: &'a DeviceInfo, devices: &[&'a DeviceInfo]) -> Option<&'a DeviceInfo> {
    if input.output.is_some() {
        return Some(input);
    }
    devices
        .iter()
        .find(|d| d.output.as_ref().is_some_and(|o| o.system_default))
        .copied()
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::Cli;
    use crate::args::{Cmd, MeasCmd};

    fn config(args: &[&str]) -> Result<MeasConfig, CliError> {
        let cli = Cli::try_parse_from(["ac2", "meas", "new"].iter().chain(args))
            .map_err(|e| CliError::Usage(e.to_string()))?;
        let Cmd::Meas {
            cmd: MeasCmd::New(n),
        } = cli.cmd
        else {
            unreachable!()
        };
        meas_config(&n)
    }

    #[test]
    fn a_sweep_takes_lf_harmonics_by_name() {
        use ac2_proto::model::LfHarmonics;
        let base = [
            "sweep", "--name", "s", "--ref", "2", "--meas", "1", "--out", "1,2", "--level",
            "-50dbfs",
        ];
        let lf = |extra: &[&str]| {
            let args: Vec<&str> = base.iter().chain(extra).copied().collect();
            match config(&args).map(|c| c.kind) {
                Ok(MeasKind::Sweep { config }) => Ok(config.lf_harmonics),
                Ok(other) => panic!("{other:?}"),
                Err(e) => Err(e),
            }
        };
        assert_eq!(lf(&[]).expect("parsed"), LfHarmonics::Standard);
        assert_eq!(
            lf(&["--lf-harmonics", "fine"]).expect("parsed"),
            LfHarmonics::Fine
        );
        assert_eq!(
            lf(&["--lf-harmonics", "standard"]).expect("parsed"),
            LfHarmonics::Standard
        );
        assert!(lf(&["--lf-harmonics", "on"]).is_err());
        let e = config(&[
            "tf",
            "--name",
            "t",
            "--ref",
            "2",
            "--meas",
            "1",
            "--lf-harmonics",
            "fine",
        ])
        .expect_err("refused");
        assert!(
            e.to_string().contains("--lf-harmonics applies to a sweep"),
            "{e}"
        );
    }

    #[test]
    fn spectrum_and_rta_take_an_average_and_meas_set_changes_it() {
        use ac2_proto::units::Seconds;
        let rta = config(&[
            "rta",
            "--name",
            "r",
            "--input",
            "1",
            "--average",
            "fifo:2400",
        ])
        .expect("rta");
        let MeasKind::Rta { config: c } = &rta.kind else {
            panic!("{rta:?}");
        };
        assert_eq!(c.averaging, SpecAveraging::Fifo { frames: 2400 });
        let spec = config(&[
            "spectrum",
            "--name",
            "s",
            "--input",
            "1",
            "--average",
            "exp:2s",
        ])
        .expect("spectrum");
        let MeasKind::Spectrum { config: c } = &spec.kind else {
            panic!("{spec:?}");
        };
        assert_eq!(
            c.averaging,
            SpecAveraging::Exponential {
                time_constant: Seconds(2.0)
            }
        );
        // Off unless asked, as the UI's dialogs.
        let plain = config(&["rta", "--name", "r", "--input", "1"]).expect("rta");
        assert!(matches!(
            plain.kind,
            MeasKind::Rta { config } if config.averaging == SpecAveraging::Off
        ));
        assert!(config(&["rta", "--name", "r", "--input", "1", "--average", "fifo:0"]).is_err());
        let e = config(&[
            "tf",
            "--name",
            "t",
            "--ref",
            "2",
            "--meas",
            "1",
            "--average",
            "exp:1s",
        ])
        .expect_err("refused");
        assert!(e.to_string().contains("tf averages over --blocks"), "{e}");

        let set = meas_set(&rta, Some(SpecAveraging::Off), None).expect("set");
        assert!(matches!(
            set.kind,
            MeasKind::Rta { config } if config.averaging == SpecAveraging::Off
        ));
        assert!(meas_set(&rta, None, None).is_err());
        let e = meas_set(&rta, None, Some(4)).expect_err("refused");
        assert!(e.to_string().contains("--blocks applies to tf"), "{e}");
        let tf = config(&["tf", "--name", "t", "--ref", "2", "--meas", "1"]).expect("tf");
        let set = meas_set(&tf, None, Some(32)).expect("set");
        assert!(matches!(
            set.kind,
            MeasKind::Transfer { config } if config.averaging == TfAveraging::Fifo { blocks: 32 }
        ));
        let e = meas_set(&tf, Some(SpecAveraging::Off), None).expect_err("refused");
        assert!(e.to_string().contains("t is a tf measurement"), "{e}");
    }

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

    #[test]
    fn smoothing_flags() {
        let tf = |extra: &[&str]| {
            let mut a = vec!["tf", "--ref", "1", "--meas", "2", "--name", "x"];
            a.extend_from_slice(extra);
            match config(&a).map(|c| c.kind) {
                Ok(MeasKind::Transfer { config }) => Ok(config.smoothing),
                Ok(other) => panic!("{other:?}"),
                Err(e) => Err(e),
            }
        };
        // Phase is smoothed with the magnitude unless asked not to.
        assert_eq!(
            tf(&["--smooth", "6"]).ok(),
            Some(Some(Smoothing {
                fraction: SmoothingFraction::Sixth,
                mode: SmoothingMode::MagnitudePhase
            }))
        );
        assert_eq!(
            tf(&["--smooth", "12", "--smooth-magnitude-only"]).ok(),
            Some(Some(Smoothing {
                fraction: SmoothingFraction::Twelfth,
                mode: SmoothingMode::Magnitude
            }))
        );
        assert!(tf(&["--smooth-magnitude-only"]).is_err(), "needs --smooth");
        assert_eq!(tf(&[]).ok(), Some(None));

        let spec = config(&["spectrum", "--input", "2", "--name", "s", "--smooth", "3"]);
        match spec.map(|c| c.kind) {
            Ok(MeasKind::Spectrum { config }) => {
                assert_eq!(config.smoothing, Some(SmoothingFraction::Third));
            }
            other => panic!("{other:?}"),
        }
        let e = config(&["rta", "--input", "2", "--name", "r", "--smooth", "3"]);
        assert!(
            matches!(&e, Err(CliError::Usage(m)) if m.contains("already are fractional-octave")),
            "{e:?}"
        );
        assert!(config(&["spl", "--input", "2", "--name", "s", "--smooth", "3"]).is_err());
        assert!(
            config(&[
                "spectrum",
                "--input",
                "2",
                "--name",
                "s",
                "--smooth",
                "3",
                "--smooth-magnitude-only"
            ])
            .is_err()
        );
    }
}
