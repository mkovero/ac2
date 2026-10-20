//! Two curves of one mic switched on an input.

use super::*;

/// Two curves of one capsule (beyerdynamic MM1, 0° and 90° incidence, the manufacturer's
/// files): labels from the files, the first import chosen on the input, the second not;
/// switching 0° → 90° → off moves the served transfer-function magnitude by exactly the
/// curves' difference (both normalised at 1 kHz, the input being uncalibrated); captures
/// record the curve in use; a chosen curve that is deleted says so instead of silently
/// applying nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_curves_of_one_mic_switched_on_an_input() {
    use ac2_core::mic_curve::MicCurve;
    use ac2_proto::cal::{CurveUse, state_input_use};
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let h = start(&dir.path().join("calibrations.json"));
    let c = connect(&h).await;
    c.call(Command::SessionOpen {
        config: session(false),
    })
    .await
    .unwrap();
    let m = match c
        .call(Command::MeasCreate {
            config: transfer("tf"),
        })
        .await
        .unwrap()
    {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    c.call(Command::MeasStart { meas: m.id }).await.unwrap();
    c.subscribe(Subscription::Meas(m.id)).unwrap();
    c.call(Command::DelaySet {
        meas: m.id,
        delay: Seconds(f64::from(ACOUSTIC_DELAY) / f64::from(FS)),
    })
    .await
    .unwrap();
    const MIC: &str = "MM1 34804";
    set_curve(&c, MIC, CurveChoice::NotChosen).await;

    let zero = fixture("449350_34804_0Grad.txt");
    let ninety = fixture("449350_34804_90Grad.txt");
    let m0 = import(&c, MIC, "449350_34804_0Grad.txt", &zero).await;
    assert_eq!(m0.curves.len(), 1);
    assert_eq!(m0.curves[0].label, "0°");
    assert_eq!(m0.curves[0].stated_sensitivity, Some(15.0));
    let m2 = import(&c, MIC, "449350_34804_90Grad.txt", &ninety).await;
    let labels: Vec<&str> = m2.curves.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["0°", "90°"]);
    // The first import was the mic's only curve, so the input chose it; the second leaves
    // the choice alone.
    let st = state_until(&c, |s| s.mics.first().is_some_and(|m| m.curves.len() == 2)).await;
    assert_eq!(st.inputs[0].curve, label("0°"));

    let lease = c.acquire_lease(false, OnDrop::Release).await.unwrap();
    fire(&lease, Signal::Pink).await;
    let grid_f = |k: i32| 1000.0 * 2f64.powf(f64::from(k) / 48.0);
    // Columns at 5, 8, 12.5 and 16 kHz (k_min = −240).
    let cols: Vec<usize> = [5000.0f64, 8000.0, 12_500.0, 16_000.0]
        .iter()
        .map(|f| ((48.0 * (f / 1000.0).log2()).round() as i32 + 240) as usize)
        .collect();
    let tf_with = |want: bool| {
        let c = &c;
        let cols = cols.clone();
        async move {
            // Two frames past the switch, so the frame is from after it.
            let mut seen = 0;
            wait(c, m.id.0, Stream::Tf, "tf", |d| {
                tf(d).filter(|f| {
                    let ok = f.meta.mic_curve == want && cols.iter().all(|&k| f.mag[k].is_finite());
                    if ok {
                        seen += 1;
                    }
                    ok && seen > 2
                })
            })
            .await
        }
    };
    settled(&c, m.id, &cols).await;
    let at0 = tf_with(true).await;
    set_curve(&c, MIC, label("90°")).await;
    let at90 = tf_with(true).await;
    set_curve(&c, MIC, CurveChoice::Off).await;
    let off = tf_with(false).await;
    let c0 = MicCurve::parse(&zero).unwrap().normalised(1000.0);
    let c90 = MicCurve::parse(&ninety).unwrap().normalised(1000.0);
    for &k in &cols {
        let f = grid_f(k as i32 - 240);
        // Displayed = raw − cₙ(f): 0° → 90° moves it by c0ₙ − c90ₙ, off by +c0ₙ.
        let want = c0.db(f) - c90.db(f);
        let got = f64::from(at90.mag[k] - at0.mag[k]);
        assert!(
            (got - want).abs() < 0.25,
            "{f:.0} Hz: 0°→90° {got:+.2} dB, curves {want:+.2}"
        );
        let got = f64::from(off.mag[k] - at0.mag[k]);
        assert!(
            (got - c0.db(f)).abs() < 0.25,
            "{f:.0} Hz: off {got:+.2} dB, 0° curve {:+.2}",
            c0.db(f)
        );
        assert!(
            (at90.phase[k] - at0.phase[k]).abs() < 5.0,
            "phase untouched"
        );
    }

    // A capture keeps exactly which curve it carries: label and content hash.
    set_curve(&c, MIC, label("90°")).await;
    tf_with(true).await;
    let t = capture(&c, m.id.0, "tf-90").await;
    let used = t.mic.as_ref().and_then(|m| m.curve.clone()).unwrap();
    assert_eq!(used, m2.curves[1]);
    let export = match c
        .call(Command::TraceExport {
            trace: t.id,
            format: ac2_proto::model::ExportFormat::Ac2Csv,
        })
        .await
        .unwrap()
    {
        ReplyBody::Export { content, .. } => String::from_utf8(content.0).unwrap(),
        other => panic!("{other:?}"),
    };
    assert!(
        export.contains("MM1 34804 (curve: 90°, in the columns; file \"449350_34804_90Grad.txt\""),
        "{}",
        export.lines().take(20).collect::<Vec<_>>().join("\n")
    );

    // Deleting the chosen curve: the input keeps the choice and says the curve is missing;
    // the frames say no curve ran.
    c.call(Command::CalCurveDelete {
        curve: MicCurveId {
            mic: MIC.into(),
            label: "90°".into(),
        },
    })
    .await
    .unwrap();
    let st = state_until(&c, |s| s.mics[0].curves.len() == 1).await;
    assert_eq!(
        state_input_use(&st, 1).curve,
        CurveUse::Missing {
            mic: MIC,
            label: "90°"
        }
    );
    tf_with(false).await;
    lease.end().await.unwrap();
    drop(c);
    tokio::task::spawn_blocking(move || h.shutdown())
        .await
        .unwrap();
}

/// Returns once the transfer of `meas` holds still at `cols`: two fresh frames 300 ms
/// apart within 0.05 dB of each other there, so the averages have settled.
async fn settled(c: &Client, meas: MeasId, cols: &[usize]) {
    let topic = Topic::Data {
        meas,
        stream: Stream::Tf,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut held: Option<(Instant, Vec<f32>)> = None;
    loop {
        let now = Instant::now();
        let mags = c.latest().unwrap().get(&topic).and_then(|f| {
            let t = tf(&f.frame.data).filter(|_| !f.stale)?;
            let m: Vec<f32> = cols.iter().map(|&k| t.mag[k]).collect();
            m.iter().all(|v| v.is_finite()).then_some(m)
        });
        if let Some(m) = mags {
            match &held {
                Some((at, prev)) if now - *at >= Duration::from_millis(300) => {
                    if prev.iter().zip(&m).all(|(a, b)| (a - b).abs() < 0.05) {
                        return;
                    }
                    held = Some((now, m));
                }
                Some(_) => {}
                None => held = Some((now, m)),
            }
        }
        assert!(now < deadline, "the transfer never settled");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}
