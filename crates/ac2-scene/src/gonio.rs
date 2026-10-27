//! Goniometer truth: the ring of recent sample-pair points with their age and landing
//! intensity, and the cooling ramp an ember follows.
//!
//! The daemon sends, per meter frame, a batch of points already rotated 45° (`x = (r − l)/√2`,
//! `y = (l + r)/√2`), auto-gained and quantised to `i8`, each with the sample's level within
//! the auto-gain window as an `i8` in `0 … 127`. The ring keeps the points of an ember's
//! life, five decays at ×1 glow ([`ring_capacity`]), with the display time they landed at.
//! What the renderer needs per ember is here: position, age and landing intensity, and how
//! it cools ([`Cooling`]: the decay and the glow); the colours of the ramp are `ac2-plot`'s.
//! A stage is named ([`AshStage`]) so tests assert *orange*, not a colour triple.

use std::collections::VecDeque;

use ac2_proto::frame::{GONIO_POINTS, METER_FPS};

use crate::primitives::{Polyline, Rect, Stroke};

/// Default cooling time constant, display seconds.
pub const EMBER_DECAY_DEFAULT_S: f32 = 0.4;
/// Settable range of the cooling time constant: 0.1 s is a bare scope trace, 2 s a
/// phosphor feel.
pub const EMBER_DECAY_RANGE_S: (f32, f32) = (0.1, 2.0);
/// An ember landing at ×1 glow is gone this many decays after landing (brightness below
/// 1 %); [`EmberRing::retire`] drops it ([`Cooling::gone_s`] for other glows).
pub const GONE_DECAYS: f32 = 5.0;

/// Default glow: a point lands at its level's brightness.
pub const EMBER_GLOW_DEFAULT: f32 = 1.0;
/// Settable range of the glow, a gain on landing intensity: ×0.5 keeps a dense programme
/// from washing the field out, ×4 lifts sparse or quiet material.
pub const EMBER_GLOW_RANGE: (f32, f32) = (0.5, 4.0);

/// The setting clamped to [`EMBER_DECAY_RANGE_S`]; a non-finite value is the default.
pub fn clamp_decay(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(EMBER_DECAY_RANGE_S.0, EMBER_DECAY_RANGE_S.1)
    } else {
        EMBER_DECAY_DEFAULT_S
    }
}

/// The glow setting clamped to [`EMBER_GLOW_RANGE`]; a non-finite value is the default.
pub fn clamp_glow(g: f32) -> f32 {
    if g.is_finite() {
        g.clamp(EMBER_GLOW_RANGE.0, EMBER_GLOW_RANGE.1)
    } else {
        EMBER_GLOW_DEFAULT
    }
}

/// How an ember cools: its time constant and its glow (a gain on landing intensity), each
/// clamped to its range, so every function that reads it sees settings it can show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cooling {
    decay_s: f32,
    glow: f32,
}

impl Default for Cooling {
    fn default() -> Self {
        Self {
            decay_s: EMBER_DECAY_DEFAULT_S,
            glow: EMBER_GLOW_DEFAULT,
        }
    }
}

impl Cooling {
    pub fn new(decay_s: f32, glow: f32) -> Self {
        Self {
            decay_s: clamp_decay(decay_s),
            glow: clamp_glow(glow),
        }
    }

    /// The cooling time constant, seconds.
    pub fn decay_s(self) -> f32 {
        self.decay_s
    }

    /// The gain on landing intensity.
    pub fn glow(self) -> f32 {
        self.glow
    }

    /// How many decays the glow shifts the ramp by: `g·e^(−t/τ)` reaches any level `ln g`
    /// decays later than `e^(−t/τ)` does, so a hotter landing is that much further from ash.
    fn head_start(self) -> f32 {
        self.glow.ln()
    }

    /// Seconds after landing an ember is gone: when `g·e^(−t/τ)` falls below `e^(−5)`
    /// (under 1 %), [`GONE_DECAYS`] plus the glow's head start, so a hotter ember is not
    /// dropped while it still shows and a cooler one goes as soon as it no longer does.
    pub fn gone_s(self) -> f32 {
        self.decay_s * (GONE_DECAYS + self.head_start())
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

/// Points a ring keeps at `cooling`: every frame of an ember's life
/// ([`Cooling::gone_s`]) at the daemon's frame rate, so an ember lives its whole ramp down
/// to ash before a newer frame pushes it out (60 frames at the 0.4 s default, 300 at 2 s,
/// 384 at 2 s and ×4, the most the ranges allow).
pub fn ring_capacity(cooling: Cooling) -> usize {
    let frames = f64::from(METER_FPS) * f64::from(cooling.gone_s());
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

    /// Drops embers older than [`Cooling::gone_s`], which no longer show.
    pub fn retire(&mut self, now: f64, cooling: Cooling) {
        let gone = f64::from(cooling.gone_s());
        while let Some(e) = self.embers.front()
            && now - e.landed_at > gone
        {
            self.embers.pop_front();
        }
    }
}

/// Brightness of an ember: landing intensity times the glow, cooling as `e^(−age/decay)`,
/// at most 1 (a boosted loud point stays full until it has cooled below it).
pub fn brightness(age_s: f32, cooling: Cooling, intensity: f32) -> f32 {
    let heat = cooling.glow * (-age_s.max(0.0) / cooling.decay_s).exp();
    (intensity.clamp(0.0, 1.0) * heat).clamp(0.0, 1.0)
}

/// Where on the blackbody ramp an ember is: a cooling body dims and reddens together, so
/// the stage follows the heat `g·e^(−age/decay)` the brightness is drawn from. A hotter
/// glow lands further up the ramp and reaches each colour `ln g` decays later (×2 stays
/// white 0.84 decays, ×0.5 lands already orange), so the colour agrees with how bright the
/// glow makes it look. The landing intensity is left out: it is the point's level within
/// the auto-gain window, a smaller grain of the same heat, not a cooler body.
///
/// At ×1 glow:
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

pub fn ash_stage(age_s: f32, cooling: Cooling) -> AshStage {
    let t = age_s.max(0.0) / cooling.decay_s - cooling.head_start();
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
        assert_eq!(clamp_glow(2.0), 2.0);
        assert_eq!(clamp_glow(0.1), 0.5);
        assert_eq!(clamp_glow(10.0), 4.0);
        assert_eq!(clamp_glow(f32::INFINITY), EMBER_GLOW_DEFAULT);
        assert_eq!(Cooling::new(9.0, 9.0), Cooling::new(2.0, 4.0));
        assert_eq!(Cooling::default(), Cooling::new(0.4, 1.0));
    }

    fn at(decay_s: f32) -> Cooling {
        Cooling::new(decay_s, 1.0)
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
        r.retire(2.6, at(0.4));
        assert_eq!(r.len(), 2);
        r.retire(3.1, at(0.4));
        assert_eq!(r.len(), 1);
        r.retire(100.0, at(0.4));
        assert!(r.is_empty());
    }

    /// A hotter glow keeps an ember until `g·e^(−t/τ)` is below `e^(−5)`: at ×4 that is
    /// `5 + ln 4` decays, so one still showing is not dropped; at ×0.5 it goes sooner.
    #[test]
    fn retire_waits_for_a_hotter_ember() {
        let hot = Cooling::new(0.4, 4.0);
        assert!((hot.gone_s() - 0.4 * (5.0 + 4f32.ln())).abs() < 1e-6);
        let just_before = hot.gone_s() - 0.01;
        assert!(brightness(just_before, hot, 1.0) > (-5f32).exp());
        let mut r = EmberRing::new(8);
        r.push_frame(0.0, &[[0, 0, 127]]);
        r.retire(f64::from(just_before), at(0.4));
        assert!(r.is_empty(), "×1 drops it after five decays");
        r.push_frame(0.0, &[[0, 0, 127]]);
        r.retire(f64::from(just_before), hot);
        assert_eq!(r.len(), 1, "×4 keeps it while it shows");
        r.retire(f64::from(hot.gone_s()) + 0.01, hot);
        assert!(r.is_empty());
        let cool = Cooling::new(0.4, 0.5);
        assert!(cool.gone_s() < at(0.4).gone_s());
        assert!(brightness(cool.gone_s(), cool, 1.0) <= (-5f32).exp() + 1e-6);
    }

    #[test]
    fn brightness_is_intensity_times_exponential() {
        assert_eq!(brightness(0.0, at(0.4), 1.0), 1.0);
        assert!((brightness(0.4, at(0.4), 1.0) - (-1f32).exp()).abs() < 1e-6);
        assert!((brightness(0.4, at(0.4), 0.5) - 0.5 * (-1f32).exp()).abs() < 1e-6);
        assert_eq!(
            brightness(-1.0, at(0.4), 1.0),
            1.0,
            "a point from the future is new"
        );
        assert!(brightness(5.0 * 0.4, at(0.4), 1.0) < 0.01);
    }

    #[test]
    fn the_glow_scales_brightness_up_to_full() {
        let g = |glow: f32| Cooling::new(0.4, glow);
        let e1 = (-1f32).exp();
        assert!((brightness(0.4, g(2.0), 0.25) - 0.5 * e1).abs() < 1e-6);
        assert!((brightness(0.4, g(0.5), 1.0) - 0.5 * e1).abs() < 1e-6);
        assert_eq!(brightness(0.0, g(4.0), 1.0), 1.0, "clamped to full");
        assert_eq!(brightness(0.4, g(4.0), 1.0), 1.0, "4/e is still over 1");
        assert!(brightness(0.8, g(4.0), 1.0) < 1.0);
        assert_eq!(
            brightness(0.0, g(1.0), 0.3),
            0.3,
            "×1 is the level as it landed"
        );
    }

    #[test]
    fn stages_follow_the_ramp_in_decays() {
        for decay in [0.1f32, 0.4, 2.0] {
            let stage = |d: f32| ash_stage(d * decay, at(decay));
            assert_eq!(stage(0.0), AshStage::White);
            assert_eq!(stage(0.1), AshStage::White);
            assert_eq!(stage(0.2), AshStage::Yellow);
            assert_eq!(stage(0.8), AshStage::Orange);
            assert_eq!(stage(2.0), AshStage::Red);
            assert_eq!(stage(3.0), AshStage::Ash);
            assert_eq!(stage(10.0), AshStage::Ash);
        }
    }

    /// The stage follows the heat: each boundary moves `ln g` decays, so a ×2 ember is
    /// still yellow when a ×1 one is orange, and ×0.5 lands orange.
    #[test]
    fn a_hotter_glow_stays_whiter_longer() {
        let stage = |d: f32, glow: f32| ash_stage(d * 0.4, Cooling::new(0.4, glow));
        assert_eq!(stage(0.5, 1.0), AshStage::Orange);
        assert_eq!(stage(0.5, 2.0), AshStage::White, "0.5 − ln 2 < 0.15");
        assert_eq!(stage(1.0, 2.0), AshStage::Yellow);
        assert_eq!(stage(0.0, 0.5), AshStage::Orange, "ln 2 = 0.69 decays on");
        assert_eq!(stage(2.0, 0.5), AshStage::Ash);
        assert_eq!(stage(3.0, 4.0), AshStage::Red, "3 − ln 4 = 1.61");
        // The boundary is where the heat crosses the same level at every glow.
        for glow in [0.5f32, 0.75, 1.5, 2.0, 3.0, 4.0] {
            let c = Cooling::new(0.4, glow);
            let to_red = 0.4 * (STAGE_DECAYS[2] + glow.ln());
            assert_eq!(ash_stage(to_red - 1e-3, c), AshStage::Orange, "{glow}");
            assert_eq!(ash_stage(to_red + 1e-3, c), AshStage::Red, "{glow}");
            let heat = glow * (-to_red / 0.4).exp();
            assert!((heat - (-STAGE_DECAYS[2]).exp()).abs() < 1e-5, "{glow}");
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
        assert_eq!(ring_capacity(at(0.4)), GONIO_POINTS * 60);
        assert_eq!(ring_capacity(at(2.0)), GONIO_POINTS * 300);
        assert_eq!(ring_capacity(at(0.1)), GONIO_POINTS * 15);
        assert_eq!(
            ring_capacity(at(9.0)),
            ring_capacity(at(2.0)),
            "clamped to the range"
        );
    }

    /// A hotter glow lives longer, so its ring holds more frames; the ranges bound it at
    /// 2 s and ×4 (`5 + ln 4` decays: 384 frames).
    #[test]
    fn the_ring_holds_a_hotter_embers_whole_life() {
        assert_eq!(ring_capacity(Cooling::new(2.0, 4.0)), GONIO_POINTS * 384);
        assert_eq!(
            ring_capacity(Cooling::new(30.0, 100.0)),
            ring_capacity(Cooling::new(2.0, 4.0)),
            "clamped to the ranges"
        );
        assert!(ring_capacity(Cooling::new(0.4, 2.0)) > ring_capacity(at(0.4)));
        assert!(ring_capacity(Cooling::new(0.4, 0.5)) < ring_capacity(at(0.4)));
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
        let mut r = EmberRing::new(ring_capacity(at(decay)));
        let frame = [[100i8, 0]; GONIO_POINTS];
        let first = [[1i8, 0]; GONIO_POINTS];
        r.push_points(0.0, &first);
        let dt = 1.0 / f64::from(METER_FPS);
        let mut t = dt;
        while t <= 9.0 {
            r.push_points(t, &frame);
            r.retire(t, at(decay));
            t += dt;
        }
        let oldest = r.age(t).next().expect("embers");
        assert_eq!(oldest.x * 127.0, 1.0, "the first frame's points at 9 s");
        r.retire(10.01, at(decay));
        assert!(
            r.age(10.01).all(|e| e.x * 127.0 != 1.0),
            "retired after five decays"
        );
    }
}
