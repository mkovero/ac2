//! Programme loudness against the refgen vectors (`tools/refgen/sets/loudness.py`): the
//! EBU Tech 3341 / 3342 cases synthesised from their text, each checked against the
//! published reading (acceptance) and the unbinned reference meter (parity), the
//! K-weighting at every rate, the histogram bound, and the meter's reset and discontinuity
//! rules.

mod ebu3341;
mod ebu3342;
mod histogram;
mod kweighting;
mod meter;
mod tones;
mod true_peak;

use ac2_core::loudness::{LoudnessMeter, LoudnessReading};
use ac2_testkit::GoldenSet;
use serde_json::Value;

pub const RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];

/// Block size the vectors are fed in: no divisor of any sub-block, so sub-blocks straddle
/// blocks.
pub const BLOCK: usize = 1000;

pub fn set(name: &str) -> GoldenSet {
    GoldenSet::load(name).expect("golden set")
}

/// `parameters` path `a/b/c` of a set.
pub fn param<'a>(g: &'a GoldenSet, path: &str) -> &'a Value {
    let mut parts = path.split('/');
    let first = parts.next().expect("path");
    let mut v = g
        .parameter(first)
        .unwrap_or_else(|| panic!("parameter {first}"));
    for p in parts {
        v = v.get(p).unwrap_or_else(|| panic!("parameter {path}"));
    }
    v
}

#[derive(Debug, Clone, Copy)]
enum Level {
    Silence,
    Constant(f64),
    Ramp(f64, f64),
}

/// A refgen programme (module doc of `loudness.py`), synthesised block by block.
#[derive(Debug, Clone)]
pub struct Synth {
    channels: usize,
    table: Vec<f64>,
    /// (samples, level) per segment.
    segments: Vec<(usize, Level)>,
    seg: usize,
    k: usize,
    n: usize,
    amp: f64,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

impl Synth {
    pub fn new(programme: &Value, fs: u32) -> Self {
        let freq = programme["freq_hz"].as_u64().expect("freq");
        let channels = programme["channels"].as_u64().expect("channels") as usize;
        let fsf = f64::from(fs);
        let period = u64::from(fs) / gcd(u64::from(fs), freq);
        let table = (0..period)
            .map(|m| (std::f64::consts::TAU * freq as f64 * m as f64 / fsf).sin())
            .collect();
        let segments = programme["segments"]
            .as_array()
            .expect("segments")
            .iter()
            .map(|s| {
                let secs = s[0].as_f64().expect("seconds");
                let len = (secs * fsf).round() as usize;
                let level = match (s[1].as_f64(), s[2].as_f64()) {
                    (Some(a), Some(b)) if a == b => Level::Constant(a),
                    (Some(a), Some(b)) => Level::Ramp(a, b),
                    _ => Level::Silence,
                };
                (len, level)
            })
            .collect();
        let mut s = Self {
            channels,
            table,
            segments,
            seg: 0,
            k: 0,
            n: 0,
            amp: 0.0,
        };
        s.enter_segment();
        s
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Sample count of each segment.
    pub fn segment_lengths(&self) -> Vec<usize> {
        self.segments.iter().map(|s| s.0).collect()
    }

    fn enter_segment(&mut self) {
        while self.seg < self.segments.len() && self.segments[self.seg].0 == 0 {
            self.seg += 1;
        }
        self.k = 0;
        if let Some(&(_, Level::Constant(a))) = self.segments.get(self.seg) {
            self.amp = 10f64.powf(a / 20.0);
        }
    }

    /// Fills `out` with the next samples; returns how many (fewer at the end).
    pub fn fill(&mut self, out: &mut [f32]) -> usize {
        let mut i = 0;
        while i < out.len() && self.seg < self.segments.len() {
            let (len, level) = self.segments[self.seg];
            let amp = match level {
                Level::Silence => 0.0,
                Level::Constant(_) => self.amp,
                Level::Ramp(a, b) => 10f64.powf((a + (b - a) * self.k as f64 / len as f64) / 20.0),
            };
            out[i] = (amp * self.table[self.n % self.table.len()]) as f32;
            i += 1;
            self.n += 1;
            self.k += 1;
            if self.k == len {
                self.seg += 1;
                self.enter_segment();
            }
        }
        i
    }
}

/// Feeds `n` samples of `synth` (or all that remain) in blocks of at most `block`.
pub fn feed(m: &mut LoudnessMeter, synth: &mut Synth, n: usize, block: usize) -> usize {
    let mut buf = vec![0.0f32; block];
    let mut done = 0;
    while done < n {
        let want = block.min(n - done);
        let got = synth.fill(&mut buf[..want]);
        if got == 0 {
            break;
        }
        let x = &buf[..got];
        if m.channels() == 2 {
            m.process(&[x, x]);
        } else {
            m.process(&[x]);
        }
        done += got;
    }
    done
}

pub fn meter(fs: u32, channels: usize) -> LoudnessMeter {
    LoudnessMeter::new(f64::from(fs), channels).expect("supported rate")
}

/// A fresh meter fed the whole programme.
pub fn run(programme: &Value, fs: u32) -> LoudnessReading {
    let mut s = Synth::new(programme, fs);
    let mut m = meter(fs, s.channels());
    feed(&mut m, &mut s, usize::MAX, BLOCK);
    m.reading()
}

#[track_caller]
pub fn assert_near(what: &str, got: f64, want: f64, tol: f64) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: {got:.6} vs {want:.6} (tolerance {tol})"
    );
}
