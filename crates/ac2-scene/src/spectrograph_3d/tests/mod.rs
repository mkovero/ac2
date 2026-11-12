use super::*;
use crate::banner::Status;
use crate::primitives::{Layer, Skyline3d};
use crate::spectrograph::SpectrographFrame;
use crate::view::FreqRange;
use ac2_proto::GridDef;
use ac2_proto::GridId;
use ac2_proto::frame::ValidityMask;
use ac2_proto::model::LevelScale;
use ac2_proto::units::Hz;

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
            at_ns: T0 + slot * SLOT_NS + SLOT_NS / 2,
            grid: self.grid,
            edges: &self.edges,
            scale: LevelScale::Dbfs,
            spec_level: None,
            level,
            validity,
        });
    }

    /// A frame ending at audio time `at_ns`.
    fn push_at(&mut self, h: &mut SpectrographHistory, at_ns: u64, level: f32) {
        self.seq += 1;
        let l = vec![level; self.edges.len()];
        h.push(&SpectrographFrame {
            seq: self.seq,
            at_ns,
            grid: self.grid,
            edges: &self.edges,
            scale: LevelScale::Dbfs,
            spec_level: None,
            level: &l,
            validity: None,
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

/// Redrawn between frames (a glide), the skyline takes nothing new: its rows are the very
/// ones the renderer already holds, so none is uploaded again. A second frame in the newest
/// slot is new.
#[test]
fn an_update_without_a_new_frame_keeps_every_row() {
    let mut h = SpectrographHistory::new(10);
    let mut f = Feed::new(&log_bins());
    let mut s = Skyline::default();
    for slot in 0..20 {
        f.push(&mut h, slot, -30.0);
    }
    s.update(&h, full_range(), SkylineTier::Full);
    let before = s.ring().to_vec();
    s.update(&h, full_range(), SkylineTier::Full);
    let kept = |a: &Option<HeatmapColumn>, b: &Option<HeatmapColumn>| match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    };
    assert!(before.iter().zip(s.ring()).all(|(a, b)| kept(a, b)));
    f.push(&mut h, 19, -10.0);
    s.update(&h, full_range(), SkylineTier::Full);
    assert_eq!(s.row_back(0).expect("newest")[50], -10.0);
    assert!(same(&s, &rebuilt(&h, full_range(), SkylineTier::Full)));
}

/// A row's z in rows from the front edge as drawn, by its ring position.
fn drawn_z(s: &Skyline, d: &DrawnRows, pos: usize) -> f64 {
    let len = s.rows();
    let j = (d.newest + len - pos) % len;
    j as f64 + f64::from(d.phase)
}

#[test]
fn rows_move_back_continuously_across_a_new_row() {
    let mut h = SpectrographHistory::new(10);
    let mut f = Feed::new(&log_bins());
    let mut s = Skyline::default();
    // k = 3: slots 6–8 make row 2, 9–11 row 3. The rows stand with slot 7 newest.
    for slot in 4..=7 {
        f.push(&mut h, slot, -20.0);
    }
    s.update(&h, full_range(), SkylineTier::Full);
    let row2 = s.newest();
    let before = s.drawn(0.0);
    assert!((before.phase - 2.0 / 3.0).abs() < 1e-6);
    assert!((drawn_z(&s, &before, row2) - 2.0 / 3.0).abs() < 1e-6);
    // The axis runs on past slot 7 while the next frame is due: row 2 moves back.
    let waiting = s.drawn(1.5);
    assert!((drawn_z(&s, &waiting, row2) - 7.0 / 6.0).abs() < 1e-6);
    // The next frame lands in slot 9: row 3 starts, in front of row 2. Drawn where the axis
    // stood, row 2 stays put and row 3 stands half a slot behind the front edge.
    f.push(&mut h, 9, -20.0);
    s.update(&h, full_range(), SkylineTier::Full);
    let row3 = s.newest();
    assert_ne!(row3, row2);
    let after = s.drawn(-0.5);
    assert!((drawn_z(&s, &after, row2) - 7.0 / 6.0).abs() < 1e-6);
    assert!((drawn_z(&s, &after, row3) - 1.0 / 6.0).abs() < 1e-6);
    // Every lead moves the rows back steadily, never back and forth.
    let mut last = f64::NEG_INFINITY;
    for i in 0..=30 {
        let lead = -1.0 + 3.0 * f64::from(i) / 30.0;
        let z = drawn_z(&s, &s.drawn(lead), row2);
        assert!(z >= last - 1e-6, "lead {lead}: {z} after {last}");
        last = z;
    }
    // No row is drawn in front of the floor, and a stalled stream waits.
    assert_eq!(s.drawn(-50.0).phase, 0.0);
    assert_eq!(f64::from(s.drawn(50.0).phase), MAX_PHASE);
}

/// Frames every 1/60 s carrying whole 1024-sample blocks at 96 kHz (audio steps of 10.7,
/// 21.3, 32 and 42.7 ms), drawn at 60 frames a second: the rows move back the same
/// distance every display frame, half a slot, and the frame just in is always drawn.
#[test]
fn rows_move_evenly_per_display_frame_under_uneven_audio_steps() {
    use crate::scroll_clock::ScrollClock;
    use std::time::{Duration, Instant};
    const FRAME_S: f64 = 1.0 / 60.0;
    const BLOCK_S: f64 = 1024.0 / 96_000.0;
    let mut h = SpectrographHistory::new(10);
    let mut f = Feed::new(&log_bins());
    let mut s = Skyline::default();
    let mut clock = ScrollClock::default();
    let t0 = Instant::now();
    let a0 = T0 as f64 / 1e9;
    let mut steps = std::collections::BTreeSet::<i64>::new();
    let mut prev: Option<f64> = None;
    let mut positions = Vec::new();
    for i in 0..600u64 {
        let t = i as f64 * FRAME_S;
        // The analysis finishes 5–25 ms before each hand-off, in whole blocks.
        let u = ((i * 7) % 16) as f64 / 15.0 * 0.020;
        let audio = ((t - 0.005 - u) / BLOCK_S).floor() * BLOCK_S;
        if let Some(p) = prev {
            steps.insert(((audio - p) / BLOCK_S).round() as i64);
        }
        prev = Some(audio);
        let audio_s = a0 + audio;
        let at = t0 + Duration::from_secs_f64(t);
        f.push_at(&mut h, (audio_s * 1e9).round() as u64, -20.0);
        clock.observe(at, audio_s);
        s.update(&h, full_range(), SkylineTier::Full);
        // Drawn 4 ms after the frame arrived.
        let lead = h.lead_slots(clock.position_s(at + Duration::from_millis(4)));
        let d = s.drawn(lead);
        let n = h.newest_slot().expect("frames");
        let k = s.bucket_slots() as f64;
        // Where the front edge stands, in slots: the newest row's start plus its phase.
        let edge = (n as f64 / k).floor() * k + f64::from(d.phase) * k;
        assert!(lead > -1.0, "frame {i}: the newest slot is drawn ({lead})");
        if i > 60 {
            positions.push(edge);
        }
    }
    assert!(steps.len() >= 3, "uneven audio steps: {steps:?}");
    let slot_s = h.slot_s();
    for w in positions.windows(2) {
        let adv = w[1] - w[0];
        assert!(
            (adv - FRAME_S / slot_s).abs() < 1e-3,
            "{adv} slots per display frame"
        );
    }
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

mod pick;
mod points;
mod scene;
