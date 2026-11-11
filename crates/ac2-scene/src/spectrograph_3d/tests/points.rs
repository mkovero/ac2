//! Peak points on the newest row, and the legend row that picks them.

use super::*;
use crate::spectrum::Quantity;
use crate::spectrum::peaks::Peak;
use crate::trace::TraceKey;
use crate::view::PeakPoints;
use ac2_proto::model::SpectrumStyle;
use ac2_proto::units::MeasId;

const KEY: TraceKey = TraceKey::Live(MeasId(1));

/// Every cell at −30 dB below 5 kHz, none above it.
fn flat_room() -> (SpectrographHistory, Skyline) {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    // A part-filled newest row: its points slide back with the rows' phase.
    for slot in 0..(h.slots() as u64 + 1) {
        let l: Vec<f32> = f
            .edges
            .iter()
            .map(|e| if e.0 < 5000.0 { -30.0 } else { f32::NAN })
            .collect();
        f.push_with(&mut h, slot, &l, None);
    }
    let mut s = Skyline::default();
    s.update(&h, full_range(), SkylineTier::Full);
    (h, s)
}

fn trace(peaks: &[Peak]) -> SpectrumTrace<'_> {
    SpectrumTrace {
        key: KEY,
        name: "Main L".into(),
        color: Color::WHITE,
        freqs: &[],
        edges: &[],
        level: &[],
        drawn: None,
        validity: None,
        peak: None,
        tracked: Some(peaks),
        scale: LevelScale::Dbfs,
        quantity: Quantity::Band,
        bin_hz: None,
        caption: String::new(),
        freshness: None,
        offset_db: 0.0,
        selected: false,
        compared: false,
        style: SpectrumStyle::Line,
        layer: 0,
        bands: None,
    }
}

fn picked(chrome: PlotChrome) -> ViewState {
    ViewState {
        chrome,
        peak_points: Some(PeakPoints {
            key: KEY,
            list: false,
            named: false,
        }),
        ..ViewState::default()
    }
}

fn scene(
    (h, s): &(SpectrographHistory, Skyline),
    peaks: &[Peak],
    view: &ViewState,
    axes: f32,
) -> Spectrograph3dScene {
    let input = Spectrograph3dInput {
        key: KEY,
        history: h,
        skyline: s,
        name: "Main L".into(),
        range: Range::new(-80.0, 0.0),
        offset_db: 0.0,
        freshness: None,
        colours: Colormap::Ice,
        camera: Pose::from(SpectrumCamera::DEFAULT),
        tier: SkylineTier::Full,
        tier_forced: false,
        axes,
        lead_slots: 0.0,
    };
    spectrograph_3d_scene(
        &[trace(peaks)],
        &Status::default(),
        Some(&input),
        view,
        &Theme::dark(),
        Viewport::new(1200.0, 600.0),
    )
}

fn peak(freq_hz: f64, level: f64) -> Peak {
    Peak { freq_hz, level }
}

#[test]
fn points_stand_on_the_sliding_newest_row_inside_the_plot() {
    let b = flat_room();
    let peaks = [peak(1000.0, -40.0), peak(100.0, -50.0)];
    let s = scene(&b, &peaks, &picked(PlotChrome::Full), 1.0);
    assert_eq!(s.points.len(), 2);
    let z = f64::from(b.1.phase()) * FLOOR_DEPTH / b.1.rows() as f64;
    assert!(b.1.phase() < 1.0, "a part-filled row");
    let inside = |p: [f32; 2]| {
        p[0] >= s.plot.x && p[0] <= s.plot.right() && p[1] >= s.plot.y && p[1] <= s.plot.bottom()
    };
    for p in &s.points {
        assert!((p.world[2] - z).abs() < 1e-12, "{p:?}");
        assert!(inside(p.pos), "{p:?}");
    }
    // Below the cells' top, a point stands at its own level: −40 of −80 … 0.
    assert!((s.points[0].world[1] - MAX_HEIGHT * 0.5).abs() < 1e-9);
    assert_eq!(s.points[0].label.as_deref(), Some("1.00 kHz  −40.0 dBFS"));
    assert!(
        s.points[0].pos[0] > s.points[1].pos[0],
        "1 kHz right of 100 Hz"
    );
}

#[test]
fn a_point_is_clamped_to_its_spike_and_keeps_its_value() {
    let b = flat_room();
    let peaks = [peak(1000.0, -10.0)];
    let s = scene(&b, &peaks, &picked(PlotChrome::Full), 1.0);
    let p = &s.points[0];
    // The cell's spike tops at −30: 50 of 80 dB.
    assert!(
        (p.world[1] - MAX_HEIGHT * 50.0 / 80.0).abs() < 1e-9,
        "{p:?}"
    );
    assert_eq!(p.label.as_deref(), Some("1.00 kHz  −10.0 dBFS"));
}

#[test]
fn no_point_over_an_empty_cell_off_the_range_or_with_a_message() {
    let b = flat_room();
    // 8 kHz: no level in its cell; 1 kHz at −90: under the range.
    let peaks = [peak(8000.0, -40.0), peak(1000.0, -90.0)];
    let s = scene(&b, &peaks, &picked(PlotChrome::Full), 1.0);
    assert!(s.points.is_empty(), "{:?}", s.points);
    let empty = (SpectrographHistory::new(30), Skyline::default());
    let s = scene(
        &empty,
        &[peak(1000.0, -40.0)],
        &picked(PlotChrome::Full),
        1.0,
    );
    assert!(s.message.is_some());
    assert!(s.points.is_empty());
}

#[test]
fn points_show_at_rest_and_traces_only_hides_them() {
    let b = flat_room();
    let peaks = [peak(1000.0, -40.0)];
    // At rest: the axes are hidden, the points are not.
    let s = scene(&b, &peaks, &picked(PlotChrome::Full), 0.0);
    assert!(s.labels.is_empty());
    assert_eq!(s.points.len(), 1);
    let drawn = |s: &Spectrograph3dScene| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.labels)
            .any(|l| l.text == "1.00 kHz  −40.0 dBFS")
    };
    assert!(drawn(&s));
    // Traces only: none, even with the axes revealed.
    let s = scene(&b, &peaks, &picked(PlotChrome::Bare), 1.0);
    assert!(s.points.is_empty());
    assert!(!drawn(&s));
    // Not picked: none.
    let s = scene(&b, &peaks, &ViewState::default(), 1.0);
    assert!(s.points.is_empty());
    assert_eq!(s.picked_cue, None);
}

#[test]
fn the_legend_row_picks_and_carries_the_cue() {
    let b = flat_room();
    let s = scene(&b, &[], &picked(PlotChrome::Full), 0.0);
    // The spectrum's legend row, above the caption; it stays at rest with the axes hidden.
    assert_eq!(s.legend_shown, ["Main L"]);
    let [hit] = s.legend_hits[..] else {
        panic!("one row: {:?}", s.legend_hits)
    };
    assert_eq!(hit.key, KEY);
    assert!(hit.rect.bottom() < s.plot.y - 16.0, "above the caption");
    assert_eq!(s.picked_cue, Some(KEY));
    let not_picked = scene(&b, &[], &ViewState::default(), 0.0);
    assert_eq!(not_picked.legend_hits.len(), 1);
    assert_eq!(not_picked.picked_cue, None);
    // Traces only hides it, as it hides the points.
    let bare = scene(&b, &[], &picked(PlotChrome::Bare), 1.0);
    assert!(bare.legend_shown.is_empty() && bare.legend_hits.is_empty());
    assert_eq!(bare.picked_cue, None);
    assert!(bare.plot.y < s.plot.y, "the plot takes the legend's room");
    // No measurement: no legend.
    let none = spectrograph_3d_scene(
        &[],
        &Status::default(),
        None,
        &ViewState::default(),
        &Theme::dark(),
        Viewport::new(1200.0, 600.0),
    );
    assert!(none.caption.is_empty());
    assert!(none.legend_hits.is_empty());
}
