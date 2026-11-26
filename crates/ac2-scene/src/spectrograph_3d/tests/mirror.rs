//! Stereo: a second channel mirrored below the floor.

use super::points::{chrome, flat_room};
use super::*;
use crate::spectrograph_3d::pick::spike;
use crate::spectrograph_3d::scene::Projector;
use crate::spectrum::Quantity;
use crate::trace::TraceKey;
use ac2_proto::model::SpectrumStyle;
use ac2_proto::units::MeasId;

const UPPER: TraceKey = TraceKey::Live(MeasId(1));
const LOWER: TraceKey = TraceKey::Live(MeasId(2));

fn trace(key: TraceKey, name: &str, compared: bool) -> SpectrumTrace<'static> {
    SpectrumTrace {
        key,
        name: name.into(),
        color: Color::WHITE,
        freqs: &[],
        edges: &[],
        level: &[],
        drawn: None,
        validity: None,
        peak: None,
        tracked: None,
        scale: LevelScale::Dbfs,
        quantity: Quantity::Band,
        bin_hz: None,
        caption: String::new(),
        freshness: None,
        offset_db: 0.0,
        selected: false,
        compared,
        mic_range: None,
        style: SpectrumStyle::Line,
        layer: u8::from(compared),
        bands: None,
        harmonics: None,
    }
}

type Built = (SpectrographHistory, Skyline);

fn scene_with(upper: &Built, lower: Option<&Built>, view: &ViewState) -> Spectrograph3dScene {
    let input = Spectrograph3dInput {
        key: UPPER,
        history: &upper.0,
        skyline: &upper.1,
        name: "Main L".into(),
        range: Range::new(-80.0, 0.0),
        offset_db: 0.0,
        freshness: None,
        colours: Colormap::Ice,
        camera: Pose::from(SpectrumCamera::DEFAULT),
        tier: SkylineTier::Full,
        tier_forced: false,
        axes: 1.0,
        lead_slots: 0.0,
        mirror: lower.map(|(h, s)| Mirror3dInput {
            key: LOWER,
            history: h,
            skyline: s,
            name: "Main R".into(),
            offset_db: 3.0,
        }),
    };
    // The spectrum's order: the pane's own curve, then the compared one.
    spectrograph_3d_scene(
        &[trace(UPPER, "Main L", false), trace(LOWER, "Main R", true)],
        &Status::default(),
        Ok(&input),
        view,
        &Theme::dark(),
        Viewport::new(1200.0, 600.0),
    )
}

fn skyline(s: &Spectrograph3dScene) -> &Skyline3d {
    s.scene
        .layers
        .iter()
        .flat_map(|l| &l.skylines)
        .next()
        .expect("a skyline")
}

#[test]
fn the_mirror_names_both_channels_and_carries_the_second() {
    let (upper, lower) = (flat_room(), flat_room());
    let s = scene_with(&upper, Some(&lower), &chrome(PlotChrome::Full));
    assert_eq!(s.legend_shown, ["Main L", "Main R · cmp · mirrored"]);
    assert!(
        s.caption
            .starts_with("Main L · mirrored Main R · offset +3.0 dB · last 30 s"),
        "{}",
        s.caption
    );
    let m = skyline(&s).mirror.as_ref().expect("the mirrored channel");
    assert_eq!(
        (m.cols, m.rows),
        (lower.1.cols() as u32, lower.1.rows() as u32)
    );
    // The second channel's own rows, by identity (NaN never equals itself).
    assert_eq!(m.data.len(), lower.1.ring().len());
    assert!(
        m.data
            .iter()
            .zip(lower.1.ring())
            .all(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                (a, b) => a.is_none() && b.is_none(),
            })
    );
    // Its own offset: its displayed levels span the pane's range, as the first's do.
    assert_eq!(m.range, [-83.0, -3.0]);
    // Framed on the floor line: the lake's box, both halves, fills the picture.
    let aspect = f64::from(s.plot.w / s.plot.h);
    let lake = view_proj(Pose::from(SpectrumCamera::DEFAULT), aspect, Stage::Lake);
    assert_eq!(s.view_proj, lake);
    for p in box_corners(Stage::Lake) {
        let n = s.view_proj.ndc(p).expect("in front of the eye");
        assert!(n[0].abs() <= 1.0 && n[1].abs() <= 1.0, "{p:?}: {n:?}");
    }
}

#[test]
fn without_a_mirror_the_view_is_as_before() {
    let upper = flat_room();
    let s = scene_with(&upper, None, &chrome(PlotChrome::Full));
    assert_eq!(s.legend_shown, ["Main L"]);
    assert!(s.caption.starts_with("Main L · last 30 s"), "{}", s.caption);
    assert!(skyline(&s).mirror.is_none());
    let aspect = f64::from(s.plot.w / s.plot.h);
    let floor = view_proj(Pose::from(SpectrumCamera::DEFAULT), aspect, Stage::Floor);
    assert_eq!(s.view_proj, floor);
}

#[test]
fn an_empty_mirror_draws_no_lake_rows() {
    let upper = flat_room();
    let empty = (SpectrographHistory::new(30), Skyline::default());
    let s = scene_with(&upper, Some(&empty), &chrome(PlotChrome::Full));
    assert!(skyline(&s).mirror.is_none());
    // Still named, so the operator sees which channel will hang below.
    assert_eq!(s.legend_shown.len(), 2);
}

#[test]
fn a_pick_never_lands_on_a_mirrored_spike() {
    let (upper, lower) = (flat_room(), flat_room());
    let s = scene_with(&upper, Some(&lower), &chrome(PlotChrome::Full));
    let f = s.pick.expect("a picture");
    let sk = &upper.1;
    let col = cell_at(sk, 1000.0, 0.0).expect("in the span").0;
    let v = sk.row_back(0).expect("a row")[col];
    let (foot, top) = spike(&f, sk.cols(), sk.rows(), col, 0, v).expect("a spike");
    let pj = Projector::new(f.view_proj, f.plot);
    let at = |p: [f64; 3]| Pointer3d {
        pos: pj.point(p).expect("before the eye"),
        px_per_pt: 1.0,
    };
    // The upper tip picks its cell; its reflection, the same distance below the floor,
    // picks nothing.
    assert_eq!(pick(&f, sk, at(top)).map(|p| p.cell), Some((col, 0)));
    let below = [top[0], 2.0 * foot[1] - top[1], top[2]];
    assert_eq!(pick(&f, sk, at(below)), None);
}
