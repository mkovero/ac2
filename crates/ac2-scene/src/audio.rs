//! An open session whose audio stopped, in words: the AUDIO STOPPED banner, the app's top
//! bar and the `ac2 status` line all say the same thing (`docs/design/audio-recovery.md`).

use ac2_proto::model::{
    AudioStopped, ClockSource, DriverResetReason, OpenSession, Recovery, StatedLatency, StopCause,
};
use ac2_proto::units::WallNs;

use crate::format;
use crate::time::local_clock;

/// An attempt that has been opening this long is waiting on an audio host that does not
/// answer (a hung server), which the operator should hear about rather than see "reopening"
/// forever.
pub const SLOW_ATTEMPT_S: f64 = 5.0;

/// The texts of a stopped session's audio.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioStoppedText {
    /// Banner headline: `AUDIO STOPPED · device not delivering since 20:36`.
    pub banner: String,
    /// Banner detail: where reopening stands, with what the backend said.
    pub detail: String,
    /// Top bar, longest first: `audio stopped · reopening (attempt 3, next in 8 s)`, then
    /// `audio stopped`.
    pub bar: [String; 2],
    /// `ac2 status` line.
    pub status: String,
}

/// Seconds as the countdown shows them: whole seconds, rounded up, so `next in 0 s` never
/// shows while an attempt is still to come.
fn countdown(s: f64) -> String {
    format!("{} s", s.max(0.0).ceil() as u64)
}

/// The texts for `s`, with `now` on the daemon's clock and `offset_s` the local UTC offset
/// in force at a wall time.
pub fn audio_stopped_text(
    s: &AudioStopped,
    now: WallNs,
    offset_s: impl Fn(WallNs) -> i32,
) -> AudioStoppedText {
    let at = local_clock(s.since, now, offset_s);
    let what = match s.cause {
        StopCause::NotDelivering { .. } => format!("device not delivering since {at}"),
        StopCause::HostEnded => format!("audio host ended the stream at {at}"),
        StopCause::DeviceChanged => format!("device changed at {at}, did not reopen"),
        StopCause::DriverReset { reason } => format!(
            "{} at {at} ({})",
            reset_headline(reason),
            reset_reason_text(reason)
        ),
    };
    let secs = |t: WallNs| (t.0 as f64 - now.0 as f64) / 1e9;
    let (long, short) = match &s.recovery {
        Recovery::Opening { attempt, started } => {
            let waited = -secs(*started);
            if waited >= SLOW_ATTEMPT_S {
                (
                    format!(
                        "reopening: attempt {attempt} has had no answer from the audio host for {}",
                        format::age(waited)
                    ),
                    format!("reopening (attempt {attempt}, no answer)"),
                )
            } else {
                (
                    format!("reopening the session (attempt {attempt})"),
                    format!("reopening (attempt {attempt})"),
                )
            }
        }
        Recovery::Waiting {
            attempt,
            error,
            next_at,
        } => {
            let next = countdown(secs(*next_at));
            (
                format!("attempt {attempt} failed: {error} · next in {next}"),
                format!("reopening (attempt {attempt}, next in {next})"),
            )
        }
    };
    AudioStoppedText {
        banner: format!("AUDIO STOPPED · {what}"),
        detail: format!("{long} · measurements paused"),
        bar: [format!("audio stopped · {short}"), "audio stopped".into()],
        status: format!("audio        STOPPED: {what}; {long}"),
    }
}

/// How soon the texts of `s` read differently: the countdown and the slow-attempt note
/// tick by the second.
pub fn changes_in(s: &AudioStopped, now: WallNs) -> f64 {
    match &s.recovery {
        Recovery::Waiting { next_at, .. } => {
            let left = (next_at.0 as f64 - now.0 as f64) / 1e9;
            if left <= 0.0 {
                1.0
            } else {
                let frac = left - left.floor();
                if frac > 0.0 { frac } else { 1.0 }
            }
        }
        Recovery::Opening { .. } => 1.0,
    }
}

/// What a driver did that reset the stream, as every surface names it: `driver asked to
/// resync`.
pub fn reset_reason_text(r: DriverResetReason) -> &'static str {
    match r {
        DriverResetReason::ResetRequest => "driver asked to be reset",
        DriverResetReason::ResyncRequest => "driver asked to resync",
        DriverResetReason::BufferSizeChange => "driver changed its buffer size",
        DriverResetReason::RateChange => "driver signalled a sample-rate change",
        DriverResetReason::ClockLost => "driver reports no sample rate",
    }
}

/// The kind of event a reset reason is: the clock moved, the clock went away, or the
/// driver reset its settings.
fn reset_headline(r: DriverResetReason) -> &'static str {
    match r {
        DriverResetReason::ResetRequest | DriverResetReason::BufferSizeChange => {
            "audio driver reset"
        }
        DriverResetReason::ResyncRequest | DriverResetReason::RateChange => "audio clock changed",
        DriverResetReason::ClockLost => "audio clock lost",
    }
}

/// A driver reset in one sentence, for a toast and the session's note: `Audio clock
/// changed (driver asked to resync): output stopped`. `output_stopped`: the generator was
/// emitting, so its output was cut and stays stopped until started again.
pub fn driver_reset_line(r: DriverResetReason, output_stopped: bool) -> String {
    let h = reset_headline(r);
    let mut c = h.chars();
    let h: String = c
        .next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default();
    let line = format!("{h} ({})", reset_reason_text(r));
    if output_stopped {
        format!("{line}: output stopped")
    } else {
        line
    }
}

/// The driver reset the session reopened after, if any (see [`driver_reset_line`]).
pub fn driver_reset_text(o: &OpenSession) -> Option<String> {
    o.driver_reset
        .map(|d| driver_reset_line(d.reason, d.output_stopped))
}

/// A driver's latency statement: `stated latency: 288 frames in / 320 out (driver
/// buffering: 32 frames in / 64 out)`. Buffering a driver does not report says so rather
/// than reading as none.
pub fn stated_latency_text(l: &StatedLatency) -> String {
    let buffering = match l.driver_buffering {
        Some(b) => format!("{} frames in / {} out", b.input_frames, b.output_frames),
        None => "not reported".into(),
    };
    format!(
        "stated latency: {} frames in / {} out (driver buffering: {buffering})",
        l.input_frames, l.output_frames
    )
}

/// A sample rate as the session shows it: `48.0 kHz`, `44.1 kHz`.
pub fn rate_text(hz: u32) -> String {
    format!("{} kHz", format::fixed(f64::from(hz) / 1000.0, 1))
}

/// The running session's audio settings in words: the app's top bar, `ac2 session status`.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionLine {
    /// `Fireface USB · 44.1 kHz · 512 frames · driver changed from 48.0 kHz`.
    pub full: String,
    /// The same without the device: `44.1 kHz · 512 frames`.
    pub short: String,
    /// The driver changed the rate or buffer size under the session (its control panel),
    /// or reset or changed its clock: the operator should notice, since every measurement
    /// restarted and a generator that was playing stopped.
    pub changed: bool,
    /// The longer story: the change, the reset, the clock source and the driver's stated
    /// latency, when there is any.
    pub detail: Option<String>,
}

/// The driver's change of the session's settings, when it made one: `the driver changed
/// the rate to 44.1 kHz (was 48.0 kHz) and the buffer to 512 frames (was 256); measurements
/// restarted at the new settings`.
pub fn driver_change_text(o: &OpenSession) -> Option<String> {
    let d = o.driver_change?;
    let mut what = Vec::new();
    if d.from_rate_hz != o.sample_rate_hz {
        what.push(format!(
            "the rate to {} (was {})",
            rate_text(o.sample_rate_hz),
            rate_text(d.from_rate_hz)
        ));
    }
    if d.from_buffer_frames != o.buffer_frames {
        what.push(format!(
            "the buffer to {} frames (was {})",
            o.buffer_frames, d.from_buffer_frames
        ));
    }
    if what.is_empty() {
        return None;
    }
    Some(format!(
        "the driver changed {}; measurements restarted at the new settings",
        what.join(" and ")
    ))
}

/// A device's clock sources, the one it runs on marked: `Internal (in use) · Word Clock ·
/// ADAT`. `None` for a device that names none (every backend but ASIO).
pub fn clock_sources_text(sources: &[ClockSource]) -> Option<String> {
    (!sources.is_empty()).then(|| {
        sources
            .iter()
            .map(|c| {
                if c.current {
                    format!("{} (in use)", c.name)
                } else {
                    c.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    })
}

/// The line for open session `o`.
pub fn session_line(o: &OpenSession) -> SessionLine {
    let rate = rate_text(o.sample_rate_hz);
    let short = format!("{rate} · {} frames", o.buffer_frames);
    let change = driver_change_text(o);
    let mut full = format!("{} · {short}", o.input_device.0);
    if let Some(d) = o.driver_change
        && d.from_rate_hz != o.sample_rate_hz
    {
        full.push_str(&format!(
            " · driver changed from {}",
            rate_text(d.from_rate_hz)
        ));
    } else if change.is_some() {
        full.push_str(" · driver changed the buffer");
    } else if let Some(d) = o.driver_reset {
        full.push_str(&format!(" · {}", reset_headline(d.reason)));
    }
    let reset = driver_reset_text(o);
    let parts: Vec<String> = [
        change.clone(),
        reset.clone(),
        o.clock_source
            .as_ref()
            .map(|c| format!("clock source: {c}")),
        o.stated_latency.as_ref().map(stated_latency_text),
    ]
    .into_iter()
    .flatten()
    .collect();
    SessionLine {
        full,
        short,
        changed: change.is_some() || reset.is_some(),
        detail: (!parts.is_empty()).then(|| parts.join(" · ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-05 20:36:12 UTC.
    const SINCE: u64 = 1_791_232_572_000_000_000;

    fn at(s: f64) -> WallNs {
        WallNs(SINCE + (s * 1e9) as u64)
    }

    fn utc(_: WallNs) -> i32 {
        0
    }

    #[test]
    fn a_device_not_delivering_names_the_time_and_the_wait_with_the_backends_words() {
        let s = AudioStopped {
            since: at(0.0),
            cause: StopCause::NotDelivering { after_ms: 1000 },
            recovery: Recovery::Waiting {
                attempt: 3,
                error: "No JACK server: start JACK".into(),
                next_at: at(20.0),
            },
        };
        let t = audio_stopped_text(&s, at(12.4), utc);
        assert_eq!(
            t.banner,
            "AUDIO STOPPED · device not delivering since 20:36"
        );
        assert_eq!(
            t.detail,
            "attempt 3 failed: No JACK server: start JACK · next in 8 s · measurements paused"
        );
        assert_eq!(
            t.bar,
            [
                "audio stopped · reopening (attempt 3, next in 8 s)".to_owned(),
                "audio stopped".to_owned()
            ]
        );
        assert_eq!(
            t.status,
            "audio        STOPPED: device not delivering since 20:36; attempt 3 failed: \
             No JACK server: start JACK · next in 8 s"
        );
        assert!((changes_in(&s, at(12.4)) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn an_attempt_without_an_answer_says_so_after_a_while() {
        let s = |started: f64| AudioStopped {
            since: at(0.0),
            cause: StopCause::HostEnded,
            recovery: Recovery::Opening {
                attempt: 1,
                started: at(started),
            },
        };
        let quick = audio_stopped_text(&s(1.0), at(2.0), utc);
        assert_eq!(
            quick.banner,
            "AUDIO STOPPED · audio host ended the stream at 20:36"
        );
        assert_eq!(
            quick.detail,
            "reopening the session (attempt 1) · measurements paused"
        );
        let hung = audio_stopped_text(&s(1.0), at(13.0), utc);
        assert_eq!(
            hung.detail,
            "reopening: attempt 1 has had no answer from the audio host for 12 s · \
             measurements paused"
        );
        assert_eq!(
            hung.bar[0],
            "audio stopped · reopening (attempt 1, no answer)"
        );
        let changed = AudioStopped {
            cause: StopCause::DeviceChanged,
            ..s(0.0)
        };
        assert_eq!(
            audio_stopped_text(&changed, at(0.5), |_| 7200).banner,
            "AUDIO STOPPED · device changed at 22:36, did not reopen"
        );
    }

    fn open(rate: u32, frames: u32) -> OpenSession {
        use ac2_proto::model::{
            BackendKind, ClockRelation, DeviceId, DeviceSelector, SessionConfig,
        };
        OpenSession {
            config: SessionConfig {
                backend: Some(BackendKind::Asio),
                input_device: DeviceSelector::Default,
                output_device: DeviceSelector::Default,
                input_channels: vec![0, 1],
                output_channels: 2,
                sample_rate_hz: Some(rate),
                buffer_frames: Some(frames),
                loopback: None,
            },
            backend: BackendKind::Asio,
            input_device: DeviceId("Fireface USB".into()),
            output_device: DeviceId("Fireface USB".into()),
            sample_rate_hz: rate,
            buffer_frames: frames,
            clock: ClockRelation::SingleCallback,
            opened_at: WallNs(SINCE),
            replay: None,
            clock_source: None,
            driver_change: None,
            driver_reset: None,
            stated_latency: None,
        }
    }

    #[test]
    fn a_session_as_opened_names_its_device_rate_and_buffer() {
        let l = session_line(&open(48_000, 256));
        assert_eq!(l.full, "Fireface USB · 48.0 kHz · 256 frames");
        assert_eq!(l.short, "48.0 kHz · 256 frames");
        assert!(!l.changed);
        assert_eq!(l.detail, None);
    }

    #[test]
    fn a_rate_the_driver_changed_says_from_what_and_that_measurements_restarted() {
        let mut o = open(44_100, 512);
        o.clock_source = Some("Word Clock".into());
        o.driver_change = Some(ac2_proto::model::DriverChange {
            from_rate_hz: 48_000,
            from_buffer_frames: 256,
        });
        let l = session_line(&o);
        assert_eq!(
            l.full,
            "Fireface USB · 44.1 kHz · 512 frames · driver changed from 48.0 kHz"
        );
        assert!(l.changed);
        assert_eq!(
            l.detail.as_deref(),
            Some(
                "the driver changed the rate to 44.1 kHz (was 48.0 kHz) and the buffer to \
                 512 frames (was 256); measurements restarted at the new settings · clock \
                 source: Word Clock"
            )
        );
    }

    #[test]
    fn clock_sources_mark_the_one_in_use() {
        let k = |name: &str, current| ClockSource {
            name: name.into(),
            current,
        };
        assert_eq!(clock_sources_text(&[]), None);
        assert_eq!(
            clock_sources_text(&[k("Internal", false), k("Word Clock", true)]).as_deref(),
            Some("Internal · Word Clock (in use)")
        );
    }

    #[test]
    fn a_driver_reset_is_named_as_a_settings_change() {
        let s = AudioStopped {
            since: at(0.0),
            cause: StopCause::DriverReset {
                reason: DriverResetReason::ResetRequest,
            },
            recovery: Recovery::Opening {
                attempt: 1,
                started: at(0.0),
            },
        };
        let t = audio_stopped_text(&s, at(0.5), utc);
        assert_eq!(
            t.banner,
            "AUDIO STOPPED · audio driver reset at 20:36 (driver asked to be reset)"
        );
    }

    #[test]
    fn a_buffer_the_driver_changed_alone_says_so() {
        let mut o = open(48_000, 512);
        o.driver_change = Some(ac2_proto::model::DriverChange {
            from_rate_hz: 48_000,
            from_buffer_frames: 256,
        });
        let l = session_line(&o);
        assert_eq!(
            l.full,
            "Fireface USB · 48.0 kHz · 512 frames · driver changed the buffer"
        );
        assert_eq!(
            driver_change_text(&o).as_deref(),
            Some(
                "the driver changed the buffer to 512 frames (was 256); measurements \
                 restarted at the new settings"
            )
        );
    }

    #[test]
    fn each_driver_reset_is_named_in_plain_words_with_whether_output_stopped() {
        use ac2_proto::model::DriverReset;
        let line = |r, stopped| driver_reset_line(r, stopped);
        assert_eq!(
            line(DriverResetReason::ResyncRequest, true),
            "Audio clock changed (driver asked to resync): output stopped"
        );
        assert_eq!(
            line(DriverResetReason::RateChange, true),
            "Audio clock changed (driver signalled a sample-rate change): output stopped"
        );
        assert_eq!(
            line(DriverResetReason::ClockLost, true),
            "Audio clock lost (driver reports no sample rate): output stopped"
        );
        assert_eq!(
            line(DriverResetReason::ResetRequest, false),
            "Audio driver reset (driver asked to be reset)"
        );
        assert_eq!(
            line(DriverResetReason::BufferSizeChange, false),
            "Audio driver reset (driver changed its buffer size)"
        );

        // Reopened at the same settings: the session line still says it, in the warning
        // colour, until a client opens the session anew.
        let mut o = open(48_000, 256);
        o.driver_reset = Some(DriverReset {
            reason: DriverResetReason::ResyncRequest,
            output_stopped: true,
        });
        let l = session_line(&o);
        assert_eq!(
            l.full,
            "Fireface USB · 48.0 kHz · 256 frames · audio clock changed"
        );
        assert!(l.changed);
        assert_eq!(
            l.detail.as_deref(),
            Some("Audio clock changed (driver asked to resync): output stopped")
        );

        // While it reopens: the banner names it with the time.
        let s = AudioStopped {
            since: at(0.0),
            cause: StopCause::DriverReset {
                reason: DriverResetReason::ClockLost,
            },
            recovery: Recovery::Opening {
                attempt: 1,
                started: at(0.0),
            },
        };
        assert_eq!(
            audio_stopped_text(&s, at(0.5), utc).banner,
            "AUDIO STOPPED · audio clock lost at 20:36 (driver reports no sample rate)"
        );
    }

    #[test]
    fn stated_latency_names_driver_buffering_or_that_it_is_not_reported() {
        use ac2_proto::model::DriverBuffering;
        let mut l = StatedLatency {
            input_frames: 288,
            output_frames: 320,
            driver_buffering: None,
        };
        assert_eq!(
            stated_latency_text(&l),
            "stated latency: 288 frames in / 320 out (driver buffering: not reported)"
        );
        l.driver_buffering = Some(DriverBuffering {
            input_frames: 32,
            output_frames: 64,
        });
        assert_eq!(
            stated_latency_text(&l),
            "stated latency: 288 frames in / 320 out (driver buffering: 32 frames in / 64 out)"
        );
        let mut o = open(48_000, 256);
        o.stated_latency = Some(l);
        let line = session_line(&o);
        assert!(!line.changed);
        assert_eq!(
            line.detail.as_deref(),
            Some(
                "stated latency: 288 frames in / 320 out (driver buffering: 32 frames in / \
                 64 out)"
            )
        );
    }
}
