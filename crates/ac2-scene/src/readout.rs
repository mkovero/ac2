//! Readout strings: comparison cursor and delay / distance.

use crate::cursor_jump::Landmark;
use crate::format;
use crate::grid::nearest_column;
use crate::trace::{DisplayTrace, TraceKey};
use crate::view::PhaseView;

/// Speed of sound in air at `temp_c` °C, m/s: `c = 331.3 · √(1 + T / 273.15)`.
pub fn speed_of_sound(temp_c: f64) -> f64 {
    331.3 * (1.0 + temp_c / 273.15).sqrt()
}

/// Acoustic path distance of `delay_s` at `temp_c` (decision A: plain delay × c(T)), m.
pub fn delay_distance_m(delay_s: f64, temp_c: f64) -> f64 {
    delay_s * speed_of_sound(temp_c)
}

/// The path a delay stands for: `4.24 m @ 20 °C`.
fn path_at(delay_s: f64, temp_c: f64) -> String {
    format!(
        "{} m @ {}",
        format::fixed(delay_distance_m(delay_s, temp_c), 2),
        format::celsius(temp_c)
    )
}

/// `12.34 ms · 4.24 m @ 20 °C`; the delay to the µs when it has a finer part (`12.502 ms`).
pub fn delay_readout(delay_s: f64, temp_c: f64) -> String {
    if !delay_s.is_finite() {
        return format::NO_VALUE.to_string();
    }
    format!("{} · {}", format::delay(delay_s), path_at(delay_s, temp_c))
}

/// A comb read from the spacing of its notches (or peaks): a copy delayed by `τ` summed in
/// cancels every `1/τ` Hz, so the spacing names the delay and the extra path the copy
/// travelled: `comb Δf 1.00 kHz · 1.00 ms · 0.34 m @ 20 °C`.
pub fn comb_text(spacing_hz: f64, temp_c: f64) -> String {
    if !(spacing_hz.is_finite() && spacing_hz > 0.0) {
        return format!("comb Δf {}", format::NO_VALUE);
    }
    let tau = 1.0 / spacing_hz;
    format!(
        "comb Δf {} · {} · {}",
        format::freq_readout(spacing_hz),
        format::ms(tau, 2),
        path_at(tau, temp_c)
    )
}

/// Difference of two arrivals (A − B), to 0.1 µs (a hundredth of a sample at 96 kHz: what
/// aligning a pair needs), with the path-length difference it stands for:
/// `+3.7 µs · +1.3 mm @ 20 °C`.
pub fn arrival_difference(a_s: f64, b_s: f64, temp_c: f64) -> String {
    let d = a_s - b_s;
    if !d.is_finite() {
        return format::NO_VALUE.to_string();
    }
    format!(
        "{} µs · {} mm @ {}",
        format::signed(d * 1e6, 1),
        format::signed(delay_distance_m(d, temp_c) * 1000.0, 1),
        format::celsius(temp_c)
    )
}

/// One trace's values at the cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorRow {
    pub key: TraceKey,
    pub name: String,
    /// Frequency of the column read (the trace's own grid).
    pub freq: String,
    pub magnitude: String,
    /// Phase in the pane's mode: degrees, or group delay in ms.
    pub phase: String,
    pub coherence: String,
}

/// The comparison cursor: one frequency, every trace's values at its nearest column.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorReadout {
    /// Column frequency the cursor snapped to (first trace with data), Hz.
    pub freq_hz: f64,
    pub freq: String,
    pub rows: Vec<CursorRow>,
}

/// Values at `hz` for every trace, the same numbers the panes draw.
pub fn cursor_readout(traces: &[DisplayTrace], hz: f64, phase: PhaseView) -> Option<CursorReadout> {
    let mut snapped = None;
    let rows = traces
        .iter()
        .filter_map(|t| {
            let i = nearest_column(&t.freqs, hz)?;
            let f = t.freqs[i];
            snapped.get_or_insert(f);
            let phase = match phase {
                PhaseView::Wrapped => format::phase_readout(t.phase_wrapped_deg[i]),
                PhaseView::Unwrapped { .. } => format::phase_readout(t.phase_unwrapped_deg[i]),
                PhaseView::GroupDelay { .. } => format::ms(t.group_delay_s[i], 2),
            };
            Some(CursorRow {
                key: t.key,
                name: t.name.clone(),
                freq: format::freq_readout(f),
                magnitude: format::db_readout(t.magnitude_db[i]),
                phase,
                coherence: format::coherence_readout(t.coherence[i]),
            })
        })
        .collect();
    let freq_hz = snapped?;
    Some(CursorReadout {
        freq_hz,
        freq: format::freq_readout(freq_hz),
        rows,
    })
}

/// One curve's values at the cursor or at its mark, as the pane draws them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkPoint {
    /// The column read, Hz.
    pub hz: f64,
    /// Level as drawn, dB.
    pub level_db: f64,
    /// Phase in the pane's mode: degrees, or group delay in seconds. `None` on a spectrum.
    pub phase: Option<f64>,
    /// The peak or notch the column is, as the cursor jumps find them.
    pub landmark: Option<Landmark>,
}

/// The cursor mark: where it is and how the cursor differs from it.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkReadout {
    /// The column the mark reads, Hz.
    pub freq_hz: f64,
    /// `mark 1.00 kHz`, `Δ +1.00 kHz · −3.2 dB · +45°`, then, when both ends are peaks or
    /// both notches, the comb they make as neighbours: `comb Δf 1.00 kHz · 1.00 ms · …`.
    pub lines: Vec<String>,
}

/// A frequency difference, signed: `+1.00 kHz`, `−250 Hz`, `0 Hz`.
pub fn freq_delta(d_hz: f64) -> String {
    if !d_hz.is_finite() {
        return format::NO_VALUE.to_string();
    }
    let text = format::freq_readout(d_hz.abs());
    if text == format::NO_VALUE {
        return "0 Hz".to_string();
    }
    let sign = if d_hz < 0.0 { format::MINUS } else { '+' };
    format!("{sign}{text}")
}

/// The difference of two phase readings in `phase`'s terms: wrapped degrees folded back
/// into ±180° (a wrapped curve's jump is not a phase change), unwrapped degrees as they
/// are, group delay in ms.
fn phase_delta(cursor: f64, mark: f64, phase: PhaseView) -> String {
    let d = cursor - mark;
    if !d.is_finite() {
        return format::NO_VALUE.to_string();
    }
    match phase {
        PhaseView::Wrapped => format::phase_readout((d + 180.0).rem_euclid(360.0) - 180.0),
        PhaseView::Unwrapped { .. } => format::phase_readout(d),
        PhaseView::GroupDelay { .. } => format!("{} ms", format::signed(d * 1000.0, 2)),
    }
}

/// The cursor (`cursor`) read against its mark (`mark`) on one curve, named (`name`) when
/// other curves are shown beside it; `temp_c` for the path a comb's delay stands for.
pub fn mark_readout(
    mark: MarkPoint,
    cursor: MarkPoint,
    phase: PhaseView,
    name: Option<&str>,
    temp_c: f64,
) -> MarkReadout {
    let at = format::freq_readout(mark.hz);
    let head = match name {
        Some(n) => format!("mark {n} {at}"),
        None => format!("mark {at}"),
    };
    let df = cursor.hz - mark.hz;
    let mut delta = format!(
        "Δ {} · {}",
        freq_delta(df),
        format::db_readout(cursor.level_db - mark.level_db)
    );
    if let (Some(c), Some(m)) = (cursor.phase, mark.phase) {
        delta = format!("{delta} · {}", phase_delta(c, m, phase));
    }
    let mut lines = vec![head, delta];
    // A comb's peaks (or notches) repeat every 1/τ; a peak and a notch sit half that apart,
    // and any other column has no spacing to read, so only like ends make a comb.
    let like = mark.landmark.is_some() && mark.landmark == cursor.landmark;
    if like && df.is_finite() && df != 0.0 {
        lines.push(comb_text(df.abs(), temp_c));
    }
    MarkReadout {
        freq_hz: mark.hz,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Color;
    use crate::trace::{PhaseRelation, TraceKey};
    use ac2_proto::units::MeasId;

    #[test]
    fn speed_of_sound_formula() {
        assert!((speed_of_sound(0.0) - 331.3).abs() < 1e-12);
        // 20 °C: 331.3 · √(293.15 / 273.15) = 343.21 m/s.
        assert!((speed_of_sound(20.0) - 343.2146).abs() < 1e-3);
    }

    #[test]
    fn delay_distance_strings() {
        // 12.34 ms × 343.2146 m/s = 4.2353 m.
        assert_eq!(delay_readout(0.012_34, 20.0), "12.34 ms · 4.24 m @ 20 °C");
        // 10 ms at 0 °C = 3.313 m.
        assert_eq!(delay_readout(0.010, 0.0), "10.00 ms · 3.31 m @ 0 °C");
        // 30 °C: c = 349.0 m/s; 1 ms → 0.349 m.
        assert_eq!(delay_readout(0.001, 30.0), "1.00 ms · 0.35 m @ 30 °C");
        assert_eq!(delay_readout(-0.0015, 22.5), "−1.50 ms · −0.52 m @ 22.5 °C");
        assert_eq!(
            delay_readout(0.000_003_7, 20.0),
            "0.004 ms · 0.00 m @ 20 °C"
        );
        assert_eq!(delay_readout(f64::NAN, 20.0), "—");
        // 3.69 µs × 343.2146 m/s = 1.27 mm.
        assert_eq!(
            arrival_difference(0.012_503_69, 0.0125, 20.0),
            "+3.7 µs · +1.3 mm @ 20 °C"
        );
        assert_eq!(
            arrival_difference(0.0, 0.000_020_8, 20.0),
            "−20.8 µs · −7.1 mm @ 20 °C"
        );
        assert_eq!(arrival_difference(f64::NAN, 0.0, 20.0), "—");
    }

    #[test]
    fn comb_and_mark_strings() {
        // 1 kHz spacing: τ = 1 ms, 343.21 mm at 20 °C.
        assert_eq!(
            comb_text(1000.0, 20.0),
            "comb Δf 1.00 kHz · 1.00 ms · 0.34 m @ 20 °C"
        );
        // 250 Hz: 4 ms, 1.3729 m.
        assert_eq!(
            comb_text(250.0, 20.0),
            "comb Δf 250 Hz · 4.00 ms · 1.37 m @ 20 °C"
        );
        assert_eq!(comb_text(0.0, 20.0), "comb Δf —");
        assert_eq!(freq_delta(1000.0), "+1.00 kHz");
        assert_eq!(freq_delta(-250.0), "−250 Hz");
        assert_eq!(freq_delta(0.0), "0 Hz");
        assert_eq!(freq_delta(f64::NAN), "—");
        let p = |hz, level_db, phase| MarkPoint {
            hz,
            level_db,
            phase,
            landmark: None,
        };
        let at = |landmark, q: MarkPoint| MarkPoint {
            landmark: Some(landmark),
            ..q
        };
        let (peak, notch) = (Landmark::Peak, Landmark::Notch);
        // Two neighbouring notches of a 1 ms comb, read wrapped: −170° − (+170°) is +20°
        // across the wrap, not −340°.
        let r = mark_readout(
            at(notch, p(500.0, -30.0, Some(170.0))),
            at(notch, p(1500.0, -26.75, Some(-170.0))),
            PhaseView::Wrapped,
            None,
            20.0,
        );
        assert_eq!(r.freq_hz, 500.0);
        assert_eq!(
            r.lines,
            [
                "mark 500 Hz",
                "Δ +1.00 kHz · +3.2 dB · +20°",
                "comb Δf 1.00 kHz · 1.00 ms · 0.34 m @ 20 °C",
            ]
        );
        let unwrapped = PhaseView::Unwrapped {
            range: crate::axis::Range::new(-720.0, 0.0),
        };
        let r = mark_readout(
            at(peak, p(2000.0, 0.0, Some(-100.0))),
            at(peak, p(1000.0, -6.0, Some(-460.0))),
            unwrapped,
            Some("m1"),
            0.0,
        );
        assert_eq!(
            r.lines,
            [
                "mark m1 2.00 kHz",
                "Δ −1.00 kHz · −6.0 dB · −360°",
                "comb Δf 1.00 kHz · 1.00 ms · 0.33 m @ 0 °C",
            ]
        );
        let gd = PhaseView::GroupDelay {
            range_ms: crate::axis::Range::new(-5.0, 5.0),
        };
        let r = mark_readout(
            p(100.0, 0.0, Some(0.002)),
            p(100.0, f64::NAN, Some(0.0035)),
            gd,
            None,
            20.0,
        );
        // On the mark itself: no spacing, so no comb; a level not drawn reads as none.
        assert_eq!(r.lines, ["mark 100 Hz", "Δ 0 Hz · — · +1.50 ms"]);
        // A spectrum has no phase.
        let r = mark_readout(
            at(notch, p(1000.0, -20.0, None)),
            at(notch, p(1250.0, -32.5, None)),
            PhaseView::Wrapped,
            None,
            20.0,
        );
        assert_eq!(r.lines[1], "Δ +250 Hz · −12.5 dB");
        assert_eq!(r.lines[2], "comb Δf 250 Hz · 4.00 ms · 1.37 m @ 20 °C");
        // A peak to a notch is half a comb's spacing, and a column that is neither has no
        // spacing at all: no comb either way.
        for (m, c) in [
            (
                at(peak, p(1000.0, -20.0, None)),
                at(notch, p(1250.0, -32.5, None)),
            ),
            (at(peak, p(1000.0, -20.0, None)), p(1250.0, -32.5, None)),
            (p(1000.0, -20.0, None), p(1250.0, -32.5, None)),
        ] {
            let r = mark_readout(m, c, PhaseView::Wrapped, None, 20.0);
            assert_eq!(r.lines, ["mark 1.00 kHz", "Δ +250 Hz · −12.5 dB"]);
        }
    }

    fn dt(key: u32, freqs: Vec<f64>, mag: Vec<f64>) -> DisplayTrace {
        let n = freqs.len();
        DisplayTrace {
            key: TraceKey::Live(MeasId(key)),
            name: format!("T{key}"),
            color: Color::WHITE,
            relation: PhaseRelation::Reference,
            shift_s: 0.0,
            offset_db: 0.0,
            inverted: false,
            freqs,
            magnitude_db: mag,
            phase_wrapped_deg: vec![-44.6; n],
            phase_unwrapped_deg: vec![-404.6; n],
            group_delay_s: vec![0.001_234; n],
            coherence: vec![0.987; n],
            alpha: vec![1.0; n],
            freshness: None,
            smoothing: None,
            note: None,
            dashed: false,
        }
    }

    #[test]
    fn cursor_rows() {
        let a = dt(1, vec![500.0, 1000.0, 2000.0], vec![1.0, -3.25, 2.0]);
        let mut b = dt(2, vec![900.0, 1100.0], vec![0.0, f64::NAN]);
        b.coherence = vec![f64::NAN, f64::NAN];
        let r =
            cursor_readout(&[a.clone(), b.clone()], 1040.0, PhaseView::Wrapped).expect("readout");
        assert_eq!(r.freq, "1.00 kHz");
        assert_eq!(r.freq_hz, 1000.0);
        assert_eq!(r.rows[0].magnitude, "−3.2 dB");
        assert_eq!(r.rows[0].phase, "−45°");
        assert_eq!(r.rows[0].coherence, "0.99");
        // Second trace snaps to its own column (1.1 kHz is nearer 1.04 kHz than 900 Hz),
        // which is invalid there.
        assert_eq!(r.rows[1].freq, "1.10 kHz");
        assert_eq!(r.rows[1].magnitude, "—");
        assert_eq!(r.rows[1].coherence, "—");
        let r = cursor_readout(
            std::slice::from_ref(&a),
            1000.0,
            PhaseView::GroupDelay {
                range_ms: crate::axis::Range::new(-5.0, 5.0),
            },
        )
        .expect("readout");
        assert_eq!(r.rows[0].phase, "1.23 ms");
        let r = cursor_readout(
            &[a],
            1000.0,
            PhaseView::Unwrapped {
                range: crate::axis::Range::new(-720.0, 0.0),
            },
        )
        .expect("readout");
        assert_eq!(r.rows[0].phase, "−405°");
        assert!(cursor_readout(&[], 1000.0, PhaseView::Wrapped).is_none());
    }
}
