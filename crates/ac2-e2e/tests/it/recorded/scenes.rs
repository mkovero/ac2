//! The rigs the app's pictures are taken of, each built on a fresh simulated rig with the
//! commands the app (or `ac2`) would send. The measurements' frames are not recorded: the
//! app's tests draw shapes of their own on these measurements (display material only).

use ac2_proto::model::{
    AlignBand, ArtnetTarget, Bandwidth, CurveChoice, DepthPolicy, EqBody, EqEdit, EqRealisation,
    EqSection, EqShape, InputKind, InputSetup, Marker, MarkerKind, MathConfig, MathDomain,
    MathExpr, MathOp, MeasConfig, MeasKind, MeasScope, Measurement, Operand, PeakQ, PlanSpec,
    Resolution, ShelfFreq, ShelfOrder, ShelfSlope, Smoothing, SmoothingFraction, SmoothingMode,
    SpecResolution, SpectrumConfig, SplConfig, StreamConfig, TfAveraging, TimeWeighting,
    TraceOwner, TransferConfig, WalkKind, WalkPointSpec, Weighting, ZoneSpec,
};
use ac2_proto::units::{Db, DbSpl, Hz, MeasId, Seconds};
use ac2_proto::{Command, ReplyBody};

use crate::harness::{Cli, R};

/// The stream every rig opens: the simulated rig's four inputs and two outputs at 48 kHz,
/// out 1 looped back on in 1.
pub fn stream() -> StreamConfig {
    StreamConfig {
        input_channels: vec![0, 1, 2, 3],
        output_channels: 2,
        sample_rate_hz: Some(48_000),
        buffer_frames: Some(256),
        ..ac2_ui::sim::demo_stream()
    }
}

fn session_meas(name: &str, kind: MeasKind) -> MeasConfig {
    MeasConfig {
        scope: MeasScope::Session,
        name: name.into(),
        kind,
    }
}

/// Reference in 1 against `input`, at 1/24 octave with 1/6-octave smoothing: the grid and
/// smoothing the pictures' transfer curves are drawn on.
fn transfer(input: u16) -> MeasKind {
    MeasKind::Transfer {
        config: TransferConfig {
            averaging: TfAveraging::Exponential {
                time_constant: Seconds(1.0),
            },
            resolution: Resolution::TwentyFourth.into(),
            smoothing: Some(Smoothing {
                fraction: SmoothingFraction::Sixth,
                mode: SmoothingMode::MagnitudePhase,
            }),
            depth: DepthPolicy::EqualConfidence,
            ..TransferConfig::with_inputs(0, input)
        },
    }
}

/// A 4096-point spectrum of `input`: the grid the pictures' spectra are drawn on.
fn spectrum(input: u16) -> MeasKind {
    MeasKind::Spectrum {
        config: SpectrumConfig {
            resolution: SpecResolution::Fixed { fft_len: 4096 },
            ..SpectrumConfig::on_input(input)
        },
    }
}

fn create(cli: &Cli, config: MeasConfig) -> R<Measurement> {
    match cli.call(Command::MeasCreate { config })? {
        ReplyBody::Measurement(m) => Ok(m),
        other => Err(format!("meas.create: {other:?}").into()),
    }
}

fn start(cli: &Cli, config: MeasConfig) -> R<MeasId> {
    let id = create(cli, config)?.id;
    cli.call(Command::MeasStart { meas: id })?;
    Ok(id)
}

/// The id of the measurement named `name`.
fn id_of(cli: &Cli, name: &str) -> R<MeasId> {
    cli.state()
        .and_then(|s| {
            s.measurements
                .iter()
                .find(|m| m.config.name == name)
                .map(|m| m.id)
        })
        .ok_or_else(|| format!("no measurement {name}").into())
}

/// The stream open with two transfer positions, a spectrum and an SPL meter, all running:
/// Main L following its delay from 12.5 ms, Delay tower held at 677 samples.
pub fn rig(cli: &Cli) -> R {
    cli.call(Command::StreamOpen { config: stream() })?;
    let main = start(cli, session_meas("Main L", transfer(1)))?;
    let tower = start(cli, session_meas("Delay tower", transfer(2)))?;
    start(cli, session_meas("Mic 1 FFT", spectrum(1)))?;
    start(
        cli,
        session_meas(
            "FOH SPL",
            MeasKind::Spl {
                config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
            },
        ),
    )?;
    cli.call(Command::DelaySet {
        meas: main,
        delay: Seconds(0.0125),
    })?;
    cli.call(Command::DelayTrack {
        meas: main,
        enabled: true,
    })?;
    cli.call(Command::DelaySet {
        meas: tower,
        delay: Seconds(677.0 / 48_000.0),
    })?;
    Ok(())
}

/// [`rig`] with a third position "Seat 3" (stopped) and "Audience", the power average of
/// the three positions smoothed at 1/6 octave.
pub fn math_average(cli: &Cli) -> R {
    rig(cli)?;
    let seat = start(cli, session_meas("Seat 3", transfer(3)))?;
    cli.call(Command::MeasStop { meas: seat })?;
    let of = vec![
        Operand::Meas {
            meas: id_of(cli, "Main L")?,
        },
        Operand::Meas {
            meas: id_of(cli, "Delay tower")?,
        },
        Operand::Meas { meas: seat },
    ];
    let mut config = MathConfig::power_average(TraceOwner::Imported, MathDomain::Transfer, of);
    config.smoothing = Some(Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::MagnitudePhase,
    });
    start(cli, session_meas("Audience", MeasKind::Math { config }))?;
    Ok(())
}

/// [`rig`] with "Mic 2 FFT" and "Mic 1 − Mic 2", their level difference.
pub fn spectrum_math(cli: &Cli) -> R {
    rig(cli)?;
    let mic2 = start(cli, session_meas("Mic 2 FFT", spectrum(2)))?;
    let config = MathConfig::of(
        TraceOwner::Imported,
        MathDomain::Spectrum,
        MathExpr::Binary {
            a: Operand::Meas {
                meas: id_of(cli, "Mic 1 FFT")?,
            },
            op: MathOp::Subtract,
            b: Operand::Meas { meas: mic2 },
        },
    );
    start(
        cli,
        session_meas("Mic 1 − Mic 2", MeasKind::Math { config }),
    )?;
    Ok(())
}

/// [`rig`] with "Delay tower ↔ Main L", their alignment in 60–120 Hz, listed under Delay
/// tower.
pub fn alignment(cli: &Cli) -> R {
    rig(cli)?;
    let tower = id_of(cli, "Delay tower")?;
    let config = MathConfig::of(
        TraceOwner::Meas { meas: tower },
        MathDomain::Transfer,
        MathExpr::Align {
            a: Operand::Meas { meas: tower },
            b: Operand::Meas {
                meas: id_of(cli, "Main L")?,
            },
            band: AlignBand {
                lo_hz: Hz(60.0),
                hi_hz: Hz(120.0),
            },
            max_delay: Seconds(0.05),
        },
    );
    start(
        cli,
        session_meas("Delay tower ↔ Main L", MeasKind::Math { config }),
    )?;
    Ok(())
}

/// [`rig`] with the plan "Hall" (a 20 × 15 m floor, FOH at its middle facing the stage) and
/// "Mains", a walk of four points with Main L's and Delay tower's mics (two steps of two),
/// none captured yet.
pub fn walk(cli: &Cli) -> R {
    rig(cli)?;
    let plan = PlanSpec {
        name: "Hall".into(),
        outline: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 15.0], [0.0, 15.0]],
        ceiling: Some(8.0),
        zones: vec![ZoneSpec {
            name: "Floor".into(),
            outline: vec![[2.0, 2.0], [18.0, 2.0], [18.0, 11.0], [2.0, 11.0]],
            ear_height: 1.2,
            weight: None,
            seats: Some(400),
        }],
        markers: vec![Marker {
            name: "FOH".into(),
            kind: MarkerKind::Foh,
            at: [10.0, 3.0, 1.2],
            heading_deg: Some(90.0),
        }],
        surfaces: vec![],
    };
    cli.call(Command::PlanSave { plan })?;
    let points = [[5.0, 9.0], [15.0, 9.0], [15.0, 5.0], [5.0, 5.0]]
        .iter()
        .enumerate()
        .map(|(i, at)| WalkPointSpec {
            label: format!("P{}", i + 1),
            at: [at[0], at[1], 1.2],
        })
        .collect();
    cli.call(Command::WalkNew(ac2_proto::WalkNewArgs {
        name: "Mains".into(),
        plan: "Hall".into(),
        kind: WalkKind::LiveTransfer,
        meas: vec![id_of(cli, "Main L")?, id_of(cli, "Delay tower")?],
        points,
    }))?;
    Ok(())
}

/// The EQ set the EQ pictures show: a 2 kHz cut and a gentle top shelf.
pub fn eq_body() -> EqBody {
    EqBody {
        name: "EQ 1".into(),
        realisation: EqRealisation::Digital {
            sample_rate_hz: Hz(48_000.0),
        },
        peak_q: PeakQ::HalfGain,
        enabled: true,
        sections: vec![
            EqSection {
                enabled: true,
                shape: EqShape::Peaking {
                    f: Hz(2000.0),
                    gain: Db(-4.5),
                    bw: Bandwidth::Q { value: 1.4 },
                    q_measured: None,
                },
            },
            EqSection {
                enabled: true,
                shape: EqShape::HighShelf {
                    f: Hz(10_000.0),
                    gain: Db(2.0),
                    order: ShelfOrder::Second {
                        slope: ShelfSlope::S { value: 1.0 },
                    },
                    f_at: ShelfFreq::Midpoint,
                },
            },
        ],
    }
}

/// [`rig`] with Main L given the set "EQ 1": the daemon adds "Main L after EQ 1", the
/// prediction of its live curve through it.
pub fn eq(cli: &Cli) -> R {
    rig(cli)?;
    let main = id_of(cli, "Main L")?;
    let body = eq_body();
    cli.call(Command::MeasEq {
        meas: main,
        edit: EqEdit::New {
            name: Some(body.name),
            realisation: Some(body.realisation),
            peak_q: Some(body.peak_q),
            sections: body.sections,
        },
    })?;
    Ok(())
}

/// [`rig`] with a rig-scope meter "FOH limit" on in 1 whose first Leq window has a 99 dB
/// limit and the meter a cue (its input is not calibrated, so the rig cannot judge it: the
/// fault cue, holding DMX 12 at 255 and 14 at 40), an OSC target on this machine (nothing
/// listens there), an Art-Net target this test holds, a 1.5 s Art-Net fade and the monitor
/// started with the daemon.
pub fn rig_monitor(cli: &Cli) -> R {
    rig(cli)?;
    let mut config = SplConfig::on_input(0, Weighting::A, TimeWeighting::Fast);
    let window = config.leq.windows.first_mut().ok_or("no Leq window")?;
    window.limit = Some(ac2_proto::units::DbSpl(99.0));
    config.cue = Some(Box::new(ac2_proto::model::Cue {
        ok: None,
        near: None,
        over: None,
        fault: Some(ac2_proto::model::CueAction {
            osc: None,
            dmx: ac2_proto::model::parse_dmx_list("12=255 14=40")?,
        }),
    }));
    start(
        cli,
        MeasConfig {
            scope: MeasScope::Rig,
            name: "FOH limit".into(),
            kind: MeasKind::Spl { config },
        },
    )?;
    // The rig settings' rev as the daemon has it now: each command bumps it.
    let base =
        || -> R<ac2_proto::units::Rev> { Ok(cli.block_on(cli.client().snapshot())?.state.rig.rev) };
    cli.call(Command::OscTarget {
        base: base()?,
        addr: Some("127.0.0.1:9000".parse()?),
    })?;
    cli.call(Command::ArtnetTarget {
        base: base()?,
        target: Some(ArtnetTarget {
            addr: artnet_node()?,
            universe: "0.0.1".parse()?,
        }),
    })?;
    cli.call(Command::ArtnetFade {
        base: base()?,
        seconds: Seconds(1.5),
    })?;
    cli.call(Command::RigMonitorAtStart {
        base: base()?,
        on: true,
    })?;
    Ok(())
}

/// A loopback socket of this test's own standing in for an Art-Net node, held for the
/// whole run: the frames sent to it reach nothing else, where Art-Net's own port could
/// reach an Art-Net program on the host.
fn artnet_node() -> R<std::net::SocketAddr> {
    static NODE: std::sync::OnceLock<std::net::UdpSocket> = std::sync::OnceLock::new();
    if NODE.get().is_none() {
        let _ = NODE.set(std::net::UdpSocket::bind("127.0.0.1:0")?);
    }
    Ok(NODE.get().ok_or("no Art-Net node socket")?.local_addr()?)
}

/// [`rig`] with the mic "MM1 34804" (its 0° and 90° curves), in 2 calibrated with it from
/// a 94 dB calibrator (the rig's own 1 kHz tone standing in for one) and using the 0° curve.
pub fn calibrations(cli: &Cli) -> R {
    rig(cli)?;
    let mic = "MM1 34804";
    for (label, file) in [
        ("0°", "449350_34804_0Grad.txt"),
        ("90°", "449350_34804_90Grad.txt"),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/mic_curves")
            .join(file);
        cli.call(Command::CalCurveImport(ac2_proto::CalCurveImportArgs {
            mic: mic.into(),
            label: Some(label.into()),
            file_name: file.into(),
            content: ac2_proto::units::Blob(std::fs::read(path)?),
            channel: None,
        }))?;
    }
    cli.call(Command::StreamInputs {
        inputs: vec![InputSetup {
            channel: 1,
            kind: InputKind::Mic {
                mic: Some(mic.into()),
                curve: CurveChoice::Curve {
                    label: "0°".into()
                },
            },
            pair: None,
        }],
        pairs: Vec::new(),
    })?;
    let lease = cli.lease(ac2_proto::model::GeneratorDesired {
        settings: ac2_proto::model::GeneratorSettings {
            signal: ac2_proto::model::Signal::Sine { freq: Hz(1000.0) },
            level: ac2_proto::units::Dbfs(-20.0),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    })?;
    // The tone reaches in 2 a block or two after the generator fires: until then the
    // daemon finds no calibrator signal there.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match cli.call(Command::CalSpl {
            channel: 1,
            mic: mic.into(),
            calibrator_level: DbSpl(94.0),
            calibrator_freq: Hz(1000.0),
        }) {
            Ok(_) => break,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
    drop(lease);
    Ok(())
}
