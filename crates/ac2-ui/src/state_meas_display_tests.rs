//! Reducer and scene tests of what the daemon keeps with a measurement about drawing it:
//! a spectrum's style, peak hold and spectrograph (B, P, Shift+B, colours), its display
//! offset, and the curves compared into its panes (C).

use super::*;
use ac2_proto::model::{Compared, SpectrumStyle};
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

const SIZE: Viewport = Viewport {
    width: 900.0,
    height: 500.0,
};

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    }
}

/// Sub (2) and "Sub 2" (9), two spectra; with `compared`, Sub 2 is compared into Sub.
fn two_spectra(compared: bool) -> State {
    let mut s = daemon_state();
    let mut s2 = meas(9, "Sub 2", spectrum());
    if let Some(d) = s2.config.kind.display_mut() {
        d.style = SpectrumStyle::Line;
    }
    s.measurements.push(s2);
    if compared {
        s.measurements[0].compared = vec![Compared::Meas { meas: MeasId(9) }];
    }
    s
}

/// Live spectra of `meas`, each at its level, in one snapshot.
fn spectra(t: &mut T, frames: &[(u32, f32)]) {
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
            },
            level: vec![*level; 5],
        });
        let f = TopicFrame {
            topic: data.topic(),
            frame: Arc::new(Frame {
                stamp: ac2_proto::samples::stamp(Some(grid.clone())),
                data,
            }),
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

/// The test grid on `state`, the spectrum pane showing Sub, frames of both spectra.
fn on_sub(state: State) -> T {
    let mut t = T::new();
    t.conn(mirror(state));
    let p = t.pane(PaneKind::Spectrum);
    t.st.update(Msg::PanePick(p, PaneMenuRow::Meas(MeasId(2))), &t.keys);
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
    let mut s = two_spectra(true);
    s.measurements[2].offset = Db(6.0);
    let mut t = on_sub(s);
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
    t.st.update(
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
    t.st.update(
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
    let mut t = on_sub(two_spectra(true));
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
        t.st.update(Msg::Command(CommandId::SpectrographMagma), &t.keys);
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

/// The pane draws its own group first (on top), then the compared curves in compare
/// order, each a layer further back.
#[test]
fn compared_curves_draw_in_compare_order_under_the_panes_own() {
    let mut s = two_spectra(false);
    s.measurements.push(meas(10, "Sub 3", spectrum()));
    s.measurements[0].compared = vec![
        Compared::Meas { meas: MeasId(10) },
        Compared::Meas { meas: MeasId(9) },
    ];
    let mut t = on_sub(s);
    spectra(&mut t, &[(2, -60.0), (9, -60.0), (10, -60.0)]);
    assert_eq!(
        drawn(&t)
            .iter()
            .map(|r| (r.0.as_str(), r.2, r.3))
            .collect::<Vec<_>>(),
        [("Sub", 0, false), ("Sub 3", 1, true), ("Sub 2", 2, true)]
    );
}

/// C on a curve of another measurement asks where first: nothing is drawn compared until
/// a measurement is ticked, and then only in that measurement's panes.
#[test]
fn c_asks_which_measurement_first() {
    let mut t = on_sub(two_spectra(false));
    t.st.update(Msg::SelectMeas(MeasId(9)), &t.keys);
    assert!(t.key("C").is_empty());
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("no chooser: {:?}", t.st.overlay);
    };
    assert_eq!(c.title, "Compare Sub 2 with…");
    // Its row chosen put Sub 2 on the spectrum pane: Sub is on no pane now.
    let labels: Vec<&str> = c.choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["  Sub"]);
    assert_eq!(drawn(&t).len(), 1, "nothing compared yet");
    let r = t.key("Enter");
    assert_eq!(
        calls(&r)
            .first()
            .map(|(c, w)| ((*c).clone(), w.to_string())),
        Some((
            Command::MeasCompare {
                meas: MeasId(2),
                compared: vec![Compared::Meas { meas: MeasId(9) }],
            },
            "Sub 2 compared into Sub (cmp)".to_owned()
        ))
    );
    t.answer(&r);
    t.key("Escape");
    let p = t.pane(PaneKind::Spectrum);
    t.st.update(Msg::PanePick(p, PaneMenuRow::Meas(MeasId(2))), &t.keys);
    assert_eq!(
        drawn(&t)
            .iter()
            .map(|r| (r.0.as_str(), r.3))
            .collect::<Vec<_>>(),
        [("Sub", false), ("Sub 2", true)]
    );
}
