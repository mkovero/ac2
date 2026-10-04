//! Sweep recorder: the reference and measurement inputs of an `ir.capture` run, from the
//! moment it is attached until the whole train of sweeps (and the silence after the last
//! one) is in. It only records; the analysis runs on its own thread once the recording is
//! handed to the control thread, so this job never holds up the fan-out.
//!
//! Audio lost while recording (a gap in the sample counter, an xrun or overflow flag) ends
//! the recording as a dropout: a sweep deconvolved across a gap would show a wrong IR, never
//! a missing one.

use std::sync::mpsc::Sender;

use ac2_proto::units::SweepId;

use super::{Analysis, Emitter, Flush, JobCmd, StampArgs};
use crate::control::ControlMsg;
use crate::fanout::Block;
use crate::sweep::Recording;

pub(crate) struct Recorder {
    id: SweepId,
    ref_idx: usize,
    mic_idx: usize,
    /// Samples to record.
    total: usize,
    /// Samples before the first sweep can arrive at the latest (output → input latency).
    lead: usize,
    /// One sweep with its silence, samples.
    period: usize,
    repeats: u8,
    reference: Vec<f32>,
    measurement: Vec<f32>,
    end: Option<u64>,
    reported: u8,
    done: bool,
    to: Sender<ControlMsg>,
}

impl Recorder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: SweepId,
        ref_idx: usize,
        mic_idx: usize,
        total: usize,
        lead: usize,
        period: usize,
        repeats: u8,
        to: Sender<ControlMsg>,
    ) -> Self {
        Self {
            id,
            ref_idx,
            mic_idx,
            total,
            lead,
            period: period.max(1),
            repeats,
            reference: Vec::with_capacity(total),
            measurement: Vec::with_capacity(total),
            end: None,
            reported: 0,
            done: false,
            to,
        }
    }

    fn finish(&mut self, result: Result<Recording, String>) {
        self.done = true;
        let _ = self.to.send(ControlMsg::SweepRecorded {
            id: self.id,
            result: Box::new(result),
        });
    }
}

impl Analysis for Recorder {
    fn push(&mut self, b: &Block) {
        if self.done {
            return;
        }
        if let Some(e) = self.end
            && (b.start_sample != e || b.flags.breaks_continuity())
        {
            self.finish(Err(format!(
                "audio was lost at sample {} while recording; play the sweep again",
                b.start_sample
            )));
            return;
        }
        self.end = Some(b.end_sample());
        let n = usize::from(b.channels).max(1);
        let want = self.total - self.reference.len();
        for frame in b.data.chunks(n).take(want) {
            self.reference.push(frame[self.ref_idx]);
            self.measurement.push(frame[self.mic_idx]);
        }
        let repeat = (self.reference.len().saturating_sub(self.lead) / self.period + 1)
            .min(usize::from(self.repeats));
        let repeat = u8::try_from(repeat).unwrap_or(self.repeats);
        if repeat > self.reported {
            self.reported = repeat;
            let _ = self.to.send(ControlMsg::SweepProgress {
                id: self.id,
                repeat,
            });
        }
        if self.reference.len() >= self.total {
            let r = Recording {
                reference: std::mem::take(&mut self.reference),
                measurement: std::mem::take(&mut self.measurement),
            };
            self.finish(Ok(r));
        }
    }

    fn command(&mut self, _c: JobCmd) {}

    fn emit(&mut self, _e: &Emitter) -> Flush {
        Flush::Done
    }

    fn capture(&mut self) -> Option<(StampArgs, ac2_proto::frame::FrameData)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_audio::BlockFlags;

    fn block(start: u64, frames: u32, flags: BlockFlags) -> Block {
        let data: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let n = (start + u64::from(i)) as f32;
                [n, -n, 0.5]
            })
            .collect();
        Block::new(start, frames, 3, flags, 0, data)
    }

    #[test]
    fn records_both_inputs_reports_progress_and_stops_at_the_length() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut r = Recorder::new(SweepId(1), 1, 0, 1000, 100, 400, 2, tx);
        let mut s = 0;
        while s < 1500 {
            r.push(&block(s, 128, BlockFlags::NONE));
            s += 128;
        }
        let msgs: Vec<ControlMsg> = rx.try_iter().collect();
        let repeats: Vec<u8> = msgs
            .iter()
            .filter_map(|m| match m {
                ControlMsg::SweepProgress { repeat, .. } => Some(*repeat),
                _ => None,
            })
            .collect();
        assert_eq!(repeats, [1, 2]);
        let rec = msgs
            .into_iter()
            .find_map(|m| match m {
                ControlMsg::SweepRecorded { result, .. } => Some(*result),
                _ => None,
            })
            .expect("recorded")
            .expect("ok");
        assert_eq!(rec.reference.len(), 1000);
        assert_eq!(rec.reference[999], -999.0);
        assert_eq!(rec.measurement[999], 999.0);
    }

    #[test]
    fn a_gap_ends_the_recording_as_a_dropout() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut r = Recorder::new(SweepId(1), 0, 1, 1000, 100, 400, 1, tx);
        r.push(&block(0, 128, BlockFlags::NONE));
        r.push(&block(256, 128, BlockFlags::NONE));
        let failed = rx
            .try_iter()
            .any(|m| matches!(m, ControlMsg::SweepRecorded { result, .. } if result.is_err()));
        assert!(failed);
    }
}
