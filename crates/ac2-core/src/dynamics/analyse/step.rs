//! One step of one input, analysed a slice at a time once its rest is over, and a level's
//! rows, finished once its last step is (attack, settle and overshoot refer to the level's
//! settled burst).

use num_complex::Complex64;

use super::dsp::{Boxcar, HARMONICS, HannBank, Mixers, Signal, butterworth_lowpass_8, cascade};
use crate::dynamics::{
    Edge, EnvelopePoint, Overshoot, Reading, StepRow, Thump, Time, WindowHarmonics,
};
use crate::weighting::Biquad;

/// Envelope points are a quarter period apart for this long after each edge (and before the
/// burst), then log-spaced.
pub const ENVELOPE_LINEAR_SECONDS: f64 = 0.050;

/// Log-spaced envelope points per decade.
pub const ENVELOPE_POINTS_PER_DECADE: f64 = 48.0;

/// The harmonic window at the start of a burst, s.
pub const ATTACK_WINDOW_SECONDS: f64 = 0.050;

/// The harmonic window after a burst, s.
pub const RELEASE_WINDOW_SECONDS: f64 = 1.0;

/// The harmonic window at the end of a rest, s.
pub const REST_WINDOW_SECONDS: f64 = 1.0;

/// A harmonic or thump reading counts only this far above its floor, dB.
pub const FLOOR_MARGIN_DB: f64 = 6.0;

/// Harmonics below this re the fundamental, dB, are rounding of f32 audio, whatever the
/// leakage floor says: a steady fundamental has no leakage, so its floor alone would let
/// rounding noise read as distortion.
pub const ARITHMETIC_FLOOR_DB: f64 = -120.0;

/// Thump is read over this long after each edge, s.
pub const THUMP_WINDOW_SECONDS: f64 = 0.200;

/// Corner of the thump low-pass, Hz.
pub const THUMP_CORNER_HZ: f64 = 200.0;

/// Lowest carrier thump is read at, Hz. The 8th-order low-pass leaves a 1 kHz carrier
/// 112 dB down; a typed 1 kHz snaps to a period up to two samples off at 44.1 kHz with a
/// peak onset (955 Hz), which the filter still leaves 108 dB down.
pub const THUMP_MIN_CARRIER_HZ: f64 = 950.0;

/// Thump is printed only where the edge's time constant is at least this many carrier
/// periods. The floor is the reference pushed through the gain the one-period envelope
/// reads, and that envelope cannot follow a gain change much shorter than a period: under
/// it the floor under-reads and an ideal, thump-free limiter reads above it. A peak onset is
/// the worst case (the detector sees the whole step at once): at 1 kHz an ideal limiter
/// whose attack measures 3.5 periods still reads 3.1 dB over its floor, at 4.5 periods
/// 2.5 dB, so from 4 periods on every ideal edge stays at least 3 dB inside the 6 dB
/// margin. Pinned by `tests::slow_thump_gate_is_the_smallest_safe_whole_period_count`.
pub const THUMP_MIN_PERIODS: u32 = 4;

/// A burst is settled when its gain moved less than this over its last 10 %, dB.
pub const SETTLED_DB: f64 = 0.05;

/// Attack and release time-constant fractions of the gain reduction (in dB).
const TAU_RISE: f64 = 0.63;
const TAU_FALL: f64 = 0.37;

/// A level's settled gain reduction under this reads "no gain reduction", dB.
pub const MIN_GR_DB: f64 = 1.0;

/// The 2 dB columns need at least this much settled gain reduction, dB.
pub const MIN_GR_2DB_DB: f64 = 3.0;

/// Where a step lies, absolute samples.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bounds {
    /// The burst's first sample.
    pub(super) s: u64,
    /// The rest's first sample.
    pub(super) e: u64,
    /// The sample after the rest.
    pub(super) end: u64,
}

impl Bounds {
    pub(super) fn burst_len(&self) -> u64 {
        self.e - self.s
    }

    /// Samples of the last 10 % of the burst (at least one).
    fn tail(&self) -> u64 {
        self.burst_len().div_ceil(10).max(1)
    }
}

/// What a step's analysis needs to know of the run.
#[derive(Debug)]
pub(super) struct Ctx<'a> {
    pub(super) mx: &'a Mixers,
    pub(super) fs: f64,
    pub(super) carrier_hz: f64,
}

impl Ctx<'_> {
    fn p(&self) -> u64 {
        self.mx.period() as u64
    }

    fn secs(&self, s: f64) -> u64 {
        (s * self.fs).round() as u64
    }

    /// Samples before the burst the analysis starts at: the envelope's 50 ms and the thump
    /// filter's settling (its slowest pole pair rings with τ ≈ Q/(π·200 Hz) ≈ 4 ms).
    pub(super) fn pre(&self) -> u64 {
        self.secs(2.0 * ENVELOPE_LINEAR_SECONDS)
    }

    /// Samples beyond `[lo, hi)` the per-sample passes read on either side: the Hann's two
    /// periods, the boxcar's half period, and one more for the boxcar under the Hann's edge.
    pub(super) fn margin(&self) -> u64 {
        let p = self.p();
        2 * p + 1 + p / 2 + 1
    }
}

/// The gain-reduction curve over a burst, reduced to what the attack and settle times need
/// once the level's settled GR is known: its first value, the running maximum's records
/// (first crossings upward), and the running maximum and minimum from the end (the last
/// excursion outside a band). Records are rare once the curve settles, so a long burst
/// costs little.
#[derive(Debug, Clone, Default)]
pub(super) struct GrTrace {
    /// Absolute sample of the first searched point.
    a: u64,
    /// Samples searched.
    n: u64,
    first: f32,
    /// `(offset, value before, value)` where the running maximum grows.
    rise: Vec<(u64, f32, f32)>,
    /// `(offset, value, value after)` where the maximum from the end grows.
    hi: Vec<(u64, f32, f32)>,
    /// The same for the minimum from the end.
    lo: Vec<(u64, f32, f32)>,
}

impl GrTrace {
    fn new(gr: &[f32], a: u64) -> Self {
        let mut t = Self {
            a,
            n: gr.len() as u64,
            ..Self::default()
        };
        let Some(&first) = gr.first() else {
            return t;
        };
        t.first = first;
        let mut best = first;
        for j in 1..gr.len() {
            if gr[j] > best {
                best = gr[j];
                t.rise.push((j as u64, gr[j - 1], gr[j]));
            }
        }
        let last = gr.len() - 1;
        let (mut mx, mut mn) = (gr[last], gr[last]);
        t.hi.push((last as u64, gr[last], gr[last]));
        t.lo.push((last as u64, gr[last], gr[last]));
        for j in (0..last).rev() {
            if gr[j] > mx {
                mx = gr[j];
                t.hi.push((j as u64, gr[j], gr[j + 1]));
            }
            if gr[j] < mn {
                mn = gr[j];
                t.lo.push((j as u64, gr[j], gr[j + 1]));
            }
        }
        t
    }

    /// First time from `from` the curve reaches `x`.
    fn first_at_least(&self, x: f64, from: u64, fs: f64) -> Time {
        if self.n == 0 {
            return Time::CantTell;
        }
        if f64::from(self.first) >= x {
            return Time::WithinPeriod;
        }
        match self.rise.iter().find(|r| f64::from(r.2) >= x) {
            None => Time::CantTell,
            Some(&(j, before, at)) => {
                let frac = crossing(f64::from(before), f64::from(at), x);
                Time::Seconds(((self.a + j - 1) as f64 + frac - from as f64) / fs)
            }
        }
    }

    /// First time from `from` after which the curve stays within `band` of `centre`.
    fn settles(&self, centre: f64, band: f64, from: u64, fs: f64) -> Time {
        if self.n == 0 {
            return Time::CantTell;
        }
        let (top, bottom) = (centre + band, centre - band);
        let above = self.hi.iter().find(|r| f64::from(r.1) > top);
        let below = self.lo.iter().find(|r| f64::from(r.1) < bottom);
        let last = match (above, below) {
            (None, None) => return Time::WithinPeriod,
            (Some(a), None) => (a, top),
            (None, Some(b)) => (b, bottom),
            (Some(a), Some(b)) => {
                if a.0 >= b.0 {
                    (a, top)
                } else {
                    (b, bottom)
                }
            }
        };
        let (&(j, at, after), edge) = last;
        if j + 1 >= self.n {
            return Time::CantTell;
        }
        let frac = crossing(f64::from(at), f64::from(after), edge);
        Time::Seconds(((self.a + j) as f64 + frac - from as f64) / fs)
    }
}

/// Where between two samples a line through them crosses `x` (0 … 1).
fn crossing(a: f64, b: f64, x: f64) -> f64 {
    if (b - a).abs() < f64::MIN_POSITIVE {
        0.0
    } else {
        ((x - a) / (b - a)).clamp(0.0, 1.0)
    }
}

/// One harmonic window's sums of per-sample power ratios re the fundamental.
#[derive(Debug, Clone, Copy, Default)]
struct HarmonicSums {
    h: [f64; HARMONICS],
    floor: [f64; HARMONICS],
    n: u64,
}

impl HarmonicSums {
    fn reading(&self) -> WindowHarmonics {
        if self.n == 0 {
            return WindowHarmonics {
                h: [Reading::CantTell; HARMONICS],
                thd: Reading::CantTell,
            };
        }
        let n = self.n as f64;
        let read = |h: f64, f: f64| {
            if h < f * 10f64.powf(FLOOR_MARGIN_DB / 10.0) {
                Reading::BelowFloor
            } else {
                Reading::Db(10.0 * h.log10())
            }
        };
        WindowHarmonics {
            h: std::array::from_fn(|k| read(self.h[k] / n, self.floor[k] / n)),
            thd: read(
                self.h.iter().sum::<f64>() / n,
                self.floor.iter().sum::<f64>() / n,
            ),
        }
    }
}

/// What one step of one input leaves for its level's rows.
#[derive(Debug, Clone)]
pub(super) struct Partial {
    pub(super) bounds: Bounds,
    pub(super) gr_end: f64,
    pub(super) settled: bool,
    attack: GrTrace,
    /// Largest |y| in the burst and when (s from its first sample); the same over its last
    /// 10 %; and the reference's two.
    y_peak: (f64, f64),
    y_tail_peak: f64,
    r_peak: f64,
    r_tail_peak: f64,
    release_tau: Time,
    recovery_2db: Time,
    recovery_0_5db: Time,
    harmonics: [WindowHarmonics; 4],
    thump: [Thump; 2],
    drift: f64,
    pub(super) envelope: Vec<EnvelopePoint>,
    /// Thump peak and RMS over their floors per edge, dB, before any gate.
    #[cfg(test)]
    pub(super) thump_excess: [[f64; 2]; 2],
}

/// Gain reduction of `y` re `r` over `[from, to)` as one window, dB (the exact one-period
/// average where the window is a period).
fn window_gr(r: Signal<'_>, y: Signal<'_>, from: u64, to: u64, mx: &Mixers, g0: f64) -> f64 {
    let (mut rs, mut ys) = (Complex64::new(0.0, 0.0), Complex64::new(0.0, 0.0));
    for v in from..to {
        let c = mx.carrier(v);
        rs += c * r.get(v);
        ys += c * y.get(v);
    }
    -(20.0 * (ys.norm() / rs.norm()).log10() - g0)
}

/// The envelope points of a step: `(sample, edge, seconds from the edge)`.
fn envelope_points(b: Bounds, ctx: &Ctx<'_>) -> Vec<(u64, Edge, f64)> {
    let q = ctx.p() as f64 / 4.0;
    let lin = ENVELOPE_LINEAR_SECONDS * ctx.fs;
    let mut out = Vec::new();
    let mut run = |edge: Edge, origin: u64, first_j: i64, end: u64| {
        let mut last: Option<i64> = None;
        let mut push = |off: i64, out: &mut Vec<(u64, Edge, f64)>| -> bool {
            let u = origin as i64 + off;
            if u < 0 || u as u64 >= end {
                return false;
            }
            if last.is_none_or(|l| off > l) {
                out.push((u as u64, edge, off as f64 / ctx.fs));
                last = Some(off);
            }
            true
        };
        let mut j = first_j;
        loop {
            let off = (j as f64 * q).round();
            if off >= lin || !push(off as i64, &mut out) {
                break;
            }
            j += 1;
        }
        let mut m = 0;
        loop {
            let off = (ENVELOPE_LINEAR_SECONDS
                * 10f64.powf(f64::from(m) / ENVELOPE_POINTS_PER_DECADE)
                * ctx.fs)
                .round();
            if !push(off as i64, &mut out) {
                break;
            }
            m += 1;
        }
    };
    run(Edge::Start, b.s, -((lin / q).floor() as i64), b.e);
    run(Edge::End, b.e - 1, 1, b.end);
    out
}

/// Where a [`StepJob`] is: its passes in order, each at its next sample.
#[derive(Debug, Clone, Copy)]
enum Stage {
    /// The fundamental's envelopes, at offset `i` from `a0`.
    Envelope(usize),
    /// The harmonics, at sample `u`.
    Harmonics(u64),
    /// The thump low-pass, at sample `u`.
    Thump(u64),
    /// The raw waveforms' peaks, at sample `u`.
    Peaks(u64),
}

/// The analysis of one step of one input, run a bounded number of samples at a time
/// ([`StepJob::run`]): every pass is a sliding sum or a filter whose state lives here, so
/// stopping between samples and resuming changes no arithmetic, and the result is the same
/// however the work is cut. `r` and `y` (the input, already shifted by its whole delay) must
/// hold `[s − pre − margin − P/2, end + margin + P/2]` until the job is done.
#[derive(Debug)]
pub(super) struct StepJob {
    b: Bounds,
    g0: f64,
    lo: u64,
    hi: u64,
    a0: u64,
    n: usize,
    stage: Stage,
    r_amp: Vec<f32>,
    y_amp: Vec<f32>,
    gr: Vec<f32>,
    rec: Vec<f32>,
    rb: Boxcar,
    yb: Boxcar,
    points: Vec<(u64, Edge, f64)>,
    next_point: usize,
    windows: [(u64, u64); 4],
    /// The output's and the reconstructed fundamental's harmonic banks, once pass 2 starts.
    banks: Option<(HannBank, HannBank)>,
    envelope: Vec<EnvelopePoint>,
    sums: [HarmonicSums; 4],
    lp_y: [Biquad; 4],
    lp_f: [Biquad; 4],
    /// Per edge: peak and sum of squares of the output, then of the floor.
    th: [[f64; 4]; 2],
    thump_len: u64,
    thump_end: u64,
    /// Largest |y| in the burst and its sample offset, over the burst's last stretch, and
    /// the reference's two.
    y_peak: f64,
    y_at: u64,
    y_tail: f64,
    r_peak: f64,
    r_tail: f64,
}

impl StepJob {
    pub(super) fn new(ctx: &Ctx<'_>, r: Signal<'_>, y: Signal<'_>, g0: f64, b: Bounds) -> Self {
        let h = ctx.p() / 2;
        let lo = b.s - ctx.pre();
        let hi = b.end;
        let a0 = lo - ctx.margin() + h;
        let a1 = hi + ctx.margin() - h;
        let n = (a1 - a0) as usize;
        let tail = b.tail();
        let thump_len = ctx.secs(THUMP_WINDOW_SECONDS);
        Self {
            b,
            g0,
            lo,
            hi,
            a0,
            n,
            stage: Stage::Envelope(0),
            r_amp: vec![0f32; n],
            y_amp: vec![0f32; n],
            gr: vec![0f32; n],
            rec: vec![0f32; n],
            rb: Boxcar::new(r, ctx.mx, a0),
            yb: Boxcar::new(y, ctx.mx, a0),
            points: envelope_points(b, ctx),
            next_point: 0,
            windows: [
                (b.s, (b.s + ctx.secs(ATTACK_WINDOW_SECONDS)).min(b.e)),
                (b.e - tail, b.e),
                (b.e, (b.e + ctx.secs(RELEASE_WINDOW_SECONDS)).min(b.end)),
                (
                    b.end.saturating_sub(ctx.secs(REST_WINDOW_SECONDS)).max(b.e),
                    b.end,
                ),
            ],
            banks: None,
            envelope: Vec::new(),
            sums: [HarmonicSums::default(); 4],
            lp_y: butterworth_lowpass_8(THUMP_CORNER_HZ, ctx.fs),
            lp_f: butterworth_lowpass_8(THUMP_CORNER_HZ, ctx.fs),
            th: [[0f64; 4]; 2],
            thump_len,
            thump_end: (b.e + thump_len).min(hi),
            y_peak: 0.0,
            y_at: 0,
            y_tail: 0.0,
            r_peak: 0.0,
            r_tail: 0.0,
        }
    }

    fn at(&self, u: u64) -> usize {
        (u - self.a0) as usize
    }

    /// Where pass 2 starts: the first envelope point, which lies before the burst.
    fn first(&self) -> u64 {
        self.points.first().map_or(self.b.s, |q| q.0).min(self.b.s)
    }

    /// Runs up to `budget` samples of the passes, taking what it used from `budget`; the
    /// step's partial once the last pass is done.
    pub(super) fn run(
        &mut self,
        ctx: &Ctx<'_>,
        r: Signal<'_>,
        y: Signal<'_>,
        budget: &mut u64,
    ) -> Option<Partial> {
        loop {
            if *budget == 0 {
                return None;
            }
            match self.stage {
                Stage::Envelope(i0) => {
                    let end = self
                        .n
                        .min(i0.saturating_add(usize::try_from(*budget).unwrap_or(usize::MAX)));
                    self.envelope_pass(ctx, r, y, i0, end);
                    *budget -= (end - i0) as u64;
                    self.stage = if end == self.n {
                        let first = self.first();
                        let rec = Signal {
                            x: &self.rec,
                            abs0: self.a0,
                        };
                        self.banks = Some((
                            HannBank::new(y, ctx.mx, first),
                            HannBank::new(rec, ctx.mx, first),
                        ));
                        Stage::Harmonics(first)
                    } else {
                        Stage::Envelope(end)
                    };
                }
                Stage::Harmonics(u0) => {
                    let end = self.hi.min(u0.saturating_add(*budget));
                    self.harmonic_pass(ctx, y, u0, end);
                    *budget -= end - u0;
                    self.stage = if end == self.hi {
                        self.banks = None;
                        Stage::Thump(self.lo)
                    } else {
                        Stage::Harmonics(end)
                    };
                }
                Stage::Thump(u0) => {
                    let end = self.thump_end.max(u0).min(u0.saturating_add(*budget));
                    self.thump_pass(r, y, u0, end);
                    *budget -= end - u0;
                    self.stage = if end >= self.thump_end {
                        Stage::Peaks(self.b.s)
                    } else {
                        Stage::Thump(end)
                    };
                }
                Stage::Peaks(u0) => {
                    let end = self.b.e.min(u0.saturating_add(*budget));
                    self.peak_pass(ctx, r, y, u0, end);
                    *budget -= end - u0;
                    if end == self.b.e {
                        return Some(self.finish(ctx, r, y));
                    }
                    self.stage = Stage::Peaks(end);
                }
            }
        }
    }

    /// Pass 1: the fundamental's envelopes, gain reduction, and the reconstructed
    /// fundamental the harmonic floor is read from.
    fn envelope_pass(&mut self, ctx: &Ctx<'_>, r: Signal<'_>, y: Signal<'_>, i0: usize, i1: usize) {
        let mx = ctx.mx;
        for i in i0..i1 {
            let u = self.a0 + i as u64;
            let (rc, yc) = (self.rb.value(r, mx), self.yb.value(y, mx));
            self.r_amp[i] = rc.norm() as f32;
            self.y_amp[i] = yc.norm() as f32;
            self.gr[i] = (-(20.0 * (yc.norm() / rc.norm()).log10() - self.g0)) as f32;
            self.rec[i] = (yc * mx.carrier(u).conj()).re as f32;
            if i + 1 < self.n {
                self.rb.advance(r, mx);
                self.yb.advance(y, mx);
            }
        }
    }

    /// Pass 2: harmonics over the windows and at the envelope points, blind wherever the
    /// 4-period window holds an edge and for 4 periods after it.
    fn harmonic_pass(&mut self, ctx: &Ctx<'_>, y: Signal<'_>, u0: u64, u1: u64) {
        let p = ctx.p();
        let b = self.b;
        let edges = [b.s, b.e, b.end];
        let blind = |u: u64| edges.iter().any(|&x| u + 2 * p >= x && u <= x + 4 * p);
        let arith = 10f64.powf(ARITHMETIC_FLOOR_DB / 10.0);
        let margin = 10f64.powf(FLOOR_MARGIN_DB / 10.0);
        let rec = Signal {
            x: &self.rec,
            abs0: self.a0,
        };
        let Some((hy, hf)) = self.banks.as_mut() else {
            return;
        };
        for u in u0..u1 {
            let point = self
                .points
                .get(self.next_point)
                .filter(|q| q.0 == u)
                .copied();
            if point.is_some() {
                self.next_point += 1;
            }
            let mut in_window = [false; 4];
            for (w, f) in in_window.iter_mut().enumerate() {
                *f = u >= self.windows[w].0 && u < self.windows[w].1;
            }
            let y1 = f64::from(self.y_amp[(u - self.a0) as usize]).powi(2);
            let mut h_db = [f32::NAN; HARMONICS];
            if (point.is_some() || in_window.contains(&true)) && !blind(u) && y1 > 0.0 {
                let (yk, fk) = (hy.value(ctx.mx), hf.value(ctx.mx));
                let rh = yk.map(|v| v * v / y1);
                let rf = fk.map(|v| (v * v / y1).max(arith));
                for w in (0..4).filter(|&w| in_window[w]) {
                    for k in 0..HARMONICS {
                        self.sums[w].h[k] += rh[k];
                        self.sums[w].floor[k] += rf[k];
                    }
                    self.sums[w].n += 1;
                }
                for k in 0..HARMONICS {
                    if rh[k] >= rf[k] * margin {
                        h_db[k] = (10.0 * rh[k].log10()) as f32;
                    }
                }
            }
            if let Some(q) = point {
                self.envelope.push(EnvelopePoint {
                    edge: q.1,
                    t: q.2,
                    gr_db: self.gr[(u - self.a0) as usize],
                    h_db,
                });
            }
            if u + 1 < self.hi {
                hy.advance(y, ctx.mx);
                hf.advance(rec, ctx.mx);
            }
        }
    }

    /// Thump: the output and the floor (the reference through the gain the envelope reads)
    /// through the same low-pass, run from before the burst so the filters have settled.
    fn thump_pass(&mut self, r: Signal<'_>, y: Signal<'_>, u0: u64, u1: u64) {
        let starts = [self.b.s, self.b.e];
        for u in u0..u1 {
            let i = self.at(u);
            let ratio = f64::from(self.y_amp[i]) / f64::from(self.r_amp[i]);
            let fy = cascade(&mut self.lp_y, y.get(u));
            let ff = cascade(
                &mut self.lp_f,
                r.get(u) * if ratio.is_finite() { ratio } else { 0.0 },
            );
            for (w, &st) in starts.iter().enumerate() {
                if u >= st && u < st + self.thump_len {
                    let th = &mut self.th[w];
                    th[0] = th[0].max(fy.abs());
                    th[1] += fy * fy;
                    th[2] = th[2].max(ff.abs());
                    th[3] += ff * ff;
                }
            }
        }
    }

    /// Peaks of the raw waveforms (an overshoot shorter than a period is averaged away in
    /// the envelope).
    fn peak_pass(&mut self, ctx: &Ctx<'_>, r: Signal<'_>, y: Signal<'_>, u0: u64, u1: u64) {
        let b = self.b;
        let tail_from = b.e - b.tail().max(ctx.p()).min(b.burst_len());
        for u in u0..u1 {
            let (yv, rv) = (y.get(u).abs(), r.get(u).abs());
            if yv > self.y_peak {
                self.y_peak = yv;
                self.y_at = u - b.s;
            }
            self.r_peak = self.r_peak.max(rv);
            if u >= tail_from {
                self.y_tail = self.y_tail.max(yv);
                self.r_tail = self.r_tail.max(rv);
            }
        }
    }

    /// What the passes leave, read off: each a scan of the stored gain reduction or a
    /// single period.
    fn finish(&mut self, ctx: &Ctx<'_>, r: Signal<'_>, y: Signal<'_>) -> Partial {
        let p = ctx.p();
        let h = p / 2;
        let fs = ctx.fs;
        let b = self.b;
        let tail = b.tail();
        let (y_peak, th, thump_len) = (self.y_peak, self.th, self.thump_len);
        let thump = if ctx.carrier_hz < THUMP_MIN_CARRIER_HZ || y_peak <= 0.0 {
            [Thump {
                peak: Reading::CantTell,
                rms: Reading::CantTell,
            }; 2]
        } else {
            th.map(|t| {
                let n = thump_len as f64;
                let read = |v: f64, floor: f64| {
                    if v < floor * 10f64.powf(FLOOR_MARGIN_DB / 20.0) || v <= 0.0 {
                        Reading::BelowFloor
                    } else {
                        Reading::Db(20.0 * (v / y_peak).log10())
                    }
                };
                Thump {
                    peak: read(t[0], t[2]),
                    rms: read((t[1] / n).sqrt(), (t[3] / n).sqrt()),
                }
            })
        };

        let gr = &self.gr;
        let at = |u: u64| (u - self.a0) as usize;
        // Settled: the gain over the burst's last 10 %, where the centred envelope lies
        // wholly inside the burst.
        let settle_from = (b.e - tail).max(b.s + p + 1);
        let settle_to = b.e.saturating_sub(h);
        let settled = settle_to > settle_from && settle_to - settle_from >= p && {
            let w = &gr[at(settle_from)..at(settle_to)];
            let q = (w.len() / 4).max(1);
            let mean = |s: &[f32]| s.iter().map(|&v| f64::from(v)).sum::<f64>() / s.len() as f64;
            (mean(&w[..q]) - mean(&w[w.len() - q..])).abs() < SETTLED_DB
        };

        // The searches skip the first period plus one sample after each edge, where the
        // one-period envelope still holds the other side of the step.
        let a_from = b.s + p + 1;
        let attack = if settle_to > a_from {
            GrTrace::new(&gr[at(a_from)..at(settle_to)], a_from)
        } else {
            GrTrace::default()
        };
        let gr_end = window_gr(r, y, b.e - p.min(b.burst_len()), b.e, ctx.mx, self.g0);
        let te = b.e - 1;
        let r_from = te + p + 1;
        let r_to = b.end.saturating_sub(h);
        let rest = if r_to > r_from {
            &gr[at(r_from)..at(r_to)]
        } else {
            &[][..]
        };
        let release_tau = first_at_most(rest, TAU_FALL * gr_end, r_from, te, fs);
        let recovery_2db = last_at_least(rest, 2.0, r_from, te, fs);
        let recovery_0_5db = last_at_least(rest, 0.5, r_from, te, fs);
        let drift = -window_gr(r, y, b.end - p, b.end, ctx.mx, self.g0);

        Partial {
            bounds: b,
            gr_end,
            settled,
            attack,
            y_peak: (y_peak, self.y_at as f64 / fs),
            y_tail_peak: self.y_tail,
            r_peak: self.r_peak,
            r_tail_peak: self.r_tail,
            release_tau,
            recovery_2db,
            recovery_0_5db,
            harmonics: self.sums.map(|s| s.reading()),
            thump,
            drift,
            envelope: std::mem::take(&mut self.envelope),
            #[cfg(test)]
            thump_excess: th.map(|t| [20.0 * (t[0] / t[2]).log10(), 10.0 * (t[1] / t[3]).log10()]),
        }
    }
}

/// First time the curve falls to `x`, from `origin`.
fn first_at_most(gr: &[f32], x: f64, a: u64, origin: u64, fs: f64) -> Time {
    let Some(&first) = gr.first() else {
        return Time::CantTell;
    };
    if f64::from(first) <= x {
        return Time::WithinPeriod;
    }
    match gr.iter().position(|&v| f64::from(v) <= x) {
        None => Time::CantTell,
        Some(j) => {
            let frac = crossing(f64::from(gr[j - 1]), f64::from(gr[j]), x);
            Time::Seconds(((a + j as u64 - 1) as f64 + frac - origin as f64) / fs)
        }
    }
}

/// First time after which the curve stays under `x`, from `origin`.
fn last_at_least(gr: &[f32], x: f64, a: u64, origin: u64, fs: f64) -> Time {
    if gr.is_empty() {
        return Time::CantTell;
    }
    match gr.iter().rposition(|&v| f64::from(v) >= x) {
        None => Time::WithinPeriod,
        Some(j) if j + 1 >= gr.len() => Time::CantTell,
        Some(j) => {
            let frac = crossing(f64::from(gr[j]), f64::from(gr[j + 1]), x);
            Time::Seconds(((a + j as u64) as f64 + frac - origin as f64) / fs)
        }
    }
}

/// Whether thump after an edge whose gain moves with time constant `tau` can be told from
/// its floor.
fn thump_readable(tau: Time, min_seconds: f64) -> bool {
    match tau {
        Time::NoGainReduction => true,
        Time::Seconds(t) => t >= min_seconds,
        Time::WithinPeriod | Time::CantTell => false,
    }
}

/// The rows of one level from its steps' partials (one input), in play order.
pub(super) fn finish_level(
    parts: Vec<Partial>,
    fs: f64,
    period: u64,
) -> Vec<(StepRow, Vec<EnvelopePoint>)> {
    // The longest settled burst (the first of equals) sets the level's settled GR and peak.
    let settled = parts
        .iter()
        .filter(|q| q.settled)
        .fold(None::<&Partial>, |best, q| match best {
            Some(b) if b.bounds.burst_len() >= q.bounds.burst_len() => Some(b),
            _ => Some(q),
        });
    let gr_s = settled.map(|q| q.gr_end);
    let settled_peaks = settled.map(|q| (q.y_tail_peak, q.r_tail_peak));
    let min_thump = f64::from(THUMP_MIN_PERIODS) * period as f64 / fs;
    let thump_len = (THUMP_WINDOW_SECONDS * fs).round() as u64;
    let none = Time::NoGainReduction;
    let no_gr = gr_s.is_some_and(|g| g < MIN_GR_DB);
    parts
        .into_iter()
        .map(|q| {
            let b = q.bounds;
            let (attack_tau, attack_2db, settle_0_5db) = match gr_s {
                None => (Time::CantTell, Time::CantTell, Time::CantTell),
                Some(_) if no_gr => (none, none, none),
                Some(g) => (
                    q.attack.first_at_least(TAU_RISE * g, b.s, fs),
                    if g >= MIN_GR_2DB_DB {
                        q.attack.settles(g, 2.0, b.s, fs)
                    } else {
                        Time::CantTell
                    },
                    q.attack.settles(g, 0.5, b.s, fs),
                ),
            };
            let release_tau = if no_gr || q.gr_end < MIN_GR_DB {
                none
            } else {
                q.release_tau
            };
            let recovery_2db = match gr_s {
                _ if no_gr => none,
                Some(g) if g >= MIN_GR_2DB_DB => q.recovery_2db,
                _ => Time::CantTell,
            };
            let recovery_0_5db = if no_gr { none } else { q.recovery_0_5db };
            let overshoot = settled_peaks.map(|(y_tail, r_tail)| Overshoot {
                db: 20.0 * (q.y_peak.0 / y_tail).log10(),
                at: q.y_peak.1,
                reference_db: 20.0 * (q.r_peak / r_tail).log10(),
            });
            // The floor follows a gain change only as fast as the one-period envelope: an
            // edge whose gain moves faster cannot be told from its floor. A burst shorter
            // than the thump window holds its end edge in the start edge's window too.
            let end_ok = thump_readable(release_tau, min_thump);
            let start_ok =
                thump_readable(attack_tau, min_thump) && (b.burst_len() >= thump_len || end_ok);
            let gate = |t: Thump, ok: bool| {
                if ok {
                    t
                } else {
                    Thump {
                        peak: Reading::CantTell,
                        rms: Reading::CantTell,
                    }
                }
            };
            let row = StepRow {
                gr_end: q.gr_end,
                gr_settled: gr_s,
                settled: q.settled,
                attack_tau,
                attack_2db,
                settle_0_5db,
                overshoot,
                release_tau,
                recovery_2db,
                recovery_0_5db,
                harmonics: q.harmonics,
                thump: [gate(q.thump[0], start_ok), gate(q.thump[1], end_ok)],
                drift: q.drift,
            };
            (row, q.envelope)
        })
        .collect()
}
