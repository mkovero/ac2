//! Smoothing of a running transfer measurement and its captures.

use super::*;

fn sixth() -> Smoothing {
    Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::MagnitudePhase,
    }
}

/// `raw` (as captured: unsmoothed, NaN gaps) smoothed by ac2-core directly: magnitude dB and
/// phase degrees.
fn core_smoothed(raw: &TraceData, s: Smoothing) -> (Vec<f64>, Vec<f64>) {
    use ac2_core::smoothing as sm;
    let grid = ac2_core::grid::LogGrid {
        ppo: 48,
        k_min: -240,
        k_max: 239,
    };
    let phase = raw.phase_deg.as_ref().unwrap();
    let h: Vec<num_complex::Complex64> = raw
        .mag_db
        .iter()
        .zip(phase)
        .map(|(m, p)| {
            num_complex::Complex64::from_polar(
                10f64.powf(f64::from(*m) / 20.0),
                f64::from(*p).to_radians(),
            )
        })
        .collect();
    let valid: Vec<bool> = h.iter().map(|z| z.re.is_finite()).collect();
    let coh = vec![1.0; h.len()];
    let fraction = match s.fraction {
        SmoothingFraction::Sixth => sm::SmoothingFraction::Sixth,
        other => unimplemented!("{other:?}"),
    };
    let mode = match s.mode {
        SmoothingMode::Magnitude => sm::SmoothingMode::Magnitude,
        SmoothingMode::MagnitudePhase => sm::SmoothingMode::MagnitudePhase,
    };
    let out = sm::Smoother::new(grid, fraction).smooth(
        sm::TfColumns {
            h: &h,
            coherence: &coh,
            valid: &valid,
        },
        mode,
    );
    out.h
        .iter()
        .map(|z| (20.0 * z.norm().log10(), z.arg().to_degrees()))
        .unzip()
}

/// Smallest difference between two angles in degrees.
fn deg_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Mean squared second difference over the band: how rough a curve is.
fn roughness(v: &[f32]) -> f64 {
    let b = band();
    b.windows(3)
        .map(|w| {
            let d = f64::from(v[w[0]]) - 2.0 * f64::from(v[w[1]]) + f64::from(v[w[2]]);
            d * d
        })
        .sum::<f64>()
        / b.len() as f64
}

/// Smoothing changes on a running transfer measurement without restarting its averages;
/// captures keep the unsmoothed curve, are served smoothed as the live curve was, and can be
/// re-smoothed at any time; averages combine the unsmoothed columns.
#[test]
fn live_smoothing_and_resmoothed_captures() {
    let mut r = rig("smoothing");
    let c = &mut r.c;
    let settled = settle(&mut r.d, c, &r.sub, r.tok, 0);
    // Restarted averages would leave the deep (low-frequency) stages settling again: only
    // 0.1 s of audio runs after the change.
    let settling = |f: &ac2_proto::Frame| match &f.data {
        FrameData::Tf(t) => t
            .validity
            .iter()
            .filter(|m| m.contains(ac2_proto::frame::ValidityMask::SETTLING))
            .count(),
        _ => unreachable!(),
    };
    let settling_before = settling(&settled);

    // Live change: same job, averages kept, new rev, frames say so.
    let mut cfg = transfer("main");
    if let MeasKind::Transfer { config } = &mut cfg.kind {
        config.smoothing = Some(sixth());
    }
    let m = match c.update_ok(MeasId(1), cfg) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    assert!(m.running);
    run(&mut r.d, 0.1);
    c.ok(Command::GenRefresh { lease_token: r.tok });
    let first = r
        .sub
        .frame(T, |f| {
            matches!(f.data, FrameData::Tf(_)) && f.stamp.config_rev.0 >= m.config_rev.0
        })
        .expect("frame with the new smoothing");
    let FrameData::Tf(tf) = &first.data else {
        unreachable!()
    };
    assert_eq!(tf.meta.smoothing, Some(sixth()));
    assert!(
        settling(&first) <= settling_before,
        "averages restarted: {} columns settling, {settling_before} before",
        settling(&first)
    );
    let shown = settle(&mut r.d, c, &r.sub, r.tok, m.config_rev.0);
    let FrameData::Tf(shown) = shown.data else {
        unreachable!()
    };

    // The capture shows what the live curve showed…
    let a = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "a".into(),
        slot: Some(1),
    }));
    assert_eq!(a.edit.smoothing, Some(sixth()));
    let smoothed = data(c, a.id);
    // Live frames carry hundredths of a dB (docs/protocol.md §5.2): within 0.005 dB.
    for i in band() {
        assert!(
            (smoothed.mag_db[i] - shown.mag[i]).abs() < 0.006,
            "{} Hz: {} vs live {}",
            grid_freq(i),
            smoothed.mag_db[i],
            shown.mag[i]
        );
    }
    // …but holds it unsmoothed: switched off, the curve is rougher; switched back, the same.
    let a = set_smoothing(c, &a, None);
    let raw = data(c, a.id);
    // The live frame's magnitude and phase are the raw columns smoothed by ac2-core: phase
    // is smoothed by default, and the capture holds exactly what the frame was made from.
    let (want_mag, want_phase) = core_smoothed(&raw, sixth());
    let mut moved: f64 = 0.0;
    let mut compared = 0;
    for i in 0..raw.mag_db.len() {
        if !(shown.phase[i].is_finite() && raw.phase_deg.as_ref().unwrap()[i].is_finite()) {
            continue;
        }
        compared += 1;
        let p = f64::from(shown.phase[i]);
        assert!(
            deg_diff(p, want_phase[i]) < 0.05,
            "{} Hz: live phase {p} vs ac2-core {}",
            grid_freq(i),
            want_phase[i]
        );
        assert!((f64::from(shown.mag[i]) - want_mag[i]).abs() < 0.006);
        moved = moved.max(deg_diff(p, f64::from(raw.phase_deg.as_ref().unwrap()[i])));
    }
    assert!(compared > 300, "{compared} columns compared");
    assert!(
        moved > 1.0,
        "phase was not smoothed: at most {moved}° from raw"
    );
    assert!(
        roughness(&raw.mag_db) > 2.0 * roughness(&smoothed.mag_db),
        "{} vs {}",
        roughness(&raw.mag_db),
        roughness(&smoothed.mag_db)
    );
    let a = set_smoothing(c, &a, Some(sixth()));
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&data(c, a.id).mag_db), bits(&smoothed.mag_db));

    // A second capture of the same result; their average uses the unsmoothed columns and
    // starts with the smoothing the inputs share.
    let b = trace(c.ok(Command::TraceCapture {
        meas: MeasId(1),
        name: "b".into(),
        slot: Some(2),
    }));
    let avg = trace(c.ok(Command::TraceAverage {
        traces: vec![a.id, b.id],
        method: AverageMethod::Power,
        reference: DelayReference::Trace { trace: a.id },
        name: "avg".into(),
    }));
    assert_eq!(avg.edit.smoothing, Some(sixth()));
    let avg = set_smoothing(c, &avg, None);
    let ad = data(c, avg.id);
    for i in band() {
        assert!(
            (ad.mag_db[i] - raw.mag_db[i]).abs() < 1e-3,
            "{}",
            ad.mag_db[i]
        );
    }

    // Smoothing is a protected edit and applies to transfer curves only.
    let mut locked = a.edit.clone();
    locked.locked = true;
    c.ok(Command::TraceUpdate {
        trace: a.id,
        edit: locked.clone(),
    });
    locked.smoothing = None;
    let e = c
        .call(Command::TraceUpdate {
            trace: a.id,
            edit: locked,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused);
    let tgt = trace(c.ok(Command::TraceImport {
        file_name: "house_curve.txt".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Target,
        ir: None,
        content: Blob(fixture("house_curve.txt")),
        owner: ac2_proto::model::TraceOwner::Imported,
    }));
    let mut te = tgt.edit.clone();
    te.smoothing = Some(sixth());
    let e = c
        .call(Command::TraceUpdate {
            trace: tgt.id,
            edit: te,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    drop(r.backend);
    r.h.shutdown();
}
