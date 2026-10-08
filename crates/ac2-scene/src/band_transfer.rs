//! The band transfer step (`docs/design/band-leq.md`, *The transfer*): three spans of band
//! logs named while the meters run — the test signal at FOH, the same signal with the mic at
//! the place the limits are for (named by the operator, `receiving room` by default), the
//! place with the system silent — their averages, and what to do next. The app's step and
//! `ac2 spl bands log` say it the same way.

use ac2_proto::model::{BAND_NOMINAL_HZ, BandLogAverage};
use ac2_proto::units::WallNs;

use crate::band_leq::band_label;
use crate::format;
use crate::leq::clock;

/// One of the three spans a band transfer is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanRole {
    /// The test signal, the mic at FOH.
    Foh,
    /// The same test signal at the same level, the mic at the place.
    Place,
    /// The place with the system silent.
    Background,
}

impl SpanRole {
    /// In the order they are measured.
    pub const ALL: [SpanRole; 3] = [SpanRole::Foh, SpanRole::Place, SpanRole::Background];

    /// Position in [`Self::ALL`].
    pub fn index(self) -> usize {
        match self {
            SpanRole::Foh => 0,
            SpanRole::Place => 1,
            SpanRole::Background => 2,
        }
    }

    /// `FOH span`, `Flat 4 bedroom span` (the place's).
    pub fn title(self, place: &str) -> String {
        match self {
            SpanRole::Foh => "FOH span".into(),
            SpanRole::Place => format!("{} span", capitalized(place)),
            SpanRole::Background => "Background span".into(),
        }
    }

    /// What is measured over it.
    pub fn what(self, place: &str) -> String {
        match self {
            SpanRole::Foh => "test signal, mic at FOH".into(),
            SpanRole::Place => format!("same test signal and level, mic in {place}"),
            SpanRole::Background => format!("system silent, mic in {place}"),
        }
    }
}

/// `place` with its first letter upper case, to begin a title.
fn capitalized(place: &str) -> String {
    let mut c = place.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Where a span stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanState {
    Unmarked,
    /// Started at `from`, still running.
    Marking {
        from: WallNs,
    },
    Marked {
        from: WallNs,
        until: WallNs,
    },
}

/// Local time of day to the second: `21:03:12`.
pub fn time_of_day(at: WallNs, offset_s: i32) -> String {
    let local = (at.0 / 1_000_000_000) as i64 + i64::from(offset_s);
    let tod = local.rem_euclid(86_400);
    format!("{}:{:02}:{:02}", tod / 3600, (tod % 3600) / 60, tod % 60)
}

fn seconds_between(from: WallNs, until: WallNs) -> f64 {
    until.0.saturating_sub(from.0) as f64 / 1e9
}

/// The span's times: `not marked`, `marking since 21:03:12 · 0:35`, `21:03:12–21:05:12 ·
/// 2:00`. `now` is on the meter's clock.
pub fn span_time_text(s: SpanState, now: WallNs, offset_s: i32) -> String {
    match s {
        SpanState::Unmarked => "not marked".into(),
        SpanState::Marking { from } => format!(
            "marking since {} · {}",
            time_of_day(from, offset_s),
            clock(seconds_between(from, now))
        ),
        SpanState::Marked { from, until } => format!(
            "{}–{} · {}",
            time_of_day(from, offset_s),
            time_of_day(until, offset_s),
            clock(seconds_between(from, until))
        ),
    }
}

/// How much of `[from, until)` the log holds: `118 of 120 s logged`, with the time measured
/// when it falls short and the seconds without a calibration.
pub fn coverage_text(a: &BandLogAverage, from: WallNs, until: WallNs) -> String {
    // Seconds are logged by their start: a span of 2.4 s can hold the starts of 3.
    let span = seconds_between(from, until)
        .ceil()
        .max(f64::from(a.seconds));
    if a.seconds == 0 {
        return format!(
            "nothing logged in its {span:.0} s: is the meter running with its band meter on?"
        );
    }
    let mut t = format!("{} of {span:.0} s logged", a.seconds);
    if a.measured.0 < f64::from(a.seconds) - 0.5 {
        t.push_str(&format!(", {:.1} s measured", a.measured.0));
    }
    if a.uncalibrated > 0 {
        t.push_str(&format!(
            "; {} s not calibrated: no dB SPL average (calibrate, then mark it again)",
            a.uncalibrated
        ));
    }
    t
}

/// One band's average: `63 Hz 80.0 dB`; `63 Hz —` without energy.
pub fn average_text(nominal_hz: f64, level: Option<f64>) -> String {
    let hz = band_label(nominal_hz);
    match level {
        Some(l) => format!("{hz} Hz {} dB", format::level(l)),
        None => format!("{hz} Hz {}", format::NO_VALUE),
    }
}

/// The averages of the shown bands (indices, low to high) in a line, dB SPL; `None`
/// without a dB SPL average.
pub fn averages_text(a: &BandLogAverage, shown: &[usize]) -> Option<String> {
    let levels = a.levels?;
    let parts: Vec<String> = shown
        .iter()
        .filter(|&&b| b < levels.len())
        .map(|&b| average_text(BAND_NOMINAL_HZ[b], levels[b].map(|l| l.0)))
        .collect();
    Some(format!("{} SPL", parts.join(" · ")))
}

/// Where the step stands, for [`next_step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Unmarked,
    Marking,
    Marked,
}

impl From<SpanState> for Phase {
    fn from(s: SpanState) -> Self {
        match s {
            SpanState::Unmarked => Phase::Unmarked,
            SpanState::Marking { .. } => Phase::Marking,
            SpanState::Marked { .. } => Phase::Marked,
        }
    }
}

/// What to do next, from the spans in [`SpanRole::ALL`] order, whether a transfer was
/// stored by this step, the meter that takes it and the place's name.
pub fn next_step(spans: [Phase; 3], stored: bool, meter: &str, place: &str) -> String {
    if stored {
        return format!(
            "Band transfer stored in {meter}: the band limits of {place} now apply at FOH with \
             each band's attenuation. Esc goes back to the Leq settings."
        );
    }
    if let Some(i) = spans.iter().position(|p| *p == Phase::Marking) {
        return match SpanRole::ALL[i] {
            SpanRole::Foh => {
                "Marking the FOH span: keep the test signal steady. Space stops it.".into()
            }
            SpanRole::Place => format!(
                "Marking the {place} span: the same test-signal level, the mic in {place}. \
                 Space stops it."
            ),
            SpanRole::Background => format!(
                "Marking the background span: the system silent, the mic in {place}. Space \
                 stops it."
            ),
        };
    }
    match spans {
        [Phase::Unmarked, ..] => "Play a steady test signal (pink noise) through the PA with the \
             mic at FOH. Space starts the FOH span and Space again stops it; or 1–9 takes the \
             last 1–9 minutes."
            .into(),
        [_, Phase::Unmarked, _] => format!(
            "Move the mic to {place} and keep the same test-signal level. Space starts the \
             {place} span and Space again stops it."
        ),
        [_, _, Phase::Unmarked] => format!(
            "Silence the system, the mic still in {place}. Space starts the background span; \
             or Enter stores the band transfer without a background (every band unchecked)."
        ),
        _ => format!("Enter computes the band transfer and stores it in {meter}."),
    }
}

/// The step's keys, for its footer.
pub const KEYS: &str = "↑/↓ place, span · Space start / stop · 1–9 the last minutes · \
                        ←/→ meter · Delete clears · Enter stores the band transfer · Esc back";

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{BAND_COUNT, LF_BAND_COUNT};
    use ac2_proto::units::{DbSpl, Seconds};

    const S: u64 = 1_000_000_000;

    #[test]
    fn span_times_read_as_local_clock_times() {
        // 2026-10-08 18:03:12 UTC, UTC+3.
        let from = WallNs((20_734 * 86_400 + 18 * 3600 + 3 * 60 + 12) * S);
        let plus3 = 3 * 3600;
        assert_eq!(time_of_day(from, plus3), "21:03:12");
        assert_eq!(
            span_time_text(SpanState::Unmarked, from, plus3),
            "not marked"
        );
        assert_eq!(
            span_time_text(SpanState::Marking { from }, WallNs(from.0 + 35 * S), plus3),
            "marking since 21:03:12 · 0:35"
        );
        let until = WallNs(from.0 + 120 * S);
        assert_eq!(
            span_time_text(SpanState::Marked { from, until }, until, plus3),
            "21:03:12–21:05:12 · 2:00"
        );
    }

    fn average(seconds: u32, measured: f64, uncalibrated: u32) -> BandLogAverage {
        let mut levels = [None; BAND_COUNT];
        for (i, l) in levels.iter_mut().enumerate().take(LF_BAND_COUNT) {
            *l = Some(DbSpl(80.0 - i as f64));
        }
        levels[3] = None;
        BandLogAverage {
            seconds,
            measured: Seconds(measured),
            uncalibrated,
            levels: (uncalibrated == 0 && seconds > 0).then_some(levels),
        }
    }

    #[test]
    fn coverage_and_averages() {
        let (from, until) = (WallNs(0), WallNs(120 * S));
        assert_eq!(
            coverage_text(&average(118, 117.8, 0), from, until),
            "118 of 120 s logged"
        );
        assert_eq!(
            coverage_text(&average(118, 100.0, 0), from, until),
            "118 of 120 s logged, 100.0 s measured"
        );
        assert_eq!(
            coverage_text(&average(118, 118.0, 30), from, until),
            "118 of 120 s logged; 30 s not calibrated: no dB SPL average (calibrate, then mark \
             it again)"
        );
        assert!(
            coverage_text(&average(0, 0.0, 0), from, until)
                .starts_with("nothing logged in its 120 s")
        );
        assert_eq!(
            coverage_text(&average(3, 3.0, 0), from, WallNs(2_400_000_000)),
            "3 of 3 s logged"
        );
        let lf: Vec<usize> = (0..LF_BAND_COUNT).collect();
        let t = averages_text(&average(118, 118.0, 0), &lf).unwrap_or_default();
        assert!(t.starts_with("20 Hz 80.0 dB · 25 Hz 79.0 dB · 31.5 Hz 78.0 dB · 40 Hz — ·"));
        assert!(t.ends_with("200 Hz 70.0 dB SPL"), "{t}");
        assert_eq!(
            averages_text(&average(118, 118.0, 0), &[5, 17]).as_deref(),
            Some("63 Hz 75.0 dB · 1000 Hz — SPL")
        );
        assert_eq!(averages_text(&average(118, 118.0, 3), &lf), None);
    }

    #[test]
    fn the_next_step_follows_the_spans() {
        use Phase::*;
        let m = "FOH SPL";
        let p = "flat 4 bedroom";
        assert!(next_step([Unmarked; 3], false, m, p).starts_with("Play a steady test signal"));
        assert!(
            next_step([Marking, Unmarked, Unmarked], false, m, p).starts_with("Marking the FOH")
        );
        assert_eq!(
            next_step([Marked, Unmarked, Unmarked], false, m, p),
            "Move the mic to flat 4 bedroom and keep the same test-signal level. Space starts \
             the flat 4 bedroom span and Space again stops it."
        );
        assert!(next_step([Marked, Marking, Unmarked], false, m, p).contains("same test-signal"));
        assert!(
            next_step([Marked, Marked, Unmarked], false, m, p)
                .contains("without a background (every band unchecked)")
        );
        assert_eq!(
            next_step([Marked; 3], false, m, p),
            "Enter computes the band transfer and stores it in FOH SPL."
        );
        assert!(next_step([Marked; 3], true, m, p).starts_with("Band transfer stored in FOH SPL"));
        assert_eq!(
            SpanRole::Place.title("receiving room"),
            "Receiving room span"
        );
        assert_eq!(
            SpanRole::Background.what("receiving room"),
            "system silent, mic in receiving room"
        );
    }
}
