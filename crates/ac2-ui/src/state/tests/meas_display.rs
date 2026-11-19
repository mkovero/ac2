//! Reducer and scene tests of what the daemon keeps with a measurement about drawing it:
//! a spectrum's style, peak hold and spectrograph (B, P, Shift+B, colours), its display
//! offset; and what a pane draws behind its lead (C's ticks).

use super::*;
use ac2_proto::model::SpectrumStyle;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

const SIZE: Viewport = Viewport::new(900.0, 500.0);

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    }
}

/// Sub (2) and "Sub 2" (9), two spectra.
fn two_spectra() -> State {
    let mut s = daemon_state();
    let mut s2 = meas(9, "Sub 2", spectrum());
    if let Some(d) = s2.config.kind.display_mut() {
        d.style = SpectrumStyle::Line;
    }
    s.measurements.push(s2);
    s
}

/// Live spectra of `meas`, each at its level, in one snapshot.
fn spectra(t: &mut T, frames: &[(u32, f32)]) {
    let stamp = ac2_proto::samples::stamp(None);
    let frames: Vec<(u32, Vec<f32>)> = frames.iter().map(|(m, l)| (*m, vec![*l; 5])).collect();
    spectra_of(t, &frames, stamp.seq, stamp.daemon_incarnation);
}

/// Live spectra of `meas`, each with its column levels, numbered `seq` by daemon
/// `incarnation`, in one snapshot.
fn spectra_of(t: &mut T, frames: &[(u32, Vec<f32>)], seq: u64, incarnation: DaemonIncarnation) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    use ac2_proto::{Frame, FrameData};
    let grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: 8,
    };
    let mut latest = Latest::default();
    for (meas, level) in frames {
        let data = FrameData::Spec(SpecFrame {
            meas: MeasId(*meas),
            meta: SpecMeta {
                window: Window::Hann,
                scale: LevelScale::Dbfs,
                level: ac2_proto::model::SpecLevel::Tone,
                cal: CalStatus::Uncalibrated,
                mic_curve: false,
                smoothing: None,
                math: None,
                bands: None,
                harmonics: None,
            },
            level: level.clone(),
        });
        let mut stamp = ac2_proto::samples::stamp(Some(grid.clone()));
        stamp.seq = seq;
        stamp.daemon_incarnation = incarnation;
        let f = TopicFrame {
            topic: data.topic(),
            frame: Arc::new(Frame { stamp, data }),
            received: Instant::now(),
            since_new: std::time::Duration::ZERO,
            age: Some(0.0),
            stale: false,
        };
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    let mut grids = std::collections::BTreeMap::new();
    grids.insert(grid.id(), Arc::new(grid));
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
}

/// The test grid on `state`, the spectrum pane showing Sub with the groups of `with` ticked
/// behind it, frames of both spectra.
fn on_sub(state: State, with: &[u32]) -> T {
    let mut t = T::new();
    t.conn(mirror(state));
    let p = t.pane(PaneKind::Spectrum);
    t.drop_meas(p, MeasId(2));
    let picks: Vec<Pick> = with
        .iter()
        .map(|m| Pick::here(EntryRef::Group { meas: MeasId(*m) }))
        .collect();
    t.st.layout.view_mut(p).expect("the pane").with = With::of(&picks);
    spectra(&mut t, &[(2, -60.0), (9, -60.0)]);
    t
}

/// What the spectrum pane draws: (name, style, layer, compared, offset).
fn drawn(t: &T) -> Vec<(String, SpectrumStyle, u8, bool, f64)> {
    let p = t.pane(PaneKind::Spectrum);
    crate::scenes::with_spectrum(&t.st, p, &Theme::dark(), now(), |traces, _, _| {
        traces
            .iter()
            .map(|c| (c.name.clone(), c.style, c.layer, c.compared, c.offset_db))
            .collect()
    })
}

/// The operator's case: two spectra, Sub 2 offset +6 dB and compared into Sub's pane. It
/// draws there at its own offset, and the legend and the cursor readout say so; the offset
/// keys change it from that pane once it is picked in the legend.
#[test]
fn a_compared_curve_keeps_its_own_offset() {
    let mut s = two_spectra();
    s.measurements[2].offset = Db(6.0);
    let mut t = on_sub(s, &[9]);
    let rows = drawn(&t);
    assert_eq!(
        rows.iter().map(|r| (r.0.as_str(), r.4)).collect::<Vec<_>>(),
        [("Sub", 0.0), ("Sub 2", 6.0)]
    );
    let p = t.pane(PaneKind::Spectrum);
    t.st.view.cursor_hz = Some(12_000.0);
    let scene = crate::scenes::spectrum(&t.st, p, &Theme::dark(), SIZE, now());
    let legend: Vec<&str> = scene.legend.iter().map(|e| e.text.as_str()).collect();
    assert!(
        legend
            .iter()
            .any(|e| e.starts_with("Sub 2") && e.contains("offset +6.0 dB")),
        "{legend:?}"
    );
    let cursor = scene.cursor.expect("the cursor readout");
    let row = cursor
        .rows
        .iter()
        .find(|(n, _)| n.starts_with("Sub 2"))
        .expect("Sub 2 in the readout");
    assert!(row.0.contains("offset +6.0 dB"), "{row:?}");
    assert!(row.1.starts_with("\u{2212}54.0"), "{row:?}");
    // Picked in Sub's legend, Sub 2 takes the offset keys there; Sub keeps its own.
    t.st.apply(
        Msg::LegendPick(p, ac2_scene::trace::TraceKey::Live(MeasId(9))),
        &t.keys,
    );
    let r = t.key_answered("Alt+Up");
    assert!(
        matches!(
            calls(&r).first(),
            Some((
                Command::MeasOffset {
                    meas: MeasId(9),
                    offset: Db(7.0)
                },
                _
            ))
        ),
        "{r:?}"
    );
    assert_eq!(t.st.meas_offset(MeasId(9)), 7.0);
    assert_eq!(t.st.meas_offset(MeasId(2)), 0.0);
    // Its pane's own measurement picked again: the keys are Sub's.
    t.st.apply(
        Msg::LegendPick(p, ac2_scene::trace::TraceKey::Live(MeasId(2))),
        &t.keys,
    );
    let r = t.key("Alt+Up");
    assert!(
        matches!(
            calls(&r).first(),
            Some((
                Command::MeasOffset {
                    meas: MeasId(2),
                    ..
                },
                _
            ))
        ),
        "{r:?}"
    );
}

/// B, P, Shift+B and the colour maps change only the spectrum pane's measurement; a curve
/// compared there draws with its own measurement's style.
#[test]
fn display_keys_change_only_the_panes_measurement() {
    let mut t = on_sub(two_spectra(), &[9]);
    let before = t.st.meas(MeasId(9)).cloned();
    for k in ["B", "P", "Shift+B"] {
        let r = t.key_answered(k);
        let c = calls(&r);
        assert_eq!(c.len(), 1, "{k}: {r:?}");
        assert!(
            matches!(
                c[0].0,
                Command::MeasUpdate {
                    meas: MeasId(2),
                    ..
                }
            ),
            "{k}: {r:?}"
        );
    }
    let r =
        t.st.apply(Msg::Command(CommandId::SpectrographMagma), &t.keys);
    t.answer(&r);
    assert_eq!(t.st.meas(MeasId(9)).cloned(), before, "Sub 2 untouched");
    let sub = t.st.meas(MeasId(2)).expect("Sub");
    let d = t.st.display_of(sub);
    assert_eq!(d.style, SpectrumStyle::Line, "bars → line");
    assert_eq!(d.peak, ac2_proto::model::PeakMode::Hold);
    assert_eq!(d.spectrograph_span_s, 60);
    assert_eq!(d.spectrograph_colours, Colormap::Magma);
    assert_eq!(
        drawn(&t)
            .iter()
            .map(|r| (r.0.as_str(), r.1))
            .collect::<Vec<_>>(),
        [("Sub", SpectrumStyle::Line), ("Sub 2", SpectrumStyle::Line)]
    );
    let r = t.key_answered("B");
    assert_eq!(t.last_toast(), "Sub: fill", "{r:?}");
    assert_eq!(
        drawn(&t)
            .iter()
            .map(|r| (r.0.as_str(), r.1))
            .collect::<Vec<_>>(),
        [("Sub", SpectrumStyle::Fill), ("Sub 2", SpectrumStyle::Line)]
    );
}

/// The pane draws its lead's group first (on top), then what is ticked behind it in tick
/// order, each a layer further back.
#[test]
fn compared_curves_draw_in_compare_order_under_the_panes_own() {
    let mut s = two_spectra();
    s.measurements.push(meas(10, "Sub 3", spectrum()));
    let mut t = on_sub(s, &[10, 9]);
    spectra(&mut t, &[(2, -60.0), (9, -60.0), (10, -60.0)]);
    assert_eq!(
        drawn(&t)
            .iter()
            .map(|r| (r.0.as_str(), r.2, r.3))
            .collect::<Vec<_>>(),
        [("Sub", 0, false), ("Sub 3", 1, true), ("Sub 2", 2, true)]
    );
}

/// C on the spectrum pane lists Sub (its lead) and Sub 2; nothing is drawn behind Sub
/// until Sub 2 is ticked, and then only in this pane, with no daemon call.
#[test]
fn c_ticks_into_the_focused_pane_only() {
    let mut t = on_sub(two_spectra(), &[]);
    let p = t.pane(PaneKind::Spectrum);
    t.st.layout.set_focus(p);
    assert!(t.key("C").is_empty());
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("no chooser: {:?}", t.st.overlay);
    };
    let labels: Vec<&str> = c.choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "\u{2713} Sub \u{b7} live \u{b7} lead",
            "  Sub 2 \u{b7} live"
        ]
    );
    assert_eq!(drawn(&t).len(), 1, "nothing ticked yet");
    t.key("Down");
    assert!(t.key("Enter").is_empty());
    assert_eq!(t.last_toast(), "Sub 2 drawn in this pane (cmp)");
    t.key("Escape");
    assert_eq!(
        drawn(&t)
            .iter()
            .map(|r| (r.0.as_str(), r.3))
            .collect::<Vec<_>>(),
        [("Sub", false), ("Sub 2", true)]
    );
    // Another spectrum pane shows Sub alone.
    let other = t.st.split_focused_pane().expect("room for a split");
    let names =
        crate::scenes::with_spectrum(&t.st, other, &Theme::dark(), now(), |traces, _, _| {
            traces.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
        });
    assert_eq!(names, ["Sub"], "a split starts with the lead alone");
}

/// P steps hold → tracking → off: the hold line is drawn only while the mode holds, tracking
/// shows the marks alone, and a hold turned on again starts afresh, not from an old one.
#[test]
fn tracking_draws_no_hold_and_a_new_hold_starts_afresh() {
    use ac2_proto::model::PeakMode;
    let mut t = on_sub(two_spectra(), &[]);
    let hold = |t: &T| -> Option<Vec<f32>> {
        let p = t.pane(PaneKind::Spectrum);
        crate::scenes::with_spectrum(&t.st, p, &Theme::dark(), now(), |traces, _, _| {
            traces
                .iter()
                .find(|c| c.name == "Sub")
                .and_then(|c| c.peak.map(<[f32]>::to_vec))
        })
    };
    let mode = |t: &T| t.st.meas(MeasId(2)).map(|m| t.st.display_of(m).peak);
    t.key_answered("P");
    assert_eq!(mode(&t), Some(PeakMode::Hold));
    spectra(&mut t, &[(2, -40.0)]);
    assert_eq!(hold(&t), Some(vec![-40.0; 5]));
    t.key_answered("P");
    assert_eq!(mode(&t), Some(PeakMode::Track));
    spectra(&mut t, &[(2, -60.0)]);
    assert_eq!(hold(&t), None, "tracking draws no hold line");
    assert!(
        !t.st.rig.folded.peaks.contains_key(&MeasId(2)),
        "nor keeps one"
    );
    t.key_answered("P");
    assert_eq!(mode(&t), Some(PeakMode::Off));
    t.key_answered("P");
    assert_eq!(mode(&t), Some(PeakMode::Hold));
    spectra(&mut t, &[(2, -60.0)]);
    assert_eq!(hold(&t), Some(vec![-60.0; 5]), "a fresh hold");
}

/// The daemon restarted (another incarnation) while the app stayed up: its frames number
/// afresh from 1, far below the `seq` the old daemon reached. The hold and the tracked peaks
/// start again from the new daemon's frames instead of waiting for `seq` to catch up.
#[test]
fn hold_and_tracks_start_afresh_after_a_daemon_restart() {
    use ac2_proto::model::PeakMode;
    let mut t = on_sub(two_spectra(), &[]);
    let hold = |t: &T| -> Option<Vec<f32>> {
        let p = t.pane(PaneKind::Spectrum);
        crate::scenes::with_spectrum(&t.st, p, &Theme::dark(), now(), |traces, _, _| {
            traces
                .iter()
                .find(|c| c.name == "Sub")
                .and_then(|c| c.peak.map(<[f32]>::to_vec))
        })
    };
    let old = DaemonIncarnation(1);
    let new = DaemonIncarnation(2);
    let flat = |l: f32| vec![(2, vec![l; 5])];
    t.key_answered("P");
    assert_eq!(
        t.st.meas(MeasId(2)).map(|m| t.st.display_of(m).peak),
        Some(PeakMode::Hold)
    );
    spectra_of(&mut t, &flat(-40.0), 3_000_000, old);
    assert_eq!(hold(&t), Some(vec![-40.0; 5]));
    let held = t.st.daemon().expect("mirrored").clone();
    let restart = |t: &mut T, held: &State, incarnation: u64| {
        t.conn(ConnEvent::Failed {
            down: ac2_scene::chrome::RigDown::Unreachable,
            target: "tcp://pupu".into(),
            error: "gone".into(),
            retry_in: std::time::Duration::from_secs(1),
        });
        t.conn(ConnEvent::Connected {
            target: "tcp://pupu".into(),
            server: "ac2d test".into(),
            client_id: ClientId("c1".into()),
            rig_id: t.st.rig.id,
            name: "test rig".into(),
        });
        t.conn(mirror_of(held.clone(), incarnation, Some("c1")));
    };
    // A brief loss of the same daemon: its numbering goes on and the hold is kept.
    restart(&mut t, &held, 1);
    spectra_of(&mut t, &flat(-60.0), 3_000_001, old);
    assert_eq!(
        hold(&t),
        Some(vec![-40.0; 5]),
        "the same daemon keeps the hold"
    );
    // Another incarnation: a fresh hold from its first frame, then folding on.
    restart(&mut t, &held, 2);
    spectra_of(&mut t, &flat(-50.0), 1, new);
    assert_eq!(hold(&t), Some(vec![-50.0; 5]), "a fresh hold");
    spectra_of(&mut t, &flat(-45.0), 2, new);
    assert_eq!(hold(&t), Some(vec![-45.0; 5]), "folding on");

    // Tracking follows the new daemon's frames too.
    t.key_answered("P");
    assert_eq!(
        t.st.meas(MeasId(2)).map(|m| t.st.display_of(m).peak),
        Some(PeakMode::Track)
    );
    let held = t.st.daemon().expect("mirrored").clone();
    let tracked = |t: &T| -> Vec<f64> {
        t.st.rig
            .folded
            .peak_tracks
            .get(&MeasId(2))
            .map_or(vec![], |p| {
                p.1.tracked().max.iter().map(|p| p.freq_hz).collect()
            })
    };
    let bump = |at: usize| {
        let mut l = vec![-80.0; 5];
        l[at] = -20.0;
        vec![(2, l)]
    };
    spectra_of(&mut t, &bump(1), 5_000, new);
    let before = tracked(&t);
    assert!(!before.is_empty(), "a peak tracked");
    restart(&mut t, &held, 3);
    spectra_of(&mut t, &bump(3), 3, DaemonIncarnation(3));
    let after = tracked(&t);
    assert!(
        !after.is_empty() && after != before,
        "{before:?} → {after:?}"
    );
}

/// Live transfer frames of measurement 1, numbered `seq`: every column at `mag` dB with
/// coherence `coh`.
fn transfer_frame(t: &mut T, seq: u64, mag: f32, coh: f32) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::FrameData;
    let mut f = ac2_proto::samples::tf_frame();
    f.stamp.seq = seq;
    if let FrameData::Tf(tf) = &mut f.data {
        tf.meas = MeasId(1);
        tf.meta.math = None;
        tf.validity = vec![ac2_proto::frame::ValidityMask::NONE; tf.mag.len()];
        tf.coh = vec![coh; tf.mag.len()];
        tf.mag = vec![mag; tf.mag.len()];
    }
    let f = TopicFrame {
        topic: f.data.topic(),
        frame: Arc::new(f),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string().into(), f);
    let mut grids = std::collections::BTreeMap::new();
    let g = ac2_proto::samples::log_grid();
    grids.insert(g.id(), Arc::new(g));
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: Instant::now(),
    })));
}

/// A transfer's peak hold is kept as a spectrum's: the highest magnitude each column has
/// reached where the curve is drawn (a column blanked for its coherence adds nothing),
/// started over by R and by turning the hold on again; tracking draws no hold.
#[test]
fn a_transfer_holds_its_highest_shown_magnitude() {
    use ac2_proto::model::PeakMode;
    let mut t = T::new();
    assert_eq!(t.st.scope(), Scope::Transfer);
    let scene = |t: &T| {
        let p = t.pane(PaneKind::Transfer);
        let size = ac2_scene::primitives::Viewport::new(1000.0, 600.0);
        crate::scenes::transfer(&t.st, p, &Theme::dark(), size, now())
    };
    let hold = |t: &T| -> Option<Vec<f64>> {
        scene(t)
            .traces
            .iter()
            .find(|c| c.key == ac2_scene::trace::TraceKey::Live(MeasId(1)))
            .and_then(|c| c.hold_db.clone())
    };
    let all = |h: Option<Vec<f64>>, want: f64| h.is_some_and(|h| h.iter().all(|v| *v == want));
    let mode = |t: &T| t.st.meas(MeasId(1)).and_then(|m| m.config.kind.peak());
    transfer_frame(&mut t, 1, 0.0, 1.0);
    assert_eq!(hold(&t), None, "no hold while the mode is off");
    assert!(scene(&t).extremes.is_none(), "nor any tracking");
    t.st.view.tf.coherence.blank_below = Some(0.5);
    t.key_answered("P");
    assert_eq!(mode(&t), Some(PeakMode::Hold));
    transfer_frame(&mut t, 2, 3.0, 1.0);
    assert!(all(hold(&t), 3.0));
    transfer_frame(&mut t, 3, -6.0, 1.0);
    assert!(all(hold(&t), 3.0), "a lower frame leaves the hold");
    transfer_frame(&mut t, 4, 12.0, 0.1);
    assert!(all(hold(&t), 3.0), "a blanked frame adds nothing");
    // R starts the hold over with the averages.
    t.key("R");
    transfer_frame(&mut t, 5, -6.0, 1.0);
    assert!(all(hold(&t), -6.0), "{:?}", hold(&t));
    t.key_answered("P");
    assert_eq!(mode(&t), Some(PeakMode::Track));
    transfer_frame(&mut t, 6, -6.0, 1.0);
    assert_eq!(hold(&t), None, "tracking draws no hold line");
    assert!(!t.st.rig.folded.peaks.contains_key(&MeasId(1)));
    assert!(
        scene(&t).extremes.is_some(),
        "the pane's measurement tracks"
    );
    t.key_answered("P");
    t.key_answered("P");
    assert_eq!(mode(&t), Some(PeakMode::Hold));
    transfer_frame(&mut t, 7, -9.0, 1.0);
    assert!(all(hold(&t), -9.0), "a fresh hold");
}

/// Peak tracking keeps a spectrum's dips beside its peaks.
#[test]
fn tracking_keeps_a_spectrums_dips_too() {
    let mut t = on_sub(two_spectra(), &[]);
    t.key_answered("P");
    t.key_answered("P");
    // A floor with one tone and one notch.
    let mut level = vec![-60.0_f32; 5];
    level[1] = -20.0;
    level[3] = -90.0;
    let stamp = ac2_proto::samples::stamp(None);
    spectra_of(&mut t, &[(2, level)], stamp.seq, stamp.daemon_incarnation);
    let marks = t.st.rig.folded.peak_tracks[&MeasId(2)].1.tracked();
    assert_eq!(marks.max[0].level, -20.0, "{marks:?}");
    assert_eq!(marks.min.len(), 1, "{marks:?}");
    assert_eq!(marks.min[0].level, -90.0);
}

/// P on the spectrum pane steps the focused pane's measurement off → peak hold → peak
/// tracking → off, the toast naming each step.
#[test]
fn p_cycles_peak_hold_and_tracking() {
    use ac2_proto::model::PeakMode;
    let mut t = T::new();
    t.key("Alt+2");
    for (want, toast) in [
        (PeakMode::Hold, "Sub: peaks: hold"),
        (PeakMode::Track, "Sub: peaks: tracking"),
        (PeakMode::Off, "Sub: peaks off"),
    ] {
        t.key_answered("P");
        let sub = t.st.meas(MeasId(2)).expect("Sub");
        assert_eq!(t.st.display_of(sub).peak, want);
        assert_eq!(t.last_toast(), toast);
    }
}
