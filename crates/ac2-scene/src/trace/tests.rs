use super::*;

#[test]
fn mic_wording() {
    use ac2_proto::model::MicCurveRef;
    use ac2_proto::units::{Hz, WallNs};
    let c = MicCurveRef {
        label: "90°".into(),
        file_name: "449350_34804_90Grad.txt".into(),
        content_hash: "0".into(),
        points: 2,
        f_lo: Hz(20.0),
        f_hi: Hz(20_000.0),
        imported_at: WallNs(0),
        stated_sensitivity: None,
    };
    let m = MicState {
        name: "MM1 34804".into(),
        curve: Some(c.clone()),
    };
    assert_eq!(
        mic_text(Some(&m), None),
        "MM1 34804 (curve 90° in the columns, file 449350_34804_90Grad.txt)"
    );
    assert_eq!(
        curve_note(Some(&m), None).as_deref(),
        Some("mic curve: MM1 34804 90°")
    );
    let a = TraceMicCurve {
        mic: "MM1 34804".into(),
        curve: MicCurveRef {
            label: "0°".into(),
            ..c
        },
        f_norm: Hz(1000.0),
    };
    assert_eq!(
        mic_text(None, Some(&a)),
        "MM1 34804 (curve 0° applied after capture, 0 dB at 1000 Hz, file \
         449350_34804_90Grad.txt)"
    );
    assert_eq!(
        curve_note(None, Some(&a)).as_deref(),
        Some("mic curve: MM1 34804 0°")
    );
    assert_eq!(mic_text(None, None), "—");
    assert_eq!(curve_note(None, None), None);
}

const EPOCH: SessionEpoch = SessionEpoch(3);

fn log_freqs(n: usize) -> Vec<f64> {
    // 20 Hz … 20 kHz, 48 per octave-ish.
    (0..n)
        .map(|i| 20.0 * 1000f64.powf(i as f64 / (n - 1) as f64))
        .collect()
}

/// Phase a TF with the reference delayed by `comp` shows for a path that is a pure
/// delay `tau`: −360·f·(tau − comp), wrapped.
fn measured_phase(freqs: &[f64], tau: f64, comp: f64) -> Vec<f32> {
    freqs
        .iter()
        .map(|f| wrap_deg(-360.0 * f * (tau - comp)) as f32)
        .collect()
}

struct Data {
    freqs: Vec<f64>,
    mag: Vec<f32>,
    phase: Vec<f32>,
    coh: Vec<f32>,
    validity: Vec<ValidityMask>,
}

impl Data {
    fn delay(n: usize, tau: f64, comp: f64) -> Self {
        let freqs = log_freqs(n);
        Self {
            phase: measured_phase(&freqs, tau, comp),
            mag: vec![0.0; n],
            coh: vec![1.0; n],
            validity: vec![ValidityMask::NONE; n],
            freqs,
        }
    }

    fn trace(&self, key: u32, time_base: TimeBase) -> TfTrace<'_> {
        TfTrace {
            key: TraceKey::Live(MeasId(key)),
            name: format!("m{key}"),
            color: Color::WHITE,
            freqs: &self.freqs,
            mag_db: &self.mag,
            phase_deg: Some(&self.phase),
            coherence: Some(&self.coh),
            validity: Some(&self.validity),
            offset_db: 0.0,
            polarity: Polarity::Normal,
            nudge: Seconds(0.0),
            delay_nudge: Seconds(0.0),
            time_base,
            freshness: None,
            smoothing: None,
            note: None,
            stored: None,
            selected: false,
            compared: false,
            unresolved: None,
        }
    }
}

fn shared(delay: f64) -> TimeBase {
    TimeBase::Shared {
        epoch: EPOCH,
        delay: Seconds(delay),
    }
}

#[test]
fn wrap() {
    assert_eq!(wrap_deg(180.0), 180.0);
    assert_eq!(wrap_deg(-180.0), 180.0);
    assert_eq!(wrap_deg(190.0), -170.0);
    assert_eq!(wrap_deg(-190.0), 170.0);
    assert_eq!(wrap_deg(720.0), 0.0);
}

/// Decision 8b / Q8 test: two otherwise identical paths with different physical
/// delays, each measured with its own delay compensated, show their relative phase in
/// shared mode.
#[test]
fn identical_paths_show_relative_delay() {
    let (t1, t2) = (0.010, 0.0115); // 10 ms and 11.5 ms
    let a = Data::delay(200, t1, t1);
    let b = Data::delay(200, t2, t2);
    // Each trace on its own is flat: perfectly compensated.
    assert!(a.phase.iter().chain(&b.phase).all(|p| p.abs() < 1e-3));
    let traces = [a.trace(1, shared(t1)), b.trace(2, shared(t2))];
    let (r, d) = display_traces(
        &traces,
        &DisplayCache::default(),
        None,
        &CoherenceStyle::default(),
    );
    assert_eq!(r.map(|r| r.key), Some(TraceKey::Live(MeasId(1))));
    assert_eq!(d[0].relation, PhaseRelation::Reference);
    assert_eq!(d[1].relation, PhaseRelation::Relative);
    assert!((d[1].shift_s - 0.0015).abs() < 1e-12);
    for (i, f) in d[1].freqs.iter().enumerate() {
        let want = wrap_deg(-360.0 * f * (t2 - t1));
        let got = d[1].phase_wrapped_deg[i];
        assert!(wrap_deg(got - want).abs() < 1e-3, "{f} Hz: {got} vs {want}");
        assert!(d[0].phase_wrapped_deg[i].abs() < 1e-3);
        // Group delay reads the 1.5 ms arrival difference; an end column of this
        // coarse grid has one neighbour, too few to fit.
        if i == 0 || i + 1 == d[1].freqs.len() {
            assert!(d[1].group_delay_s[i].is_nan());
            continue;
        }
        assert!((d[1].group_delay_s[i] - 0.0015).abs() < 1e-6, "{f}");
        assert!(d[0].group_delay_s[i].abs() < 1e-6);
    }
    // At 1 kHz a 1.5 ms lag is −540° ≡ 180°.
    let k = crate::grid::nearest_column(&d[1].freqs, 1000.0).expect("col");
    let want = wrap_deg(-360.0 * d[1].freqs[k] * 0.0015);
    assert!((d[1].phase_wrapped_deg[k] - want).abs() < 1e-3);
    // Picking the other trace as reference flips the sign.
    let (_, d) = display_traces(
        &traces,
        &DisplayCache::default(),
        Some(TraceKey::Live(MeasId(2))),
        &CoherenceStyle::default(),
    );
    assert!((d[0].shift_s + 0.0015).abs() < 1e-12);
    assert!((d[0].group_delay_s[50] + 0.0015).abs() < 1e-6);
    assert_eq!(d[1].relation, PhaseRelation::Reference);
}

#[test]
fn nudge_and_independent() {
    let a = Data::delay(100, 0.01, 0.01);
    let b = Data::delay(100, 0.02, 0.02);
    let mut tb = b.trace(2, shared(0.02));
    tb.nudge = Seconds(0.0005);
    let ta = a.trace(1, shared(0.01));
    let (_, d) = display_traces(
        &[ta.clone(), tb],
        &DisplayCache::default(),
        None,
        &CoherenceStyle::default(),
    );
    // Δ = τ_k − τ_ref − ν = 10 ms − 0.5 ms.
    assert!((d[1].shift_s - 0.0095).abs() < 1e-12);
    // Independent (imported) trace keeps its own alignment, nudge only.
    let mut ti = b.trace(3, TimeBase::Independent);
    ti.nudge = Seconds(0.001);
    let (_, d) = display_traces(
        &[ta.clone(), ti],
        &DisplayCache::default(),
        None,
        &CoherenceStyle::default(),
    );
    assert_eq!(d[1].relation, PhaseRelation::Independent);
    assert!((d[1].shift_s + 0.001).abs() < 1e-12);
    assert!((d[1].group_delay_s[10] + 0.001).abs() < 1e-6);
    // Another epoch is independent too.
    let to = b.trace(
        4,
        TimeBase::Shared {
            epoch: SessionEpoch(9),
            delay: Seconds(0.02),
        },
    );
    let (_, d) = display_traces(
        &[ta, to],
        &DisplayCache::default(),
        None,
        &CoherenceStyle::default(),
    );
    assert_eq!(d[1].relation, PhaseRelation::Independent);
    assert_eq!(d[1].shift_s, 0.0);
}

#[test]
fn reference_falls_back_to_first_shared() {
    let a = Data::delay(10, 0.0, 0.0);
    let ti = a.trace(1, TimeBase::Independent);
    let ts = a.trace(2, shared(0.004));
    let traces = [ti, ts];
    // Wanting an independent trace cannot give a delay: first shared one is used.
    let r = resolve_reference(&traces, Some(TraceKey::Live(MeasId(1)))).expect("ref");
    assert_eq!(r.key, TraceKey::Live(MeasId(2)));
    assert_eq!(r.delay, Seconds(0.004));
    assert!(resolve_reference(&traces[..1], None).is_none());
}

#[test]
fn polarity_offset_and_rotation_leave_magnitude() {
    let a = Data::delay(50, 0.001, 0.001);
    let mut t = a.trace(1, shared(0.001));
    t.offset_db = -6.0;
    t.polarity = Polarity::Inverted;
    let d = display_trace(&t, None, &CoherenceStyle::default());
    assert!(d.magnitude_db.iter().all(|m| *m == -6.0));
    assert!(
        d.phase_wrapped_deg
            .iter()
            .all(|p| (p.abs() - 180.0).abs() < 1e-3)
    );
    assert!(d.inverted);
}

#[test]
fn stored_display_math_is_kept_until_what_it_depends_on_changes() {
    let mut raw = crate::distortion::tests::data();
    let n = raw.mag_db.len();
    raw.phase_deg = Some((0..n).map(|i| (i as f32 * 37.0) % 360.0 - 180.0).collect());
    raw.coherence = Some((0..n).map(|i| (i % 10) as f32 / 10.0).collect());
    let data = Arc::new(raw);
    let freqs = crate::distortion::tests::grid().centres();
    let live = Data::delay(n, 0.001, 0.001);
    let cache = DisplayCache::default();
    let style = CoherenceStyle {
        blank_below: Some(0.3),
        ..CoherenceStyle::default()
    };
    // NaN gaps make `==` useless on whole traces; their debug text compares them.
    let fresh = |traces: &[TfTrace<'_>], style: &CoherenceStyle| {
        format!(
            "{:?}",
            display_traces(traces, &DisplayCache::default(), None, style)
        )
    };
    let kept = |traces: &[TfTrace<'_>], style: &CoherenceStyle| {
        format!("{:?}", display_traces(traces, &cache, None, style))
    };
    let mut traces = vec![
        live.trace(1, shared(0.001)),
        TfTrace::stored(&data, &freqs, Color::WHITE),
    ];
    let first = kept(&traces, &style);
    assert_eq!(first, fresh(&traces, &style));
    assert_eq!(cache.len(), 1);
    // Unchanged: the kept result, identical to working it out again.
    assert_eq!(kept(&traces, &style), first);
    // Every input it depends on lays it out again.
    traces[1].offset_db = 3.0;
    traces[1].name = "renamed".into();
    let edited = kept(&traces, &style);
    assert_ne!(edited, first);
    assert_eq!(edited, fresh(&traces, &style));
    assert!(edited.contains("renamed"));
    traces[1].polarity = Polarity::Inverted;
    traces[1].nudge = Seconds(0.0005);
    assert_eq!(kept(&traces, &style), fresh(&traces, &style));
    // The reference moves: its delay changes the stored trace's rotation.
    traces[0] = live.trace(1, shared(0.002));
    assert_eq!(kept(&traces, &style), fresh(&traces, &style));
    let other = CoherenceStyle::default();
    assert_eq!(kept(&traces, &other), fresh(&traces, &other));
    // New data for the trace (a re-fetch) replaces the kept result.
    let mut refetched = (*data).clone();
    refetched.mag_db[9] = 12.0;
    let refetched = Arc::new(refetched);
    traces[1] = TfTrace::stored(&refetched, &freqs, Color::WHITE);
    assert_eq!(kept(&traces, &other), fresh(&traces, &other));
    let (_, d) = display_traces(&traces, &cache, None, &other);
    assert_eq!(d[1].magnitude_db[9], 12.0);
    assert_eq!(cache.len(), 1);
    // A trace no longer shown is forgotten.
    traces.pop();
    display_traces(&traces, &cache, None, &other);
    assert!(cache.is_empty());
}

#[test]
fn invalid_columns_are_gaps_everywhere() {
    let mut a = Data::delay(20, 0.0, 0.0);
    a.validity[5] = ValidityMask::SETTLING;
    a.mag[7] = f32::NAN;
    let d = display_trace(&a.trace(1, shared(0.0)), None, &CoherenceStyle::default());
    for i in [5, 7] {
        assert!(d.magnitude_db[i].is_nan());
        assert!(d.phase_wrapped_deg[i].is_nan());
        assert!(d.group_delay_s[i].is_nan());
        assert!(d.coherence[i].is_nan());
    }
    assert!(d.magnitude_db[6].is_finite());
    // Group delay needs three columns in its span: on this coarse grid the span is the
    // two neighbours, so a column next to a gap has none.
    assert!(d.group_delay_s[3].is_finite() && d.group_delay_s[9].is_finite());
    for i in [4, 6, 8] {
        assert!(d.group_delay_s[i].is_nan(), "{i}");
    }
}

#[test]
fn coherence_blanking_and_alpha() {
    let style = CoherenceStyle {
        blank_below: Some(0.5),
        alpha: true,
        alpha_floor: 0.2,
        alpha_full_at: 0.9,
    };
    assert_eq!(coherence_alpha(0.5, &style), 0.2);
    assert_eq!(coherence_alpha(0.9, &style), 1.0);
    assert_eq!(coherence_alpha(0.95, &style), 1.0);
    assert!((coherence_alpha(0.7, &style) - 0.6).abs() < 1e-6);
    assert_eq!(coherence_alpha(f64::NAN, &style), 1.0);
    let off = CoherenceStyle {
        alpha: false,
        ..style
    };
    assert_eq!(coherence_alpha(0.1, &off), 1.0);
    let default = CoherenceStyle::default();
    assert_eq!(coherence_alpha(0.0, &default), 0.15);

    let mut a = Data::delay(10, 0.0, 0.0);
    a.coh[3] = 0.3;
    let d = display_trace(&a.trace(1, shared(0.0)), None, &style);
    // Blanked: gap in magnitude and phase, coherence itself still shown.
    assert!(d.magnitude_db[3].is_nan() && d.phase_wrapped_deg[3].is_nan());
    assert!((d.coherence[3] - 0.3).abs() < 1e-6);
    assert!(d.magnitude_db[2].is_finite());
    assert_eq!(d.alpha[2], 1.0);
}

#[test]
fn unwrap_follows_a_long_delay() {
    // 2 ms uncompensated: phase falls 720° per kHz; unwrapping recovers the line.
    let a = Data::delay(400, 0.002, 0.0);
    let d = display_trace(
        &a.trace(1, TimeBase::Independent),
        None,
        &CoherenceStyle::default(),
    );
    let i0 = 0;
    for i in 1..200 {
        let want = -360.0 * (d.freqs[i] - d.freqs[i0]) * 0.002;
        let got = d.phase_unwrapped_deg[i] - d.phase_unwrapped_deg[i0];
        assert!((got - want).abs() < 1e-2, "{}: {got} vs {want}", d.freqs[i]);
        assert!((d.group_delay_s[i] - 0.002).abs() < 1e-6);
    }
}

/// A live TF frame of a pure-delay path `tau` measured with the reference delayed by
/// `applied`, of which `nudged` came from the operator's delay steps.
fn live_frame(freqs: &[f64], tau: f64, applied: f64, nudged: f64) -> TfFrame {
    let n = freqs.len();
    TfFrame {
        meas: MeasId(1),
        meta: ac2_proto::frame::TfMeta {
            delay: Seconds(applied),
            nudged: Seconds(nudged),
            smoothing: None,
            mic_curve: false,
            math: None,
            unresolved: None,
        },
        mag: vec![0.0; n],
        phase: measured_phase(freqs, tau, applied),
        coh: vec![1.0; n],
        validity: vec![ValidityMask::NONE; n],
    }
}

/// Drawn phase of every curve: a live measurement (arrival 10 ms) with its capture and a
/// sweep run (arrival 12 ms), before and after the measurement's delay is stepped by
/// `step`, with the reference `wanted`.
fn stepped(wanted: Option<TraceKey>, step: f64) -> (Vec<DisplayTrace>, Vec<DisplayTrace>) {
    let (tau, run_tau) = (0.010, 0.012);
    let cap = Data::delay(300, tau, tau);
    let run = Data::delay(300, run_tau, run_tau);
    let mut stamp = ac2_proto::samples::stamp(None);
    stamp.session_epoch = EPOCH;
    let draw = |frame: &TfFrame| {
        let live = TfTrace::live(
            frame,
            &stamp,
            &cap.freqs,
            "TF 2",
            Color::WHITE,
            Freshness::Fresh { age_s: 0.0 },
        );
        let mut c = cap.trace(9, shared(tau));
        c.key = TraceKey::Stored(TraceId(1));
        let mut r = run.trace(9, shared(run_tau));
        r.key = TraceKey::Stored(TraceId(2));
        display_traces(
            &[live, c, r],
            &DisplayCache::default(),
            wanted,
            &CoherenceStyle::default(),
        )
        .1
    };
    let before = draw(&live_frame(&cap.freqs, tau, tau, 0.0));
    let after = draw(&live_frame(&cap.freqs, tau, tau + step, step));
    (before, after)
}

/// The measurement's delay stepped later moves its live curve by e^{+jωΔ} — what a
/// display nudge `ν = Δ` does to a trace — and no stored curve, with the live curve as
/// the reference and with a stored sweep run as the reference. Earlier moves it back.
#[test]
fn a_delay_step_moves_the_live_curve_alone() {
    for step in [0.000_2, -0.000_05] {
        // What a display nudge of the step does to a curve.
        let plain = Data::delay(300, 0.010, 0.010);
        let mut t = plain.trace(1, shared(0.010));
        let draw = |t: &TfTrace<'_>| {
            display_traces(
                std::slice::from_ref(t),
                &DisplayCache::default(),
                None,
                &CoherenceStyle::default(),
            )
            .1
            .remove(0)
        };
        let unnudged = draw(&t);
        t.nudge = Seconds(step);
        let nudged = draw(&t);
        for wanted in [None, Some(TraceKey::Stored(TraceId(2)))] {
            let (before, after) = stepped(wanted, step);
            let r = if wanted.is_some() { 2 } else { 0 };
            assert_eq!(after[r].relation, PhaseRelation::Reference, "{wanted:?}");
            for (i, f) in before[0].freqs.iter().enumerate() {
                let moved =
                    wrap_deg(after[0].phase_wrapped_deg[i] - before[0].phase_wrapped_deg[i]);
                let want = wrap_deg(360.0 * f * step);
                assert!(
                    wrap_deg(moved - want).abs() < 1e-3,
                    "{wanted:?} {f} Hz: {moved} vs {want}"
                );
                let by_nudge =
                    wrap_deg(nudged.phase_wrapped_deg[i] - unnudged.phase_wrapped_deg[i]);
                assert!(wrap_deg(moved - by_nudge).abs() < 1e-3, "{f} Hz");
            }
            for k in [1, 2] {
                assert_eq!(after[k].shift_s.to_bits(), before[k].shift_s.to_bits());
                assert_eq!(
                    after[k].phase_wrapped_deg, before[k].phase_wrapped_deg,
                    "{wanted:?}"
                );
            }
            // The live curve's arrival against the reference is unchanged; the step
            // draws as a nudge.
            assert_eq!(after[0].shift_s.to_bits(), before[0].shift_s.to_bits());
        }
    }
}

/// A path whose group delay is known exactly: a second-order Butterworth high-pass at
/// 3 Hz and 3.7 µs of delay, measured with a phase ripple of 0.03° and period 1.1 Hz
/// (an echo 0.9 s out, the size the sweep's LF phase shows). The neighbour difference
/// reads the ripple's own slope, up to 476 µs, next to the high-pass's 2.7 ms at 16 Hz;
/// a fit over most of a period averages it out.
#[test]
fn group_delay_fit_holds_at_low_frequencies() {
    use std::f64::consts::{PI, SQRT_2};
    let (f0, delay) = (3.0, 3.7e-6);
    let ripple = |f: f64| 0.03 * (2.0 * PI * f / 1.1).sin();
    // Phase lead of the high-pass, degrees, continuous above f0.
    let hp = |f: f64| {
        let (w, w0) = (2.0 * PI * f, 2.0 * PI * f0);
        (PI - (w * w0 * SQRT_2).atan2(w0 * w0 - w * w)).to_degrees()
    };
    let path = |f: f64| hp(f) - 360.0 * f * delay;
    let exact = |f: f64| {
        let h = 1e-4;
        -(path(f + h) - path(f - h)) / (2.0 * h) / 360.0
    };
    let freqs: Vec<f64> = (0..=240)
        .map(|k| 10.0 * 2f64.powf(f64::from(k) / 48.0))
        .collect();
    let n = freqs.len();
    let data = Data {
        phase: freqs
            .iter()
            .map(|&f| wrap_deg(path(f) + ripple(f)) as f32)
            .collect(),
        mag: vec![0.0; n],
        coh: vec![f32::NAN; n],
        validity: vec![ValidityMask::NONE; n],
        freqs,
    };
    // Worst relative error of the fit and of the neighbour difference in `lo..=hi` Hz.
    let worst = |smoothing: Option<Smoothing>, lo: f64, hi: f64| {
        let mut t = data.trace(1, TimeBase::Independent);
        t.coherence = None;
        t.smoothing = smoothing;
        let d = display_trace(&t, None, &CoherenceStyle::default());
        let u = &d.phase_unwrapped_deg;
        let (mut fit, mut central) = (0.0f64, 0.0f64);
        for i in (1..n - 1).filter(|&i| (lo..=hi).contains(&d.freqs[i])) {
            let want = exact(d.freqs[i]);
            let cd = -(u[i + 1] - u[i - 1]) / (360.0 * (d.freqs[i + 1] - d.freqs[i - 1]));
            fit = fit.max((d.group_delay_s[i] / want - 1.0).abs());
            central = central.max((cd / want - 1.0).abs());
        }
        (fit, central)
    };
    let (fit, central) = worst(None, 16.0, 25.0);
    eprintln!(
        "16–25 Hz, 1/12 oct: fit within {:.1} %, neighbour difference {:.1} %",
        fit * 100.0,
        central * 100.0
    );
    assert!(fit < 0.05, "fit {fit}");
    assert!(central > 0.10, "neighbour difference {central}");

    // A 1/6-octave phase-smoothed trace gets its group delay over 1/6 octave, which
    // spans the ripple's period further up too.
    let sixth = Smoothing {
        fraction: ac2_proto::model::SmoothingFraction::Sixth,
        mode: SmoothingMode::MagnitudePhase,
    };
    assert_eq!(group_delay_span(Some(sixth)), 6);
    let (fit, central) = worst(Some(sixth), 16.0, 50.0);
    eprintln!(
        "16–50 Hz, 1/6 oct: fit within {:.1} %, neighbour difference {:.1} %",
        fit * 100.0,
        central * 100.0
    );
    assert!(fit < 0.035, "fit {fit}");
    assert!(central > 0.20, "neighbour difference {central}");
    // Magnitude-only smoothing leaves the phase, and its span, as measured.
    let mag_only = Smoothing {
        mode: SmoothingMode::Magnitude,
        ..sixth
    };
    assert_eq!(group_delay_span(Some(mag_only)), GROUP_DELAY_SPAN_B);
    assert_eq!(group_delay_span(None), 12);
}
