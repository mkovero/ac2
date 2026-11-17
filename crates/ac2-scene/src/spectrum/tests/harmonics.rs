use super::*;
use crate::canvas::tests::intersects;
use crate::plate::PlateKind;
use crate::spectrum::harmonics::{RATIO_DB_MIN, orders_text, rows};
use crate::spectrum::peaks::Peak;
use ac2_proto::frame::{
    HARMONIC_DB_MIN, HarmonicLevel, HarmonicReading, HarmonicUnreadable, SpecHarmonics,
};
use ac2_proto::units::Db;

fn harmonic(order: u8, level: f64, floor: f64, resolved: bool) -> HarmonicLevel {
    HarmonicLevel {
        order,
        level: Db(level),
        floor: Db(floor),
        resolved,
    }
}

/// H2 and H3 stand out of the floor, H4…H10 do not; their floors fall from −118 dB.
fn orders() -> Vec<HarmonicLevel> {
    let mut h = vec![
        harmonic(2, -96.4, -120.0, true),
        harmonic(3, -101.2, -119.0, true),
    ];
    h.extend((4..=10).map(|k| {
        let floor = -118.0 - f64::from(k - 4);
        harmonic(k, floor, floor, false)
    }));
    h
}

/// A sine on a bin of a 65 536-point FFT at 48 kHz, read cleanly.
fn on_bin() -> SpecHarmonics {
    SpecHarmonics {
        f0: Hz(999.0234375),
        on_bin: true,
        reading: HarmonicReading::Read {
            fundamental: -12.3,
            harmonics: orders(),
            thd: Some(Db(-95.1)),
            thd_bound: Db(-94.9),
            thd_n: Db(-88.0),
            leakage: None,
        },
    }
}

fn unreadable(reason: HarmonicUnreadable) -> SpecHarmonics {
    SpecHarmonics {
        f0: Hz(1000.0),
        on_bin: false,
        reading: HarmonicReading::Unreadable { reason },
    }
}

fn text(h: &SpecHarmonics, scale: LevelScale) -> String {
    rows(h, scale)
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn view(chrome: PlotChrome) -> ViewState {
    ViewState {
        spectrum: SpectrumView {
            level: Range::new(-140.0, 0.0),
            ..SpectrumView::default()
        },
        chrome,
        ..ViewState::default()
    }
}

/// A curve with the sine at 1 kHz and harmonics at 2 and 3.16 kHz columns.
fn levels(f: &[f64]) -> Vec<f32> {
    f.iter()
        .map(|hz| match *hz {
            x if (x - 1000.0).abs() < 1.0 => -12.3,
            x if (x - 1995.26).abs() < 1.0 => -108.7,
            x if (x - 3162.28).abs() < 1.0 => -113.5,
            _ => -130.0,
        })
        .collect()
}

fn scene_of(h: Option<&SpecHarmonics>, v: &ViewState) -> SpectrumScene {
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.harmonics = h;
    spectrum_scene(&[t], &Status::default(), v, &Theme::dark(), SIZE)
}

#[test]
fn a_sine_on_a_bin_reads_its_harmonics_thd_and_thd_n() {
    assert_eq!(
        text(&on_bin(), LevelScale::Dbfs),
        "sine 999.02 Hz · on bin\n\
         H1  −12.3 dBFS\n\
         H2  −96.4 dB\n\
         H3  −101.2 dB\n\
         H4–H10  < −118.0 dB\n\
         THD  −95.1 dB · 0.00176 %\n\
         THD+N  −88.0 dB · 0.00398 %"
    );
}

#[test]
fn a_calibrated_fundamental_reads_in_db_spl() {
    let rows = text(&on_bin(), LevelScale::DbSpl);
    assert_eq!(rows.lines().nth(1), Some("H1  −12.3 dB SPL"));
}

/// Between bins the window's own leakage is in THD+N: within 10 dB of it the row is a
/// limit and names the window, never a figure of the device.
#[test]
fn thd_n_between_bins_is_a_limit_while_leakage_is_in_it() {
    let between = |thd_n: f64, leakage: f64| {
        let mut h = on_bin();
        h.f0 = Hz(1000.0);
        h.on_bin = false;
        if let HarmonicReading::Read {
            thd_n: n,
            leakage: l,
            ..
        } = &mut h.reading
        {
            *n = Db(thd_n);
            *l = Some(Db(leakage));
        }
        text(&h, LevelScale::Dbfs)
    };
    assert_eq!(
        between(-31.0, -31.2),
        "sine 1000 Hz · between bins\n\
         H1  −12.3 dBFS\n\
         H2  −96.4 dB\n\
         H3  −101.2 dB\n\
         H4–H10  < −118.0 dB\n\
         THD  −95.1 dB · 0.00176 %\n\
         THD+N  < −31.0 dB · window leakage (sine between bins)"
    );
    // A window whose leakage lies far under the noise leaves the figure the device's.
    assert_eq!(
        between(-88.0, -110.0).lines().last(),
        Some("THD+N  −88.0 dB · 0.00398 %")
    );
    assert_eq!(
        between(-88.0, -78.0).lines().last(),
        Some("THD+N  < −88.0 dB · window leakage (sine between bins)")
    );
}

#[test]
fn no_resolved_harmonic_leaves_thd_a_limit() {
    let mut h = on_bin();
    if let HarmonicReading::Read {
        harmonics,
        thd,
        thd_bound,
        ..
    } = &mut h.reading
    {
        *harmonics = (2..=10)
            .map(|k| harmonic(k, -121.0 - f64::from(k), -121.0 - f64::from(k), false))
            .collect();
        *thd = None;
        *thd_bound = Db(-112.3);
    }
    assert_eq!(
        text(&h, LevelScale::Dbfs),
        "sine 999.02 Hz · on bin\n\
         H1  −12.3 dBFS\n\
         H2–H10  < −123.0 dB\n\
         THD  < −112.3 dB\n\
         THD+N  −88.0 dB · 0.00398 %"
    );
}

/// Unresolved orders share one row at the highest of their limits, runs of neighbours
/// folded; an unresolved harmonic standing over its floor is below its level.
#[test]
fn unresolved_orders_fold_into_one_limit_row() {
    assert_eq!(orders_text(&[4]), "H4");
    assert_eq!(orders_text(&[4, 5, 6, 7, 8, 9, 10]), "H4–H10");
    assert_eq!(orders_text(&[4, 6, 7, 8, 9, 10]), "H4, H6–H10");
    assert_eq!(orders_text(&[2, 4, 6]), "H2, H4, H6");
    let mut h = on_bin();
    if let HarmonicReading::Read { harmonics, .. } = &mut h.reading {
        *harmonics = vec![
            harmonic(2, -96.4, -120.0, true),
            harmonic(3, -117.0, -119.0, false),
            harmonic(4, -90.0, -119.5, true),
            harmonic(5, -121.0, -121.0, false),
            harmonic(6, -122.0, -122.0, false),
        ];
    }
    let rows = text(&h, LevelScale::Dbfs);
    assert_eq!(
        rows.lines().skip(2).take(3).collect::<Vec<_>>(),
        ["H2  −96.4 dB", "H4  −90.0 dB", "H3, H5–H6  < −117.0 dB"]
    );
}

/// A ratio at the wire's lowest value is a power of exactly zero, not a reading: every
/// such row is a limit at the lowest ratio the plate writes.
#[test]
fn digital_silence_reads_as_a_limit_never_as_minus_400() {
    let h = SpecHarmonics {
        f0: Hz(1000.0),
        on_bin: true,
        reading: HarmonicReading::Read {
            fundamental: -6.0,
            harmonics: vec![harmonic(2, HARMONIC_DB_MIN, HARMONIC_DB_MIN, false)],
            thd: None,
            thd_bound: Db(HARMONIC_DB_MIN),
            thd_n: Db(HARMONIC_DB_MIN),
            leakage: None,
        },
    };
    assert_eq!(RATIO_DB_MIN, -200.0);
    assert_eq!(
        text(&h, LevelScale::Dbfs),
        "sine 1000 Hz · on bin\n\
         H1  −6.0 dBFS\n\
         H2  < −200.0 dB\n\
         THD  < −200.0 dB\n\
         THD+N  < −200.0 dB"
    );
}

#[test]
fn an_unreadable_sine_says_why() {
    let why = |r| text(&unreadable(r), LevelScale::Dbfs);
    assert_eq!(
        why(HarmonicUnreadable::RectangularOffBin),
        "sine 1000 Hz · between bins\nnot read: a rectangular window needs the sine on a bin"
    );
    assert_eq!(
        why(HarmonicUnreadable::TooLow),
        "sine 1000 Hz · between bins\nnot read: the sine is too low for this FFT length"
    );
    assert_eq!(
        why(HarmonicUnreadable::OutOfBand),
        "sine 1000 Hz · between bins\nnot read: the sine is above this spectrum's range"
    );
    assert_eq!(
        why(HarmonicUnreadable::NoFundamental),
        "sine 1000 Hz · between bins\nnot read: no sine in this input"
    );
}

#[test]
fn without_a_sine_there_is_no_plate() {
    let s = scene_of(None, &view(PlotChrome::Full));
    assert!(s.harmonics.is_empty());
    assert_eq!(s.harmonic_list, None);
    assert_eq!(s.harmonics_box, None);
    assert!(s.plates().iter().all(|b| b.kind != PlateKind::Harmonics));
}

/// The plate hangs from the plot's right edge under the unit's line, clear of the legend;
/// the fundamental and each resolved harmonic are marked on the curve in their order's
/// colour, the unresolved ones are not.
#[test]
fn the_plate_hangs_on_the_right_and_resolved_harmonics_are_marked() {
    let h = on_bin();
    let s = scene_of(Some(&h), &view(PlotChrome::Full));
    assert_eq!(
        s.harmonic_list.as_deref(),
        Some(text(&h, LevelScale::Dbfs).as_str())
    );
    let b = s.harmonics_box.expect("the harmonics plate");
    assert_eq!(b.kind, PlateKind::Harmonics);
    assert_eq!(b.area, s.plot);
    assert_eq!(b.rect.right(), s.plot.right() - crate::legend::INSET);
    let m = crate::plate::Metrics::new(&Theme::dark(), 1.0);
    assert_eq!(b.rect.y + m.pad_y, s.plot.y + 22.0);
    let legend = s.legend_box.as_ref().expect("legend").plate;
    assert!(!intersects(b.rect, legend.rect));
    assert_eq!(
        s.plates().iter().map(|b| b.kind).collect::<Vec<_>>(),
        [PlateKind::Legend, PlateKind::Harmonics]
    );
    let theme = Theme::dark();
    let labels = &s.scene.layers[2].labels;
    let marks = &s.harmonics[0].marks;
    assert_eq!(
        marks.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
        ["H1", "H2", "H3"]
    );
    let (xm, ym) = (s.x_axis.mapping, s.y_axis.mapping);
    for (m, (hz, level)) in
        marks
            .iter()
            .zip([(1000.0, -12.3), (1995.26, -108.7), (3162.28, -113.5)])
    {
        assert!((m.at[0] - xm.to_px(hz)).abs() < 0.5, "{m:?}");
        assert!((m.at[1] - ym.to_px(level)).abs() < 0.5, "{m:?}");
        let color = crate::distortion::order_color(&theme, m.order);
        // Its name over the mark, and its row on the plate, in the order's colour.
        let named: Vec<_> = labels
            .iter()
            .filter(|l| l.text.starts_with(&format!("{}  ", m.label)) || l.text == m.label)
            .collect();
        assert_eq!(named.len(), 2, "{}", m.label);
        assert!(named.iter().all(|l| l.color == color));
    }
}

/// Traces only (T) hides the marks and the plate, as it hides the peaks'.
#[test]
fn traces_only_hides_the_plate_and_the_marks() {
    let h = on_bin();
    let s = scene_of(Some(&h), &view(PlotChrome::Bare));
    assert_eq!(s.harmonic_list, None);
    assert_eq!(s.harmonics_box, None);
    assert!(s.harmonics[0].marks.is_empty());
    assert!(s.scene.layers[2].labels.iter().all(|l| l.text != "H1"));
}

/// Under the cursor's values and the peak list, covering neither; where they leave no
/// room under them it stands beside them.
#[test]
fn the_plate_goes_under_the_cursor_values_and_the_peak_list() {
    let h = on_bin();
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let peaks = [Peak {
        freq_hz: 1000.0,
        level: -12.3,
    }];
    let scene = |size: Viewport, tracked: bool| {
        let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
        t.harmonics = Some(&h);
        t.tracked = tracked.then_some(&peaks[..]);
        let mut v = view(PlotChrome::Full);
        v.cursor_hz = Some(1000.0);
        spectrum_scene(&[t], &Status::default(), &v, &Theme::dark(), size)
    };
    let s = scene(SIZE, false);
    let readout = s.readout_box.expect("cursor values").rect;
    let b = s.harmonics_box.expect("plate").rect;
    assert!(b.y > readout.bottom());
    assert_eq!(b.right(), s.plot.right() - crate::legend::INSET);

    let s = scene(Viewport::new(800.0, 600.0), true);
    let list = s.peaks_box.expect("peak list").rect;
    let b = s.harmonics_box.expect("plate").rect;
    assert!(b.y > list.bottom());
    assert_eq!(b.right(), s.plot.right() - crate::legend::INSET);
    assert!(b.bottom() <= s.plot.bottom());

    // A low pane: no room under the two, so beside them, covering neither nor the legend.
    let s = scene(Viewport::new(800.0, 220.0), true);
    let b = s.harmonics_box.expect("plate").rect;
    for other in s.plates().iter().filter(|p| p.kind != PlateKind::Harmonics) {
        assert!(!intersects(b, other.rect), "{:?} {b:?}", other.kind);
    }
    let p = s.peaks_box.expect("peak list").rect;
    assert!(b.right() < p.x, "{b:?} {p:?} {:?}", s.plot);
}

/// Dragged to the bottom-left at twice the text size, the plate sits there; the marks
/// stay on the curve.
#[test]
fn a_dragged_plate_stays_where_it_was_dropped() {
    let h = on_bin();
    let auto = scene_of(Some(&h), &view(PlotChrome::Full));
    let mut v = view(PlotChrome::Full);
    v.plates.harmonics.move_to(0.0, 1.0);
    v.plates.harmonics.set_scale(2.0);
    let s = scene_of(Some(&h), &v);
    let b = s.harmonics_box.expect("plate");
    assert_eq!(b.scale, 2.0);
    assert_eq!(b.rect.x, s.plot.x + crate::legend::INSET);
    assert_eq!(b.rect.bottom(), s.plot.bottom() - crate::legend::INSET);
    let row = s.scene.layers[2]
        .labels
        .iter()
        .find(|l| l.text == "THD  −95.1 dB · 0.00176 %")
        .expect("row drawn");
    assert_eq!(row.size, 2.0 * Theme::dark().small_font_size);
    assert_eq!(s.harmonics[0].marks, auto.harmonics[0].marks);
}

/// Two live curves reading the sine: a block each, headed by the curve's name.
#[test]
fn two_curves_each_head_their_block() {
    let (a, b) = (on_bin(), unreadable(HarmonicUnreadable::NoFundamental));
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let mut left = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    left.name = "Main L".into();
    left.harmonics = Some(&a);
    let mut right = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    right.key = TraceKey::Live(MeasId(2));
    right.name = "Main R".into();
    right.harmonics = Some(&b);
    // A curve without a reading (a stored one) has no block.
    let mut stored = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    stored.key = TraceKey::Live(MeasId(3));
    let s = spectrum_scene(
        &[left, stored, right],
        &Status::default(),
        &view(PlotChrome::Full),
        &Theme::dark(),
        SIZE,
    );
    assert_eq!(
        s.harmonic_list,
        Some(format!(
            "Main L\n{}\nMain R\n{}",
            text(&a, LevelScale::Dbfs),
            text(&b, LevelScale::Dbfs)
        ))
    );
    assert_eq!(s.harmonics.len(), 2);
}

/// One curve spread by a display offset is headed too, as its peak rows are.
#[test]
fn an_offset_curve_heads_its_block_with_the_offset() {
    let h = on_bin();
    let g = third_octaves();
    let (f, e) = (g.centres(), g.edges());
    let level = levels(&f);
    let mut t = rta_trace(&f, &e, &level, LevelScale::Dbfs);
    t.harmonics = Some(&h);
    t.offset_db = 3.0;
    let s = spectrum_scene(
        &[t],
        &Status::default(),
        &view(PlotChrome::Full),
        &Theme::dark(),
        SIZE,
    );
    let list = s.harmonic_list.expect("plate");
    assert_eq!(list.lines().next(), Some("RTA · offset +3.0 dB"));
}

#[test]
fn the_hover_hint_names_the_plate() {
    let hint = crate::plate::hint(crate::plate::PlateHover {
        kind: PlateKind::Harmonics,
        grip: false,
    });
    assert_eq!(
        hint,
        "drag: move the harmonics · its corner: text size · double-click: back to its place and size"
    );
}
