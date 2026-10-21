//! Peak tracking: the few highest peaks (or deepest dips) of a curve, kept steady from frame
//! to frame. Display logic on the values the curve already shows (local extremes of the
//! drawn columns), not analysis: nothing here is measured that the curve does not show.
//! A dip is a peak of the curve turned upside down, so both follow the same rules.

use ac2_proto::frame::ValidityMask;
use ac2_proto::model::Unresolved;

/// How many peaks a tracked curve marks. Five rows fit the list overlay under the cursor
/// readout on a small pane, and cover a tone with its first few harmonics or the main
/// resonances of a room — past that the list is a table nobody reads live.
pub const TRACKED: usize = 5;

/// How far a maximum must stand above the higher of the lowest points between it and the
/// next higher point on either side (its topographic prominence), dB. The ripple of an
/// averaged noise floor is a few dB from column to column; 6 dB (twice the power) is clear
/// of that, so a mark sits on something that stands out of the curve, not on noise.
pub const PROMINENCE_DB: f64 = 6.0;

/// Peaks closer than this, in octaves, are one peak: the stronger is kept. A tone's window
/// skirt or a resonance's shoulder makes side maxima right beside it, and on a ten-octave
/// axis 1/6 octave is the width of a marker box or more, so two marks never overlap.
pub const SPACING_OCT: f64 = 1.0 / 6.0;

/// A marked peak's head start when the marks are chosen again, dB. A steady source's
/// averaged levels wander a dB or two between frames; a rival must beat a marked peak by
/// more than that to take its place, so marks do not hop between two similar peaks.
pub const HOLD_DB: f64 = 3.0;

/// How much louder a mark must be than the one listed above it to move up the list, dB:
/// two peaks of about the same level keep their rows instead of swapping every frame.
pub const ORDER_DB: f64 = 1.0;

/// Which way an extreme points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Extreme {
    /// A peak: higher than the curve around it.
    #[default]
    Max,
    /// A dip: lower than the curve around it.
    Min,
}

impl Extreme {
    /// `level` as a height: how far it stands out in this direction.
    fn height(self, level: f64) -> f64 {
        match self {
            Self::Max => level,
            Self::Min => -level,
        }
    }
}

/// One peak (or dip): where it is and the curve's level there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub freq_hz: f64,
    /// The column's own level, what the curve and the cursor read there.
    pub level: f64,
}

/// A curve's columns, as the peak finder reads them.
#[derive(Clone, Copy, Debug)]
pub struct PeakInput<'a> {
    /// Column centre frequencies, ascending.
    pub freqs: &'a [f64],
    pub level: &'a [f32],
    pub validity: Option<&'a [ValidityMask]>,
    /// Where columns are finer than the analysis resolves (a multi-resolution spectrum).
    pub unresolved: Option<&'a Unresolved>,
    /// Narrowband bins (`true`) or fractional-octave bands (`false`).
    pub narrow: bool,
}

impl PeakInput<'_> {
    /// Column `i`'s level, or NaN where it may not hold a peak: a gap, a column flagged
    /// invalid, or one inside an unresolved range — there several columns share one bin of
    /// the window behind them, so their shape (and any maximum in it) is interpolation.
    fn value(&self, i: usize) -> f64 {
        let ok = self
            .validity
            .is_none_or(|v| v.get(i).is_some_and(|m| *m == ValidityMask::NONE));
        let resolved = self.unresolved.is_none_or(|u| {
            let f = self.freqs[i];
            !u.ranges.iter().any(|r| f >= r.lo.0 && f <= r.hi.0)
        });
        match self.level.get(i) {
            Some(l) if ok && resolved && l.is_finite() => f64::from(*l),
            _ => f64::NAN,
        }
    }
}

/// Every peak of the curve that stands out by [`PROMINENCE_DB`], merged to one per
/// [`SPACING_OCT`], loudest first.
pub fn find_peaks(input: &PeakInput<'_>) -> Vec<Peak> {
    let n = input.freqs.len().min(input.level.len());
    let v: Vec<f64> = (0..n).map(|i| input.value(i)).collect();
    find_extremes(&input.freqs[..n], &v, Extreme::Max, input.narrow)
}

/// Every extreme of `values` (dB, column-aligned with `freqs`; NaN where no extreme may
/// be) pointing `dir` that stands out by [`PROMINENCE_DB`], merged to one per
/// [`SPACING_OCT`], the most outstanding (highest peak, deepest dip) first. `narrow`: the
/// columns are points on a curve, not bands, and the frequency is interpolated.
pub fn find_extremes(freqs: &[f64], values: &[f64], dir: Extreme, narrow: bool) -> Vec<Peak> {
    let n = freqs.len().min(values.len());
    let v: Vec<f64> = values[..n].iter().map(|x| dir.height(*x)).collect();
    let mut found: Vec<(usize, Peak)> = Vec::new();
    for i in 0..n {
        let b = v[i];
        if !b.is_finite() {
            continue;
        }
        let left = if i > 0 { v[i - 1] } else { f64::NAN };
        let right = v.get(i + 1).copied().unwrap_or(f64::NAN);
        // Strictly above the left, not below the right: a flat top counts once, at its
        // first column.
        if !(left.is_nan() || b > left) || right > b {
            continue;
        }
        if prominence(&v, i) < PROMINENCE_DB {
            continue;
        }
        let freq_hz = if narrow {
            interpolated_freq(freqs, &v, i)
        } else {
            // A band's level is the power in the whole band: no point in it is the peak's
            // frequency more than another, so it reads the band's centre.
            freqs[i]
        };
        found.push((
            i,
            Peak {
                freq_hz,
                level: dir.height(b),
            },
        ));
    }
    found.sort_by(|a, b| dir.height(b.1.level).total_cmp(&dir.height(a.1.level)));
    let mut kept: Vec<Peak> = Vec::new();
    for (_, p) in found {
        if !kept.iter().any(|k| near(k.freq_hz, p.freq_hz)) {
            kept.push(p);
        }
    }
    kept
}

/// Within [`SPACING_OCT`] of each other.
fn near(a: f64, b: f64) -> bool {
    (a / b).log2().abs() < SPACING_OCT
}

/// Height of `v[i]` above the higher of the lowest points on each side before a higher
/// point; a gap (NaN) ends a side as the curve's end does: across a gap nothing is known.
pub(crate) fn prominence(v: &[f64], i: usize) -> f64 {
    let peak = v[i];
    let side = |it: &mut dyn Iterator<Item = usize>| {
        let mut low = peak;
        for j in it {
            let x = v[j];
            if x.is_nan() || x > peak {
                break;
            }
            low = low.min(x);
        }
        low
    };
    let l = side(&mut (0..i).rev());
    let r = side(&mut (i + 1..v.len()));
    peak - l.max(r)
}

/// The frequency of the vertex of the parabola through the dB levels of the column and its
/// two neighbours. A tone between two bins spreads into both; on a log (dB) scale the
/// window's main lobe is close to a parabola, so the vertex lands nearer the tone than the
/// bin centre does (within a fraction of a bin instead of up to half). Without both
/// neighbours the bin centre is all there is.
fn interpolated_freq(freqs: &[f64], v: &[f64], i: usize) -> f64 {
    let f = freqs[i];
    let (Some(&a), Some(&c)) = (i.checked_sub(1).and_then(|j| v.get(j)), v.get(i + 1)) else {
        return f;
    };
    let b = v[i];
    let den = a - 2.0 * b + c;
    if !(a.is_finite() && c.is_finite()) || den >= 0.0 {
        return f;
    }
    let p = (0.5 * (a - c) / den).clamp(-0.5, 0.5);
    if p >= 0.0 {
        f + p * (freqs[i + 1] - f)
    } else {
        f + p * (f - freqs[i - 1])
    }
}

/// How closely the gaps between dips must agree to be read as a comb, as a fraction of
/// their mean. A delayed copy summed with the direct sound cancels wherever the delay is an
/// odd number of half periods: notches exactly every `1/τ` Hz. Dip frequencies come within
/// a percent or so of the notch on a 1/48-octave grid, so a real comb agrees far better
/// than 10 %.
pub const COMB_TOLERANCE: f64 = 0.1;

/// How many neighbouring dips in a row must be evenly spaced to be read as a comb. Two
/// gaps agreeing within 10 % happens by chance among unrelated dips often enough; three in
/// a row seldom does.
pub const COMB_MIN_DIPS: usize = 4;

/// The spacing of the longest run of at least [`COMB_MIN_DIPS`] neighbouring `dips` (all
/// the dips found, not only the marked ones: the deepest few of a comb are seldom
/// neighbours) evenly spaced on a linear frequency axis, as a comb's notches are; `None`
/// when there is no such run — an uneven set says nothing about one delay. Of two runs as
/// long, the lower: the comb is clearest where its notches are furthest apart in octaves.
pub fn comb_spacing(dips: &[Peak]) -> Option<f64> {
    let mut f: Vec<f64> = dips.iter().map(|p| p.freq_hz).collect();
    f.sort_by(f64::total_cmp);
    let mut best: Option<(usize, f64)> = None;
    for a in 0..f.len() {
        for b in a + COMB_MIN_DIPS - 1..f.len() {
            let run = &f[a..=b];
            let mean = (run[run.len() - 1] - run[0]) / (run.len() - 1) as f64;
            let even = mean > 0.0
                && run
                    .windows(2)
                    .all(|w| (w[1] - w[0] - mean).abs() <= COMB_TOLERANCE * mean);
            if even && best.is_none_or(|(n, _)| run.len() > n) {
                best = Some((run.len(), mean));
            }
        }
    }
    best.map(|(_, spacing)| spacing)
}

/// The marked peaks of one curve, kept by the UI between frames like a peak hold: display
/// state, it only remembers what was marked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeakTracker {
    dir: Extreme,
    peaks: Vec<Peak>,
}

impl PeakTracker {
    /// A tracker of peaks ([`Extreme::Max`]) or dips ([`Extreme::Min`]).
    pub fn new(dir: Extreme) -> Self {
        Self {
            dir,
            peaks: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.peaks.clear();
    }

    /// The marks, in list order: the most outstanding first, two within [`ORDER_DB`] in
    /// the order they had.
    pub fn peaks(&self) -> &[Peak] {
        &self.peaks
    }

    /// Chooses the marks again from a frame's `found` extremes ([`find_extremes`], in the
    /// tracker's direction); choosing again from the same ones changes nothing. One within
    /// [`SPACING_OCT`] of a mark is that mark, moved: it gets [`HOLD_DB`] of head start and
    /// keeps its row unless it falls behind by more than [`ORDER_DB`].
    pub fn update(&mut self, found: &[Peak]) {
        let dir = self.dir;
        let was = |p: &Peak| self.peaks.iter().position(|q| near(q.freq_hz, p.freq_hz));
        let mut scored: Vec<(f64, Option<usize>, Peak)> = found
            .iter()
            .map(|p| {
                let w = was(p);
                (
                    dir.height(p.level) + if w.is_some() { HOLD_DB } else { 0.0 },
                    w,
                    *p,
                )
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(TRACKED);
        // Rows as they were, new marks after them loudest first; then a mark moves up past
        // one it beats by more than ORDER_DB. Each swap leaves one fewer such pair, so the
        // passes end.
        scored.sort_by(|a, b| {
            let ka = a.1.unwrap_or(usize::MAX);
            let kb = b.1.unwrap_or(usize::MAX);
            ka.cmp(&kb)
                .then(dir.height(b.2.level).total_cmp(&dir.height(a.2.level)))
        });
        let mut next: Vec<Peak> = scored.into_iter().map(|s| s.2).collect();
        loop {
            let mut swapped = false;
            for k in 1..next.len() {
                if dir.height(next[k].level) > dir.height(next[k - 1].level) + ORDER_DB {
                    next.swap(k, k - 1);
                    swapped = true;
                }
            }
            if !swapped {
                break;
            }
        }
        self.peaks = next;
    }
}

#[cfg(test)]
mod tests;
