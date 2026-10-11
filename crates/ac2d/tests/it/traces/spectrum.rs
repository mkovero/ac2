//! Spectrum measurements: FFT length, window, smoothing, multi-resolution and display columns versus captured bins.

use super::*;

/// A running spectrum's FFT length and window change by `meas.update`: the job restarts
/// on the new bin grid and its average starts afresh from the update.
#[test]
fn spectrum_fft_length_and_window_change_restart_it() {
    let mut r = rig("spec-fft");
    let (mut c2, sub) = connect(&r.h, &[b"d/2/spec"]);
    let spec = |fft_len, window| MeasConfig {
        name: "fft".into(),
        kind: MeasKind::Spectrum {
            config: SpectrumConfig {
                resolution: ac2_proto::model::SpecResolution::Fixed { fft_len },
                window,
                averaging: SpecAveraging::Exponential {
                    time_constant: Seconds(2.0),
                },
                ..SpectrumConfig::on_input(1)
            },
        },
    };
    c2.ok(Command::MeasCreate {
        config: spec(4096, Window::Hann),
    });
    c2.ok(Command::MeasStart { meas: MeasId(2) });
    run(&mut r.d, 0.5);
    let old = sub
        .frame(T, |f| matches!(f.data, FrameData::Spec(_)))
        .expect("spec frame");
    let at_update = end_sample(&r.d);
    let m = match c2.ok(Command::MeasUpdate {
        meas: MeasId(2),
        config: spec(1024, Window::FlatTop),
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    assert!(m.running);
    for _ in 0..3 {
        run(&mut r.d, 0.3);
        r.c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let f = sub
        .frame(T, |f| {
            matches!(f.data, FrameData::Spec(_)) && f.stamp.config_rev.0 >= m.config_rev.0
        })
        .expect("spec frame of the new length");
    assert_ne!(f.stamp.grid_id, old.stamp.grid_id, "a new bin grid");
    assert!(
        f.stamp.config_applied_at.0 >= at_update,
        "the average starts at the update: {} < {at_update}",
        f.stamp.config_applied_at.0
    );
    let FrameData::Spec(live) = f.data else {
        unreachable!()
    };
    assert_eq!(live.meta.window, Window::FlatTop);
    // An out-of-range length is refused and the spectrum stays as it is.
    let e = c2
        .call(Command::MeasUpdate {
            meas: MeasId(2),
            config: spec(131_072, Window::Hann),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(e.msg.contains("power of two in 64..=65536"), "{e:?}");
    drop(r.backend);
    r.h.shutdown();
}

/// A narrowband spectrum's smoothing changes live; frames say it is applied; a capture
/// holds the unsmoothed bins, starts with the measurement's smoothing and is served as the
/// live frame showed it; the live frame is the ac2-core linear-bin smoother of the raw bins.
#[test]
fn spectrum_smoothing_live_and_captured() {
    let mut r = rig("spec-smoothing");
    let (mut c2, sub) = connect(&r.h, &[b"d/2/spec"]);
    let spec = |smoothing| MeasConfig {
        name: "fft".into(),
        kind: MeasKind::Spectrum {
            config: SpectrumConfig {
                resolution: ac2_proto::model::SpecResolution::Fixed { fft_len: 4096 },
                smoothing,
                ..SpectrumConfig::on_input(1)
            },
        },
    };
    c2.ok(Command::MeasCreate { config: spec(None) });
    c2.ok(Command::MeasStart { meas: MeasId(2) });
    run(&mut r.d, 0.5);
    let m = match c2.ok(Command::MeasUpdate {
        meas: MeasId(2),
        config: spec(Some(SmoothingFraction::Sixth)),
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    assert!(m.running);
    for _ in 0..3 {
        run(&mut r.d, 0.3);
        r.c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    // A spectrum is stamped where its frame ended: one hop or less before the newest audio.
    let end = end_sample(&r.d) - u64::from(SpectrumConfig::display_hop(4096, FS));
    let f = sub
        .frame(T, |f| {
            matches!(f.data, FrameData::Spec(_))
                && f.stamp.config_rev.0 >= m.config_rev.0
                && f.stamp.audio_sample.0 + 1 >= end
        })
        .expect("smoothed spec frame");
    let FrameData::Spec(live) = f.data else {
        unreachable!()
    };
    assert_eq!(live.meta.smoothing, Some(SmoothingFraction::Sixth));

    let t = trace(c2.ok(Command::TraceCapture {
        meas: MeasId(2),
        name: "fft".into(),
        slot: None,
    }));
    let power = Smoothing {
        fraction: SmoothingFraction::Sixth,
        mode: SmoothingMode::Magnitude,
    };
    assert_eq!(t.edit.smoothing, Some(power));
    // Every bin, on the FFT's own grid.
    let bins = ac2_proto::GridDef::Linear {
        fs: Hz(f64::from(FS)),
        n: 4096,
    };
    assert_eq!(t.grid_id, bins.id());
    let served = data(&mut c2, t.id);
    let t = set_smoothing(&mut c2, &t, None);
    let raw = data(&mut c2, t.id);
    let p: Vec<f64> = raw
        .mag_db
        .iter()
        .map(|l| 10f64.powf(f64::from(*l) / 10.0))
        .collect();
    let valid: Vec<bool> = raw.mag_db.iter().map(|l| l.is_finite()).collect();
    let want = ac2_core::smoothing::LinearSmoother::new(
        raw.mag_db.len(),
        ac2_core::smoothing::SmoothingFraction::Sixth,
    )
    .smooth(&p, &valid);
    // The live frame is the smoothed bins gathered into display columns, each the highest
    // level among its bins; the capture keeps every bin and smooths to the same curve.
    let cols = ac2_proto::BinColumns::new(f64::from(FS), 4096, 96);
    let log_bins = ac2_proto::GridDef::LogBins {
        fs: Hz(f64::from(FS)),
        n: 4096,
        ppo: 96,
    };
    assert_eq!(f.stamp.grid_id, Some(log_bins.id()));
    assert_eq!(live.level.len(), cols.len());
    assert_eq!(raw.mag_db.len(), 2049);
    for (c, w) in cols.first_bin.windows(2).enumerate() {
        let bins = w[0] as usize..w[1] as usize;
        let top = |v: &dyn Fn(usize) -> f64| {
            bins.clone()
                .filter(|&k| k > 0 && valid[k])
                .map(v)
                .fold(f64::NAN, f64::max)
        };
        let want_c = top(&|k| 10.0 * want[k].log10());
        if want_c.is_nan() {
            continue;
        }
        // Live levels go in hundredths of a dB.
        assert!(
            (f64::from(live.level[c]) - want_c).abs() < 0.006,
            "column {c}: live {} vs ac2-core {want_c}",
            live.level[c]
        );
        let served_c = top(&|k| f64::from(served.mag_db[k]));
        assert!((served_c - want_c).abs() < 1e-3, "column {c}");
    }
    let (mut rough_raw, mut rough_live) = (0.0, 0.0);
    for k in 101..raw.mag_db.len() {
        if valid[k] && valid[k - 1] {
            rough_raw += f64::from(raw.mag_db[k] - raw.mag_db[k - 1]).powi(2);
            rough_live += f64::from(served.mag_db[k] - served.mag_db[k - 1]).powi(2);
        }
    }
    assert!(rough_live * 10.0 < rough_raw, "{rough_live} vs {rough_raw}");

    // Re-smoothed at another width; either mode, as a spectrum has no phase.
    let third = Smoothing {
        fraction: SmoothingFraction::Third,
        mode: SmoothingMode::MagnitudePhase,
    };
    let t = set_smoothing(&mut c2, &t, Some(third));
    assert_ne!(data(&mut c2, t.id).mag_db, served.mag_db);
    drop(r.backend);
    r.h.shutdown();
}

/// A spectrum switched from one FFT to multi-resolution by `meas.update`: frames move to
/// the log column grid and name the bands; a capture keeps the unsmoothed columns on the
/// same grid and, re-smoothed at the measurement's setting, reads as the live frame did.
#[test]
fn multires_spectrum_live_and_captured() {
    use ac2_proto::model::{Resolution, SpecResolution};
    let mut r = rig("spec-multires");
    let (mut c2, sub) = connect(&r.h, &[b"d/2/spec"]);
    let spec = |resolution, smoothing| MeasConfig {
        name: "multi".into(),
        kind: MeasKind::Spectrum {
            config: SpectrumConfig {
                resolution,
                smoothing,
                ..SpectrumConfig::on_input(1)
            },
        },
    };
    c2.ok(Command::MeasCreate {
        config: spec(SpecResolution::Fixed { fft_len: 4096 }, None),
    });
    c2.ok(Command::MeasStart { meas: MeasId(2) });
    run(&mut r.d, 0.3);
    let multi = SpecResolution::Multi {
        grid: Resolution::TwentyFourth,
    };
    let m = match c2.ok(Command::MeasUpdate {
        meas: MeasId(2),
        config: spec(multi, Some(SmoothingFraction::Sixth)),
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    for _ in 0..4 {
        run(&mut r.d, 0.3);
        r.c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let g = ac2_core::multires::MultiResLadder::grid(f64::from(FS), 24);
    let log = ac2_proto::GridDef::Log {
        ppo: 24,
        k_min: g.k_min,
        k_max: g.k_max,
    };
    // The longest window (32 768 samples at 48 kHz) has had time for spectra.
    let f = sub
        .frame(T, |f| {
            matches!(&f.data, FrameData::Spec(s) if f.stamp.config_rev.0 >= m.config_rev.0
                && s.meta.bands.as_ref().is_some_and(|b| b.bands[0].age.is_some()))
        })
        .expect("multi-resolution frame");
    assert_eq!(f.stamp.grid_id, Some(log.id()));
    let FrameData::Spec(live) = f.data else {
        unreachable!()
    };
    assert_eq!(live.level.len(), g.len());
    let bands = live.meta.bands.expect("bands");
    assert_eq!(bands.bands.len(), 6);
    assert!((bands.bands[0].window.0 - 32_768.0 / 48_000.0).abs() < 1e-9);
    assert!((bands.bands[5].window.0 - 1024.0 / 48_000.0).abs() < 1e-9);
    assert_eq!(bands.unresolved.resolution, Resolution::TwentyFourth);

    let t = trace(c2.ok(Command::TraceCapture {
        meas: MeasId(2),
        name: "multi".into(),
        slot: None,
    }));
    assert_eq!(t.grid_id, log.id());
    let served = data(&mut c2, t.id);
    assert_eq!(served.mag_db.len(), g.len());
    let t = set_smoothing(&mut c2, &t, None);
    let raw = data(&mut c2, t.id);
    assert_ne!(raw.mag_db, served.mag_db, "the capture is unsmoothed");
    // A FIFO deeper than the bands' memory bound is refused.
    let e = c2
        .call(Command::MeasUpdate {
            meas: MeasId(2),
            config: MeasConfig {
                name: "multi".into(),
                kind: MeasKind::Spectrum {
                    config: SpectrumConfig {
                        resolution: multi,
                        averaging: SpecAveraging::Fifo { frames: 1000 },
                        ..SpectrumConfig::on_input(1)
                    },
                },
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(e.msg.contains("multi-resolution"), "{e:?}");
    drop(r.backend);
    r.h.shutdown();
}

/// A default 65 536-point spectrum publishes display columns, each the highest bin level in
/// it, and captures every bin: the capture of the last frame holds exactly the bins the
/// frame's columns were gathered from, and a tone between bins keeps its level live.
#[test]
fn spectrum_live_columns_are_the_captured_bins_maxima() {
    let mut r = rig("spec-columns");
    let (mut c2, sub) = connect(&r.h, &[b"d/2/spec"]);
    r.c.ok(Command::GenSet {
        lease_token: r.tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Sine { freq: Hz(1234.5) },
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    c2.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "fft".into(),
            kind: MeasKind::Spectrum {
                config: SpectrumConfig::on_input(1),
            },
        },
    });
    c2.ok(Command::MeasStart { meas: MeasId(2) });
    for _ in 0..4 {
        run(&mut r.d, 0.5);
        r.c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    // No more audio: the last frame published is the one a capture stores.
    let mut last = None;
    while let Some(f) = sub.frame(Duration::from_millis(500), |f| {
        matches!(f.data, FrameData::Spec(_))
    }) {
        last = Some(f);
    }
    let f = last.expect("spec frame");
    let FrameData::Spec(live) = f.data else {
        unreachable!()
    };
    let t = trace(c2.ok(Command::TraceCapture {
        meas: MeasId(2),
        name: "fft".into(),
        slot: None,
    }));
    let bins = data(&mut c2, t.id).mag_db;
    assert_eq!(bins.len(), 32_769);
    let cols = ac2_proto::BinColumns::new(f64::from(FS), 65_536, 96);
    assert_eq!(live.level.len(), cols.len());
    // Live columns go in hundredths of a dB (docs/protocol.md §5.3); the capture is formed
    // in the daemon from the analysis itself, never from a frame, and keeps every bin's f32.
    let on_wire_grid = |v: f32| v.is_finite() && f32::from((v * 100.0).round() as i16) / 100.0 == v;
    let finite: Vec<f32> = bins.iter().copied().filter(|v| v.is_finite()).collect();
    assert!(
        finite.iter().filter(|v| !on_wire_grid(**v)).count() > finite.len() / 2,
        "the capture lost precision"
    );
    for (c, w) in cols.first_bin.windows(2).enumerate() {
        let top = bins[w[0] as usize..w[1] as usize]
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .fold(f32::NAN, f32::max);
        assert!(
            (live.level[c] - top).abs() <= 0.005 + 1e-4 || (live.level[c].is_nan() && top.is_nan()),
            "column {c}: {} vs {top}",
            live.level[c]
        );
    }
    // The tone: its peak bin, half a bin off centre, reads the same live.
    let peak = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert!((peak(&live.level) - peak(&bins)).abs() <= 0.005 + 1e-4);
    assert!(peak(&bins) > -40.0, "{}", peak(&bins));
    drop(r.backend);
    r.h.shutdown();
}
