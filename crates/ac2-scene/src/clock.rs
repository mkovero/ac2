//! A rig's clocks as the Connection page and `ac2 status` say them
//! (`docs/design/multi-rig.md` §4): the capture clock against the clock it was measured
//! against, and the system clock as the OS states it. Nothing is inferred: the OS's bound
//! is said as its bound, never as a "PTP" or "NTP" label, and no figure is shown before a
//! minute of audio.

use ac2_proto::model::{ClockBasis, Clocks, DeviceClock, SampleClock, SystemClock};

use crate::format;

/// The capture clock: `+6 ppm against the system clock`, `measuring…`, `no audio open`.
///
/// A figure is said against the system clock only when it was measured against the
/// disciplined clock and the OS says that clock is synchronised; unsynced (or not stated),
/// the system clock follows nothing but the crystal, so the figure is against this
/// computer's own clock, as one measured against the crystal directly is.
pub fn sample_clock_text(c: Option<&DeviceClock>, system: &SystemClock) -> String {
    match c.map(|c| c.rate) {
        None => "no audio open".into(),
        Some(SampleClock::Measuring) => "measuring…".into(),
        Some(SampleClock::Measured { ppm, against, .. }) => {
            let basis = match (against, system) {
                (ClockBasis::SystemClock, SystemClock::Synced { .. }) => "the system clock",
                _ => "this computer's own clock",
            };
            format!("{} ppm against {basis}", format::signed(ppm, 0))
        }
        Some(SampleClock::Synthetic) => "none (a replayed recording)".into(),
    }
}

/// The system clock, after the words *system clock*: `synced, OS bound ±3.0 ms`.
pub fn system_clock_text(s: &SystemClock) -> String {
    match s {
        SystemClock::Synced { maxerror_us } => format!("synced, OS bound {}", bound(*maxerror_us)),
        SystemClock::Unsynced { maxerror_us } => {
            format!("unsynced, OS bound {}", bound(*maxerror_us))
        }
        SystemClock::NotStated => "not stated by the OS".into(),
    }
}

/// The Connection page's line:
/// `clock: +6 ppm against the system clock · system clock synced, OS bound ±3.0 ms`.
pub fn clock_line(c: &Clocks) -> String {
    format!(
        "clock: {} · system clock {}",
        sample_clock_text(c.capture.as_ref(), &c.system),
        system_clock_text(&c.system)
    )
}

/// The line with the rig named, for a page listing more than one rig.
pub fn rig_clock_line(rig: &str, c: &Clocks) -> String {
    format!("{rig} · {}", clock_line(c))
}

/// `ac2 status`'s two lines, under its 13-column labels.
pub fn status_lines(c: &Clocks) -> [String; 2] {
    let text = sample_clock_text(c.capture.as_ref(), &c.system);
    let sample = match c.capture.as_ref().map(|c| c.rate) {
        Some(SampleClock::Measured { .. }) => format!("{text} (last minute)"),
        _ => text,
    };
    [
        format!("sample clock {sample}"),
        format!("system clock {}", system_clock_text(&c.system)),
    ]
}

/// The OS's error bound, µs, as `±3.0 ms`: microseconds under 0.1 ms, seconds from 1 s.
fn bound(us: u32) -> String {
    match us {
        0..100 => format!("±{us} µs"),
        100..1_000_000 => format!("±{} ms", format::fixed(f64::from(us) / 1e3, 1)),
        _ => format!("±{} s", format::fixed(f64::from(us) / 1e6, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::DeviceId;
    use ac2_proto::units::Seconds;

    fn clocks(rate: Option<SampleClock>, system: SystemClock) -> Clocks {
        Clocks {
            system,
            capture: rate.map(|rate| DeviceClock {
                device: DeviceId("system".into()),
                rate,
            }),
            stamp_map: None,
        }
    }

    fn measured(ppm: f64) -> Option<SampleClock> {
        Some(SampleClock::Measured {
            ppm,
            span: Seconds(59.9),
            against: ClockBasis::SystemClock,
        })
    }

    fn on_crystal(ppm: f64) -> Option<SampleClock> {
        Some(SampleClock::Measured {
            ppm,
            span: Seconds(59.9),
            against: ClockBasis::HostCrystal,
        })
    }

    #[test]
    fn each_status_reads_as_stated() {
        let synced = SystemClock::Synced { maxerror_us: 3027 };
        assert_eq!(
            clock_line(&clocks(measured(6.3), synced)),
            "clock: +6 ppm against the system clock · system clock synced, OS bound ±3.0 ms"
        );
        assert_eq!(
            clock_line(&clocks(
                measured(-8.25),
                SystemClock::Synced { maxerror_us: 504 }
            )),
            "clock: −8 ppm against the system clock · system clock synced, OS bound ±0.5 ms"
        );
        assert_eq!(
            clock_line(&clocks(
                Some(SampleClock::Measuring),
                SystemClock::Unsynced {
                    maxerror_us: 16_000_000
                }
            )),
            "clock: measuring… · system clock unsynced, OS bound ±16.0 s"
        );
        assert_eq!(
            clock_line(&clocks(None, SystemClock::NotStated)),
            "clock: no audio open · system clock not stated by the OS"
        );
        assert_eq!(
            clock_line(&clocks(
                Some(SampleClock::Synthetic),
                SystemClock::Synced { maxerror_us: 40 }
            )),
            "clock: none (a replayed recording) · system clock synced, OS bound ±40 µs"
        );
        assert_eq!(
            rig_clock_line("pupu", &clocks(measured(0.2), synced)),
            "pupu · clock: 0 ppm against the system clock · system clock synced, OS bound ±3.0 ms"
        );
    }

    /// Measured against the crystal, or against a system clock the OS says follows
    /// nothing: never said as the system clock's.
    #[test]
    fn a_figure_off_a_synced_clock_is_against_the_computers_own() {
        assert_eq!(
            clock_line(&clocks(on_crystal(8.2), SystemClock::NotStated)),
            "clock: +8 ppm against this computer's own clock · system clock not stated by the OS"
        );
        assert_eq!(
            clock_line(&clocks(
                on_crystal(8.2),
                SystemClock::Synced { maxerror_us: 504 }
            )),
            "clock: +8 ppm against this computer's own clock · system clock synced, OS bound ±0.5 ms"
        );
        let unsynced = clocks(
            measured(5.6),
            SystemClock::Unsynced {
                maxerror_us: 16_000_000,
            },
        );
        assert_eq!(
            status_lines(&unsynced),
            [
                "sample clock +6 ppm against this computer's own clock (last minute)".to_string(),
                "system clock unsynced, OS bound ±16.0 s".to_string(),
            ]
        );
    }

    #[test]
    fn status_says_what_the_figure_is_against() {
        let c = clocks(measured(6.0), SystemClock::Synced { maxerror_us: 3027 });
        assert_eq!(
            status_lines(&c),
            [
                "sample clock +6 ppm against the system clock (last minute)".to_string(),
                "system clock synced, OS bound ±3.0 ms".to_string(),
            ]
        );
        let c = clocks(Some(SampleClock::Measuring), SystemClock::NotStated);
        assert_eq!(status_lines(&c)[0], "sample clock measuring…");
    }
}
