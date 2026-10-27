//! Goniometer truth: the ring of recent sample-pair points with their age and landing
//! intensity, and the cooling ramp an ember follows.
//!
//! The daemon sends, per meter frame, a batch of points already rotated 45° (`x = (r − l)/√2`,
//! `y = (l + r)/√2`), auto-gained and quantised to `i8`, each with the sample's level within
//! the auto-gain window as an `i8` in `0 … 127`. The ring keeps the points of the last five
//! decays ([`ring_capacity`]) with the display time they landed at. What the renderer needs per ember is here: position,
//! age and landing intensity; the colours of the ramp are `ac2-plot`'s. A stage is named
//! ([`AshStage`]) so tests assert *orange*, not a colour triple.

use std::collections::VecDeque;

use ac2_proto::frame::{GONIO_POINTS, METER_FPS};

use crate::primitives::{Polyline, Rect, Stroke};

/// Default cooling time constant, display seconds.
pub const EMBER_DECAY_DEFAULT_S: f32 = 0.4;
/// Settable range of the cooling time constant: 0.1 s is a bare scope trace, 2 s a
/// phosphor feel.
pub const EMBER_DECAY_RANGE_S: (f32, f32) = (0.1, 2.0);
/// An ember is gone this many decays after landing (brightness below 1 %); [`EmberRing::retire`]
/// drops it.
pub const GONE_DECAYS: f32 = 5.0;

/// The setting clamped to [`EMBER_DECAY_RANGE_S`]; a non-finite value is the default.
pub fn clamp_decay(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(EMBER_DECAY_RANGE_S.0, EMBER_DECAY_RANGE_S.1)
    } else {
        EMBER_DECAY_DEFAULT_S
    }
}

/// The auto-gain window: a point lands at full intensity at the pair's recent peak and at
/// none this far below it (the daemon's figure, `ac2_core::stereo::GONIO_WINDOW_DB`).
pub const WINDOW_DB: f32 = 30.0;

/// Landing intensity `0 … 1` of a point as the gonio frame carries it (`[x, y]`, auto-gained,
/// `±127` full scale): its level within the auto-gain window, 1 at radius 127, 0 at
/// [`WINDOW_DB`] below (radius ≈ 4). The 45° rotation keeps the sample pair's magnitude, so
/// the radius alone is the level and the wire needs no third coordinate.
pub fn landing_intensity(point: [i8; 2]) -> f32 {
    let (x, y) = (f32::from(point[0]), f32::from(point[1]));
    let radius = (x * x + y * y).sqrt();
    if radius < 0.5 {
        return 0.0;
    }
    (1.0 + 20.0 * (radius / 127.0).log10() / WINDOW_DB).clamp(0.0, 1.0)
}

/// One point as the wire carries it: `[x, y, intensity]`, `x`/`y` in `−127 … 127` for
/// `−1 … 1`, intensity in `0 … 127` for `0 … 1` (negative intensity reads as 0).
pub type WirePoint = [i8; 3];

/// A landed point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ember {
    /// `−1 … 1`, right positive.
    pub x: f32,
    /// `−1 … 1`, up positive.
    pub y: f32,
    /// Display time it landed, seconds.
    pub landed_at: f64,
    /// Level at landing within the auto-gain window, `0 … 1`.
    pub intensity: f32,
}

/// An ember as seen at one display instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmberAge {
    pub x: f32,
    pub y: f32,
    /// Seconds since landing (never negative).
    pub age_s: f32,
    pub intensity: f32,
}

/// Points a ring keeps at the cooling time `decay_s`: every frame of the last
/// [`GONE_DECAYS`] decays at the daemon's frame rate, so an ember lives its whole ramp down
/// to ash before a newer frame pushes it out (60 frames at the 0.4 s default, 300 at 2 s).
pub fn ring_capacity(decay_s: f32) -> usize {
    let frames = f64::from(METER_FPS) * f64::from(GONE_DECAYS) * f64::from(clamp_decay(decay_s));
    // The decay is an f32 on a 0.1 s grid: 0.4 is 0.4000000059…, whose last bit would
    // otherwise round a whole frame up.
    GONIO_POINTS * (frames - 1e-4).ceil().max(1.0) as usize
}

/// Fixed-capacity ring of the most recent embers, oldest first. Allocates on a capacity
/// change only; when full, a new frame pushes the oldest points out, so the capacity bounds
/// both memory and how far back the field remembers ([`ring_capacity`]).
#[derive(Clone, Debug)]
pub struct EmberRing {
    embers: VecDeque<Ember>,
    capacity: usize,
}

impl EmberRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            embers: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Resizes the ring to `n` points; shrinking drops the oldest, so the newest stay.
    pub fn set_capacity(&mut self, n: usize) {
        while self.embers.len() > n {
            self.embers.pop_front();
        }
        if n > self.embers.capacity() {
            self.embers.reserve_exact(n - self.embers.len());
        } else {
            self.embers.shrink_to(n);
        }
        self.capacity = n;
    }

    pub fn len(&self) -> usize {
        self.embers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.embers.is_empty()
    }

    /// Lands one frame's points at display time `now`.
    pub fn push_frame(&mut self, now: f64, points: &[WirePoint]) {
        for p in points {
            self.land(Ember {
                x: f32::from(p[0]) / 127.0,
                y: f32::from(p[1]) / 127.0,
                landed_at: now,
                intensity: f32::from(p[2].max(0)) / 127.0,
            });
        }
    }

    /// Lands one gonio frame's `[x, y]` points at display time `now`, each at its
    /// [`landing_intensity`].
    pub fn push_points(&mut self, now: f64, points: &[[i8; 2]]) {
        for p in points {
            self.land(Ember {
                x: f32::from(p[0]) / 127.0,
                y: f32::from(p[1]) / 127.0,
                landed_at: now,
                intensity: landing_intensity(*p),
            });
        }
    }

    fn land(&mut self, e: Ember) {
        if self.capacity == 0 {
            return;
        }
        if self.embers.len() == self.capacity {
            self.embers.pop_front();
        }
        self.embers.push_back(e);
    }

    /// Every ember with its age at `now`, oldest first.
    pub fn age(&self, now: f64) -> impl Iterator<Item = EmberAge> + '_ {
        self.embers.iter().map(move |e| EmberAge {
            x: e.x,
            y: e.y,
            age_s: (now - e.landed_at).max(0.0) as f32,
            intensity: e.intensity,
        })
    }

    /// Drops embers older than [`GONE_DECAYS`] decays, which no longer show.
    pub fn retire(&mut self, now: f64, decay_s: f32) {
        let gone = f64::from(GONE_DECAYS * clamp_decay(decay_s));
        while let Some(e) = self.embers.front()
            && now - e.landed_at > gone
        {
            self.embers.pop_front();
        }
    }
}

/// Brightness of an ember: landing intensity cooling as `e^(−age/decay)`.
pub fn brightness(age_s: f32, decay_s: f32, intensity: f32) -> f32 {
    (intensity.clamp(0.0, 1.0) * (-age_s.max(0.0) / clamp_decay(decay_s)).exp()).clamp(0.0, 1.0)
}

/// Where on the blackbody ramp an ember is, by age in decays: a cooling body dims and
/// reddens together, so the stage follows the same exponential as the brightness.
///
/// | stage | age / decay | brightness left |
/// |---|---|---|
/// | White (white-yellow) | 0 … 0.15 | 100 … 86 % |
/// | Yellow | 0.15 … 0.4 | 86 … 67 % |
/// | Orange | 0.4 … 1.2 | 67 … 30 % |
/// | Red (deep red) | 1.2 … 2.5 | 30 … 8 % |
/// | Ash (grey) | 2.5 … 5 | 8 … 1 %, then gone ([`GONE_DECAYS`]) |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AshStage {
    White,
    Yellow,
    Orange,
    Red,
    Ash,
}

/// Stage boundaries in decays, in [`AshStage`] order.
pub const STAGE_DECAYS: [f32; 4] = [0.15, 0.4, 1.2, 2.5];

pub fn ash_stage(age_s: f32, decay_s: f32) -> AshStage {
    let t = age_s.max(0.0) / clamp_decay(decay_s);
    if t < STAGE_DECAYS[0] {
        AshStage::White
    } else if t < STAGE_DECAYS[1] {
        AshStage::Yellow
    } else if t < STAGE_DECAYS[2] {
        AshStage::Orange
    } else if t < STAGE_DECAYS[3] {
        AshStage::Red
    } else {
        AshStage::Ash
    }
}

/// The largest centred square inside `rect`: the field the figure is drawn in.
pub fn square(rect: Rect) -> Rect {
    let side = rect.w.min(rect.h).max(0.0);
    Rect::new(
        rect.x + (rect.w - side) / 2.0,
        rect.y + (rect.h - side) / 2.0,
        side,
        side,
    )
}

/// Logical pixel position of a point `(x, y)` in `−1 … 1` within `square` (y up).
pub fn place(square: Rect, x: f32, y: f32) -> [f32; 2] {
    let half = square.w / 2.0;
    [square.x + half + x * half, square.y + half - y * half]
}

/// The grid: the square's outline and its two diagonals (the L and R axes), as polylines
/// in `stroke`. Plot chrome: the caller leaves it out when **T** hides decorations.
pub fn grid_polylines(square: Rect, stroke: Stroke) -> Vec<Polyline> {
    let (x0, y0, x1, y1) = (square.x, square.y, square.right(), square.bottom());
    let line = |points: Vec<[f32; 2]>| Polyline {
        points,
        alpha: vec![],
        stroke,
        clip: None,
    };
    vec![
        line(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]),
        line(vec![[x0, y0], [x1, y1]]),
        line(vec![[x1, y0], [x0, y1]]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_setting_clamps() {
        assert_eq!(clamp_decay(0.4), 0.4);
        assert_eq!(clamp_decay(0.01), 0.1);
        assert_eq!(clamp_decay(9.0), 2.0);
        assert_eq!(clamp_decay(f32::NAN), EMBER_DECAY_DEFAULT_S);
    }

    #[test]
    fn ring_holds_capacity_oldest_first() {
        let mut r = EmberRing::new(4);
        r.push_frame(0.0, &[[127, 0, 127], [-127, 0, 64]]);
        r.push_frame(1.0, &[[0, 127, 0], [0, -127, -5], [10, 10, 127]]);
        assert_eq!(r.len(), 4);
        let v: Vec<EmberAge> = r.age(1.5).collect();
        assert_eq!(v[0].x, -1.0);
        assert_eq!(v[0].age_s, 1.5);
        assert!((v[0].intensity - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(v[1].y, 1.0);
        assert_eq!(v[1].age_s, 0.5);
        assert_eq!(v[2].intensity, 0.0, "negative intensity reads as 0");
        assert_eq!(v[3].age_s, 0.5);
    }

    #[test]
    fn age_never_negative_and_zero_capacity_is_harmless() {
        let mut r = EmberRing::new(1);
        r.push_frame(2.0, &[[0, 0, 127]]);
        assert_eq!(r.age(1.0).next().expect("one ember").age_s, 0.0);
        let mut z = EmberRing::new(0);
        z.push_frame(0.0, &[[0, 0, 127]]);
        assert!(z.is_empty());
    }

    #[test]
    fn landing_intensity_spans_the_window() {
        assert_eq!(landing_intensity([0, 127]), 1.0);
        assert_eq!(landing_intensity([127, 127]), 1.0, "past the gain clamps");
        assert_eq!(landing_intensity([0, 0]), 0.0);
        assert_eq!(landing_intensity([0, 4]), 0.0, "30 dB down is radius 4");
        // 15 dB down: halfway.
        let r = (127.0 * 10f32.powf(-15.0 / 20.0)).round() as i8;
        assert!((landing_intensity([0, r]) - 0.5).abs() < 0.02);
        let mut ring = EmberRing::new(4);
        ring.push_points(1.0, &[[0, 127], [-90, 0]]);
        let v: Vec<EmberAge> = ring.age(1.0).collect();
        assert_eq!((v[0].x, v[0].y, v[0].intensity), (0.0, 1.0, 1.0));
        assert!(v[1].intensity > 0.9 && v[1].intensity < 1.0);
    }

    #[test]
    fn retire_drops_only_the_gone() {
        let mut r = EmberRing::new(8);
        r.push_frame(0.0, &[[0, 0, 127]]);
        r.push_frame(1.0, &[[0, 0, 127]]);
        r.push_frame(2.5, &[[0, 0, 127]]);
        // decay 0.4: gone after 2 s.
        r.retire(2.6, 0.4);
        assert_eq!(r.len(), 2);
        r.retire(3.1, 0.4);
        assert_eq!(r.len(), 1);
        r.retire(100.0, 0.4);
        assert!(r.is_empty());
    }

    #[test]
    fn brightness_is_intensity_times_exponential() {
        assert_eq!(brightness(0.0, 0.4, 1.0), 1.0);
        assert!((brightness(0.4, 0.4, 1.0) - (-1f32).exp()).abs() < 1e-6);
        assert!((brightness(0.4, 0.4, 0.5) - 0.5 * (-1f32).exp()).abs() < 1e-6);
        assert_eq!(
            brightness(-1.0, 0.4, 1.0),
            1.0,
            "a point from the future is new"
        );
        assert!(brightness(5.0 * 0.4, 0.4, 1.0) < 0.01);
    }

    #[test]
    fn stages_follow_the_ramp_in_decays() {
        for decay in [0.1f32, 0.4, 2.0] {
            let at = |d: f32| ash_stage(d * decay, decay);
            assert_eq!(at(0.0), AshStage::White);
            assert_eq!(at(0.1), AshStage::White);
            assert_eq!(at(0.2), AshStage::Yellow);
            assert_eq!(at(0.8), AshStage::Orange);
            assert_eq!(at(2.0), AshStage::Red);
            assert_eq!(at(3.0), AshStage::Ash);
            assert_eq!(at(10.0), AshStage::Ash);
        }
    }

    #[test]
    fn square_and_placement() {
        let sq = square(Rect::new(10.0, 20.0, 200.0, 100.0));
        assert_eq!(sq, Rect::new(60.0, 20.0, 100.0, 100.0));
        assert_eq!(place(sq, 0.0, 0.0), [110.0, 70.0]);
        assert_eq!(place(sq, 1.0, 1.0), [160.0, 20.0], "up is smaller y");
        assert_eq!(place(sq, -1.0, -1.0), [60.0, 120.0]);
        let g = grid_polylines(sq, Stroke::solid(crate::Color::WHITE, 1.0));
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].points.len(), 5);
        assert_eq!(g[1].points, vec![[60.0, 20.0], [160.0, 120.0]]);
    }

    #[test]
    fn the_ring_holds_five_decays_of_frames() {
        assert_eq!(ring_capacity(0.4), GONIO_POINTS * 60);
        assert_eq!(ring_capacity(2.0), GONIO_POINTS * 300);
        assert_eq!(ring_capacity(0.1), GONIO_POINTS * 15);
        assert_eq!(
            ring_capacity(9.0),
            ring_capacity(2.0),
            "clamped to the range"
        );
    }

    #[test]
    fn a_smaller_ring_keeps_the_newest_points() {
        let mut r = EmberRing::new(8);
        for k in 0..8i8 {
            r.push_points(f64::from(k), &[[k, 0]]);
        }
        r.set_capacity(3);
        assert_eq!(r.capacity(), 3);
        let xs: Vec<f32> = r.age(10.0).map(|e| e.x * 127.0).collect();
        assert_eq!(xs, vec![5.0, 6.0, 7.0]);
        r.push_points(11.0, &[[9, 0]]);
        assert_eq!(r.len(), 3, "the new capacity bounds it");
        r.set_capacity(10);
        r.push_points(12.0, &[[10, 0]]);
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn an_ember_at_two_seconds_lives_its_whole_ramp() {
        let decay = 2.0;
        let mut r = EmberRing::new(ring_capacity(decay));
        let frame = [[100i8, 0]; GONIO_POINTS];
        let first = [[1i8, 0]; GONIO_POINTS];
        r.push_points(0.0, &first);
        let dt = 1.0 / f64::from(METER_FPS);
        let mut t = dt;
        while t <= 9.0 {
            r.push_points(t, &frame);
            r.retire(t, decay);
            t += dt;
        }
        let oldest = r.age(t).next().expect("embers");
        assert_eq!(oldest.x * 127.0, 1.0, "the first frame's points at 9 s");
        r.retire(10.01, decay);
        assert!(
            r.age(10.01).all(|e| e.x * 127.0 != 1.0),
            "retired after five decays"
        );
    }
}
