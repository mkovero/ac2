//! BS.1770 / R128 loudness of the programme sources (docs/design/programme-meter.md §6.1),
//! beside the programme meters on the thread that holds the blocks. The set of sources
//! follows the declared kinds and pairs; a source keeps its state while it stays in the
//! set, so editing one pair does not restart another's integrated loudness.

use ac2_core::loudness::{LoudnessMeter, supports_rate};
use ac2_core::ppm::dbtp;
use ac2_proto::cal;
use ac2_proto::frame::{LoudnessSource, SourceLoudness};
use ac2_proto::model::{InputSetup, Pair};
use ac2_proto::units::{Db, Dbtp, Lufs, Seconds, WallNs};

struct Source {
    source: LoudnessSource,
    /// Metered positions of the source's channels; `n` of them are used.
    at: [usize; 2],
    n: usize,
    meter: LoudnessMeter,
    /// Highest linear true peak of the source's channels since the reset.
    max_true_peak: f32,
    since: u64,
}

/// Every loudness source of one stream.
pub(super) struct Loudness {
    sample_rate: f64,
    sources: Vec<Source>,
}

impl Loudness {
    pub(super) fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            sources: Vec::new(),
        }
    }

    /// Makes the sources those of `inputs` and `pairs` over the metered `channels`: a source
    /// still in the set keeps its meter, a new one starts afresh at `now` with its sub-block
    /// grid at the next block metered. No sources at a rate BS.1770 is not defined for here.
    pub(super) fn declare(
        &mut self,
        inputs: &[InputSetup],
        pairs: &[Pair],
        channels: &[u16],
        now: u64,
    ) {
        let wanted = if supports_rate(self.sample_rate) {
            cal::loudness_sources(|ch| cal::input_kind(inputs, ch), pairs, channels)
        } else {
            Vec::new()
        };
        let mut old = std::mem::take(&mut self.sources);
        for source in wanted {
            if let Some(i) = old.iter().position(|s| s.source == source) {
                self.sources.push(old.swap_remove(i));
                continue;
            }
            let pos = |ch: u16| channels.iter().position(|&c| c == ch);
            let (at, n) = match source {
                LoudnessSource::Pair { left, right } => match (pos(left), pos(right)) {
                    (Some(l), Some(r)) => ([l, r], 2),
                    _ => continue,
                },
                LoudnessSource::Input { channel } => match pos(channel) {
                    Some(m) => ([m, m], 1),
                    None => continue,
                },
            };
            // The rate was checked above and a source has one or two channels, so the meter
            // always builds; a release build skips the source rather than stop the meters.
            let meter = LoudnessMeter::new(self.sample_rate, n);
            debug_assert!(meter.is_ok(), "{meter:?}");
            let Ok(meter) = meter else {
                continue;
            };
            self.sources.push(Source {
                source,
                at,
                n,
                meter,
                max_true_peak: 0.0,
                since: now,
            });
        }
    }

    /// Meters one block: `chans` holds each metered input's samples, `true_peak` each
    /// one's linear true peak over the block (the programme meters' detector).
    pub(super) fn push(&mut self, chans: &[Vec<f32>], true_peak: &[f32]) {
        for s in &mut self.sources {
            let block = [&chans[s.at[0]][..], &chans[s.at[1]][..]];
            s.meter.process(&block[..s.n]);
            for &m in &s.at[..s.n] {
                s.max_true_peak = s.max_true_peak.max(true_peak[m]);
            }
        }
    }

    /// The capture lost samples (or may have) before the next block: a window spanning the
    /// gap would mix two signals, so the windows restart after it.
    pub(super) fn discontinuity(&mut self) {
        for s in &mut self.sources {
            s.meter.discontinuity();
        }
    }

    /// Resets every source with an input in `channels` (empty: every source), as of `now`.
    /// Integrated, range, the maxima and the duration restart at the next sub-block
    /// boundary; the max true peak at the next block, as the meters' holds do.
    pub(super) fn reset(&mut self, channels: &[u16], now: u64) {
        for s in &mut self.sources {
            if channels.is_empty() || channels.iter().any(|&c| s.source.has(c)) {
                s.meter.reset();
                s.max_true_peak = 0.0;
                s.since = now;
            }
        }
    }

    pub(super) fn readings(&self) -> Vec<SourceLoudness> {
        self.sources
            .iter()
            .map(|s| {
                let r = s.meter.reading();
                SourceLoudness {
                    source: s.source,
                    momentary: Lufs(r.momentary),
                    short_term: Lufs(r.short_term),
                    integrated: r.integrated.map(Lufs),
                    range: r.range.map(Db),
                    max_momentary: Lufs(r.max_momentary),
                    max_short_term: Lufs(r.max_short_term),
                    max_true_peak: Dbtp(f64::from(dbtp(s.max_true_peak))),
                    duration: Seconds(r.duration_samples as f64 / self.sample_rate),
                    since: WallNs(s.since),
                }
            })
            .collect()
    }
}
