//! An open session whose audio stopped, in words: the AUDIO STOPPED banner, the app's top
//! bar and the `ac2 status` line all say the same thing (`docs/design/audio-recovery.md`).

use ac2_proto::model::{AudioStopped, Recovery, StopCause};
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
}
