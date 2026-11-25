//! `rec.*` and `stream.replay`: raw capture files (`docs/design/raw-capture.md`).
//!
//! A recording is a [`Recorder`] job on the capture fan-out; the control thread starts it,
//! forwards every committed change of the measurements, generator, inputs and calibrations
//! into its timeline, mirrors its progress as the `recording` entity, and ends it on
//! `rec.stop`, on a stream close or reopen, and at shutdown — always finalising the file
//! and saying why in the sidecar. A daemon that dies while recording leaves a sidecar
//! without an end; the next start finishes it as `interrupted` ([`raw::recover`]).
//!
//! A replay opens a stream on a [`ReplayBackend`] built from a recording: the recorded
//! inputs under their device numbers, on the recorded device's id (so its calibrations
//! apply), no outputs. Measurements carry over like on any reopen.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use ac2_audio::{
    BlockFlags, FrameSource, OpenSource, ReplayBackend, ReplayConfig, ReplayMark, ReplaySpeed,
};
use ac2_proto::event::Change;
use ac2_proto::model::{
    CalEntry, DeviceSelector, DiscontinuityCause, MeasKind, RecordRequest, RecordingEnd,
    RecordingRef, RecordingRun, RecordingStatus, RemoteGaps, ReplayInfo, ReplayPace, StreamConfig,
};
use ac2_proto::units::{SampleIndex, Seconds, StreamEpoch, WallNs};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};
use ac2_traces::raw::{
    self, AudioFile, ChannelRole, Initial, Limits, Mark, RawError, RecordedChannel, RecordedDevice,
    Sidecar, Software, TimelineChange, WavReader, WavWriter,
};

use super::Control;
use crate::jobs::record::{Progress, QUEUE_S, RecordCmd, Recorder, SharedProgress};
use crate::jobs::{self, JobHandle, block_index};
use crate::remote::replay::ReplayMarks;
use crate::route::Peer;
use crate::stream::Runtime;
use crate::util::{internal, invalid, missing, perr, refused, wall_ns};

/// How long an unfinished recording's audio file must have been left alone before it is
/// taken for one whose daemon died: a live recorder flushes its buffer every few seconds
/// at the lowest rates.
const RECOVER_QUIET: std::time::Duration = std::time::Duration::from_secs(30);

/// The recording in progress.
pub(super) struct ActiveRecording {
    token: u64,
    job: JobHandle,
    cmds: std::sync::mpsc::Sender<RecordCmd>,
    progress: SharedProgress,
    run: RecordingRun,
}

fn raw_err(e: RawError) -> ProtoError {
    let code = match &e {
        RawError::NotFound(_) => ErrorCode::NotFound,
        RawError::BadName(_) | RawError::Sidecar { .. } | RawError::Audio { .. } => {
            ErrorCode::Invalid
        }
        RawError::Exists(_) => ErrorCode::Refused,
        RawError::Io { .. } => ErrorCode::Internal,
    };
    perr(code, e.to_string())
}

/// `rec-YYYY-MM-DDThh-mm-ss` of Unix ns, a valid recording name.
fn default_name(ns: u64) -> String {
    let iso = ac2_proto::spl_log::utc_iso(ns);
    format!("rec-{}", iso.get(..19).unwrap_or(&iso).replace(':', "-"))
}

/// What a recorded input was used for, from the measurements and the stream.
fn roles(c: &Control, rt: &Runtime, input: u16) -> Vec<ChannelRole> {
    let mut out = Vec::new();
    if rt.open.config.loopback.is_some_and(|l| l.input == input) {
        out.push(ChannelRole::Loopback);
    }
    for m in &c.store.state().measurements {
        let name = m.config.name.clone();
        match &m.config.kind {
            MeasKind::Transfer { config } => {
                if config.reference_input == input {
                    out.push(ChannelRole::Reference {
                        measurement: name.clone(),
                    });
                }
                if config.measurement_input == input {
                    out.push(ChannelRole::Measured { measurement: name });
                }
            }
            MeasKind::Spectrum { config } if config.input == input => {
                out.push(ChannelRole::Analysed { measurement: name });
            }
            MeasKind::Rta { config } if config.input == input => {
                out.push(ChannelRole::Analysed { measurement: name });
            }
            MeasKind::Spl { config } if config.input == input => {
                out.push(ChannelRole::Analysed { measurement: name });
            }
            _ => {}
        }
    }
    out
}

/// Block flags a recorded discontinuity replays with.
fn replay_flags(causes: &[DiscontinuityCause]) -> BlockFlags {
    causes.iter().fold(BlockFlags::NONE, |f, c| {
        f | match c {
            DiscontinuityCause::Xrun => BlockFlags::XRUN,
            DiscontinuityCause::Overflow => BlockFlags::OVERFLOW,
            DiscontinuityCause::Gap
            | DiscontinuityCause::ConfigChange
            | DiscontinuityCause::RecorderBehind => BlockFlags::DISCONTINUITY,
        }
    })
}

struct WavSource(WavReader);

impl FrameSource for WavSource {
    fn read(&mut self, out: &mut [f32]) -> std::io::Result<usize> {
        self.0.read(out)
    }
}

impl Control {
    fn lock_progress(p: &SharedProgress) -> Progress {
        p.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(super) fn rec_start(
        &mut self,
        client: &Peer,
        req: RecordRequest,
    ) -> Result<ReplyBody, ProtoError> {
        if self.recording.is_some() {
            return Err(refused("a recording is running: stop it first (rec.stop)"));
        }
        let Some(rt) = self.stream.as_ref() else {
            return Err(refused("no open stream to record"));
        };
        if req.inputs.is_empty() {
            return Err(invalid("no inputs to record"));
        }
        // With a remote input the recorder reads the joined feed, whose blocks carry the
        // remote columns after the local ones, and their marks.
        let joined = self.job_map(rt, &req.inputs, false)?;
        let map = joined.as_deref().unwrap_or(&rt.input_map);
        let mut cols = Vec::with_capacity(req.inputs.len());
        for (i, &input) in req.inputs.iter().enumerate() {
            if req.inputs[..i].contains(&input) {
                return Err(invalid(format!("input {} listed twice", input + 1)));
            }
            cols.push(block_index(map, input).ok_or_else(|| {
                invalid(format!("input {} is not captured by the stream", input + 1))
            })?);
        }
        let d = req.max_duration.0;
        if !(d.is_finite() && d > 0.0 && d <= RecordRequest::MAX_DURATION_S) {
            return Err(invalid(format!(
                "the duration bound must be above 0 and at most {} h",
                RecordRequest::MAX_DURATION_S / 3600.0
            )));
        }
        let channels = u16::try_from(req.inputs.len()).map_err(|_| invalid("too many inputs"))?;
        if let Some(b) = req.max_bytes
            && b < raw::wav::file_bytes(channels, u64::from(rt.sample_rate))
        {
            return Err(invalid(
                "the size bound must allow at least one second of audio",
            ));
        }
        let Some(dir) = self.s.recording_dir.clone() else {
            return Err(perr(
                ErrorCode::Unsupported,
                "this daemon has no recording directory",
            ));
        };
        std::fs::create_dir_all(&dir)
            .map_err(|e| internal(format!("cannot create {}: {e}", dir.display())))?;
        let now = wall_ns();
        let name = match &req.name {
            Some(n) => {
                raw::validate_name(n).map_err(raw_err)?;
                n.clone()
            }
            None => {
                let base = default_name(now);
                (1..)
                    .map(|i| {
                        if i == 1 {
                            base.clone()
                        } else {
                            format!("{base}-{i}")
                        }
                    })
                    .find(|n| {
                        !raw::audio_path(&dir, n).exists() && !raw::sidecar_path(&dir, n).exists()
                    })
                    .unwrap_or(base)
            }
        };
        if raw::sidecar_path(&dir, &name).exists() || raw::audio_path(&dir, &name).exists() {
            return Err(raw_err(RawError::Exists(name)));
        }
        let path = raw::audio_path(&dir, &name);
        let writer = WavWriter::create(&path, channels, rt.sample_rate)
            .map_err(|e| internal(format!("cannot create {}: {e}", path.display())))?;

        let names = self.input_channel_names(rt);
        let st = self.store.state();
        let latest = rt.fanout.latest.load(std::sync::atomic::Ordering::Acquire);
        let sidecar = Sidecar {
            format: raw::FORMAT.into(),
            version: raw::VERSION,
            software: Software {
                ac2: format!("ac2d {}", env!("CARGO_PKG_VERSION")),
                build: env!("AC2_BUILD_ID").into(),
                protocol: ac2_proto::PROTO_VERSION,
            },
            audio: AudioFile {
                file: format!("{name}{}", raw::AUDIO_SUFFIX),
                sample_rate_hz: rt.sample_rate,
                channels: req
                    .inputs
                    .iter()
                    .map(|&input| RecordedChannel {
                        input,
                        remote: self.remote_input(input).cloned(),
                        name: match self.remote_input(input) {
                            Some(r) => Some(format!("{} In {}", r.rig_name, r.channel + 1)),
                            None => names.get(usize::from(input)).cloned(),
                        },
                        mic: st
                            .inputs
                            .iter()
                            .find(|i| i.channel == input)
                            .and_then(|i| i.kind.mic().map(str::to_owned)),
                        roles: roles(self, rt, input),
                    })
                    .collect(),
            },
            device: RecordedDevice {
                backend: rt.open.backend,
                input_device: rt.open.input_device.clone(),
                output_device: rt.open.output_device.clone(),
                buffer_frames: rt.open.buffer_frames,
                clock: rt.open.clock,
                stream_epoch: rt.epoch,
                loopback: rt.open.config.loopback,
            },
            start: Mark::new(latest, now),
            end: None,
            limits: Limits {
                max_duration: req.max_duration,
                max_bytes: req.max_bytes,
            },
            started_by: client.id().clone(),
            initial: Initial {
                measurements: st.measurements.clone(),
                delays: st.delays.clone(),
                generator: st.generator.clone(),
                inputs: st.inputs.clone(),
                calibrations: st
                    .calibrations
                    .iter()
                    .filter(|c| {
                        c.key.device == rt.open.input_device && req.inputs.contains(&c.key.channel)
                    })
                    .cloned()
                    .collect::<Vec<CalEntry>>(),
            },
            timeline: Vec::new(),
            discontinuities: Vec::new(),
            remote_gaps: Vec::new(),
            remote_slips: Vec::new(),
        };
        if let Err(e) = raw::write_sidecar(&dir, &name, &sidecar) {
            drop(writer);
            let _ = std::fs::remove_file(&path);
            return Err(raw_err(e));
        }

        let token = self.ids.token();
        let (cmds, rx) = std::sync::mpsc::channel();
        let progress: SharedProgress = Arc::new(Mutex::new(Progress::default()));
        let recorder = Recorder::new(
            token,
            dir,
            name.clone(),
            writer,
            sidecar,
            cols,
            rx,
            Arc::clone(&progress),
            self.s.to_self.clone(),
        );
        let fid = self.ids.fanout();
        let (job, feed) = jobs::spawn(
            "ac2d-record".into(),
            self.job_env(rt),
            fid,
            Box::new(recorder),
        )
        .map_err(|e| internal(format!("cannot start the recorder: {e}")))?;
        feed.queue.limit.store(
            (QUEUE_S * f64::from(rt.sample_rate)) as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        if joined.is_some() {
            rt.fanout.attach_joined(fid, feed);
        } else {
            rt.fanout.attach(fid, feed);
        }
        let run = RecordingRun {
            name,
            path: path.to_string_lossy().into_owned(),
            inputs: req.inputs.clone(),
            sample_rate_hz: rt.sample_rate,
            stream_epoch: rt.epoch,
            start_sample: SampleIndex(latest),
            started_at: WallNs(now),
            started_by: client.id().clone(),
            frames: 0,
            bytes: raw::wav::HEADER_BYTES,
            discontinuities: 0,
            remote_gaps: RemoteGaps::default(),
            max_duration: req.max_duration,
            max_bytes: req.max_bytes,
            status: RecordingStatus::Recording,
        };
        tracing::info!("recording {} started: {}", run.name, run.path);
        self.recording = Some(ActiveRecording {
            token,
            job,
            cmds,
            progress,
            run: run.clone(),
        });
        self.commit(Change::Recording(run.clone()));
        Ok(ReplyBody::Recording(run))
    }

    pub(super) fn rec_stop(&mut self) -> Result<ReplyBody, ProtoError> {
        if self.recording.is_none() {
            return Err(refused("nothing is being recorded"));
        }
        self.end_recording(RecordingEnd::Stopped);
        self.store
            .state()
            .recording
            .clone()
            .map(ReplyBody::Recording)
            .ok_or_else(|| internal("the recording vanished"))
    }

    pub(super) fn rec_list(&self) -> Result<ReplyBody, ProtoError> {
        let Some(dir) = &self.s.recording_dir else {
            return Ok(ReplyBody::Recordings(Vec::new()));
        };
        let rows = raw::list(dir).map_err(raw_err)?;
        Ok(ReplyBody::Recordings(
            rows.iter().map(|(n, s)| raw::listing(dir, n, s)).collect(),
        ))
    }

    /// Ends the recording in progress, if any, for `reason`: the recorder finalises the
    /// file and sidecar before its thread is joined.
    pub(super) fn end_recording(&mut self, reason: RecordingEnd) {
        let Some(a) = self.recording.take() else {
            return;
        };
        let _ = a.cmds.send(RecordCmd::End(reason.clone()));
        if let Some(rt) = &self.stream {
            rt.fanout.detach(a.job.fanout_id);
        }
        drop(a.job);
        let p = Self::lock_progress(&a.progress);
        let mut run = a.run;
        run.frames = p.frames;
        run.bytes = p.bytes;
        run.discontinuities = p.discontinuities;
        run.remote_gaps = p.remote_gaps;
        if let Some(s) = p.start_sample {
            run.start_sample = SampleIndex(s);
        }
        run.status = RecordingStatus::Ended {
            reason: p.end.unwrap_or(reason),
        };
        self.commit(Change::Recording(run));
    }

    /// The recording under way, by its token: [`Self::warn_recording_ended`] tells whether a
    /// command ended it.
    pub(super) fn recording_token(&self) -> Option<u64> {
        self.recording.as_ref().map(|a| a.token)
    }

    /// Puts `recording_ended` on the reply of the command that ran since `before` was taken,
    /// when that command closed or reopened the stream under the recording: the operator
    /// who reopens a device or loads a session learns that the recording stopped with it,
    /// not later from the recording's state.
    pub(super) fn warn_recording_ended(&mut self, before: Option<u64>) {
        let Some(token) = before else {
            return;
        };
        if self.recording_token() == Some(token) {
            return;
        }
        if let Some(run) = &self.store.state().recording {
            self.reply_warnings
                .push(ac2_proto::Warning::RecordingEnded {
                    recording: RecordingRef::Name {
                        name: run.name.clone(),
                    },
                });
        }
    }

    /// The recorder reported progress.
    pub(super) fn recording_progress(&mut self, token: u64) {
        let Some(a) = self.recording.as_mut().filter(|a| a.token == token) else {
            return;
        };
        let p = Self::lock_progress(&a.progress);
        let mut run = a.run.clone();
        run.frames = p.frames;
        run.bytes = p.bytes.max(raw::wav::HEADER_BYTES);
        run.discontinuities = p.discontinuities;
        run.remote_gaps = p.remote_gaps;
        if let Some(s) = p.start_sample {
            run.start_sample = SampleIndex(s);
        }
        a.run = run.clone();
        self.commit(Change::Recording(run));
    }

    /// The recorder ended on its own (a bound, a write failure).
    pub(super) fn recording_ended(&mut self, token: u64) {
        if self.recording.as_ref().is_some_and(|a| a.token == token) {
            // The recorder has finalised; its own reason wins over this fallback.
            self.end_recording(RecordingEnd::Stopped);
        }
    }

    /// Hands a committed change to the recorder's timeline.
    pub(super) fn note_for_recording(&self, change: &Change) {
        let Some(a) = &self.recording else {
            return;
        };
        let note = match change {
            Change::Measurement(p) => TimelineChange::Measurement(Box::new(p.clone())),
            Change::Delay(p) => TimelineChange::Delay(p.clone()),
            Change::Generator(g) => TimelineChange::Generator(g.clone()),
            Change::Inputs(i) => TimelineChange::Inputs(i.inputs.clone()),
            Change::Calibration(c) => TimelineChange::Calibration(c.clone()),
            // Named one by one: a new kind of change has to say here whether a replay of
            // the recording needs it.
            Change::Stream(_)
            | Change::DelayFinding(_)
            | Change::Trace(_)
            | Change::LineCalibration(_)
            | Change::Mic(_)
            | Change::Outputs(_)
            | Change::SplLog(_)
            | Change::Timing(_)
            | Change::Clocks(_)
            | Change::Remote(_)
            | Change::Sweep(_)
            | Change::Dynamics(_)
            | Change::Autosave(_)
            | Change::Recording(_)
            | Change::Working(_)
            | Change::Rig(_)
            | Change::Plan(_)
            | Change::Walk(_)
            | Change::WalkPlan(_)
            | Change::Sim(_)
            | Change::SimMap(_)
            | Change::SimFit(_)
            | Change::Server(_) => return,
        };
        let at_sample = self.stream.as_ref().map_or(0, |r| {
            r.fanout.latest.load(std::sync::atomic::Ordering::Acquire)
        });
        let _ = a.cmds.send(RecordCmd::Note {
            at_sample,
            wall_ns: wall_ns(),
            change: Box::new(note),
        });
    }

    /// Finishes recordings a previous daemon left unfinished; mirrors the newest of them.
    pub(super) fn recover_recordings(&mut self) {
        let Some(dir) = self.s.recording_dir.clone() else {
            return;
        };
        super::file_ops::finish_deletes(&dir);
        let mut newest: Option<RecordingRun> = None;
        for (name, r) in raw::recover(&dir, wall_ns(), RECOVER_QUIET) {
            match r {
                Ok(frames) => {
                    tracing::warn!(
                        "recording {name} was never finished (the daemon stopped while it \
                         ran): finished with {frames} frames, marked interrupted"
                    );
                    if let Ok(s) = raw::read_sidecar(&dir, &name) {
                        let run = run_of(&dir, &name, &s);
                        if newest
                            .as_ref()
                            .is_none_or(|n| n.started_at.0 <= run.started_at.0)
                        {
                            newest = Some(run);
                        }
                    }
                }
                Err(e) => tracing::error!("recording {name} could not be finished: {e}"),
            }
        }
        if let Some(run) = newest {
            self.commit(Change::Recording(run));
        }
    }

    // -- replay ------------------------------------------------------------------------------

    /// Lets a replay stream play once its measurements are attached to the fan-out.
    pub(super) fn release_replay(&self) {
        let (Some(rt), Some(b)) = (&self.stream, &self.replay_backend) else {
            return;
        };
        if rt.open.replay.is_some() {
            let b = Arc::clone(b);
            rt.fanout.then(Box::new(move || b.release()));
        }
    }

    /// The directory and name a reference names. Paths are for clients on the local pair
    /// only.
    pub(super) fn recording_path(
        &self,
        client: &Peer,
        r: &RecordingRef,
    ) -> Result<(PathBuf, String), ProtoError> {
        match r {
            RecordingRef::Name { name } => {
                raw::validate_name(name).map_err(raw_err)?;
                let dir = self.s.recording_dir.clone().ok_or_else(|| {
                    missing("this daemon has no recording directory; name the file by its path")
                })?;
                Ok((dir, name.clone()))
            }
            RecordingRef::Path { path } => {
                if !client.is_local() {
                    return Err(refused(
                        "remote clients name recordings; paths are for local clients only",
                    ));
                }
                let p = PathBuf::from(path);
                if !p.is_absolute() {
                    return Err(invalid(
                        "a recording path must be absolute (the daemon's working directory is \
                         not the client's)",
                    ));
                }
                raw::split_path(&p).ok_or_else(|| {
                    invalid(format!(
                        "{path}: name the recording's {} or {} file",
                        raw::AUDIO_SUFFIX,
                        raw::SIDECAR_SUFFIX
                    ))
                })
            }
        }
    }

    pub(super) fn stream_replay(
        &mut self,
        client: &Peer,
        r: &RecordingRef,
        pace: ReplayPace,
    ) -> Result<ReplyBody, ProtoError> {
        let (dir, name) = self.recording_path(client, r)?;
        let s = raw::read_sidecar(&dir, &name).map_err(raw_err)?;
        let path = dir.join(&s.audio.file);
        if s.end.is_none() {
            return Err(refused(format!(
                "{name} is still being recorded; stop it first"
            )));
        }
        let reader = WavReader::open(&path).map_err(|err| {
            raw_err(RawError::Audio {
                path: path.clone(),
                err,
            })
        })?;
        let info = reader.info();
        drop(reader);
        if info.sample_rate != s.audio.sample_rate_hz
            || usize::from(info.channels) != s.audio.channels.len()
        {
            return Err(invalid(format!(
                "{}: {} channels at {} Hz, the sidecar says {} at {} Hz",
                path.display(),
                info.channels,
                info.sample_rate,
                s.audio.channels.len(),
                s.audio.sample_rate_hz
            )));
        }
        let inputs = replay_inputs(&s.audio.channels);
        let max = inputs.iter().max().copied().unwrap_or(0);
        let input_names = (0..=max)
            .map(|i| {
                inputs
                    .iter()
                    .position(|&c| c == i)
                    .and_then(|k| s.audio.channels[k].name.clone())
                    .unwrap_or_else(|| match inputs.contains(&i) {
                        true => format!("In {}", i + 1),
                        false => format!("In {} (not recorded)", i + 1),
                    })
            })
            .collect();
        let marks = replay_marks(&s);
        let open_path = path.clone();
        let open: OpenSource = Arc::new(move || {
            WavReader::open(&open_path)
                .map(|r| Box::new(WavSource(r)) as Box<dyn FrameSource>)
                .map_err(|e| std::io::Error::other(e.to_string()))
        });
        let block = if s.device.buffer_frames == 0 {
            256
        } else {
            s.device.buffer_frames
        };
        let device = s.device.input_device.0.clone();
        let backend = Arc::new(ReplayBackend::new(ReplayConfig {
            device_id: device.clone(),
            device_name: format!("recording {name}"),
            sample_rate: s.audio.sample_rate_hz,
            block_frames: block,
            inputs: inputs.clone(),
            input_names,
            frames: info.frames,
            marks: s
                .discontinuities
                .iter()
                .map(|d| ReplayMark {
                    frame: d.frame,
                    lost_frames: d.lost_frames,
                    flags: replay_flags(&d.causes),
                })
                .collect(),
            speed: match pace {
                ReplayPace::Realtime => ReplaySpeed::Realtime,
                ReplayPace::Fast => ReplaySpeed::Fast,
            },
            open,
        }));
        let config = StreamConfig {
            backend: Some(ac2_proto::model::BackendKind::Replay),
            input_device: DeviceSelector::Id {
                id: ac2_proto::model::DeviceId(device),
            },
            output_device: DeviceSelector::Default,
            input_channels: inputs,
            spare_inputs: 0,
            output_channels: 0,
            sample_rate_hz: Some(s.audio.sample_rate_hz),
            buffer_frames: Some(block),
            loopback: None,
            remote_inputs: Vec::new(),
            link_offset: None,
        };
        let replay = ReplayInfo {
            name: name.clone(),
            path: path.to_string_lossy().into_owned(),
            frames: info.frames,
            end_sample: SampleIndex(
                raw::stream_sample_of(&s, info.frames) - s.start.stream_sample.0,
            ),
            pace,
            recorded_start_sample: s.start.stream_sample,
            recorded_at: s.start.wall_ns,
        };
        self.replay_backend = Some(backend);
        self.replay_marks = marks;
        let r = self.open_stream(client, config, Some(replay));
        if r.is_err() {
            self.replay_backend = None;
            self.replay_marks = None;
        }
        tracing::info!("replaying {name} ({:?})", pace);
        r.map(ReplyBody::Stream)
    }
}

/// The device inputs a replay offers, in file channel order: a local input keeps its number;
/// a remote one (another rig's, joined live) becomes an input after the highest local one,
/// named as recorded.
fn replay_inputs(channels: &[raw::RecordedChannel]) -> Vec<u16> {
    let mut next = channels
        .iter()
        .filter(|c| c.remote.is_none() && ac2_proto::model::remote_slot(c.input).is_none())
        .map(|c| c.input + 1)
        .max()
        .unwrap_or(0);
    channels
        .iter()
        .map(|c| {
            if c.remote.is_none() && ac2_proto::model::remote_slot(c.input).is_none() {
                c.input
            } else {
                next += 1;
                next - 1
            }
        })
        .collect()
}

/// A recording's remote-channel marks in replay samples (the replay starts at sample 0); the
/// replay's block columns are the file's channels.
fn replay_marks(s: &Sidecar) -> Option<Arc<ReplayMarks>> {
    let at = |sample: SampleIndex| sample.0.saturating_sub(s.start.stream_sample.0);
    ReplayMarks::new(
        s.remote_gaps.iter().map(|g| {
            let from = at(g.stream_sample);
            (g.channel, from..from + g.frames, g.late)
        }),
        s.remote_slips
            .iter()
            .map(|l| (l.channel, at(l.stream_sample), l.samples)),
    )
    .map(Arc::new)
}

/// The `recording` entity of a finished recording read back from its sidecar.
fn run_of(dir: &std::path::Path, name: &str, s: &Sidecar) -> RecordingRun {
    let frames = s.end.as_ref().map_or(0, |e| e.frames);
    let channels = u16::try_from(s.audio.channels.len()).unwrap_or(u16::MAX);
    RecordingRun {
        name: name.to_owned(),
        path: dir.join(&s.audio.file).to_string_lossy().into_owned(),
        inputs: s.audio.channels.iter().map(|c| c.input).collect(),
        sample_rate_hz: s.audio.sample_rate_hz,
        stream_epoch: StreamEpoch(s.device.stream_epoch.0),
        start_sample: s.start.stream_sample,
        started_at: s.start.wall_ns,
        started_by: s.started_by.clone(),
        frames,
        bytes: raw::wav::file_bytes(channels, frames),
        discontinuities: u32::try_from(s.discontinuities.len()).unwrap_or(u32::MAX),
        remote_gaps: s.remote_gap_count(),
        max_duration: Seconds(s.limits.max_duration.0),
        max_bytes: s.limits.max_bytes,
        status: RecordingStatus::Ended {
            reason: s
                .end
                .as_ref()
                .map_or(RecordingEnd::Interrupted, |e| e.reason.clone()),
        },
    }
}
