//! Raw capture recorder (`rec.start`, `docs/design/raw-capture.md`): writes the chosen inputs
//! of every captured block to a float WAV file, keeps the sidecar's discontinuity list and
//! configuration timeline, and finalises both when the recording ends.
//!
//! It is one more consumer of the capture fan-out, on its own thread: the audio callback
//! only fills the capture ring, and the disk is touched here. The fan-out queues at most
//! [`QUEUE_S`] of audio for it; if the disk stalls longer, the fan-out drops batches for
//! this consumer alone, the next block arrives with a jump in its sample index, and the
//! jump is written into the sidecar as a `recorder_behind` discontinuity. Nothing is ever
//! spliced silently: the file holds only captured frames, and every place where captured
//! frames do not follow each other is listed with its session sample and the samples lost.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use ac2_audio::BlockFlags;
use ac2_proto::model::{DiscontinuityCause, RecordingEnd};
use ac2_proto::units::{SampleIndex, WallNs};
use ac2_traces::raw::{
    self, Discontinuity, End, Mark, Sidecar, TimelineChange, TimelineEntry, WavWriter,
};

use super::{Analysis, Emitter, Flush, JobCmd, StampArgs};
use crate::control::ControlMsg;
use crate::fanout::Block;
use crate::util::wall_ns;

/// Audio the fan-out may queue for the recorder: enough to ride out a slow card's write
/// stalls, small enough to stay a few MB per channel.
pub(crate) const QUEUE_S: f64 = 10.0;

/// How often progress goes to the control thread, in seconds of audio.
const PROGRESS_S: u64 = 1;

/// What control sends the recorder besides audio.
pub(crate) enum RecordCmd {
    /// A configuration change committed at session sample `at_sample`.
    Note {
        at_sample: u64,
        wall_ns: u64,
        change: Box<TimelineChange>,
    },
    /// The reason the recording ends when the job is stopped next.
    End(RecordingEnd),
}

/// Where the recording stands; shared with control, which reads it for state and after the
/// job has stopped.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Progress {
    /// Session sample of the file's first frame, once a block has arrived.
    pub(crate) start_sample: Option<u64>,
    pub(crate) frames: u64,
    pub(crate) bytes: u64,
    pub(crate) discontinuities: u32,
    /// Set once finalised.
    pub(crate) end: Option<RecordingEnd>,
}

/// Shared [`Progress`].
pub(crate) type SharedProgress = Arc<Mutex<Progress>>;

pub(crate) struct Recorder {
    token: u64,
    dir: PathBuf,
    name: String,
    writer: Option<WavWriter>,
    sidecar: Sidecar,
    /// Block channel of each file channel.
    cols: Vec<usize>,
    /// Frames the duration bound allows, and the frames the size bound allows.
    max_frames: u64,
    max_bytes_frames: Option<u64>,
    buf: Vec<f32>,
    /// One past the last session sample written.
    expected: Option<u64>,
    reported: u64,
    end_reason: Option<RecordingEnd>,
    cmds: Receiver<RecordCmd>,
    progress: SharedProgress,
    to: Sender<ControlMsg>,
}

impl Recorder {
    /// A recorder writing `writer` (a fresh file) and the sidecar `sidecar` (its `start`
    /// is filled in from the first block) of recording `name` in `dir`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        token: u64,
        dir: PathBuf,
        name: String,
        writer: WavWriter,
        sidecar: Sidecar,
        cols: Vec<usize>,
        cmds: Receiver<RecordCmd>,
        progress: SharedProgress,
        to: Sender<ControlMsg>,
    ) -> Self {
        let rate = f64::from(sidecar.audio.sample_rate);
        let max_frames = (sidecar.limits.max_duration.0 * rate).floor().max(0.0) as u64;
        let per_frame = u64::from(writer.channels()) * 4;
        let max_bytes_frames = sidecar
            .limits
            .max_bytes
            .map(|b| b.saturating_sub(raw::wav::HEADER_BYTES) / per_frame.max(1));
        Self {
            token,
            dir,
            name,
            writer: Some(writer),
            sidecar,
            cols,
            max_frames,
            max_bytes_frames,
            buf: Vec::new(),
            expected: None,
            reported: 0,
            end_reason: None,
            cmds,
            progress,
            to,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn frames(&self) -> u64 {
        self.writer.as_ref().map_or(0, WavWriter::frames)
    }

    fn drain_cmds(&mut self) {
        loop {
            match self.cmds.try_recv() {
                Ok(RecordCmd::Note {
                    at_sample,
                    wall_ns,
                    change,
                }) => {
                    let start = self.sidecar.start.session_sample.0;
                    let frame = raw::frame_of(start, &self.sidecar.discontinuities, at_sample);
                    self.sidecar.timeline.push(TimelineEntry {
                        at_sample: SampleIndex(at_sample),
                        frame,
                        wall_ns: WallNs(wall_ns),
                        change: *change,
                    });
                }
                Ok(RecordCmd::End(r)) => self.end_reason = Some(r),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    fn publish(&mut self) {
        let frames = self.frames();
        let bytes = self.writer.as_ref().map_or(0, WavWriter::bytes);
        let n = u32::try_from(self.sidecar.discontinuities.len()).unwrap_or(u32::MAX);
        {
            let mut p = self.lock();
            p.frames = frames;
            p.bytes = bytes;
            p.discontinuities = n;
        }
        let _ = self
            .to
            .send(ControlMsg::RecordingProgress { token: self.token });
    }

    /// Finishes the audio file and writes the final sidecar; idempotent.
    fn finish(&mut self, reason: RecordingEnd) {
        let Some(w) = self.writer.take() else {
            return;
        };
        self.drain_cmds();
        let frames = w.frames();
        let bytes = w.bytes();
        let reason = match w.finish() {
            Ok(_) => reason,
            Err(e) => match reason {
                RecordingEnd::WriteFailed { .. } => reason,
                _ => RecordingEnd::WriteFailed { msg: e.to_string() },
            },
        };
        if self.expected.is_none() {
            // Nothing arrived: the file starts where the session stood.
            self.sidecar.start = Mark::new(self.sidecar.start.session_sample.0, wall_ns());
        }
        let end_sample = raw::session_sample_of(&self.sidecar, frames);
        self.sidecar.end = Some(End {
            at: Mark::new(end_sample, wall_ns()),
            frames,
            reason: reason.clone(),
        });
        if let Err(e) = raw::write_sidecar(&self.dir, &self.name, &self.sidecar) {
            tracing::error!("recording {}: sidecar not written: {e}", self.name);
        }
        tracing::info!(
            "recording {} ended ({reason:?}): {frames} frames, {} discontinuities",
            self.name,
            self.sidecar.discontinuities.len()
        );
        let n = u32::try_from(self.sidecar.discontinuities.len()).unwrap_or(u32::MAX);
        let mut p = self.lock();
        p.frames = frames;
        p.bytes = bytes;
        p.discontinuities = n;
        p.end = Some(reason);
    }

    /// Ends the recording on its own (a bound, a write failure) and tells control.
    fn end_now(&mut self, reason: RecordingEnd) {
        self.finish(reason);
        let _ = self
            .to
            .send(ControlMsg::RecordingEnded { token: self.token });
    }

    fn note_discontinuity(&mut self, b: &Block, expected: u64) {
        let gap = b.start_sample != expected;
        let mut causes = Vec::new();
        if b.flags.contains(BlockFlags::XRUN) {
            causes.push(DiscontinuityCause::Xrun);
        }
        if b.flags.contains(BlockFlags::OVERFLOW) {
            causes.push(DiscontinuityCause::Overflow);
        }
        if b.flags.contains(BlockFlags::CONFIG_CHANGE) {
            causes.push(DiscontinuityCause::ConfigChange);
        }
        if b.flags.contains(BlockFlags::DISCONTINUITY) {
            causes.push(DiscontinuityCause::Gap);
        } else if gap {
            // The device's own blocks follow each other; the fan-out skipped this
            // consumer's batches because it was behind.
            causes.push(DiscontinuityCause::RecorderBehind);
        }
        self.sidecar.discontinuities.push(Discontinuity {
            frame: self.frames(),
            session_sample: SampleIndex(b.start_sample),
            lost_frames: b.start_sample.saturating_sub(expected),
            estimated: b.flags.contains(BlockFlags::GAP_ESTIMATED),
            causes,
        });
    }
}

impl Analysis for Recorder {
    fn push(&mut self, b: &Block) {
        if self.writer.is_none() {
            return;
        }
        // An end reason from control takes effect when the job stops, after the blocks
        // queued before it: `rec.stop` keeps everything captured until it was asked.
        self.drain_cmds();
        match self.expected {
            None => {
                // Flags on the first block describe what happened before the recording.
                self.sidecar.start = Mark::new(b.start_sample, b.wall_ns);
                // Notes that arrived before the first block refer to samples before it.
                for t in &mut self.sidecar.timeline {
                    t.frame = 0;
                }
                if let Err(e) = raw::write_sidecar(&self.dir, &self.name, &self.sidecar) {
                    self.end_now(RecordingEnd::WriteFailed { msg: e.to_string() });
                    return;
                }
                self.lock().start_sample = Some(b.start_sample);
                let _ = self
                    .to
                    .send(ControlMsg::RecordingProgress { token: self.token });
            }
            Some(e) if b.start_sample != e || b.flags.breaks_continuity() => {
                self.note_discontinuity(b, e);
            }
            Some(_) => {}
        }
        self.expected = Some(b.end_sample());

        let written = self.frames();
        let (limit, reason) = match self.max_bytes_frames {
            Some(f) if f < self.max_frames => (f, RecordingEnd::SizeLimit),
            _ => (self.max_frames, RecordingEnd::DurationLimit),
        };
        let take = u64::from(b.frames).min(limit.saturating_sub(written)) as usize;
        let n = usize::from(b.channels).max(1);
        self.buf.clear();
        for frame in b.data.chunks_exact(n).take(take) {
            self.buf.extend(self.cols.iter().map(|&c| frame[c]));
        }
        if let Some(w) = self.writer.as_mut()
            && let Err(e) = w.write(&self.buf)
        {
            self.end_now(RecordingEnd::WriteFailed { msg: e.to_string() });
            return;
        }
        let frames = self.frames();
        if frames >= limit {
            self.end_now(reason);
            return;
        }
        let every = PROGRESS_S * u64::from(self.sidecar.audio.sample_rate);
        if frames >= self.reported + every {
            self.reported = frames;
            self.publish();
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

impl Drop for Recorder {
    fn drop(&mut self) {
        // Stopped by control (`rec.stop`, the session closing, the daemon shutting down):
        // control sent the reason first.
        self.drain_cmds();
        let r = self
            .end_reason
            .clone()
            .unwrap_or(RecordingEnd::DaemonShutdown);
        self.finish(r);
    }
}
