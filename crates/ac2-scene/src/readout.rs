//! Readout strings: comparison cursor and delay / distance.

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

/// `12.34 ms · 4.24 m @ 20 °C`.
pub fn delay_readout(delay_s: f64, temp_c: f64) -> String {
    if !delay_s.is_finite() {
        return format::NO_VALUE.to_string();
    }
    format!(
        "{} · {} m @ {}",
        format::ms(delay_s, 2),
        format::fixed(delay_distance_m(delay_s, temp_c), 2),
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
        assert_eq!(delay_readout(f64::NAN, 20.0), "—");
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
