//! The recording indicator (`REC 1:23 · 23.0 MB`, then `recorded take 1 · 1:23 · 23.0 MB`)
//! and the replay label (`REPLAY take 1 · 0:12 / 1:23`). Shared by the app's top bar and
//! `ac2 rec status` / `ac2 session status`.
//!
//! Elapsed time is the audio in the file (frames ÷ rate), not wall time: a recording that
//! lost audio says so as dropouts rather than as a longer clock.

use ac2_proto::model::{OpenSession, RecordingEnd, RecordingRun, RecordingStatus};

/// How the indicator is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingTone {
    /// Writing: the record colour.
    Recording,
    /// Finished normally: dim.
    Quiet,
    /// Finished by a failure or an interruption, or audio was lost: warning colour.
    Warning,
}

/// The indicator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingLabel {
    /// Shown text.
    pub text: String,
    /// Colour.
    pub tone: RecordingTone,
    /// Longer text on hover.
    pub detail: String,
}

/// Longest failure message in the label; the whole one is in the detail.
pub const MAX_REASON_CHARS: usize = 48;

/// A recording clock: `0:07`, `12:34`, `1:02:03`.
pub fn clock(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return crate::format::NO_VALUE.to_string();
    }
    let s = seconds.floor() as u64;
    if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    }
}

/// A file size in decimal units: `116 B`, `940 kB`, `23.0 MB`, `4.29 GB`.
pub fn bytes(b: u64) -> String {
    let v = b as f64;
    if b < 1000 {
        format!("{b} B")
    } else if v < 1e6 {
        format!("{:.0} kB", v / 1e3)
    } else if v < 1e9 {
        format!("{:.1} MB", v / 1e6)
    } else {
        format!("{:.2} GB", v / 1e9)
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn dropouts(n: u32) -> String {
    match n {
        0 => String::new(),
        1 => " · 1 dropout".to_owned(),
        n => format!(" · {n} dropouts"),
    }
}

/// Seconds of audio in `r`'s file.
pub fn recorded_seconds(r: &RecordingRun) -> f64 {
    if r.sample_rate_hz == 0 {
        return 0.0;
    }
    r.frames as f64 / f64::from(r.sample_rate_hz)
}

/// The indicator for `r`; `input_name` names a device input (zero-based) the way the
/// session shows it.
pub fn recording_label(r: &RecordingRun, input_name: impl Fn(u16) -> String) -> RecordingLabel {
    let t = clock(recorded_seconds(r));
    let size = bytes(r.bytes);
    let inputs = r
        .inputs
        .iter()
        .map(|&i| input_name(i))
        .collect::<Vec<_>>()
        .join(", ");
    let lost = dropouts(r.discontinuities);
    let gaps = if r.discontinuities > 0 {
        " The audio is not continuous where the sidecar lists dropouts."
    } else {
        ""
    };
    match &r.status {
        RecordingStatus::Recording => {
            let bound = match r.max_bytes {
                Some(b) => format!(
                    "{} or {}",
                    crate::format::duration(r.max_duration.0),
                    bytes(b)
                ),
                None => crate::format::duration(r.max_duration.0),
            };
            RecordingLabel {
                text: format!("REC {t} · {size}{lost}"),
                tone: if r.discontinuities > 0 {
                    RecordingTone::Warning
                } else {
                    RecordingTone::Recording
                },
                detail: format!(
                    "Recording {inputs} to {}; it stops by itself after {bound}.{gaps}",
                    r.path
                ),
            }
        }
        RecordingStatus::Ended { reason } => {
            let saved = format!("recorded {} · {t} · {size}{lost}", r.name);
            let (text, tone, why) = match reason {
                RecordingEnd::Stopped => (saved, RecordingTone::Quiet, "stopped"),
                RecordingEnd::DurationLimit => (
                    format!("{saved} (time limit)"),
                    RecordingTone::Quiet,
                    "the time limit was reached",
                ),
                RecordingEnd::SizeLimit => (
                    format!("{saved} (size limit)"),
                    RecordingTone::Quiet,
                    "the size limit was reached",
                ),
                RecordingEnd::SessionClosed => (
                    format!("{saved} (session closed)"),
                    RecordingTone::Quiet,
                    "the audio session closed",
                ),
                RecordingEnd::SessionReopened => (
                    format!("{saved} (session reopened)"),
                    RecordingTone::Warning,
                    "the audio session reopened (device or configuration change)",
                ),
                RecordingEnd::AudioStopped => (
                    format!("{saved} (audio stopped)"),
                    RecordingTone::Warning,
                    "the audio stopped (the device stopped delivering or its host ended the stream)",
                ),
                RecordingEnd::DaemonShutdown => (
                    format!("{saved} (daemon stopped)"),
                    RecordingTone::Quiet,
                    "the daemon shut down",
                ),
                RecordingEnd::WriteFailed { msg } => (
                    format!("recording failed: {}", shorten(msg, MAX_REASON_CHARS)),
                    RecordingTone::Warning,
                    "writing failed",
                ),
                RecordingEnd::Interrupted => (
                    format!("recording interrupted: {} · {t}", r.name),
                    RecordingTone::Warning,
                    "the daemon stopped without finishing it; it was finished from what had \
                     reached the disk",
                ),
            };
            let msg = match reason {
                RecordingEnd::WriteFailed { msg } => format!(" ({msg})"),
                _ => String::new(),
            };
            RecordingLabel {
                text,
                tone: if r.discontinuities > 0 {
                    RecordingTone::Warning
                } else {
                    tone
                },
                detail: format!(
                    "{inputs}: {t} of audio in {}, ended because {why}{msg}.{gaps}",
                    r.path
                ),
            }
        }
    }
}

/// What the replay label says on hover.
pub const REPLAY_DETAIL: &str = "The session plays a recording, not a device: the \
                                 measurements analyse the recorded inputs, and nothing can \
                                 be played.";

/// `REPLAY take 1 · 0:12 / 1:23` for a replay session, `REPLAY take 1 · ended` once its
/// last frame went by, `REPLAY take 1 · 1:23` when the position is not known (`played`
/// `None`); `None` for a live session. `played` is the newest sample handed on (the
/// keepalive's).
pub fn replay_label(o: &OpenSession, played: Option<u64>) -> Option<String> {
    let r = o.replay.as_ref()?;
    let rate = f64::from(o.sample_rate_hz.max(1));
    let end = r.end_sample.0;
    let total = end as f64 / rate;
    Some(match played {
        None => format!("REPLAY {} · {}", r.name, clock(total)),
        Some(p) if p + 1 >= end => format!("REPLAY {} · ended", r.name),
        Some(p) => format!(
            "REPLAY {} · {} / {}",
            r.name,
            clock((p + 1) as f64 / rate),
            clock(total)
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{RecordingRun, ReplayInfo, ReplayPace};
    use ac2_proto::units::{ClientId, SampleIndex, Seconds, SessionEpoch, WallNs};

    fn run(status: RecordingStatus, frames: u64, gaps: u32) -> RecordingRun {
        RecordingRun {
            name: "take 1".into(),
            path: "/r/take 1.wav".into(),
            inputs: vec![0, 1],
            sample_rate_hz: 48_000,
            session_epoch: SessionEpoch(1),
            start_sample: SampleIndex(0),
            started_at: WallNs(0),
            started_by: ClientId("a".into()),
            frames,
            bytes: 116 + frames * 8,
            discontinuities: gaps,
            max_duration: Seconds(600.0),
            max_bytes: None,
            status,
        }
    }

    fn name(i: u16) -> String {
        ["Loop return", "Room mic"][usize::from(i)].to_owned()
    }

    #[test]
    fn clocks_and_sizes() {
        assert_eq!(clock(7.9), "0:07");
        assert_eq!(clock(754.0), "12:34");
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(bytes(116), "116 B");
        assert_eq!(bytes(940_000), "940 kB");
        assert_eq!(bytes(23_040_116), "23.0 MB");
        assert_eq!(bytes(4_294_967_296), "4.29 GB");
    }

    #[test]
    fn recording_shows_time_and_size_by_name() {
        let l = recording_label(&run(RecordingStatus::Recording, 48_000 * 83, 0), name);
        assert_eq!(l.text, "REC 1:23 · 31.9 MB");
        assert_eq!(l.tone, RecordingTone::Recording);
        assert!(l.detail.contains("Loop return, Room mic"), "{}", l.detail);
        assert!(l.detail.contains("10 min 00 s"), "{}", l.detail);
        let l = recording_label(&run(RecordingStatus::Recording, 48_000, 2), name);
        assert_eq!(l.text, "REC 0:01 · 384 kB · 2 dropouts");
        assert_eq!(l.tone, RecordingTone::Warning);
    }

    #[test]
    fn an_ended_recording_says_why() {
        let ended = |reason| RecordingStatus::Ended { reason };
        let l = recording_label(&run(ended(RecordingEnd::Stopped), 480_000, 0), name);
        assert_eq!(l.text, "recorded take 1 · 0:10 · 3.8 MB");
        assert_eq!(l.tone, RecordingTone::Quiet);
        let l = recording_label(&run(ended(RecordingEnd::DurationLimit), 480_000, 0), name);
        assert_eq!(l.text, "recorded take 1 · 0:10 · 3.8 MB (time limit)");
        let l = recording_label(
            &run(
                ended(RecordingEnd::WriteFailed {
                    msg: "No space left on device (os error 28)".into(),
                }),
                480_000,
                0,
            ),
            name,
        );
        assert_eq!(
            l.text,
            "recording failed: No space left on device (os error 28)"
        );
        assert_eq!(l.tone, RecordingTone::Warning);
        let l = recording_label(&run(ended(RecordingEnd::Interrupted), 96_000, 0), name);
        assert_eq!(l.text, "recording interrupted: take 1 · 0:02");
        assert_eq!(l.tone, RecordingTone::Warning);
    }

    #[test]
    fn replay_counts_to_the_end_of_the_file() {
        let mut o = ac2_proto::samples::state().session.open.expect("open");
        assert_eq!(replay_label(&o, Some(10)), None);
        o.replay = Some(ReplayInfo {
            name: "take 1".into(),
            path: "/r/take 1.wav".into(),
            frames: 48_000 * 83,
            end_sample: SampleIndex(48_000 * 83),
            pace: ReplayPace::Realtime,
            recorded_start_sample: SampleIndex(0),
            recorded_at: WallNs(0),
        });
        assert_eq!(
            replay_label(&o, None).as_deref(),
            Some("REPLAY take 1 · 1:23")
        );
        assert_eq!(
            replay_label(&o, Some(48_000 * 12)).as_deref(),
            Some("REPLAY take 1 · 0:12 / 1:23")
        );
        assert_eq!(
            replay_label(&o, Some(48_000 * 83)).as_deref(),
            Some("REPLAY take 1 · ended")
        );
    }
}
