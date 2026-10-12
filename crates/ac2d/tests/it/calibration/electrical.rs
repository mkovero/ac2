//! Electrical calibration from a measured voltage and the mic's data-sheet sensitivity.

use super::*;

fn electrical(
    mic: &str,
    volts: f64,
    mic_sensitivity: Option<f64>,
    replace_acoustic: bool,
) -> Command {
    Command::CalSplElectrical {
        input: 1,
        mic: mic.into(),
        connection: ElectricalConnection::InLine,
        volts: Volts(volts),
        freq: Hz(1000.0),
        mic_sensitivity: mic_sensitivity.map(MvPerPa),
        loading: None,
        uncertainty: None,
        replace_acoustic,
    }
}

/// `cmd` with the mic's output and the preamp's input impedance, Ω.
fn loaded(mut cmd: Command, mic_ohms: f64, input_ohms: f64) -> Command {
    if let Command::CalSplElectrical { loading, .. } = &mut cmd {
        *loading = Some(MicLoading {
            mic_impedance: Ohms(mic_ohms),
            input_impedance: Ohms(input_ohms),
        });
    }
    cmd
}

/// Retries `cmd` while the tone the generator just started is not there or not steady yet.
async fn steady(c: &Client, cmd: Command) -> Result<ReplyBody, ClientError> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match c.call(cmd.clone()).await {
            // The tone just went on: nothing, or not settled, yet.
            Err(ClientError::Daemon(p))
                if p.msg.contains("not steady") || p.msg.contains("no tone") => {}
            other => return other,
        }
        assert!(Instant::now() < deadline, "level never steady");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn refused(r: Result<ReplyBody, ClientError>, what: &str) -> String {
    match r {
        Err(ClientError::Daemon(p)) if p.code == ErrorCode::Refused => p.msg,
        other => panic!("{what}: expected a refusal, got {other:?}"),
    }
}

/// Electrical calibration on the fake rig (Q7 §11): a 1 kHz tone at a known level on input
/// 2, a "measured" voltage and the data-sheet sensitivity of the mic's curve file; the SPL
/// meter then reads what the hand calculation says, with the method in its status.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_electrical_calibration_end_to_end() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("calibrations.json");
    let h = start(&store);
    let c = connect(&h).await;

    // No session: refused like cal.spl.
    let e = c
        .call(electrical("MM1 34804", 0.015, None, false))
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Daemon(p) if p.code == ErrorCode::Invalid));

    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    let m = match c
        .call(Command::MeasCreate {
            config: spl("meter", 1),
        })
        .await
        .unwrap()
    {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    c.call(Command::MeasStart { meas: m.id }).await.unwrap();
    c.subscribe(Subscription::Meas(m.id)).unwrap();

    // The mic's curve file states 15.0 mV/Pa; importing it on input 2 names the mic there.
    import(
        &c,
        "MM1 34804",
        "449350_34804_0Grad.txt",
        &fixture("449350_34804_0Grad.txt"),
    )
    .await;

    // Unit slips and a mic without a data-sheet value are invalid before anything is read.
    for (cmd, want) in [
        (electrical("MM1 34804", 0.0, None, false), "outside 0.1 mV"),
        (
            electrical("MM1 34804", 0.015, Some(15_000.0), false),
            "outside 0.1",
        ),
        (electrical("ECM", 0.015, None, false), "no curve file"),
        (
            loaded(electrical("MM1 34804", 0.015, None, false), 0.0, 2000.0),
            "mic output impedance 0 Ω",
        ),
        (
            loaded(electrical("MM1 34804", 0.015, None, false), 330.0, -1.0),
            "preamp input impedance -1 Ω",
        ),
    ] {
        match c.call(cmd).await {
            Err(ClientError::Daemon(p)) if p.code == ErrorCode::Invalid => {
                assert!(p.msg.contains(want), "{want:?} in {}", p.msg);
            }
            other => panic!("{want}: {other:?}"),
        }
    }
    // Silence: no tone.
    let msg = refused(
        c.call(electrical("MM1 34804", 0.015, None, false)).await,
        "silence",
    );
    assert!(msg.contains("no tone signal"), "{msg}");

    // A tone too low for a good reading: −70 dBFS out, −76 dBFS in.
    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    let tone = |level: f64| GeneratorDesired {
        settings: GeneratorSettings {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Dbfs(level),
            band: None,
            outputs: vec![0],
        },
        armed: true,
        firing: true,
    };
    lease.set(tone(-70.0)).await.unwrap();
    let msg = refused(
        steady(&c, electrical("MM1 34804", 0.015, None, false)).await,
        "too low",
    );
    assert!(msg.contains("too low"), "{msg}");

    // The tone at −20 dBFS out reads −26.02 dBFS on the input. "Measured" 15 mV there with
    // 15 mV/Pa: 0 dBFS = 15 mV · 10^(26.02/20) ≈ 0.300 V, sensitivity
    // 20·lg(15 mV / (15 mV/Pa · 20 µPa)) − L = 93.98 + 26.02 = 120.0 dB.
    lease.set(tone(-20.0)).await.unwrap();
    spl_until(&c, "the louder tone", |m| (m.level + 26.02).abs() < 0.3).await;
    let e = match steady(&c, electrical("MM1 34804", 0.015, None, false))
        .await
        .unwrap()
    {
        ReplyBody::Calibration(e) => e,
        other => panic!("{other:?}"),
    };
    let l = e.spl.measured.0;
    assert!((l + 26.02).abs() < 0.06, "{l}");
    let CalMethod::Electrical {
        connection,
        volts,
        full_scale,
        mic_sensitivity,
        mic_sensitivity_from,
        loading,
        uncertainty,
    } = &e.spl.method
    else {
        panic!("{:?}", e.spl.method)
    };
    assert_eq!(*connection, ElectricalConnection::InLine);
    assert_eq!(*volts, Volts(0.015));
    assert!((full_scale.0 - 0.015 / 10f64.powf(l / 20.0)).abs() < 1e-12);
    assert_eq!(*mic_sensitivity, MvPerPa(15.0));
    assert_eq!(
        *mic_sensitivity_from,
        SensitivitySource::DataSheet {
            label: "0°".into(),
            file_name: "449350_34804_0Grad.txt".into()
        }
    );
    assert_eq!(*loading, None);
    assert_eq!(*uncertainty, Db(1.0));
    // By hand: 15 mV / 15 mV/Pa = 1 Pa = 93.98 dB SPL, read at L dBFS.
    let want = 20.0 * (1.0 / 20e-6f64).log10() - l;
    assert!((e.spl.sensitivity.0 - want).abs() < 1e-9);
    assert!((e.spl.sensitivity.0 - 120.0).abs() < 0.07);

    // The SPL meter reads 93.98 dB SPL (1 Pa at the mic gives 15 mV), verified, electrical.
    let m = spl_until(&c, "electrically calibrated", |m| {
        matches!(
            m.cal,
            CalStatus::Verified {
                basis: CalBasis::Electrical {
                    data_sheet: true,
                    ..
                },
                ..
            }
        ) && m.scale == LevelScale::DbSpl
    })
    .await;
    assert!((m.level - 93.98).abs() < 0.2, "{}", m.level);

    // A calibrator replaces it without asking; an electrical one replaces a calibrator's
    // only on purpose.
    let r = steady(
        &c,
        Command::CalSpl {
            input: 1,
            mic: "MM1 34804".into(),
            calibrator_level: DbSpl(94.0),
            calibrator_freq: Hz(1000.0),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(&r, ReplyBody::Calibration(e) if matches!(e.spl.method, CalMethod::Acoustic { .. })),
        "{r:?}"
    );
    let msg = refused(
        c.call(electrical("MM1 34804", 0.015, None, false)).await,
        "acoustic",
    );
    assert!(msg.contains("acoustic calibration"), "{msg}");
    // With the MM1's 330 Ω driving a 2 kΩ input the open-circuit 15 mV/Pa reads as
    // 15 · 2000/2330 mV/Pa at the input: 0 dBFS is 20·lg(2330/2000) = 1.327 dB more SPL.
    let r = steady(
        &c,
        loaded(
            electrical("MM1 34804", 0.015, Some(15.0), true),
            330.0,
            2000.0,
        ),
    )
    .await
    .unwrap();
    let ReplyBody::Calibration(e2) = r else {
        panic!("{r:?}")
    };
    assert!(matches!(
        e2.spl.method,
        CalMethod::Electrical {
            mic_sensitivity: MvPerPa(15.0),
            mic_sensitivity_from: SensitivitySource::Typed,
            loading: Some(_),
            ..
        }
    ));
    let want =
        20.0 * (1.0 / 20e-6f64).log10() - e2.spl.measured.0 + 20.0 * (2330.0f64 / 2000.0).log10();
    assert!((e2.spl.sensitivity.0 - want).abs() < 1e-9);
    assert!((e2.spl.sensitivity.0 - e.spl.sensitivity.0 - 1.3265).abs() < 0.06);
    let st = state_until(&c, |s| {
        s.calibrations
            .iter()
            .any(|x| matches!(x.spl.method, CalMethod::Electrical { .. }))
    })
    .await;
    assert_eq!(st.calibrations.len(), 1);

    lease.end().await.unwrap();
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();

    // The method is in the store file.
    let h = start(&store);
    let c = connect(&h).await;
    let st = state_until(&c, |_| true).await;
    assert_eq!(st.calibrations.len(), 1);
    assert_eq!(st.calibrations[0].spl, e2.spl);
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}
