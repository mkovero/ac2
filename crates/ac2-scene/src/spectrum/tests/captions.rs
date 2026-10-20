use super::*;

#[test]
fn calibration_captions() {
    const H: u64 = 3_600_000_000_000;
    let now = WallNs(100 * H);
    assert_eq!(cal_caption(CalStatus::Uncalibrated, None, now), "");
    assert_eq!(
        cal_caption(CalStatus::Uncalibrated, Some("mic curve: MM1 90°"), now),
        " · mic curve: MM1 90°"
    );
    let at = WallNs(97 * H - 59_000_000_000);
    assert_eq!(
        cal_caption(
            CalStatus::Verified {
                calibrated_at: at,
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0)
                }
            },
            None,
            now
        ),
        " · cal 94 dB · 3 h ago"
    );
    assert_eq!(
        cal_caption(
            CalStatus::Verified {
                calibrated_at: at,
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0)
                }
            },
            Some("mic curve off"),
            now
        ),
        " · cal 94 dB · 3 h ago · mic curve off"
    );
    assert_eq!(
        cal_caption(
            CalStatus::Verified {
                calibrated_at: WallNs(100 * H - 600_000_000_000),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0)
                }
            },
            None,
            now
        ),
        " · cal 94 dB · 10 min ago"
    );
    // A calibration stamped after the frame (clock stepped back) is not in the future.
    assert_eq!(
        cal_caption(
            CalStatus::Verified {
                calibrated_at: WallNs(101 * H),
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0)
                }
            },
            None,
            now
        ),
        " · cal 94 dB · just now"
    );
    // The mismatch says so instead of an age: the age would be another mic's.
    assert_eq!(
        cal_caption(
            CalStatus::OtherMicOrInput {
                calibrated_at: at,
                basis: ac2_proto::model::CalBasis::Acoustic {
                    calibrator_level: ac2_proto::units::DbSpl(94.0)
                }
            },
            Some("mic curve: MM1 0°"),
            now
        ),
        " · cal from other mic / input · mic curve: MM1 0°"
    );
}

#[test]
fn live_traces_carry_the_calibration_in_their_caption() {
    use ac2_proto::frame::{RtaMeta, SpecMeta};
    const H: u64 = 3_600_000_000_000;
    let cal = CalStatus::Verified {
        calibrated_at: WallNs(H),
        basis: ac2_proto::model::CalBasis::Acoustic {
            calibrator_level: ac2_proto::units::DbSpl(94.0),
        },
    };
    let rta = RtaFrame {
        meas: MeasId(1),
        meta: RtaMeta {
            fraction: BandFraction::Third,
            weighting: Weighting::A,
            scale: LevelScale::DbSpl,
            cal,
            mic_curve: true,
            math: None,
        },
        level: vec![],
        validity: vec![],
    };
    let fresh = Freshness::from_age(0.0);
    let t = SpectrumTrace::rta(
        &rta,
        WallNs(3 * H),
        Some("mic curve: MM1 90°"),
        &[],
        &[],
        "RTA",
        Color::WHITE,
        fresh,
    );
    assert_eq!(
        t.caption,
        "1/3 oct · A-weighted · cal 94 dB · 2 h ago · mic curve: MM1 90°"
    );
    let spec = SpecFrame {
        meas: MeasId(2),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::DbSpl,
            level: SpecLevel::Tone,
            cal,
            mic_curve: false,
            smoothing: None,
            math: None,
            bands: None,
        },
        level: vec![],
    };
    let t = SpectrumTrace::spectrum(
        &spec,
        WallNs(25 * H),
        None,
        &[],
        &[],
        "FFT",
        Color::WHITE,
        fresh,
    );
    assert_eq!(t.caption, "Hann window · cal 94 dB · 1 d ago");
    assert_eq!(t.quantity, Quantity::Tone);
    let mut smoothed = spec.clone();
    smoothed.meta.smoothing = Some(SmoothingFraction::Sixth);
    let t = SpectrumTrace::spectrum(
        &smoothed,
        WallNs(25 * H),
        None,
        &[],
        &[],
        "FFT",
        Color::WHITE,
        fresh,
    );
    assert_eq!(t.caption, "Hann window · cal 94 dB · 1 d ago");
    assert_eq!(t.quantity, Quantity::SmoothedTone(SmoothingFraction::Sixth));
}

#[test]
fn unit_labels() {
    use BinWidth as B;
    let bin = B::Hz(48_000.0 / 32_768.0);
    assert_eq!(
        level_unit(LevelScale::Dbfs, Quantity::Tone, bin),
        "dBFS per 1.46 Hz bin (tone)"
    );
    assert_eq!(
        level_unit(LevelScale::Dbfs, Quantity::Tone, B::Unknown),
        "dBFS (tone)"
    );
    assert_eq!(
        level_unit(LevelScale::Dbfs, Quantity::Band, B::Unknown),
        "dBFS (band)"
    );
    assert_eq!(
        level_unit(LevelScale::DbSpl, Quantity::Band, B::Unknown),
        "dB SPL (band)"
    );
    assert_eq!(
        level_unit(
            LevelScale::Dbfs,
            Quantity::SmoothedTone(SmoothingFraction::Sixth),
            B::Hz(48_000.0 / 4096.0)
        ),
        "dBFS per 11.7 Hz bin (tone, 1/6 oct smoothed)"
    );
    assert_eq!(
        level_unit_forms(
            LevelScale::DbSpl,
            Quantity::narrow(SpecLevel::Tone, Some(SmoothingFraction::Third)),
            bin
        ),
        [
            "dB SPL per 1.46 Hz bin (tone, 1/3 oct smoothed)",
            "dB SPL per 1.46 Hz bin (1/3 oct smoothed)",
            "dB SPL per 1.46 Hz bin",
            "dB SPL/bin",
            "dB SPL",
        ]
    );
    assert_eq!(
        level_unit_forms(LevelScale::Dbfs, Quantity::Tone, B::Mixed),
        [
            "dBFS per bin, mixed widths (tone)",
            "dBFS per bin, mixed widths",
            "dBFS/bin",
            "dBFS",
        ]
    );
    assert_eq!(
        level_unit_forms(LevelScale::DbSpl, Quantity::Band, B::Unknown),
        ["dB SPL (band)", "dB SPL"]
    );
    assert_eq!(fraction_label(BandFraction::TwentyFourth), "1/24 oct");
    assert_eq!(window_label(Window::FlatTop), "flat-top window");
}

#[test]
fn mixed_units_are_flagged() {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = vec![-40.0f32; f.len()];
    let a = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    let b = rta_trace(&f, &e, &level, LevelScale::DbSpl);
    let s = spectrum_scene(
        &[a, b],
        &Status::default(),
        &ViewState::default(),
        &Theme::dark(),
        SIZE,
    );
    assert_eq!(s.unit, "mixed units");
}

/// A spectrum's axis names its bin width; a narrower pane shortens the unit to what
/// fits beside the caption, down to the bare scale, and never runs it into the caption.
#[test]
fn tone_unit_names_the_bin_and_fits_narrow_panes() {
    let g = GridDef::LogBins {
        fs: Hz(48_000.0),
        n: 32_768,
        ppo: 96,
    };
    let cols = crate::grid::columns(&g);
    let level = vec![-60.0f32; cols.freqs.len()];
    let mut t = SpectrumTrace {
        key: TraceKey::Stored(ac2_proto::units::TraceId(1)),
        name: "Main L".into(),
        color: Color::WHITE,
        freqs: &cols.freqs,
        edges: &cols.edges,
        level: &level,
        drawn: None,
        validity: None,
        peak: None,
        tracked: None,
        scale: LevelScale::DbSpl,
        quantity: Quantity::narrow(SpecLevel::Tone, Some(SmoothingFraction::Third)),
        bin_hz: cols.bin_hz,
        caption: "Hann window".into(),
        freshness: None,
        offset_db: 0.0,
        selected: false,
        compared: false,
        bands: None,
        style: ac2_proto::model::SpectrumStyle::Line,
        layer: 0,
    };
    let theme = Theme::dark();
    let scene = |traces: &[SpectrumTrace<'_>], w: f32| {
        spectrum_scene(
            traces,
            &Status::default(),
            &ViewState::default(),
            &theme,
            Viewport {
                width: w,
                height: 300.0,
            },
        )
    };
    let wide = scene(std::slice::from_ref(&t), 900.0);
    assert_eq!(wide.unit, "dB SPL per 1.46 Hz bin (tone, 1/3 oct smoothed)");
    let help = wide.unit_help.as_deref().expect("help");
    assert!(help.contains("1.46 Hz") && help.contains("RTA"), "{help}");
    let mut seen = Vec::new();
    for w in [900.0, 480.0, 440.0, 400.0, 330.0, 260.0, 200.0, 120.0] {
        let s = scene(std::slice::from_ref(&t), w);
        assert_unit_clear_of_caption(&s, w);
        seen.push((s.unit, s.caption));
    }
    seen.dedup();
    let u = |a: &str, c: &str| (a.to_string(), c.to_string());
    assert_eq!(
        seen,
        [
            u(
                "dB SPL per 1.46 Hz bin (tone, 1/3 oct smoothed)",
                "Hann window"
            ),
            u("dB SPL per 1.46 Hz bin (1/3 oct smoothed)", "Hann window"),
            u("dB SPL per 1.46 Hz bin", "Hann window"),
            u("dB SPL/bin", "Hann window"),
            // The window goes before the unit shrinks to the bare scale.
            u("dB SPL/bin", ""),
            u("dB SPL", ""),
        ]
    );
    // Spectra on two FFT lengths share the axis as per bin, mixed widths.
    let g2 = GridDef::LogBins {
        fs: Hz(48_000.0),
        n: 4096,
        ppo: 96,
    };
    let cols2 = crate::grid::columns(&g2);
    let level2 = vec![-60.0f32; cols2.freqs.len()];
    let mut u = t.clone();
    u.freqs = &cols2.freqs;
    u.edges = &cols2.edges;
    u.level = &level2;
    u.bin_hz = cols2.bin_hz;
    t.quantity = Quantity::Tone;
    u.quantity = Quantity::Tone;
    let s = scene(&[t, u], 900.0);
    assert_eq!(s.unit, "dB SPL per bin, mixed widths (tone)");
}

/// A density reads per hertz on any FFT length or resolution: spectra on two FFT
/// lengths share one axis, a tone and a density spectrum do not.
#[test]
fn density_unit_is_per_hertz_on_any_resolution() {
    use BinWidth as B;
    assert_eq!(
        level_unit_forms(LevelScale::Dbfs, Quantity::Density, B::MultiRes),
        [
            "dBFS/Hz (density, multi-res)",
            "dBFS/Hz (density)",
            "dBFS/Hz"
        ]
    );
    assert_eq!(
        level_unit_forms(
            LevelScale::DbSpl,
            Quantity::narrow(SpecLevel::Density, Some(SmoothingFraction::Sixth)),
            B::Hz(1.46)
        ),
        [
            "dB SPL/Hz (density, 1/6 oct smoothed)",
            "dB SPL/Hz (density)",
            "dB SPL/Hz"
        ]
    );
    assert_eq!(
        value_unit(LevelScale::DbSpl, Quantity::Density),
        "dB SPL/Hz"
    );
    assert_eq!(value_unit(LevelScale::Dbfs, Quantity::Tone), "dBFS");

    let cols = |n: u32| {
        crate::grid::columns(&GridDef::LogBins {
            fs: Hz(48_000.0),
            n,
            ppo: 96,
        })
    };
    let (a, b) = (cols(65_536), cols(4096));
    let (la, lb) = (vec![-90.0f32; a.freqs.len()], vec![-90.0f32; b.freqs.len()]);
    fn trace<'a>(c: &'a crate::grid::GridColumns, l: &'a [f32], id: u32) -> SpectrumTrace<'a> {
        SpectrumTrace {
            key: TraceKey::Stored(ac2_proto::units::TraceId(id)),
            name: format!("S{id}"),
            color: Color::WHITE,
            freqs: &c.freqs,
            edges: &c.edges,
            level: l,
            drawn: None,
            validity: None,
            peak: None,
            tracked: None,
            scale: LevelScale::Dbfs,
            quantity: Quantity::Density,
            bin_hz: c.bin_hz,
            caption: "Hann window".into(),
            freshness: None,
            offset_db: 0.0,
            selected: false,
            compared: false,
            bands: None,
            style: ac2_proto::model::SpectrumStyle::Line,
            layer: 0,
        }
    }
    let (ta, tb) = (trace(&a, &la, 1), trace(&b, &lb, 2));
    let scene = |traces: &[SpectrumTrace<'_>]| {
        spectrum_scene(
            traces,
            &Status::default(),
            &ViewState::default(),
            &Theme::dark(),
            SIZE,
        )
    };
    let s = scene(&[ta.clone(), tb.clone()]);
    assert_eq!(s.unit, "dBFS/Hz (density)");
    let help = s.unit_help.as_deref().expect("help");
    assert!(
        help.contains("per hertz") && help.contains("3.4 dB"),
        "{help}"
    );
    let mut tone = tb;
    tone.quantity = Quantity::Tone;
    assert_eq!(scene(&[ta, tone]).unit, "mixed units");
}

/// The unit's label and the drawn caption never overlap.
fn assert_unit_clear_of_caption(s: &SpectrumScene, w: f32) {
    if s.caption.is_empty() {
        return;
    }
    let caption = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .find(|l| l.text == s.caption)
        .expect("caption");
    let cap = crate::canvas::tests::label_box(caption);
    assert!(
        s.unit_rect.right() < cap.x,
        "{w}: {:?} vs {:?} at {cap:?}",
        s.unit,
        s.caption
    );
}

/// The rig's calibration caption (an electrical calibration with its data sheet and a
/// mic curve) beside the per-bin unit: from 300 to 1300 px the two never overlap; the
/// caption gives up the data sheet, then the window, then its tail, and the unit keeps
/// at least `dB SPL/bin`.
#[test]
fn a_long_calibration_caption_leaves_the_unit_room() {
    let g = GridDef::LogBins {
        fs: Hz(48_000.0),
        n: 32_768,
        ppo: 96,
    };
    let cols = crate::grid::columns(&g);
    let level = vec![-60.0f32; cols.freqs.len()];
    let rig = "Hann window · electrical cal 1 kHz (data sheet 15.0 mV/Pa) ±1 dB · 2 d ago \
               · mic curve: MM1 34804 90°";
    let t = SpectrumTrace {
        key: TraceKey::Stored(ac2_proto::units::TraceId(1)),
        name: "Mic".into(),
        color: Color::WHITE,
        freqs: &cols.freqs,
        edges: &cols.edges,
        level: &level,
        drawn: None,
        validity: None,
        peak: None,
        tracked: None,
        scale: LevelScale::DbSpl,
        quantity: Quantity::Tone,
        bin_hz: cols.bin_hz,
        caption: rig.into(),
        freshness: None,
        offset_db: 0.0,
        selected: false,
        compared: false,
        bands: None,
        style: ac2_proto::model::SpectrumStyle::Line,
        layer: 0,
    };
    let theme = Theme::dark();
    let mut captions = Vec::new();
    for w in (300..=1300).step_by(50) {
        let w = w as f32;
        let size = Viewport {
            width: w,
            height: 300.0,
        };
        let s = spectrum_scene(
            std::slice::from_ref(&t),
            &Status::default(),
            &ViewState::default(),
            &theme,
            size,
        );
        assert_unit_clear_of_caption(&s, w);
        assert_ne!(s.unit, "dB SPL", "{w}");
        captions.push(s.caption);
    }
    captions.dedup();
    assert_eq!(captions.first().map(String::as_str), Some(""));
    assert!(
        captions.contains(&"electrical cal 1 kHz ±1 dB".to_string()),
        "{captions:?}"
    );
    assert_eq!(captions.last().map(String::as_str), Some(rig));
    assert_eq!(
        caption_forms(rig),
        [
            rig,
            "Hann window · electrical cal 1 kHz ±1 dB · 2 d ago · mic curve: MM1 34804 90°",
            "electrical cal 1 kHz ±1 dB · 2 d ago · mic curve: MM1 34804 90°",
            "electrical cal 1 kHz ±1 dB · 2 d ago",
            "electrical cal 1 kHz ±1 dB",
            "",
        ]
    );
}
