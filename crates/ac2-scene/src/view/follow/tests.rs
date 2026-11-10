//! The follower on scripted frames at 10 frames a second, arrival times their own: music
//! made of songs and gaps, level steps, an idle mic, tones over a floor.

use super::*;

/// A small deterministic generator: the same script every run.
struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// About normal, standard deviation `sd`, bounded at ±2·√3·sd.
    fn noise(&mut self, sd: f64) -> f64 {
        let s: f64 = (0..4).map(|_| self.uniform()).sum::<f64>() - 2.0;
        s * sd * 3f64.sqrt()
    }
}

const DT: f64 = 0.1;

/// A pane at 10 frames a second: the follower decides on every frame (the app does on every
/// tick), a move lands at once (its ease is shorter than the decision's windows), and the
/// clipped frames (top over the range) are counted on the range each frame arrived on.
struct Sim {
    f: Follow,
    r: Range,
    t: f64,
    moves: Vec<(f64, Why, Range)>,
    clipped_s: f64,
    run_s: f64,
    longest_s: f64,
}

impl Sim {
    /// A pane framed on its first frame, as a first show is.
    fn fitted(first: Sample) -> Self {
        Self::on(level::fit([first.floor, first.top]).expect("finite"))
    }

    fn on(r: Range) -> Self {
        Self {
            f: Follow::default(),
            r,
            t: 0.0,
            moves: Vec::new(),
            clipped_s: 0.0,
            run_s: 0.0,
            longest_s: 0.0,
        }
    }

    fn frame(&mut self, s: Sample) {
        self.t += DT;
        if s.top > self.r.hi {
            self.clipped_s += DT;
            self.run_s += DT;
            self.longest_s = self.longest_s.max(self.run_s);
        } else {
            self.run_s = 0.0;
        }
        self.f.push(self.t, s);
        if let Some(m) = self.f.decide(self.t, self.r) {
            self.moves.push((self.t, m.why, m.to));
            self.r = m.to;
        }
    }

    fn whys(&self) -> Vec<Why> {
        self.moves.iter().map(|m| m.1).collect()
    }
}

/// A music frame at song level `l`: the top moves frame to frame, the floor some 40 dB under.
fn music(rng: &mut Rng, l: f64) -> Sample {
    let top = l + rng.noise(2.0);
    Sample {
        top,
        floor: top - 40.0 - 8.0 * rng.uniform(),
    }
}

/// Pink room noise about 70 dB under the music.
fn room(rng: &mut Rng) -> Sample {
    let top = -78.0 + rng.noise(1.0);
    Sample {
        top,
        floor: top - 20.0,
    }
}

#[test]
fn a_show_of_songs_and_gaps_moves_up_at_most_once_and_never_down() {
    for seed in 1..=10u64 {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
        let l0 = -10.0 + 8.0 * (rng.uniform() - 0.5);
        let first = music(&mut rng, l0);
        let mut sim = Sim::fitted(first);
        let mut t = 0.0;
        while t < 3600.0 {
            let l = -10.0 + 8.0 * (rng.uniform() - 0.5);
            for _ in 0..1800 {
                sim.frame(music(&mut rng, l));
            }
            let gap = 300 + (rng.uniform() * 300.0) as usize;
            for _ in 0..gap {
                sim.frame(room(&mut rng));
            }
            t += 180.0 + gap as f64 * DT;
        }
        let whys = sim.whys();
        assert!(!whys.contains(&Why::Down), "seed {seed}: {whys:?}");
        assert!(
            whys.iter().filter(|w| **w == Why::Up).count() <= 1,
            "seed {seed}: {whys:?}"
        );
        assert!(sim.longest_s <= 2.0, "seed {seed}: {} s", sim.longest_s);
        assert!(sim.clipped_s < 10.0, "seed {seed}: {} s", sim.clipped_s);
    }
}

#[test]
fn a_level_step_moves_up_once_within_the_window_onto_its_target() {
    let mut rng = Rng(7);
    let mut sim = Sim::fitted(music(&mut rng, -30.0));
    for _ in 0..600 {
        sim.frame(music(&mut rng, -30.0));
    }
    assert!(sim.moves.is_empty(), "{:?}", sim.moves);
    let step_at = sim.t;
    let mut tops = Vec::new();
    for _ in 0..600 {
        let s = music(&mut rng, -10.0);
        tops.push(s.top);
        sim.frame(s);
        if !sim.moves.is_empty() && tops.len() < 40 {
            break;
        }
    }
    assert_eq!(sim.whys(), vec![Why::Up], "{:?}", sim.moves);
    let (at, _, r) = sim.moves[0];
    assert!(at - step_at <= UP_WINDOW_S + 1e-9, "{}", at - step_at);
    // The window's maximum lands within a step of its place under the top.
    let window = &tops[tops.len().saturating_sub(30)..];
    let max = window.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let target = r.hi - TOP_PAD * r.span();
    assert!(
        (max - target).abs() <= nice(r.span() / 8.0),
        "{max} vs {target}"
    );
    // Then it stays.
    for _ in 0..6000 {
        sim.frame(music(&mut rng, -10.0));
    }
    assert_eq!(sim.moves.len(), 1, "{:?}", sim.moves);
}

#[test]
fn an_idle_mic_moves_down_once_after_the_slow_window_and_music_back_up_once() {
    let mut rng = Rng(11);
    let mut sim = Sim::fitted(music(&mut rng, -10.0));
    for _ in 0..3000 {
        sim.frame(music(&mut rng, -10.0));
    }
    assert!(sim.moves.is_empty(), "{:?}", sim.moves);
    let idle_at = sim.t;
    for _ in 0..12_000 {
        sim.frame(room(&mut rng));
    }
    assert_eq!(sim.whys(), vec![Why::Down], "{:?}", sim.moves);
    assert!(sim.moves[0].0 - idle_at >= SLOW_WINDOW_S, "{:?}", sim.moves);
    for _ in 0..3000 {
        sim.frame(music(&mut rng, -10.0));
    }
    assert_eq!(sim.whys(), vec![Why::Down, Why::Up], "{:?}", sim.moves);
}

#[test]
fn a_tone_over_a_pink_floor_stays_put_for_ten_minutes() {
    let freqs: Vec<f64> = (0..=96 * 10)
        .map(|i| 20.0 * 2f64.powf(i as f64 / 96.0))
        .collect();
    let mut rng = Rng(3);
    let frame = |rng: &mut Rng| -> Vec<f64> {
        freqs
            .iter()
            .map(|f| {
                if (*f / 1000.0).log2().abs() < 1.0 / 192.0 {
                    -20.0 + rng.noise(0.1)
                } else {
                    -60.0 - 3.0 * (f / 1000.0).log2() + rng.noise(3.0)
                }
            })
            .collect()
    };
    let first = frame(&mut rng);
    let mut sim = Sim::on(level::fit(first.iter().copied()).expect("finite"));
    for _ in 0..6000 {
        let v = frame(&mut rng);
        sim.frame(Sample::of([(&freqs[..], &v[..])]).expect("columns"));
    }
    assert!(sim.moves.is_empty(), "{:?}", sim.moves);
    assert_eq!(sim.clipped_s, 0.0);
}

/// A frame's level at a frequency, from the noise.
type Level = fn(&mut Rng, f64) -> f64;

/// Pink noise read on a fine narrowband grid (its column maxima ride above the slope) and
/// as 1/3-octave band power (flat, higher): each is placed on its own drawn values, so each
/// lands within a step of its target after the same level step.
#[test]
fn narrowband_and_rta_pink_noise_each_land_on_their_target() {
    let narrow: Vec<f64> = (0..=96 * 10)
        .map(|i| 20.0 * 2f64.powf(i as f64 / 96.0))
        .collect();
    let bands: Vec<f64> = (0..30).map(|i| 25.0 * 2f64.powf(i as f64 / 3.0)).collect();
    let cases: [(&[f64], Level); 2] = [
        (&narrow, |rng, f| {
            -70.0 - 3.0 * (f / 1000.0).log2() + rng.noise(2.5).abs()
        }),
        (&bands, |rng, _| -40.0 + rng.noise(0.5)),
    ];
    for (freqs, level) in cases {
        let mut rng = Rng(5);
        let frame = |rng: &mut Rng, gain: f64| -> Vec<f64> {
            freqs.iter().map(|f| level(rng, *f) + gain).collect()
        };
        let first = frame(&mut rng, 0.0);
        let mut sim = Sim::on(level::fit(first.iter().copied()).expect("finite"));
        let mut tops = Vec::new();
        for i in 0..600 {
            let v = frame(&mut rng, if i < 100 { 0.0 } else { 20.0 });
            let s = Sample::of([(freqs, &v[..])]).expect("columns");
            tops.push(s.top);
            sim.frame(s);
            if sim.moves.len() == 1 && tops.len() > 100 {
                break;
            }
        }
        assert_eq!(sim.whys(), vec![Why::Up], "{:?}", sim.moves);
        let r = sim.r;
        let max = tops[tops.len() - 30..]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let target = r.hi - TOP_PAD * r.span();
        assert!(
            (max - target).abs() <= nice(r.span() / 8.0),
            "{} columns: {max} vs {target}",
            freqs.len()
        );
    }
}

#[test]
fn a_compared_curve_on_another_grid_can_be_the_top() {
    let own_f = [100.0, 1000.0, 10_000.0];
    let own = [-40.0, -30.0, -50.0];
    let rta_f = [
        31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0,
    ];
    let rta = [
        -20.0, -10.0, -25.0, -30.0, -30.0, -30.0, -30.0, -30.0, -30.0, -30.0,
    ];
    let s = Sample::of([(&own_f[..], &own[..]), (&rta_f[..], &rta[..])]).expect("columns");
    assert_eq!(s.top, -10.0);
    let alone = Sample::of([(&own_f[..], &own[..])]).expect("columns");
    assert_eq!(alone.top, -30.0);
    assert_eq!(alone.with(s).top, -10.0);
}

/// Columns outside 20 Hz – 20 kHz, the frequency axis's limits, never count: the shown zoom
/// is no input, so zooming the shared frequency axis cannot move a level.
#[test]
fn only_the_axis_limits_count_never_the_zoom() {
    let f = [5.0, 10.0, 100.0, 1000.0, 24_000.0];
    let l = [10.0, 0.0, -40.0, -30.0, 5.0];
    let s = Sample::of([(&f[..], &l[..])]).expect("columns");
    assert_eq!(s.top, -30.0);
    // NaN and infinite columns (no valid value) are dropped.
    let l = [f64::NAN, 0.0, f64::INFINITY, -30.0, f64::NAN];
    assert_eq!(Sample::of([(&f[..], &l[..])]).map(|s| s.top), Some(-30.0));
    assert_eq!(Sample::of([(&f[..2], &l[..2])]), None);
}

/// A restarted daemon (another epoch or incarnation) is a new signal history: the old frames
/// decide nothing, the new ones only once they cover the window.
#[test]
fn a_cleared_ring_decides_nothing_on_the_old_history() {
    let mut f = Follow::default();
    let r = Range::new(-80.0, 0.0);
    let loud = Sample {
        top: 10.0,
        floor: -30.0,
    };
    for i in 1..=25 {
        f.push(f64::from(i) * DT, loud);
    }
    f.clear();
    assert!(f.is_empty());
    let t0 = 2.5;
    for i in 1..30 {
        let t = t0 + f64::from(i) * DT;
        f.push(t, loud);
        assert_eq!(f.decide(t, r), None, "at {t}");
    }
    let t = t0 + 3.1;
    f.push(t, loud);
    assert_eq!(f.decide(t, r).map(|m| m.why), Some(Why::Up));
}

/// A stream that stops counts no time: no decision on a stalled pane, and the frames after
/// it start the window again.
#[test]
fn a_stream_that_stops_counts_no_time() {
    let mut f = Follow::default();
    let r = Range::new(-80.0, 0.0);
    let loud = Sample {
        top: 10.0,
        floor: -30.0,
    };
    for i in 1..=20 {
        f.push(f64::from(i) * DT, loud);
    }
    for i in 0..100 {
        assert_eq!(f.decide(2.0 + f64::from(i) * DT, r), None);
    }
    for i in 1..30 {
        let t = 20.0 + f64::from(i) * DT;
        f.push(t, loud);
        assert_eq!(f.decide(t, r), None, "at {t}");
    }
    f.push(23.1, loud);
    assert!(f.decide(23.1, r).is_some());
}

#[test]
fn frames_far_apart_decide_and_a_hole_holds_the_decision() {
    let r = Range::new(-80.0, 0.0);
    let loud = Sample {
        top: 10.0,
        floor: -30.0,
    };
    // A slow RTA: a frame every 0.3 s still covers the window.
    let mut f = Follow::default();
    let mut moved = None;
    for i in 1..=20 {
        let t = f64::from(i) * 0.3;
        f.push(t, loud);
        if f.decide(t, r).is_some() {
            moved = Some(t);
            break;
        }
    }
    assert!(moved.is_some_and(|t| (3.0..=3.3).contains(&t)), "{moved:?}");
    // A 3 s hole in the window: no decision until 3 s of frames after it.
    let mut f = Follow::default();
    for i in 1..=10 {
        f.push(f64::from(i) * DT, loud);
    }
    for i in 0..30 {
        let t = 4.0 + f64::from(i) * DT;
        f.push(t, loud);
        assert_eq!(f.decide(t, r), None, "at {t}");
    }
    f.push(7.0, loud);
    assert!(f.decide(7.0, r).is_some());
}

#[test]
fn an_ease_is_monotonic_keeps_its_span_and_ends_on_the_grid() {
    let from = Range::new(-80.0, 0.0);
    let mut f = Follow::default();
    for i in 1..=31 {
        f.push(
            f64::from(i) * DT,
            Sample {
                top: 12.0,
                floor: -30.0,
            },
        );
    }
    let m = f.decide(3.1, from).expect("up");
    let step = nice(from.span() / 8.0);
    assert_eq!(m.to.span(), from.span());
    assert!(
        (m.to.hi / step - (m.to.hi / step).round()).abs() < 1e-9,
        "{:?}",
        m.to
    );
    let e = Ease {
        from,
        to: m.to,
        start_s: 10.0,
    };
    let mut last = e.at(10.0);
    assert_eq!(last, from);
    for i in 1..=100 {
        let r = e.at(10.0 + f64::from(i) * 0.01);
        assert!(r.lo >= last.lo && r.hi >= last.hi, "{r:?} after {last:?}");
        assert!((r.span() - from.span()).abs() < 1e-9);
        last = r;
    }
    assert_eq!(e.at(10.0 + EASE_S), m.to);
    assert!(e.done(10.0 + EASE_S));
    assert_eq!(e.shade(10.4), 1.0);
    assert!(e.shade(10.0 + EASE_S + SHADE_S / 2.0) > 0.0);
    assert_eq!(e.shade(10.0 + EASE_S + SHADE_S), 0.0);
}

/// Fitted on a flat −110 dBFS floor (the fit's narrowest span), then music: up at once onto
/// the music, then one widen once the slow window holds it; never a second in ten minutes,
/// and no move narrows.
#[test]
fn a_floor_fitted_pane_moves_up_then_widens_once() {
    let flat = Sample {
        top: -110.0,
        floor: -110.0,
    };
    let mut sim = Sim::fitted(flat);
    let mut rng = Rng(13);
    for _ in 0..6000 {
        sim.frame(music(&mut rng, -12.0));
    }
    assert_eq!(sim.whys(), vec![Why::Up, Why::Widen], "{:?}", sim.moves);
    let mut span = level::fit([-110.0, -110.0]).expect("fit").span();
    for (_, _, r) in &sim.moves {
        assert!(r.span() >= span, "{:?}", sim.moves);
        span = r.span();
    }
    let r = sim.r;
    assert!(r.hi >= -12.0, "{r:?}");
}

#[test]
fn a_down_puts_the_maximum_at_or_below_its_target() {
    let r = Range::new(-80.0, 0.0);
    let mut f = Follow::default();
    let quiet = Sample {
        top: -63.0,
        floor: -75.0,
    };
    let mut t = 0.0;
    let m = loop {
        t += DT;
        f.push(t, quiet);
        if let Some(m) = f.decide(t, r) {
            break m;
        }
        assert!(t < 300.0);
    };
    assert_eq!(m.why, Why::Down);
    assert!(t >= SLOW_WINDOW_S);
    assert!(-63.0 <= m.to.hi - TOP_PAD * m.to.span(), "{:?}", m.to);
    assert!(-63.0 + nice(r.span() / 8.0) > m.to.hi - TOP_PAD * m.to.span());
}

/// A widened range ends on its own grid lines (`nice(span / 8)`, the step a later shift
/// moves by and the labels step by), holds the content, and holds its extent in at most
/// [`WIDEN_TO`] of it, for content anywhere in the usual levels.
#[test]
fn a_widened_range_ends_on_its_own_grid() {
    let on = |x: f64, step: f64| ((x / step).round() * step - x).abs() < 1e-6;
    let mut checked = 0;
    for top10 in -1400..=200 {
        for extent10 in (5..=1500).step_by(7) {
            let top = f64::from(top10) / 10.0;
            let floor = top - f64::from(extent10) / 10.0;
            let r = widened(floor, top).expect("finite");
            if r.lo <= crate::view::LEVEL_LIMIT_LO || r.hi >= crate::view::LEVEL_LIMIT_HI {
                continue;
            }
            let step = nice(r.span() / 8.0);
            assert!(
                on(r.lo, step) && on(r.hi, step),
                "{floor} – {top}: {r:?}, step {step}"
            );
            assert!(r.lo <= floor && r.hi >= top, "{floor} – {top}: {r:?}");
            assert!(
                top - floor <= WIDEN_TO * r.span() + 1e-9,
                "{floor} – {top}: {r:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 100_000);
}
