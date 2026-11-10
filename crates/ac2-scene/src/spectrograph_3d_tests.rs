use super::*;
use crate::banner::Status;
use crate::primitives::{Layer, Skyline3d};
use crate::spectrograph::SpectrographFrame;
use crate::view::FreqRange;
use ac2_proto::GridDef;
use ac2_proto::GridId;
use ac2_proto::frame::ValidityMask;
use ac2_proto::model::LevelScale;
use ac2_proto::units::{Hz, WallNs};

/// Frames one slot apart.
const SLOT_NS: u64 = 1_000_000_000 / 30;
const T0: u64 = 1_000 * 1_000_000_000;

fn log_bins() -> GridDef {
    GridDef::LogBins {
        fs: Hz(48_000.0),
        n: 16_384,
        ppo: 96,
    }
}

fn third_octaves() -> GridDef {
    GridDef::IecBands {
        fraction: ac2_proto::model::BandFraction::Third,
        centres: (-17..=13)
            .map(|x| Hz(1000.0 * 10f64.powf(f64::from(x) / 10.0)))
            .collect(),
    }
}

struct Feed {
    edges: Vec<(f64, f64)>,
    grid: GridId,
    seq: u64,
}

impl Feed {
    fn new(g: &GridDef) -> Self {
        Self {
            edges: g.edges(),
            grid: g.id(),
            seq: 0,
        }
    }

    fn push_with(
        &mut self,
        h: &mut SpectrographHistory,
        slot: u64,
        level: &[f32],
        validity: Option<&[ValidityMask]>,
    ) {
        self.seq += 1;
        h.push(&SpectrographFrame {
            seq: self.seq,
            at: WallNs(T0 + slot * SLOT_NS + SLOT_NS / 2),
            grid: self.grid,
            edges: &self.edges,
            scale: LevelScale::Dbfs,
            spec_level: None,
            level,
            validity,
        });
    }

    fn push(&mut self, h: &mut SpectrographHistory, slot: u64, level: f32) {
        let l = vec![level; self.edges.len()];
        self.push_with(h, slot, &l, None);
    }

    /// Every column at `floor`, the one holding `hz` at `peak`.
    fn tone(&mut self, h: &mut SpectrographHistory, slot: u64, hz: f64, floor: f32, peak: f32) {
        let l: Vec<f32> = self
            .edges
            .iter()
            .map(|e| if e.0 <= hz && hz < e.1 { peak } else { floor })
            .collect();
        self.push_with(h, slot, &l, None);
    }
}

fn full_range() -> Range {
    Range::new(20.0, 20_000.0)
}

fn same(a: &Skyline, b: &Skyline) -> bool {
    let rows = |s: &Skyline| -> Vec<Option<Vec<u32>>> {
        (0..s.rows())
            .map(|j| {
                s.row_back(j)
                    .map(|r| r.iter().map(|v| v.to_bits()).collect())
            })
            .collect()
    };
    a.cols() == b.cols()
        && a.rows() == b.rows()
        && a.newest() == b.newest()
        && a.phase() == b.phase()
        && rows(a) == rows(b)
}

fn rebuilt(h: &SpectrographHistory, freq: Range, tier: SkylineTier) -> Skyline {
    let mut s = Skyline::default();
    s.update(h, freq, tier);
    s
}

#[test]
fn rows_are_whole_slots_per_span_and_tier() {
    let table = [
        (10, SkylineTier::Full, 3, 100),
        (30, SkylineTier::Full, 9, 100),
        (60, SkylineTier::Full, 19, 95),
        (120, SkylineTier::Full, 38, 95),
        (10, SkylineTier::Lite, 6, 50),
        (30, SkylineTier::Lite, 19, 48),
        (60, SkylineTier::Lite, 38, 48),
        (120, SkylineTier::Bare, 75, 48),
    ];
    for (span, tier, k, r) in table {
        let slots = crate::spectrograph::slots(span);
        assert_eq!(bucket_slots(slots, tier), k, "{span} s {tier:?}");
        assert_eq!(ring_rows(slots, k), r + 1, "{span} s {tier:?}");
    }
    assert_eq!(grid(SkylineTier::Full).0, 192);
    assert_eq!(grid(SkylineTier::Lite).0, 128);
}

#[test]
fn a_tone_stands_in_one_column_and_bands_fill_theirs() {
    let mut h = SpectrographHistory::new(10);
    let mut f = Feed::new(&log_bins());
    f.tone(&mut h, 0, 1000.0, -90.0, -10.0);
    let s = rebuilt(&h, full_range(), SkylineTier::Full);
    assert_eq!(s.cols(), 192);
    let row = s.row_back(0).expect("the newest row");
    let hot: Vec<usize> = (0..row.len()).filter(|&i| row[i] > -50.0).collect();
    assert_eq!(hot.len(), 1, "{hot:?}");
    let x = hot[0] as f64 + 0.5;
    let hz = 20.0 * 1000f64.powf(x / 192.0);
    assert!((hz / 1000.0).log2().abs() < 1.0 / 19.0, "{hz}");

    // A third-octave band is wider than a column: it fills several; an invalid band is a gap.
    let g = third_octaves();
    let mut h = SpectrographHistory::new(10);
    let mut f = Feed::new(&g);
    let n = f.edges.len();
    let mut level = vec![-60.0; n];
    let mut valid = vec![ValidityMask::NONE; n];
    let bad = 10;
    level[bad] = -20.0;
    valid[bad] = ValidityMask(1);
    let loud = 5;
    level[loud] = -5.0;
    f.push_with(&mut h, 0, &level, Some(&valid));
    let s = rebuilt(&h, full_range(), SkylineTier::Full);
    let row = s.row_back(0).expect("the newest row");
    assert!(row.iter().filter(|v| **v == -5.0).count() >= 4);
    let (blo, bhi) = f.edges[bad];
    let gap = (0..row.len())
        .filter(|&i| {
            let c = 20.0 * 1000f64.powf((i as f64 + 0.5) / 192.0);
            c > blo * 1.02 && c < bhi / 1.02
        })
        .collect::<Vec<_>>();
    assert!(!gap.is_empty());
    assert!(gap.iter().all(|&i| row[i].is_nan()), "{gap:?}");
}

#[test]
fn rows_are_the_highest_level_of_their_slots_anchored_on_absolute_slots() {
    let mut h = SpectrographHistory::new(10);
    let mut f = Feed::new(&log_bins());
    let k = bucket_slots(h.slots(), SkylineTier::Full) as u64;
    assert_eq!(k, 3);
    let mut s = Skyline::default();
    // Start mid-row: slot 4 is the second slot of row 1.
    for slot in 4..10 {
        f.push(&mut h, slot, -(slot as f32));
        s.update(&h, full_range(), SkylineTier::Full);
    }
    // Rows by absolute slot: 3–5 (4, 5 in), 6–8, 9 (newest, one slot filled).
    assert_eq!(s.row_back(0).expect("row 3")[50], -9.0);
    assert_eq!(s.row_back(1).expect("row 2")[50], -6.0);
    assert_eq!(s.row_back(2).expect("row 1")[50], -4.0);
    assert!(s.row_back(3).is_none(), "no frames before slot 4");
    assert_eq!(s.phase(), 1.0 / 3.0);
    f.push(&mut h, 10, 0.0);
    s.update(&h, full_range(), SkylineTier::Full);
    assert_eq!(s.phase(), 2.0 / 3.0);
    assert_eq!(s.row_back(0).expect("row 3")[50], 0.0);
    assert_eq!(s.row_back(1).expect("row 2")[50], -6.0);
}

#[test]
fn the_oldest_row_drops_slots_as_they_leave_the_history() {
    let mut h = SpectrographHistory::new(10);
    let mut f = Feed::new(&log_bins());
    let n = h.slots() as u64;
    let mut s = Skyline::default();
    // Slot 0 is loud, the rest quiet: once slot 0 leaves, its row is quiet too.
    f.push(&mut h, 0, 0.0);
    for slot in 1..n {
        f.push(&mut h, slot, -50.0);
    }
    s.update(&h, full_range(), SkylineTier::Full);
    let oldest = |s: &Skyline| {
        (0..s.rows())
            .rev()
            .find_map(|j| s.row_back(j).map(|r| r[50]))
            .expect("a row")
    };
    assert_eq!(oldest(&s), 0.0);
    f.push(&mut h, n, -50.0);
    s.update(&h, full_range(), SkylineTier::Full);
    assert_eq!(oldest(&s), -50.0);
    assert!(same(&s, &rebuilt(&h, full_range(), SkylineTier::Full)));
}

#[test]
fn gaps_are_empty_rows() {
    let mut h = SpectrographHistory::new(10);
    let mut f = Feed::new(&log_bins());
    f.push(&mut h, 0, -20.0);
    h.mark_break();
    f.push(&mut h, 30, -20.0);
    let s = rebuilt(&h, full_range(), SkylineTier::Full);
    let empty = (0..s.rows()).filter(|j| s.row_back(*j).is_none()).count();
    assert!(empty >= 8, "{empty}");
}

/// A small deterministic generator for the folding property.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

#[test]
fn folding_frame_by_frame_equals_rebuilding() {
    for (seed, span, tier) in [
        (1, 10, SkylineTier::Full),
        (2, 30, SkylineTier::Lite),
        (3, 60, SkylineTier::Full),
        (4, 120, SkylineTier::Bare),
    ] {
        let mut rng = Lcg(seed);
        let mut h = SpectrographHistory::new(span);
        let mut f = Feed::new(&third_octaves());
        let mut s = Skyline::default();
        let mut slot = 0;
        for i in 0..900 {
            match rng.next(20) {
                // Two frames in one slot, a skip of a few slots, a break, a long jump.
                0 => {}
                1 => slot += rng.next(9),
                2 => h.mark_break(),
                3 if i % 300 == 0 => slot += h.slots() as u64 + 5,
                _ => slot += 1,
            }
            let n = f.edges.len();
            let l: Vec<f32> = (0..n).map(|_| -(rng.next(100) as f32)).collect();
            f.push_with(&mut h, slot, &l, None);
            s.update(&h, full_range(), tier);
            assert!(
                same(&s, &rebuilt(&h, full_range(), tier)),
                "seed {seed} frame {i}"
            );
        }
    }
}

#[test]
fn span_grid_tier_or_frequency_changes_rebuild() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    for slot in 0..200 {
        f.tone(&mut h, slot, 1000.0, -80.0, -(slot as f32 % 30.0));
    }
    let mut s = Skyline::default();
    s.update(&h, full_range(), SkylineTier::Full);
    let narrow = Range::new(500.0, 2000.0);
    s.update(&h, narrow, SkylineTier::Full);
    assert!(same(&s, &rebuilt(&h, narrow, SkylineTier::Full)));
    assert_eq!(s.span_hz(), Some((500.0, 2000.0)));
    s.update(&h, narrow, SkylineTier::Lite);
    assert_eq!(s.cols(), 128);
    assert!(same(&s, &rebuilt(&h, narrow, SkylineTier::Lite)));
    // A new grid (an RTA) starts the history over: the skyline with it.
    let mut g = Feed::new(&third_octaves());
    g.push(&mut h, 300, -30.0);
    s.update(&h, narrow, SkylineTier::Lite);
    assert!(same(&s, &rebuilt(&h, narrow, SkylineTier::Lite)));
    assert_eq!(
        (0..s.rows()).filter(|j| s.row_back(*j).is_some()).count(),
        1
    );
    // Cleared: nothing.
    h.clear();
    s.update(&h, narrow, SkylineTier::Lite);
    assert!((0..s.rows()).all(|j| s.row_back(j).is_none()));
}

#[test]
fn the_phase_advances_per_frame_from_the_data() {
    let mut h = SpectrographHistory::new(120);
    let mut f = Feed::new(&log_bins());
    let mut s = Skyline::default();
    let k = bucket_slots(h.slots(), SkylineTier::Full) as u64;
    assert_eq!(k, 38);
    let mut phases = Vec::new();
    for slot in 38..42 {
        f.push(&mut h, slot, -20.0);
        s.update(&h, full_range(), SkylineTier::Full);
        phases.push(s.phase());
    }
    // The bucket boundary sits where the absolute slot count says, so the first phase is
    // wherever that lands; each frame after it moves one slot's share.
    assert!(
        phases[0] > 0.0 && phases[0] <= 1.0 - 3.0 / 38.0,
        "{phases:?}"
    );
    for (i, p) in phases.iter().enumerate() {
        assert!((p - phases[0] - i as f32 / 38.0).abs() < 1e-6, "{phases:?}");
    }
    // No new frame, no motion.
    s.update(&h, full_range(), SkylineTier::Full);
    assert_eq!(s.phase(), phases[3]);
}

#[test]
fn camera_steps_clamp_and_round_trip() {
    let d = SpectrumCamera::DEFAULT;
    assert_eq!(
        step(d, 1, 1, 1),
        SpectrumCamera {
            yaw_deg: 15,
            pitch_deg: 13,
            zoom_step: 1
        }
    );
    let far = step(d, 100, 100, 100);
    assert_eq!((far.yaw_deg, far.pitch_deg, far.zoom_step), (80, 75, 4));
    let near = step(d, -100, -100, -100);
    assert_eq!((near.yaw_deg, near.pitch_deg, near.zoom_step), (-80, 2, -8));
    assert!(far.check().is_ok() && near.check().is_ok());
    // A drag follows the pointer continuously and snaps to whole degrees on release.
    let p = drag(Pose::from(d), 10.0, -5.0);
    assert!((p.yaw_deg + 3.0).abs() < 1e-9 && (p.pitch_deg - 9.0).abs() < 1e-9);
    let p = drag(Pose::from(d), 1.0, 0.0);
    assert_eq!(p.snapped().yaw_deg, 0);
    assert_eq!(Pose::from(far).snapped(), far);
    assert_eq!(
        drag(Pose::from(d), -1e6, 1e6).snapped(),
        SpectrumCamera {
            yaw_deg: 80,
            pitch_deg: 2,
            zoom_step: 0
        }
    );
}

#[test]
fn the_eye_stays_outside_the_skyline_at_every_clamp_corner() {
    for aspect in [0.6, 1.6, 4.0, 12.0] {
        for yaw in [-80, 0, 80] {
            for pitch in [2, 75] {
                for zoom in [-8, 4] {
                    let c = SpectrumCamera {
                        yaw_deg: yaw,
                        pitch_deg: pitch,
                        zoom_step: zoom,
                    };
                    let vp = view_proj(Pose::from(c), aspect);
                    assert!(
                        outside_box(vp.eye, EYE_CLEARANCE * 0.99),
                        "{c:?} at {aspect}: {:?}",
                        vp.eye
                    );
                    // The clamp moves the eye, never the stored step.
                    assert_eq!(Pose::from(c).snapped(), c);
                }
            }
        }
    }
}

#[test]
fn the_default_camera_frames_the_floor() {
    for aspect in [1.0, 1.78, 3.0] {
        let vp = view_proj(Pose::from(SpectrumCamera::DEFAULT), aspect);
        for p in box_corners() {
            let n = vp.ndc(p).expect("in front of the eye");
            assert!(
                n[0].abs() <= 1.0 && n[1].abs() <= 1.0,
                "{p:?} at {aspect}: {n:?}"
            );
        }
        // Centred up and down: no dead band under a low camera's floor.
        let ys: Vec<f64> = box_corners()
            .iter()
            .map(|p| vp.ndc(*p).expect("in front")[1])
            .collect();
        let lo = ys.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!((lo + hi).abs() < 1e-9, "{lo} … {hi} at {aspect}");
        // Frequency reads left to right; the newest row is nearer than the oldest.
        let l = vp.ndc([-1.0, 0.0, 0.0]).expect("left");
        let r = vp.ndc([1.0, 0.0, 0.0]).expect("right");
        assert!(l[0] < -0.5 && r[0] > 0.5);
        assert!(vp.clip([0.0, 0.0, 0.0])[3] < vp.clip([0.0, 0.0, FLOOR_DEPTH])[3]);
    }
    // The camera's right is world +x at every yaw and pitch: 20 Hz on the left, never
    // mirrored.
    for yaw in [-80, -40, 0, 40, 80] {
        for pitch in [2, 8, 75] {
            let vp = view_proj(
                Pose::from(SpectrumCamera {
                    yaw_deg: yaw,
                    pitch_deg: pitch,
                    zoom_step: 0,
                }),
                1.78,
            );
            let mid = FLOOR_DEPTH / 2.0;
            let l = vp.ndc([-0.1, 0.0, mid]).expect("left");
            let r = vp.ndc([0.1, 0.0, mid]).expect("right");
            assert!(l[0] < r[0], "yaw {yaw} pitch {pitch}: {l:?} {r:?}");
        }
    }
    // Zooming in brings the eye closer.
    let a = eye_distance(Pose::from(SpectrumCamera::DEFAULT), 1.78);
    let b = eye_distance(
        Pose {
            zoom_step: 2.0,
            ..Pose::from(SpectrumCamera::DEFAULT)
        },
        1.78,
    );
    assert!((b / a - 2f64.powf(-0.25)).abs() < 1e-9);
}

#[test]
fn auto_tier_by_adapter() {
    let f = |software, name: &str, max, integrated| AdapterFacts {
        software,
        name: name.into(),
        max_texture_2d: max,
        integrated,
    };
    assert_eq!(
        auto_tier(&f(true, "llvmpipe", 8192, false)),
        Some(SkylineTier::Bare)
    );
    assert_eq!(
        auto_tier(&f(false, "V3D 4.2", 4096, true)),
        None,
        "V3D on GLES: no tier fills in time"
    );
    assert_eq!(
        auto_tier(&f(false, "V3D 4.2.14.0", 8192, false)),
        None,
        "v3dv Vulkan"
    );
    assert_eq!(
        auto_tier(&f(false, "Some iGPU", 4096, false)),
        Some(SkylineTier::Lite)
    );
    assert_eq!(
        auto_tier(&f(false, "Intel UHD 620", 16384, true)),
        Some(SkylineTier::Lite)
    );
    assert_eq!(
        auto_tier(&f(false, "Apple M2", 16384, true)),
        Some(SkylineTier::Full)
    );
    assert_eq!(
        auto_tier(&f(false, "NVIDIA RTX", 32768, false)),
        Some(SkylineTier::Full)
    );
    assert_eq!(
        Quality::Auto.tier(Some(SkylineTier::Lite)),
        Some((SkylineTier::Lite, false))
    );
    assert_eq!(
        Quality::Bare.tier(Some(SkylineTier::Full)),
        Some((SkylineTier::Bare, true))
    );
    assert_eq!(
        Quality::Full.tier(Some(SkylineTier::Bare)),
        Some((SkylineTier::Full, true))
    );
    // An adapter without the view: no setting brings it back.
    for q in Quality::ALL {
        assert_eq!(q.tier(None), None, "{q:?}");
    }
    assert_eq!(Quality::Bare.step(1), Quality::Auto);
}

struct Built {
    h: SpectrographHistory,
    s: Skyline,
}

fn pink_room(span: u32, tier: SkylineTier) -> Built {
    let mut h = SpectrographHistory::new(span);
    let mut f = Feed::new(&log_bins());
    for slot in 0..h.slots() as u64 {
        let l: Vec<f32> = f
            .edges
            .iter()
            .map(|e| (-30.0 - 3.0 * (e.0.max(20.0) / 1000.0).log2()) as f32)
            .collect();
        f.push_with(&mut h, slot, &l, None);
    }
    let mut s = Skyline::default();
    s.update(&h, full_range(), tier);
    Built { h, s }
}

fn scene_3d(
    b: &Built,
    view: &ViewState,
    camera: SpectrumCamera,
    forced: bool,
) -> Spectrograph3dScene {
    let tier = if forced {
        SkylineTier::Lite
    } else {
        SkylineTier::Full
    };
    scene_3d_as(b, view, camera, tier, forced, Colormap::Ice)
}

fn scene_3d_as(
    b: &Built,
    view: &ViewState,
    camera: SpectrumCamera,
    tier: SkylineTier,
    forced: bool,
    colours: Colormap,
) -> Spectrograph3dScene {
    let input = Spectrograph3dInput {
        history: &b.h,
        skyline: &b.s,
        name: "Main L".into(),
        range: Range::new(-80.0, 0.0),
        offset_db: 0.0,
        freshness: None,
        colours,
        camera: Pose::from(camera),
        tier,
        tier_forced: forced,
    };
    spectrograph_3d_scene(
        &[],
        &Status::default(),
        Some(&input),
        view,
        &Theme::dark(),
        Viewport {
            width: 1200.0,
            height: 600.0,
        },
    )
}

fn texts(s: &Spectrograph3dScene, axis: LabelAxis) -> Vec<&str> {
    s.labels
        .iter()
        .filter(|l| l.axis == axis)
        .map(|l| l.text.as_str())
        .collect()
}

fn skylines(s: &Scene) -> Vec<&Skyline3d> {
    s.layers.iter().flat_map(|l| &l.skylines).collect()
}

#[test]
fn labels_read_frequency_time_and_level() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    let s = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    let f = texts(&s, LabelAxis::Frequency);
    for want in ["100", "1k", "10k"] {
        assert!(f.contains(&want), "{want} in {f:?}");
    }
    let t = texts(&s, LabelAxis::Time);
    assert!(t.contains(&"now"), "{t:?}");
    assert!(
        t.iter().any(|l| l.starts_with('−') && l.ends_with(" s")),
        "{t:?}"
    );
    let l = texts(&s, LabelAxis::Level);
    assert!(l.len() >= 3, "{l:?}");
    assert_eq!(texts(&s, LabelAxis::Unit), ["dBFS"]);
    // Frequency reads left to right.
    let x = |t: &str| {
        s.labels
            .iter()
            .find(|l| l.axis == LabelAxis::Frequency && l.text == t)
            .map(|l| l.pos[0])
            .expect("label")
    };
    assert!(x("100") < x("1k") && x("1k") < x("10k"));
    assert!(s.time_right);
}

/// No label is cut by the field's edge, at any pane size or camera, and the height scale's
/// numbers stand on one side of its post.
#[test]
fn labels_stay_whole_inside_the_field() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    for (w, h) in [
        (1570.0, 950.0),
        (1200.0, 600.0),
        (730.0, 430.0),
        (420.0, 300.0),
    ] {
        for (yaw, pitch) in [(0, 8), (-35, 30), (80, 2), (-80, 75)] {
            let input = Spectrograph3dInput {
                history: &b.h,
                skyline: &b.s,
                name: "Main L".into(),
                range: Range::new(-100.0, 0.0),
                offset_db: 0.0,
                freshness: None,
                colours: Colormap::Ice,
                camera: Pose::from(SpectrumCamera {
                    yaw_deg: yaw,
                    pitch_deg: pitch,
                    zoom_step: 0,
                }),
                tier: SkylineTier::Full,
                tier_forced: false,
            };
            let s = spectrograph_3d_scene(
                &[],
                &Status::default(),
                Some(&input),
                &view,
                &Theme::dark(),
                Viewport {
                    width: w,
                    height: h,
                },
            );
            let drawn: Vec<&Label> = s
                .scene
                .layers
                .iter()
                .flat_map(|l| &l.labels)
                .filter(|l| s.labels.iter().any(|a| a.pos == l.pos && a.text == l.text))
                .collect();
            assert_eq!(drawn.len(), s.labels.len(), "{w}×{h} {yaw}/{pitch}");
            for l in &drawn {
                let r = label_box(l);
                assert!(
                    r.x >= s.plot.x
                        && r.right() <= s.plot.right()
                        && r.y >= s.plot.y
                        && r.bottom() <= s.plot.bottom(),
                    "{:?} at {w}×{h} {yaw}/{pitch}: {r:?} outside {:?}",
                    l.text,
                    s.plot
                );
            }
            let sides: Vec<HAlign> = s
                .labels
                .iter()
                .filter(|a| a.axis == LabelAxis::Level)
                .filter_map(|a| drawn.iter().find(|l| l.pos == a.pos).map(|l| l.anchor.h))
                .collect();
            assert!(
                sides.windows(2).all(|p| p[0] == p[1]),
                "{w}×{h} {yaw}/{pitch}: {sides:?}"
            );
            // A 1080p pane at the default camera: the numbers outside the post (right of
            // its unit), the scale whole up to the range's top.
            if w > 1500.0 && yaw == 0 {
                let unit_x = s
                    .labels
                    .iter()
                    .find(|a| a.axis == LabelAxis::Unit)
                    .map(|a| a.pos[0])
                    .expect("unit");
                assert_eq!(sides.first(), Some(&HAlign::Left));
                assert!(
                    s.labels
                        .iter()
                        .filter(|a| a.axis == LabelAxis::Level)
                        .all(|a| a.pos[0] > unit_x),
                    "{:?}",
                    s.labels
                );
                assert!(texts(&s, LabelAxis::Level).contains(&"0"));
            }
        }
    }
}

#[test]
fn the_time_axis_follows_the_eye_and_decades_survive_a_steep_yaw() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    let at = |yaw| {
        scene_3d(
            &b,
            &view,
            SpectrumCamera {
                yaw_deg: yaw,
                ..SpectrumCamera::DEFAULT
            },
            false,
        )
    };
    let now = |s: &Spectrograph3dScene| {
        s.labels
            .iter()
            .find(|l| l.text == "now")
            .map(|l| l.pos[0])
            .expect("now")
    };
    let (left, right) = (at(-40), at(40));
    assert!(!left.time_right && right.time_right);
    assert!(now(&left) < left.plot.x + left.plot.w / 2.0);
    assert!(now(&right) > right.plot.x + right.plot.w / 2.0);
    let steep = at(80);
    let f = texts(&steep, LabelAxis::Frequency);
    assert!(!f.is_empty());
    assert!(f.len() < texts(&at(0), LabelAxis::Frequency).len());
    // Thinned: no two labels share a place.
    for (i, a) in steep.labels.iter().enumerate() {
        for b in &steep.labels[i + 1..] {
            assert!(
                (a.pos[0] - b.pos[0]).abs() > 1.0 || (a.pos[1] - b.pos[1]).abs() > 1.0,
                "{a:?} {b:?}"
            );
        }
    }
}

#[test]
fn chrome_levels_and_draw_order() {
    let b = pink_room(30, SkylineTier::Full);
    let mut view = ViewState::default();
    let full = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    let lines = |s: &Spectrograph3dScene| -> usize {
        s.scene.layers.iter().map(|l| l.polylines.len()).sum()
    };
    let dashed = |s: &Spectrograph3dScene| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.polylines)
            .filter(|p| p.stroke.dash.is_some())
            .count()
    };
    assert_eq!(dashed(&full), 1, "the ceiling line");
    view.chrome = PlotChrome::NoGrid;
    let no_grid = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    assert!(lines(&no_grid) < lines(&full));
    assert_eq!(dashed(&no_grid), 1);
    assert!(!no_grid.labels.is_empty());
    view.chrome = PlotChrome::Bare;
    let bare = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    assert!(bare.labels.is_empty());
    assert_eq!(dashed(&bare), 0);
    assert_eq!(skylines(&bare.scene).len(), 1, "spikes stay");
    // Field and floor band under the spikes; scale, ceiling and labels over them.
    let idx = |pred: &dyn Fn(&Layer) -> bool| full.scene.layers.iter().position(pred);
    let field = idx(&|l| l.rects.iter().any(|r| r.color == FIELD)).expect("field");
    let band = idx(&|l| !l.polygons.is_empty()).expect("floor band");
    let sky = idx(&|l| !l.skylines.is_empty()).expect("skyline");
    let ceiling = idx(&|l| l.polylines.iter().any(|p| p.stroke.dash.is_some())).expect("ceiling");
    let labels = idx(&|l| l.labels.iter().any(|t| t.text == "1k")).expect("labels");
    assert!(field <= band && band < sky && sky <= ceiling && sky <= labels);
}

#[test]
fn the_glow_follows_the_ceiling_and_precedes_the_labels() {
    let b = pink_room(30, SkylineTier::Full);
    let s = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    let glows: Vec<_> = s.scene.layers.iter().flat_map(|l| &l.glows).collect();
    assert_eq!(glows.len(), 1);
    assert_eq!(glows[0].id, skylines(&s.scene)[0].id);
    assert_eq!(glows[0].gain, GLOW_ICE);
    // One layer: glows draw after its polylines (the ceiling) and before its labels.
    let l = s
        .scene
        .layers
        .iter()
        .find(|l| !l.glows.is_empty())
        .expect("glow layer");
    assert!(
        l.polylines.iter().any(|p| p.stroke.dash.is_some()),
        "ceiling"
    );
    assert!(l.labels.iter().any(|t| t.text == "1k"), "labels");
    let sky = s.scene.layers.iter().position(|l| !l.skylines.is_empty());
    let glow = s.scene.layers.iter().position(|l| !l.glows.is_empty());
    assert!(sky <= glow);
}

#[test]
fn the_glow_by_tier_and_colour_map() {
    let view = ViewState::default();
    let glows = |tier, colours| {
        let b = pink_room(30, tier);
        let s = scene_3d_as(&b, &view, SpectrumCamera::DEFAULT, tier, true, colours);
        s.scene
            .layers
            .iter()
            .flat_map(|l| l.glows.clone())
            .map(|g| g.gain)
            .collect::<Vec<_>>()
    };
    assert_eq!(glows(SkylineTier::Lite, Colormap::Ice), [GLOW_ICE]);
    assert_eq!(glows(SkylineTier::Full, Colormap::Viridis), [GLOW_MAP]);
    assert!(glows(SkylineTier::Bare, Colormap::Ice).is_empty());
    assert_eq!(SkylineTier::Full.bloom_taps(), Some(9));
    assert_eq!(SkylineTier::Lite.bloom_taps(), Some(5));
}

#[test]
fn the_skyline_primitive_carries_the_grid_and_the_camera() {
    let b = pink_room(30, SkylineTier::Full);
    let s = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    let sk = skylines(&s.scene);
    assert_eq!(sk.len(), 1);
    let k = sk[0];
    assert_eq!((k.cols, k.rows), (192, 101));
    assert_eq!(k.data.len(), 101);
    assert_eq!(k.range, [-80.0, 0.0]);
    assert_eq!(k.colormap, Colormap::Ice);
    assert_eq!(k.rect, s.plot);
    assert_eq!(k.look, LOOK);
    assert_eq!(k.view_proj, s.view_proj.gpu());
    assert!(k.phase > 0.0 && k.phase <= 1.0);
}

#[test]
fn caption_names_the_cell_and_a_forced_tier() {
    let b = pink_room(30, SkylineTier::Full);
    let view = ViewState::default();
    let s = scene_3d(&b, &view, SpectrumCamera::DEFAULT, false);
    assert_eq!(
        s.caption,
        "Main L · last 30 s · dBFS (tone) · 1/19 oct · 0.30 s"
    );
    let lite = pink_room(30, SkylineTier::Lite);
    let s = scene_3d(&lite, &view, SpectrumCamera::DEFAULT, true);
    assert_eq!(
        s.caption,
        "Main L · last 30 s · dBFS (tone) · 1/13 oct · 0.63 s · lite"
    );
}

#[test]
fn empty_and_missing_say_so() {
    let h = SpectrographHistory::new(30);
    let mut s = Skyline::default();
    s.update(&h, full_range(), SkylineTier::Full);
    let b = Built { h, s };
    let sc = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    assert_eq!(sc.message.as_deref(), Some("Main L: no frames yet"));
    assert!(skylines(&sc.scene).is_empty());
    let none = spectrograph_3d_scene(
        &[],
        &Status::default(),
        None,
        &ViewState::default(),
        &Theme::light(),
        Viewport {
            width: 800.0,
            height: 400.0,
        },
    );
    assert_eq!(
        none.message.as_deref(),
        Some("no spectrum or RTA measurement")
    );
    // The field is dark in the light theme too.
    assert!(
        none.scene
            .layers
            .iter()
            .any(|l| l.rects.iter().any(|r| r.color == FIELD))
    );
}

#[test]
fn the_floor_band_fades_with_depth() {
    let b = pink_room(30, SkylineTier::Full);
    let s = scene_3d(&b, &ViewState::default(), SpectrumCamera::DEFAULT, false);
    let polys: Vec<_> = s.scene.layers.iter().flat_map(|l| &l.polygons).collect();
    assert!(polys.len() >= 10);
    let lum = |c: Color| c.r + c.g + c.b;
    assert!(lum(polys[0].color) > lum(polys[polys.len() - 1].color));
    assert!(lum(polys[polys.len() - 1].color) >= lum(FIELD));
}

#[test]
fn heights_follow_the_range_and_offset() {
    let b = pink_room(30, SkylineTier::Full);
    let input = Spectrograph3dInput {
        history: &b.h,
        skyline: &b.s,
        name: "Main L".into(),
        range: Range::new(-60.0, -10.0),
        offset_db: 6.0,
        freshness: None,
        colours: Colormap::Viridis,
        camera: Pose::from(SpectrumCamera::DEFAULT),
        tier: SkylineTier::Full,
        tier_forced: false,
    };
    let s = spectrograph_3d_scene(
        &[],
        &Status::default(),
        Some(&input),
        &ViewState {
            freq: FreqRange::default(),
            ..ViewState::default()
        },
        &Theme::dark(),
        Viewport {
            width: 900.0,
            height: 500.0,
        },
    );
    // The offset moves the levels up, so the range in stored levels moves down.
    assert_eq!(skylines(&s.scene)[0].range, [-66.0, -16.0]);
    assert!(s.caption.starts_with("Main L · offset"), "{}", s.caption);
}
