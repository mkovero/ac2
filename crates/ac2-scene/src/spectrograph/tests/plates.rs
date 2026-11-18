//! The plates of the spectrograph alone: the legend, the tracked peaks and the harmonics.

use super::*;

/// Alone, the spectrograph has the legend plate of every view over the curve it draws (the
/// stored one beside it is not drawn here, so it has no row), in the plot; with the
/// measurement's peak mode tracking (P), its peaks are marked on the newest row and listed.
/// In the split the spectrum part's legend and marks are the only ones.
#[test]
fn alone_the_legend_plate_and_tracked_peaks_and_the_split_leaves_them_to_the_spectrum() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    f.push(&mut h, T0, -40.0);
    let key = crate::trace::TraceKey::Live(ac2_proto::units::MeasId(1));
    let trace = |key, name: &str| SpectrumTrace {
        key,
        name: name.into(),
        color: crate::primitives::Color::WHITE,
        freqs: &[],
        edges: &[],
        level: &[],
        drawn: None,
        validity: None,
        peak: None,
        tracked: None,
        scale: LevelScale::Dbfs,
        quantity: crate::spectrum::Quantity::Tone,
        bin_hz: None,
        caption: String::new(),
        freshness: Some(crate::time::Freshness::Stopped { age_s: 1.0 }),
        offset_db: 0.0,
        selected: false,
        compared: false,
        bands: None,
        harmonics: None,
        style: ac2_proto::model::SpectrumStyle::Line,
        layer: 0,
    };
    let stored = crate::trace::TraceKey::Stored(ac2_proto::units::TraceId(7));
    let peaks = [
        crate::spectrum::peaks::Peak {
            freq_hz: 1000.0,
            level: -40.0,
        },
        crate::spectrum::peaks::Peak {
            freq_hz: 100.0,
            level: -50.0,
        },
    ];
    let mut traces = [trace(key, "Main"), trace(stored, "Main S1")];
    traces[0].tracked = Some(crate::spectrum::peaks::Tracked {
        max: &peaks,
        min: &[],
    });
    let input = SpectrographInput {
        key,
        history: &h,
        name: "Main".into(),
        range: Range::new(-100.0, 0.0),
        offset_db: 0.0,
        freshness: None,
        colours: Colormap::default(),
        lead_slots: 0.0,
    };
    let mut view = ViewState::default();
    view.spectrum.mode = SpectrumMode::Spectrograph;
    let size = Viewport::new(800.0, 500.0);
    let at = |view: &ViewState| {
        spectrograph_scene(
            &traces,
            &Status::default(),
            Some(&input),
            view,
            &Theme::dark(),
            size,
        )
    };
    let s = at(&view);
    assert_eq!(s.legend_shown, ["Main · stopped"]);
    let drawn = s.scene.layers.iter().flat_map(|l| &l.labels);
    assert!(drawn.into_iter().any(|l| l.text == "Main · stopped"));
    let [hit] = s.legend_hits[..] else {
        panic!("one row: {:?}", s.legend_hits)
    };
    assert_eq!(hit.key, key);
    let lb = s.legend_box.as_ref().expect("legend plate");
    assert_eq!(lb.rect.x, s.plot.x + crate::legend::INSET);
    assert_eq!(lb.rect.y, s.plot.y + crate::legend::INSET);
    // Tracked: a box and a rank per peak at the plot's top, the list on its plate.
    assert_eq!(s.peaks.len(), 1);
    assert_eq!(s.peaks[0].rows.len(), 2);
    let list = s.peak_list.as_deref().expect("peak list");
    assert!(list.starts_with("max\n1  1.00 kHz  −40.0 dBFS"), "{list}");
    let ranks: Vec<&str> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.labels)
        .filter(|l| l.text == "1" || l.text == "2")
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(ranks, ["1", "2"]);
    assert_eq!(
        s.plates().iter().map(|p| p.kind).collect::<Vec<_>>(),
        [
            crate::plate::PlateKind::Legend,
            crate::plate::PlateKind::Peaks
        ]
    );
    // Traces only: the legend stays, as on every view; the peaks' marks and list go.
    view.chrome = crate::view::PlotChrome::Bare;
    let bare = at(&view);
    assert_eq!(bare.legend_hits.len(), 1);
    assert!(bare.peak_list.is_none() && bare.peaks_box.is_none());
    view.chrome = crate::view::PlotChrome::Full;
    // Hidden, as every legend.
    view.legend.hidden = true;
    let hidden = at(&view);
    assert!(hidden.legend_box.is_none() && hidden.legend_hits.is_empty());
    view.legend.hidden = false;
    view.spectrum.mode = SpectrumMode::Split;
    let s = at(&view);
    assert!(s.legend_hits.is_empty() && s.legend_shown.is_empty());
    assert!(s.peaks.is_empty() && s.peaks_box.is_none());
    let sp = s.spectrum.expect("the spectrum part");
    assert_eq!(sp.legend_hits.len(), 2);
    assert!(sp.peak_list.is_some(), "the spectrum lists them");
}

/// Alone, the spectrograph has the harmonics plate of the spectrum view over the
/// measurement it draws, bottom-left clear of its peak list and the legend, without marks (no
/// level axis); the split leaves it to the spectrum part.
#[test]
fn alone_the_harmonics_plate_and_the_split_leaves_it_to_the_spectrum() {
    use ac2_proto::frame::{HarmonicReading, HarmonicUnreadable, SpecHarmonics};
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    f.push(&mut h, T0, -40.0);
    let key = crate::trace::TraceKey::Live(ac2_proto::units::MeasId(1));
    let reading = SpecHarmonics {
        f0: ac2_proto::units::Hz(1000.0),
        on_bin: false,
        reading: HarmonicReading::Unreadable {
            reason: HarmonicUnreadable::RectangularOffBin,
        },
    };
    let peaks = [crate::spectrum::peaks::Peak {
        freq_hz: 1000.0,
        level: -40.0,
    }];
    let traces = [SpectrumTrace {
        key,
        name: "Main".into(),
        color: crate::primitives::Color::WHITE,
        freqs: &[],
        edges: &[],
        level: &[],
        drawn: None,
        validity: None,
        peak: None,
        tracked: Some(crate::spectrum::peaks::Tracked {
            max: &peaks,
            min: &[],
        }),
        scale: LevelScale::Dbfs,
        quantity: crate::spectrum::Quantity::Tone,
        bin_hz: None,
        caption: String::new(),
        freshness: None,
        offset_db: 0.0,
        selected: false,
        compared: false,
        bands: None,
        harmonics: Some(&reading),
        style: ac2_proto::model::SpectrumStyle::Line,
        layer: 0,
    }];
    let input = SpectrographInput {
        key,
        history: &h,
        name: "Main".into(),
        range: Range::new(-100.0, 0.0),
        offset_db: 0.0,
        freshness: None,
        colours: Colormap::default(),
        lead_slots: 0.0,
    };
    let mut view = ViewState::default();
    view.spectrum.mode = SpectrumMode::Spectrograph;
    let at = |view: &ViewState| {
        spectrograph_scene(
            &traces,
            &Status::default(),
            Some(&input),
            view,
            &Theme::dark(),
            Viewport::new(800.0, 500.0),
        )
    };
    let s = at(&view);
    assert_eq!(
        s.harmonic_list.as_deref(),
        Some("sine 1000 Hz · between bins\nnot read: a rectangular window needs the sine on a bin")
    );
    let b = s.harmonics_box.expect("the harmonics plate");
    assert_eq!(b.kind, crate::plate::PlateKind::Harmonics);
    let list = s.peaks_box.expect("the peak list").rect;
    assert!(b.rect.right() < list.x);
    assert_eq!(b.rect.x, s.plot.x + crate::legend::INSET);
    assert_eq!(b.rect.bottom(), s.plot.bottom() - crate::legend::INSET);
    assert_eq!(
        s.plates().iter().map(|p| p.kind).collect::<Vec<_>>(),
        [
            crate::plate::PlateKind::Legend,
            crate::plate::PlateKind::Peaks,
            crate::plate::PlateKind::Harmonics
        ]
    );
    view.chrome = crate::view::PlotChrome::Bare;
    let bare = at(&view);
    assert!(bare.harmonic_list.is_none() && bare.harmonics_box.is_none());
    view.chrome = crate::view::PlotChrome::Full;
    view.spectrum.mode = SpectrumMode::Split;
    let s = at(&view);
    assert!(s.harmonic_list.is_none() && s.harmonics_box.is_none());
    let sp = s.spectrum.expect("the spectrum part");
    assert!(sp.harmonic_list.is_some(), "the spectrum has the plate");
}
