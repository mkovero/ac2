use super::*;
use ac2_proto::units::Samples;

fn texts(b: &[Banner]) -> Vec<&str> {
    b.iter().map(|b| b.text.as_str()).collect()
}

pub(crate) fn everything() -> Status {
    Status {
        daemon_silence_s: 3.2,
        protection: ProtectionFlags::CLIP
            .with(ProtectionFlags::NO_REFERENCE)
            .with(ProtectionFlags::CHECK_ROUTING)
            .with(ProtectionFlags::NO_SIGNAL),
        frame_age_s: Some(4.25),
        stale_after_s: None,
        timing: Some(TimingState::Jumped {
            from: Samples(480),
            to: Samples(-512),
        }),
        clock_drift_ppm: Some(-52.4),
        audio_stopped: None,
        no_delay_estimate: Some(NoDelayEstimate::NotFound),
        math: None,
        log_not_saved: None,
        file_damaged: None,
        drive: Default::default(),
    }
}

#[test]
fn a_log_not_saved_is_a_warning_under_the_measurement_warnings_a_damaged_file_last() {
    let log = crate::autosave::LogFailureText {
        banner: "LOG NOT SAVED · disk full".into(),
        detail: "SPL log of Main not saved for 12 min: disk full; retried every 30 s".into(),
    };
    let b = banners(&Status {
        log_not_saved: Some(log.clone()),
        file_damaged: Some(crate::autosave::DamageText {
            banner: "FILE DAMAGED · rig settings".into(),
            detail: "rig settings rig.json damaged: torn".into(),
        }),
        no_delay_estimate: Some(NoDelayEstimate::NotFound),
        clock_drift_ppm: Some(3.24),
        ..Status::default()
    });
    assert_eq!(
        texts(&b),
        [
            "CLOCK DRIFT · 3.2 ppm",
            "LOG NOT SAVED · disk full",
            "NO DELAY ESTIMATE",
            "FILE DAMAGED · rig settings"
        ]
    );
    assert_eq!(b[1].severity, Severity::Warning);
    assert_eq!(b[3].severity, Severity::Info);
    assert_eq!(b[1].detail.as_deref(), Some(log.detail.as_str()));
}

#[test]
fn clock_drift_alone_names_the_value_and_what_it_spares() {
    let b = banners(&Status {
        clock_drift_ppm: Some(3.24),
        timing: Some(TimingState::Locked {
            offset: Samples(2000),
        }),
        ..Status::default()
    });
    assert_eq!(texts(&b), ["CLOCK DRIFT · 3.2 ppm"]);
    assert_eq!(
        b[0].detail.as_deref(),
        Some("output and input clocks differ (0.03 ms per 10 s); loopback TF unaffected")
    );
    assert_eq!(drift_ppm(150.4), "150 ppm");
    assert_eq!(drift_ppm(-9.94), "9.9 ppm");
}

#[test]
fn audio_stopped_follows_the_daemon_and_replaces_stale() {
    use ac2_proto::model::{AudioStopped, Recovery, StopCause};
    use ac2_proto::units::WallNs;
    let stopped = AudioStopped {
        since: WallNs(1_791_232_572_000_000_000),
        cause: StopCause::NotDelivering { after_ms: 1000 },
        recovery: Recovery::Opening {
            attempt: 1,
            started: WallNs(1_791_232_573_000_000_000),
        },
    };
    let b = banners(&Status {
        daemon_silence_s: 2.0,
        frame_age_s: Some(4.0),
        protection: ProtectionFlags::NO_SIGNAL,
        audio_stopped: Some(crate::audio::audio_stopped_text(
            &stopped,
            WallNs(1_791_232_574_000_000_000),
            |_| 0,
        )),
        ..Status::default()
    });
    assert_eq!(
        texts(&b),
        [
            "DAEMON NOT RESPONDING",
            "AUDIO STOPPED · device not delivering since 20:36",
            "NO SIGNAL"
        ]
    );
    assert_eq!(b[1].severity, Severity::Fault);
    assert_eq!(
        b[1].detail.as_deref(),
        Some("reopening the audio I/O (attempt 1) · measurements paused")
    );
}

#[test]
fn healthy_shows_nothing() {
    assert!(banners(&Status::default()).is_empty());
    let s = Status {
        daemon_silence_s: 1.5,
        frame_age_s: Some(1.0),
        timing: Some(TimingState::Locked { offset: Samples(3) }),
        ..Status::default()
    };
    assert!(banners(&s).is_empty());
}

#[test]
fn priority_order_and_text() {
    let b = banners(&everything());
    assert_eq!(
        texts(&b),
        [
            "DAEMON NOT RESPONDING",
            "CLIP",
            "NO REFERENCE",
            "CHECK ROUTING",
            "NO SIGNAL",
            "STALE · 4.2 s",
            "OUTPUT TIMING JUMP",
            "CLOCK DRIFT · 52 ppm",
            "NO DELAY ESTIMATE"
        ]
    );
    assert_eq!(
        b[7].detail.as_deref(),
        Some("output and input clocks differ (0.52 ms per 10 s); loopback TF unaffected")
    );
    assert_eq!(b[7].severity, Severity::Warning);
    assert_eq!(b[0].detail.as_deref(), Some("no keepalive for 3.2 s"));
    assert_eq!(
        b[6].detail.as_deref(),
        Some("loopback offset 480 → −512 samples")
    );
    assert_eq!(b[0].severity, Severity::Fault);
    assert_eq!(b[3].severity, Severity::Fault);
    assert_eq!(b[5].severity, Severity::Warning);
    assert_eq!(b[8].severity, Severity::Info);
    // Severity never increases down the list.
    assert!(b.windows(2).all(|w| w[0].severity <= w[1].severity));
}

#[test]
fn check_routing_ranks_right_after_no_reference() {
    let only = |p: ProtectionFlags| {
        texts(&banners(&Status {
            protection: p,
            ..Status::default()
        }))
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
    };
    assert_eq!(only(ProtectionFlags::CHECK_ROUTING), ["CHECK ROUTING"]);
    assert_eq!(
        only(
            ProtectionFlags::NO_SIGNAL
                .with(ProtectionFlags::CHECK_ROUTING)
                .with(ProtectionFlags::NO_REFERENCE)
        ),
        ["NO REFERENCE", "CHECK ROUTING", "NO SIGNAL"]
    );
    let b = banners(&Status {
        protection: ProtectionFlags::CHECK_ROUTING,
        ..Status::default()
    });
    assert_eq!(b[0].severity, Severity::Fault);
    assert!(
        b[0].detail
            .as_deref()
            .is_some_and(|d| d.contains("input patch"))
    );
    // Other flags raise nothing.
    assert!(only(ProtectionFlags::WEAK_REFERENCE.with(ProtectionFlags::DISCONTINUITY)).is_empty());
}

#[test]
fn no_delay_estimate_says_why() {
    use ac2_proto::model::{
        DelayBand, DelayConfidence, DelayFinding, DelayOutcome, DelayState, MeasConfig, Resolution,
        TfAveraging, TransferConfig,
    };
    use ac2_proto::units::{MeasId, Rev, Seconds, WallNs};
    let refused = DelayFinding {
        outcome: DelayOutcome::NoEstimate {
            reasons: vec![NoEstimateReason::LowPsr, NoEstimateReason::LowBandSnr],
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
        band: DelayBand::Sub,
        observation: Seconds(4.0),
        candidates: vec![],
        found_at: WallNs(0),
    };
    let mut m = Measurement {
        id: MeasId(1),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: "tf".into(),
            kind: MeasKind::Transfer {
                config: TransferConfig {
                    reference_input: 0,
                    measurement_input: 1,
                    averaging: TfAveraging::Fifo { blocks: 4 },
                    resolution: Resolution::Twelfth,
                    smoothing: None,
                    depth: ac2_proto::model::DepthPolicy::EqualConfidence,
                },
            },
        },
        config_rev: Rev(1),
        running: true,
        delay: Some(DelayState {
            applied: Seconds(0.0),
            applied_samples: 0.0,
            nudged: Seconds(0.0),
            nudged_samples: 0.0,
            tracking: false,
            awaiting_pick: false,
            last_finding: Some(refused),
        }),
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
    };
    let why = no_delay_estimate(&m);
    assert_eq!(
        why,
        Some(NoDelayEstimate::Refused(vec![
            NoEstimateReason::LowPsr,
            NoEstimateReason::LowBandSnr
        ]))
    );
    let b = banners(&Status {
        no_delay_estimate: why,
        ..Status::default()
    });
    assert_eq!(b[0].text, "NO DELAY ESTIMATE");
    assert_eq!(
        b[0].detail.as_deref(),
        Some("finder: no clear peak, too noisy in band")
    );
    // No finding and not tracking: the operator's delay stands.
    if let Some(d) = &mut m.delay {
        d.last_finding = None;
    }
    assert_eq!(no_delay_estimate(&m), None);
    if let Some(d) = &mut m.delay {
        d.tracking = true;
    }
    assert_eq!(no_delay_estimate(&m), Some(NoDelayEstimate::NotFound));
    // An ambiguous finding nobody picked from: tracking is paused, and says so.
    if let Some(d) = &mut m.delay {
        d.awaiting_pick = true;
    }
    let why = no_delay_estimate(&m);
    assert_eq!(why, Some(NoDelayEstimate::AwaitingPick));
    let b = banners(&Status {
        no_delay_estimate: why,
        ..Status::default()
    });
    assert_eq!(b[0].text, "NO DELAY ESTIMATE");
    assert_eq!(
        b[0].detail.as_deref(),
        Some("ambiguous: pick a candidate or set the delay · tracking paused")
    );
}

#[test]
fn stale_threshold() {
    let at = |age| {
        texts(&banners(&Status {
            frame_age_s: Some(age),
            ..Status::default()
        }))
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
    };
    assert!(at(1.0).is_empty());
    assert_eq!(at(1.04), ["STALE · 1.0 s"]);
    assert_eq!(at(75.0), ["STALE · 1 min"]);
    // A once-a-second stream's frame 1.5 s old is between frames, not stale.
    let once = |age| {
        banners(&Status {
            frame_age_s: Some(age),
            stale_after_s: Some(3.0),
            ..Status::default()
        })
    };
    assert!(once(1.5).is_empty());
    assert_eq!(texts(&once(3.5)), ["STALE · 3.5 s"]);
}

#[test]
fn stacking_and_overflow() {
    let b = banners(&everything());
    let area = Rect::new(0.0, 0.0, 800.0, 400.0);
    let rows = layout_banners(&b, area);
    assert_eq!(rows.len(), MAX_BANNERS);
    assert_eq!(rows[0].text, "DAEMON NOT RESPONDING");
    assert_eq!(rows[1].text, "CLIP");
    assert_eq!(rows[2].text, "+7 more");
    assert_eq!(rows[2].severity, Severity::Fault);
    assert_eq!(
        rows[2].detail.as_deref(),
        Some(
            "NO REFERENCE · CHECK ROUTING · NO SIGNAL · STALE · 4.2 s · OUTPUT TIMING JUMP · CLOCK DRIFT · 52 ppm · NO DELAY ESTIMATE"
        )
    );
    // Stacked downwards without overlap, centred, capped width.
    for w in rows.windows(2) {
        assert!(w[1].rect.y >= w[0].rect.bottom() + BANNER_GAP - 1e-3);
    }
    assert_eq!(rows[0].rect.w, BANNER_MAX_WIDTH);
    assert!((rows[0].rect.x + rows[0].rect.w / 2.0 - 400.0).abs() < 1e-3);
    assert_eq!(rows[0].rect.y, BANNER_PAD);

    // Exactly MAX_BANNERS fit without an overflow row.
    let rows = layout_banners(&b[..3], area);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].text, "NO REFERENCE");

    // A short pane holds fewer rows.
    let short = Rect::new(
        0.0,
        0.0,
        300.0,
        2.0 * BANNER_PAD + 2.0 * BANNER_HEIGHT + BANNER_GAP,
    );
    let rows = layout_banners(&b, short);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].text, "+8 more");
    assert_eq!(rows[0].rect.w, 284.0);
    assert!(layout_banners(&b, Rect::new(0.0, 0.0, 300.0, 20.0)).is_empty());
}

#[test]
fn strip_height_follows_the_rows() {
    let size = Viewport {
        width: 800.0,
        height: 600.0,
    };
    let none = banner_strip(&[], 48.0, 740.0, size);
    assert!(none.rows.is_empty());
    assert_eq!(none.rect.h, 0.0);
    let b = banners(&everything());
    let one = banner_strip(&b[..1], 48.0, 740.0, size);
    assert_eq!(one.rect.h, 2.0 * BANNER_PAD + BANNER_HEIGHT);
    let all = banner_strip(&b, 48.0, 740.0, size);
    assert_eq!(all.rows.len(), MAX_BANNERS);
    assert_eq!(
        all.rect.h,
        2.0 * BANNER_PAD + 3.0 * BANNER_HEIGHT + 2.0 * BANNER_GAP
    );
    // Rows are centred over the given span and inside the strip.
    for r in &all.rows {
        assert!((r.rect.x + r.rect.w / 2.0 - (48.0 + 370.0)).abs() < 1e-3);
        assert!(r.rect.y >= all.rect.y && r.rect.bottom() <= all.rect.bottom());
    }
    // A short view gives the strip at most half its height.
    let short = banner_strip(
        &b,
        0.0,
        300.0,
        Viewport {
            width: 300.0,
            height: 100.0,
        },
    );
    assert!(short.rect.h <= 50.0);
    assert_eq!(short.rows.len(), 1);
    assert_eq!(short.rows[0].text, "+9 more");
}

#[test]
fn draw_uses_severity_colours() {
    let theme = Theme::dark();
    let rows = layout_banners(&banners(&everything()), Rect::new(0.0, 0.0, 800.0, 400.0));
    let mut layer = Layer::default();
    draw_banners(&mut layer, &rows, &theme);
    assert_eq!(layer.rects.len(), 3);
    assert_eq!(layer.rects[0].color, theme.banner_fault.background);
    assert_eq!(layer.labels[0].text, "DAEMON NOT RESPONDING");
    assert_eq!(layer.labels[0].color, theme.banner_fault.text);
}

/// A long headline (the audio host's stop with its time) shortens part by part in a
/// narrow pane, as the SPL pane at its default split is, and never leaves its row.
#[test]
fn long_headlines_shorten_to_their_row() {
    let theme = Theme::dark();
    let text = "AUDIO STOPPED · audio host ended the stream at 22:36";
    let s = Status {
        audio_stopped: Some(crate::audio::AudioStoppedText {
            banner: text.into(),
            detail: "reopening (attempt 2) · measurements paused".into(),
            bar: [String::new(), String::new()],
            status: String::new(),
        }),
        ..Status::default()
    };
    let mut seen = Vec::new();
    for w in [200.0, 300.0, 360.0, 420.0, 600.0, 900.0, 1300.0] {
        let rows = layout_banners(&banners(&s), Rect::new(0.0, 0.0, w, 200.0));
        let mut layer = Layer::default();
        draw_banners(&mut layer, &rows, &theme);
        let l = &layer.labels[0];
        let right = l.pos[0] + crate::canvas::text_width(&l.text, l.size);
        assert!(
            right <= rows[0].rect.right() - 10.0 + 0.01,
            "{w}: {}",
            l.text
        );
        assert_eq!(rows[0].text, text);
        seen.push(l.text.clone());
    }
    assert_eq!(seen[0], "AUDIO STOPPED");
    assert_eq!(seen.last().map(String::as_str), Some(text));
    assert_eq!(fit_headline("AUDIO STOPPED", 60.0, 14.0), "AUDIO…");
}

/// NO REFERENCE's detail follows this app's stimulus, and every variant fits beside the
/// banner text in a full-width row in every theme.
#[test]
fn no_reference_reminds_of_the_stimulus_keys() {
    use crate::stimulus::Drive;
    let detail = |drive: Drive| {
        let s = Status {
            protection: ProtectionFlags::NO_REFERENCE,
            drive: drive.clone(),
            ..Status::default()
        };
        let b = banners(&s);
        assert_eq!(b[0].text, "NO REFERENCE");
        let d = b[0].detail.clone().unwrap_or_default();
        for theme in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
            let rows = layout_banners(&b, Rect::new(0.0, 0.0, 800.0, 200.0));
            let mut layer = Layer::default();
            draw_banners(&mut layer, &rows, &theme);
            assert!(
                layer.labels.iter().any(|l| l.text == d),
                "{d} not drawn at {} px",
                theme.font_size
            );
        }
        d
    };
    assert_eq!(
        detail(Drive::Idle {
            arm: "Space".into(),
            fire: "Enter".into(),
        }),
        "stimulus off: Space arms, Enter starts it"
    );
    assert_eq!(
        detail(Drive::Armed {
            fire: "Enter".into(),
        }),
        "stimulus armed: Enter starts it"
    );
    assert_eq!(
        detail(Drive::SweepView),
        "stimulus off: arm it from a transfer pane"
    );
    assert_eq!(
        detail(Drive::Playing),
        "reference silent: check the loopback cable"
    );
}

#[test]
fn narrow_rows_drop_the_detail() {
    let theme = Theme::dark();
    let s = Status {
        no_delay_estimate: Some(NoDelayEstimate::NotFound),
        ..Status::default()
    };
    let draw = |w: f32| {
        let rows = layout_banners(&banners(&s), Rect::new(0.0, 0.0, w, 200.0));
        let mut layer = Layer::default();
        draw_banners(&mut layer, &rows, &theme);
        (rows, layer)
    };
    let (rows, wide) = draw(800.0);
    assert_eq!(wide.labels.len(), 2);
    assert!(rows[0].detail.is_some());
    // Too narrow for both: the banner text stays, the detail goes (it stays in the
    // row data for other uses).
    let (rows, narrow) = draw(300.0);
    assert_eq!(narrow.labels.len(), 1);
    assert_eq!(narrow.labels[0].text, "NO DELAY ESTIMATE");
    assert!(rows[0].detail.is_some());
}

/// A math average without enough positions is a fault right under the signal faults;
/// one missing positions is a warning after STALE; a full one says nothing.
#[test]
fn math_average_banners() {
    use crate::math::MathStatus;
    use ac2_proto::frame::{MathState, OperandState, OperandStatus as MemberStatus};
    use ac2_proto::model::{MathConfig, MathDomain, Operand, PhaseBasis};
    use ac2_proto::units::MeasId;
    let avg = |s: &[MemberStatus]| {
        let of: Vec<Operand> = (1..=s.len() as u32)
            .map(|m| Operand::Meas { meas: MeasId(m) })
            .collect();
        let c = MathConfig::power_average(
            ac2_proto::model::TraceOwner::Imported,
            MathDomain::Transfer,
            of.clone(),
        );
        let a = MathState {
            operands: of
                .into_iter()
                .zip(s)
                .map(|(operand, status)| OperandState {
                    operand,
                    status: *status,
                })
                .collect(),
            phase: PhaseBasis::SharedTimeBase,
        };
        Some((
            "Audience".to_string(),
            MathStatus::new(&c, &a, |o| match o {
                Operand::Meas { meas } => format!("Seat {}", meas.0),
                Operand::Trace { trace } => format!("trace {trace}"),
            }),
        ))
    };
    let base = Status {
        protection: ProtectionFlags::NO_SIGNAL,
        frame_age_s: Some(2.0),
        ..Status::default()
    };
    let none = Status {
        math: avg(&[MemberStatus::Included, MemberStatus::Stopped]),
        ..base.clone()
    };
    let b = banners(&none);
    assert_eq!(
        texts(&b),
        [
            "NO SIGNAL",
            "NO AVERAGE · 1 OF 2 POSITIONS",
            "STALE · 2.0 s"
        ]
    );
    assert_eq!(b[1].severity, Severity::Fault);
    assert_eq!(
        b[1].detail.as_deref(),
        Some("Audience: left out Seat 2: stopped")
    );
    let short = Status {
        math: avg(&[
            MemberStatus::Included,
            MemberStatus::Included,
            MemberStatus::Settling,
        ]),
        ..base.clone()
    };
    let b = banners(&short);
    assert_eq!(
        texts(&b),
        ["NO SIGNAL", "STALE · 2.0 s", "AVERAGE · 2 OF 3 POSITIONS"]
    );
    assert_eq!(b[2].severity, Severity::Warning);
    let full = Status {
        math: avg(&[MemberStatus::Included; 3]),
        ..base
    };
    assert_eq!(texts(&banners(&full)), ["NO SIGNAL", "STALE · 2.0 s"]);
}
