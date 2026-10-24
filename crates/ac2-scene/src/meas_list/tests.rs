use super::*;
use ac2_proto::model::*;
use ac2_proto::units::*;

pub(crate) fn tf(id: u32, name: &str) -> Measurement {
    Measurement {
        id: MeasId(id),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: name.into(),
            kind: MeasKind::Transfer {
                config: TransferConfig {
                    reference_input: 0,
                    measurement_input: 1,
                    averaging: TfAveraging::Exponential {
                        time_constant: Seconds(1.0),
                    },
                    resolution: Resolution::FortyEighth,
                    smoothing: None,
                    depth: DepthPolicy::EqualConfidence,
                },
            },
        },
        config_rev: Rev(1),
        running: true,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
        eq: Vec::new(),
    }
}

fn item(m: &Measurement) -> MeasItem<'_> {
    MeasItem {
        meas: m,
        expression: None,
        offset_db: 0.0,
        inverted: false,
        hidden: false,
        color: Color::from_rgba8([0, 0, u8::try_from(m.id.0).unwrap_or(0), 255]),
    }
}

/// A measurement with its own panes says how many, last on its row.
#[test]
fn rows_say_a_measurement_has_its_own_panes() {
    let mut m = tf(2, "TF 2");
    m.delay = None;
    m.layout = Some(ac2_proto::samples::pane_layout());
    assert_eq!(
        meas_row(&item(&m), None, true).text,
        "TF  TF 2\n     running · 3 own panes"
    );
    m.layout = Some(ac2_proto::model::PaneLayout::Pane {
        pane: Box::new(ac2_proto::samples::pane_layout().panes()[0].clone()),
    });
    assert!(
        meas_row(&item(&m), None, true)
            .text
            .ends_with("· 1 own pane")
    );
}

#[test]
fn rows_say_one_delay_and_its_offset_from_the_arrival() {
    let state = |applied: f64, nudged: f64| DelayState {
        applied: Seconds(applied / 48_000.0),
        applied_samples: applied,
        nudged: Seconds(nudged / 48_000.0),
        nudged_samples: nudged,
        tracking: false,
        awaiting_pick: false,
        last_finding: None,
    };
    let mut m = tf(2, "TF 2");
    m.delay = Some(state(600.0, 0.0));
    let r = meas_row(&item(&m), None, true);
    assert_eq!(r.text, "TF  TF 2\n     running · delay 12.50 ms");
    // 0.1 ms later than the arrival.
    m.delay = Some(state(604.8, 4.8));
    let r = meas_row(&item(&m), None, true);
    assert_eq!(
        r.text,
        "TF  TF 2\n     running · delay 12.60 ms (+0.10 ms from arrival)"
    );
    // One sample earlier.
    m.delay = Some(state(599.0, -1.0));
    let r = meas_row(&item(&m), None, true);
    assert_eq!(
        r.text,
        "TF  TF 2\n     running · delay 12.479 ms (−0.021 ms from arrival)"
    );
    let mut plain = tf(3, "TF 3");
    plain.delay = None;
    assert_eq!(
        meas_row(&item(&plain), None, true).text,
        "TF  TF 3\n     running"
    );
}

#[test]
fn tf_rows_say_a_resolution_other_than_the_default() {
    let mut m = tf(4, "Room");
    let MeasKind::Transfer { config } = &mut m.config.kind else {
        unreachable!()
    };
    config.resolution = Resolution::NinetySixth;
    config.smoothing = Some(Smoothing::of(SmoothingFraction::Sixth));
    let smoothing = format::smoothing(config.smoothing);
    assert_eq!(
        meas_row(&item(&m), None, true).text,
        format!("TF  Room\n     running · resolution 1/96 oct · {smoothing}")
    );
}

#[test]
fn spectrum_and_rta_rows_say_their_averaging() {
    let mut m = tf(4, "Room");
    m.config.kind = MeasKind::Rta {
        config: RtaConfig::on_input(1, BandFraction::Third),
    };
    assert_eq!(
        meas_row(&item(&m), None, true).text,
        "RTA  Room\n     running"
    );
    m.config.kind = MeasKind::Rta {
        config: RtaConfig {
            averaging: SpecAveraging::Fifo { frames: 2400 },
            ..RtaConfig::on_input(1, BandFraction::Third)
        },
    };
    assert_eq!(
        meas_row(&item(&m), None, true).text,
        "RTA  Room\n     running · avg FIFO 2400 frames"
    );
    m.config.kind = MeasKind::Spectrum {
        config: SpectrumConfig {
            averaging: SpecAveraging::Exponential {
                time_constant: Seconds(2.0),
            },
            smoothing: Some(SmoothingFraction::Sixth),
            level: ac2_proto::model::SpecLevel::Tone,
            ..SpectrumConfig::on_input(1)
        },
    };
    assert_eq!(
        meas_row(&item(&m), None, true).text,
        "FFT  Room\n     running · avg exp τ 2 s · smoothed 1/6 oct"
    );
    if let MeasKind::Spectrum { config } = &mut m.config.kind {
        config.resolution = SpecResolution::MULTI;
    }
    assert_eq!(
        meas_row(&item(&m), None, true).text,
        "FFT  Room\n     running · multi-res 1/48 oct · tone · avg exp τ 2 s · smoothed 1/6 oct"
    );
    if let MeasKind::Spectrum { config } = &mut m.config.kind {
        config.level = ac2_proto::model::SpecLevel::Density;
    }
    assert_eq!(
        meas_row(&item(&m), None, true).text,
        "FFT  Room\n     running · multi-res 1/48 oct · density · avg exp τ 2 s · smoothed 1/6 oct"
    );
    if let MeasKind::Spectrum { config } = &mut m.config.kind {
        config.resolution = SpecResolution::Fixed { fft_len: 65_536 };
    }
    assert_eq!(
        meas_row(&item(&m), None, true).text,
        "FFT  Room\n     running · density · avg exp τ 2 s · smoothed 1/6 oct"
    );
    assert_eq!(
        format::spec_averaging(SpecAveraging::Exponential {
            time_constant: Seconds(0.125)
        }),
        "exp τ 0.125 s"
    );
    assert_eq!(
        format::spec_averaging(SpecAveraging::Fifo { frames: 1 }),
        "FIFO 1 frame"
    );
}

#[test]
fn rows_name_the_state_and_say_hidden() {
    let m = tf(2, "TF 2");
    let r = meas_row(&item(&m), Some(MeasId(2)), true);
    assert_eq!(r.text, "TF  TF 2\n     running");
    assert_eq!(r.mark, Mark::Active);
    let hidden = MeasItem {
        hidden: true,
        offset_db: 3.0,
        inverted: true,
        ..item(&m)
    };
    let r = meas_row(&hidden, Some(MeasId(2)), false);
    assert_eq!(r.text, "TF  TF 2\n     running · hidden · inv · +3.0 dB");
    assert!(r.hidden);
    assert_eq!(r.mark, Mark::Selected, "a stored trace has the keys");
    assert_eq!(meas_row(&item(&m), Some(MeasId(1)), true).mark, Mark::None);
}

#[test]
fn delete_asks_and_a_math_operand_refuses() {
    let m = tf(2, "TF 2");
    let c = delete_confirm(&m);
    assert_eq!(c.title, "Delete measurement TF 2?");
    assert_eq!(
        c.lines,
        [
            "transfer function · running",
            "Its live curve and settings go; it owns no stored traces."
        ]
    );
    assert_eq!(c.hint, "Delete or Backspace deletes it · Esc or N keeps it");
    assert!(!c.refused);
    let r = delete_refused(&m, &["Avg".to_owned()]);
    assert_eq!(r.title, "TF 2 cannot be deleted");
    assert_eq!(
        r.lines,
        [
            "transfer function · an operand of the math channel Avg",
            "Edit or delete Avg first: it computes from TF 2."
        ]
    );
    assert_eq!(r.hint, "Enter or Esc closes");
    assert!(r.refused);
    let r = delete_refused(&m, &["Avg".to_owned(), "Sum".to_owned()]);
    assert_eq!(
        r.lines[0],
        "transfer function · an operand of the math channels Avg, Sum"
    );
    assert_eq!(
        r.lines[1],
        "Edit or delete them first: they compute from TF 2."
    );
}

pub(crate) fn with_kind(id: u32, name: &str, kind: MeasKind, running: bool) -> Measurement {
    Measurement {
        id: MeasId(id),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: name.into(),
            kind,
        },
        running,
        ..tf(id, name)
    }
}

pub(crate) fn sweep_config() -> SweepConfig {
    SweepConfig {
        reference_input: 0,
        measurement_input: 1,
        outputs: vec![0],
        level: Dbfs(-50.0),
        sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
        repeats: 1,
        gate: None,
        tail: None,
        lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
        resolution: ac2_proto::model::Resolution::FortyEighth,
    }
}

pub(crate) fn trace(id: u32, name: &str, owner: TraceOwner, source: TraceSource) -> TraceMeta {
    TraceMeta {
        id: TraceId(id),
        edit: TraceEdit {
            name: name.into(),
            color: Rgb { r: 1, g: 2, b: 3 },
            visible: true,
            locked: false,
            order: id,
            offset: Db(0.0),
            polarity: Polarity::Normal,
            delay_nudge: Seconds(0.0),
            slot: None,
            smoothing: None,
            owner,
        },
        kind: if matches!(source, TraceSource::Sweep { .. }) {
            TraceKind::Sweep
        } else {
            TraceKind::Transfer
        },
        source,
        grid_id: ac2_proto::GridId(1),
        delay: Seconds(0.0),
        depth: None,
        cal: CalState::Uncalibrated,
        mic: None,
        mic_curve: None,
        created_at: WallNs(0),
        eq_in: Vec::new(),
    }
}

pub(crate) fn captured() -> TraceSource {
    TraceSource::Captured {
        meas: MeasId(1),
        meas_name: "Main L".into(),
        epoch: SessionEpoch(1),
        at_sample: SampleIndex(0),
    }
}

pub(crate) fn run(number: u32) -> TraceSource {
    TraceSource::Sweep {
        meas: MeasId(2),
        meas_name: "Genelec 1 m".into(),
        run: SweepId(number),
        number,
        epoch: SessionEpoch(1),
        sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
        level: Dbfs(-50.0),
        repeats: 1,
        lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
        reference_input: 0,
        measurement_input: 1,
    }
}

/// The operator's example: a transfer function with two captures and math made on it, a
/// sweep measurement with two runs, an SPL meter, an imported file.
struct Rig {
    meas: Vec<Measurement>,
    traces: Vec<TraceMeta>,
}

fn rig() -> Rig {
    let main = TraceOwner::Meas { meas: MeasId(1) };
    let math = MathConfig::of(
        main,
        MathDomain::Transfer,
        MathExpr::Binary {
            a: Operand::Trace { trace: TraceId(1) },
            op: MathOp::Divide,
            b: Operand::Trace { trace: TraceId(2) },
        },
    );
    Rig {
        meas: vec![
            tf(1, "Main L"),
            with_kind(
                2,
                "Genelec 1 m",
                MeasKind::Sweep {
                    config: sweep_config(),
                },
                false,
            ),
            with_kind(
                3,
                "FOH SPL",
                MeasKind::Spl {
                    config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
                },
                true,
            ),
            with_kind(4, "pre ÷ post", MeasKind::Math { config: math }, true),
        ],
        traces: vec![
            trace(1, "pre-EQ", main, captured()),
            trace(2, "post-EQ", main, captured()),
            trace(3, "Run 1", TraceOwner::Meas { meas: MeasId(2) }, run(1)),
            trace(4, "Run 2", TraceOwner::Meas { meas: MeasId(2) }, run(2)),
            trace(
                5,
                "1083 94cm",
                TraceOwner::Imported,
                TraceSource::Imported {
                    file_name: "1083 94cm.txt".into(),
                    format: ImportFormat::AnalyzerText,
                    notes: vec![],
                    ir_file: None,
                },
            ),
        ],
    }
}

fn rows(
    r: &Rig,
    collapsed: &BTreeSet<TraceOwner>,
    selected: Option<MeasId>,
    trace: Option<TraceId>,
) -> Vec<TreeRow> {
    tree_rows(&TreeInput {
        meas: r.meas.iter().map(item).collect(),
        traces: r
            .traces
            .iter()
            .map(|meta| TraceItem {
                meta,
                has_data: true,
                color: Color::from_rgba8([1, 2, 3, 255]),
                eq: None,
            })
            .collect(),
        collapsed,
        selected,
        selected_trace: trace,
        keys_on_trace: trace.is_some(),
        live_selected: false,
        sweep: None,
        compared: Vec::new(),
        rig: None,
    })
}

/// A compared row says `cmp` as the legend tags its curve; others are unchanged.
#[test]
fn compared_rows_are_marked() {
    let r = rig();
    let none = BTreeSet::new();
    let plain = rows(&r, &none, None, None);
    let marked = tree_rows(&TreeInput {
        meas: r.meas.iter().map(item).collect(),
        traces: r
            .traces
            .iter()
            .map(|meta| TraceItem {
                meta,
                has_data: true,
                color: Color::from_rgba8([1, 2, 3, 255]),
                eq: None,
            })
            .collect(),
        collapsed: &none,
        selected: None,
        selected_trace: None,
        keys_on_trace: false,
        live_selected: false,
        sweep: None,
        compared: vec![TreeKey::Trace(TraceId(5)), TreeKey::Live(MeasId(1))],
        rig: None,
    });
    for (a, b) in plain.iter().zip(&marked) {
        if matches!(b.key, TreeKey::Trace(TraceId(5)) | TreeKey::Live(MeasId(1))) {
            assert!(b.details.iter().all(|d| d.ends_with(" · cmp")), "{b:?}");
            assert!(b.describe.ends_with("compared (cmp)"));
        } else {
            assert_eq!(a, b);
        }
    }
}

#[test]
fn the_tree_lists_each_measurement_with_what_it_owns() {
    let r = rig();
    let none = BTreeSet::new();
    let t = rows(&r, &none, Some(MeasId(1)), None);
    let lines: Vec<String> = t
        .iter()
        .map(|row| {
            let lead = match (row.depth, row.last) {
                (0, _) => "",
                (_, false) => "  ├ ",
                (_, true) => "  └ ",
            };
            format!("{lead}{} · {}", row.name, row.details[0])
        })
        .collect();
    assert_eq!(
        lines,
        [
            "TF  Main L · running",
            "  ├ Main L (live) · live",
            "  ├ pre-EQ · capture",
            "  ├ post-EQ · capture",
            "  └ pre ÷ post · math (live)",
            "SWEEP  Genelec 1 m · 2 runs",
            "  ├ Run 1 · sweep run · 3 s −50.0 dBFS",
            "  └ Run 2 · sweep run · 3 s −50.0 dBFS",
            "SPL  FOH SPL · running",
            "Imported · 1 trace",
            "  └ 1083 94cm · imported",
        ]
    );
    assert_eq!(t[0].mark, Mark::Active);
    assert_eq!(t[0].collapsed, Some(false));
    assert_eq!(t[1].key, TreeKey::Live(MeasId(1)));
    assert_eq!(t[4].key, TreeKey::Math(MeasId(4)));
    // Every row with a curve has a dot in its curve's colour, filled while shown; a
    // header stands for a group and has none.
    let colour = |id: u32| item(r.meas.iter().find(|m| m.id == MeasId(id)).expect("m")).color;
    assert_eq!(t[1].dot, Some((colour(1), true)), "the live curve");
    assert_eq!(t[2].dot, Some((Color::from_rgba8([1, 2, 3, 255]), true)));
    assert_eq!(t[4].dot, Some((colour(4), true)), "the math result");
    for row in &t {
        assert_eq!(row.dot.is_none(), row.depth == 0, "{row:?}");
    }
    // The order V steps through: group by group.
    let metas: Vec<&TraceMeta> = r.traces.iter().rev().collect();
    let ms: Vec<&Measurement> = r.meas.iter().collect();
    let order: Vec<u32> = trace_order(&ms, &metas).iter().map(|t| t.id.0).collect();
    assert_eq!(order, [1, 2, 3, 4, 5]);
}

#[test]
fn a_trace_selected_after_the_measurement_has_the_keys() {
    let r = rig();
    let none = BTreeSet::new();
    let t = rows(&r, &none, Some(MeasId(1)), Some(TraceId(3)));
    assert_eq!(t[0].mark, Mark::Selected, "outlined: a trace has the keys");
    let run1 = t
        .iter()
        .find(|x| x.key == TreeKey::Trace(TraceId(3)))
        .expect("row");
    assert_eq!(run1.mark, Mark::Active);
    // A math channel selected is marked like a measurement.
    let t = rows(&r, &none, Some(MeasId(4)), None);
    let m = t
        .iter()
        .find(|x| x.key == TreeKey::Math(MeasId(4)))
        .expect("row");
    assert_eq!(m.mark, Mark::Active);
    assert_eq!(t[0].mark, Mark::None);
}

/// The live-curve row picked (Tab, a click): it is marked, its header outlined as when
/// a stored trace has the keys; a stored trace selected after it takes the mark.
#[test]
fn a_live_row_selected_is_marked_and_its_header_outlined() {
    let r = rig();
    let none = BTreeSet::new();
    let input = |trace: Option<TraceId>| TreeInput {
        meas: r.meas.iter().map(item).collect(),
        traces: r
            .traces
            .iter()
            .map(|meta| TraceItem {
                meta,
                has_data: true,
                color: Color::from_rgba8([1, 2, 3, 255]),
                eq: None,
            })
            .collect(),
        collapsed: &none,
        selected: Some(MeasId(1)),
        selected_trace: trace,
        keys_on_trace: trace.is_some(),
        live_selected: true,
        sweep: None,
        compared: Vec::new(),
        rig: None,
    };
    let mark = |t: &[TreeRow], k: TreeKey| t.iter().find(|x| x.key == k).expect("row").mark;
    let t = tree_rows(&input(None));
    assert_eq!(mark(&t, TreeKey::Meas(MeasId(1))), Mark::Selected);
    assert_eq!(mark(&t, TreeKey::Live(MeasId(1))), Mark::Active);
    let t = tree_rows(&input(Some(TraceId(3))));
    assert_eq!(mark(&t, TreeKey::Live(MeasId(1))), Mark::None);
    assert_eq!(mark(&t, TreeKey::Trace(TraceId(3))), Mark::Active);
    // Not picked: the header has the mark, the live row none.
    let t = rows(&r, &none, Some(MeasId(1)), None);
    assert_eq!(mark(&t, TreeKey::Meas(MeasId(1))), Mark::Active);
    assert_eq!(mark(&t, TreeKey::Live(MeasId(1))), Mark::None);
}

#[test]
fn folded_groups_hide_their_rows_and_say_how_many() {
    let r = rig();
    let folded: BTreeSet<TraceOwner> =
        [TraceOwner::Meas { meas: MeasId(1) }, TraceOwner::Imported].into();
    let t = rows(&r, &folded, None, None);
    let names: Vec<(&str, &str)> = t
        .iter()
        .map(|x| (x.name.as_str(), x.details[0].as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("TF  Main L", "running · 4 folded"),
            ("SWEEP  Genelec 1 m", "2 runs"),
            ("Run 1", "sweep run · 3 s −50.0 dBFS"),
            ("Run 2", "sweep run · 3 s −50.0 dBFS"),
            ("SPL  FOH SPL", "running"),
            ("Imported", "1 trace · folded"),
        ]
    );
    assert_eq!(t[0].collapsed, Some(true));
}

#[test]
fn hidden_curves_read_hidden_and_a_sweep_says_it_plays() {
    let mut r = rig();
    for t in &mut r.traces {
        if t.edit.owner == (TraceOwner::Meas { meas: MeasId(2) }) {
            t.edit.visible = false;
        }
    }
    let none = BTreeSet::new();
    let items: Vec<MeasItem<'_>> = r
        .meas
        .iter()
        .map(|m| MeasItem {
            hidden: m.id == MeasId(1),
            ..item(m)
        })
        .collect();
    let playing = SweepRun {
        id: SweepId(9),
        meas: MeasId(2),
        owner: ClientId("ui".into()),
        name: "Run 3".into(),
        reference_input: 0,
        measurement_input: 1,
        outputs: vec![0],
        level: Dbfs(-50.0),
        sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
        sweep_duration: Seconds(3.0),
        post_roll: Seconds(1.0),
        repeats: 2,
        gate: None,
        resolution: ac2_proto::model::Resolution::FortyEighth,
        lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
        status: SweepStatus::Playing { repeat: 1 },
        started_at: WallNs(0),
    };
    let t = tree_rows(&TreeInput {
        meas: items,
        traces: r
            .traces
            .iter()
            .map(|meta| TraceItem {
                meta,
                has_data: true,
                color: Color::from_rgba8([1, 2, 3, 255]),
                eq: None,
            })
            .collect(),
        collapsed: &none,
        selected: None,
        selected_trace: None,
        keys_on_trace: false,
        live_selected: false,
        sweep: Some(&playing),
        compared: Vec::new(),
        rig: None,
    });
    assert_eq!(t[0].details[0], "running · hidden");
    assert!(t[0].hidden && t[1].hidden);
    assert_eq!(t[1].details[0], "live · hidden");
    assert_eq!(t[1].dot, Some((item(&r.meas[0]).color, false)), "a ring");
    let sweep = t
        .iter()
        .find(|x| x.key == TreeKey::Meas(MeasId(2)))
        .expect("row");
    assert_eq!(sweep.details[0], "playing 1/2");
    assert!(sweep.hidden, "every run of it is hidden");
}

#[test]
fn deleting_a_measurement_with_traces_asks_keep_delete_or_cancel() {
    let m = tf(1, "Main L");
    let c = delete_choices(&m, 3, 0, &[], &[]);
    assert_eq!(c.title, "Delete measurement Main L?");
    assert_eq!(
        c.lines,
        [
            "transfer function · running · it has 3 traces.",
            "Keep: they stay, listed under Imported. Delete: they go with Main L; it cannot be \
             undone."
        ]
    );
    let labels: Vec<&str> = c.choices.iter().map(|x| x.label.as_str()).collect();
    assert_eq!(
        labels,
        ["Keep them (move to Imported)", "Delete them too", "Cancel"]
    );
    assert_eq!(c.default, KEEP);
    assert_eq!(c.hint, "←/→ choose · Enter confirms · Esc cancels");
    let c = delete_choices(&m, 1, 1, &["pre ÷ Main L".into()], &[]);
    assert_eq!(
        c.lines[0],
        "transfer function · running · it has 1 trace and 1 math channel."
    );
    assert_eq!(
        c.choices[KEEP].blocked.as_deref(),
        Some("the math channel pre ÷ Main L would lose an operand")
    );
    assert_eq!(c.default, CANCEL, "Keep is refused: Enter cancels");
    assert_eq!(owned_words(0, 2), "2 math channels");
}

/// A meter kept by the rig is listed first, under a Rig header that says where cues go;
/// the session's measurements follow under their own header; a cued meter's row carries
/// its chip and says the state.
#[test]
fn rig_meters_lead_the_tree_with_their_cue_chip() {
    let mut r = rig();
    let foh = r
        .meas
        .iter_mut()
        .find(|m| m.id == MeasId(3))
        .expect("test value");
    foh.config.scope = MeasScope::Rig;
    let mut osc = ac2_proto::model::Rig {
        osc_target: Some("10.0.0.5:9000".parse().expect("test value")),
        ..Default::default()
    };
    osc.cues.push(RigCue {
        meas: MeasId(3),
        subject: CueSubject::Meter,
        state: CueState::Over,
    });
    let none = BTreeSet::new();
    let rows = tree_rows(&TreeInput {
        meas: r.meas.iter().map(item).collect(),
        traces: Vec::new(),
        collapsed: &none,
        selected: None,
        selected_trace: None,
        keys_on_trace: false,
        live_selected: false,
        sweep: None,
        compared: Vec::new(),
        rig: Some(&osc),
    });
    let names: Vec<&str> = rows
        .iter()
        .filter(|r| r.depth == 0)
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Rig",
            "SPL  FOH SPL",
            "Session",
            "TF  Main L",
            "SWEEP  Genelec 1 m"
        ]
    );
    assert_eq!(rows[0].key, TreeKey::Rig);
    assert_eq!(rows[0].details[0], "1 meter · OSC to 10.0.0.5:9000");
    let foh = rows
        .iter()
        .find(|r| r.key == TreeKey::Meas(MeasId(3)))
        .expect("test value");
    assert_eq!(foh.chip, Some(CueState::Over));
    assert!(foh.describe.ends_with("OSC cue OVER"), "{}", foh.describe);
    assert!(
        rows.iter()
            .filter(|r| r.key != TreeKey::Meas(MeasId(3)))
            .all(|r| r.chip.is_none())
    );
    // V steps through the rig's groups first, as listed.
    let all: Vec<&Measurement> = r.meas.iter().collect();
    assert_eq!(group_order(&all)[0], TraceOwner::Meas { meas: MeasId(3) });
    // With no rig meter, no headers: the tree reads as before.
    r.meas
        .iter_mut()
        .for_each(|m| m.config.scope = MeasScope::Session);
    let plain = rows_of(&r, &osc);
    assert!(
        plain
            .iter()
            .all(|r| !matches!(r.key, TreeKey::Rig | TreeKey::Session))
    );
}

fn rows_of(r: &Rig, osc: &ac2_proto::model::Rig) -> Vec<TreeRow> {
    let none = BTreeSet::new();
    tree_rows(&TreeInput {
        meas: r.meas.iter().map(item).collect(),
        traces: Vec::new(),
        collapsed: &none,
        selected: None,
        selected_trace: None,
        keys_on_trace: false,
        live_selected: false,
        sweep: None,
        compared: Vec::new(),
        rig: Some(osc),
    })
}
