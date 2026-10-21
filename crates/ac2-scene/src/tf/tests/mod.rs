use super::*;
use crate::canvas::tests::segments;
use crate::time::Freshness;
use crate::trace::TimeBase;
use ac2_proto::frame::ValidityMask;
use ac2_proto::model::{Polarity, Smoothing, SmoothingFraction, SmoothingMode};
use ac2_proto::units::{MeasId, Seconds, SessionEpoch, TraceId};

mod extremes;
mod layout;
mod mark;
mod panes;
mod plates;

const SIZE: Viewport = Viewport {
    width: 960.0,
    height: 600.0,
};

struct Cols {
    freqs: Vec<f64>,
    mag: Vec<f32>,
    phase: Vec<f32>,
    coh: Vec<f32>,
    validity: Vec<ValidityMask>,
}

fn cols(n: usize) -> Cols {
    // 1/12 octave around 1 kHz, with a column at exactly 1 kHz.
    let freqs: Vec<f64> = (0..n)
        .map(|i| 1000.0 * 2f64.powf((i as f64 - (n / 2) as f64) / 12.0))
        .collect();
    Cols {
        mag: vec![0.0; n],
        phase: vec![0.0; n],
        coh: vec![1.0; n],
        validity: vec![ValidityMask::NONE; n],
        freqs,
    }
}

fn trace(c: &Cols, key: TraceKey, delay: f64) -> TfTrace<'_> {
    TfTrace {
        key,
        name: match key {
            TraceKey::Live(m) => format!("m{}", m.0),
            TraceKey::Stored(t) => format!("t{}", t.0),
        },
        color: Theme::dark().trace_color(0),
        freqs: &c.freqs,
        mag_db: &c.mag,
        phase_deg: Some(&c.phase),
        coherence: Some(&c.coh),
        validity: Some(&c.validity),
        offset_db: 0.0,
        polarity: Polarity::Normal,
        nudge: Seconds(0.0),
        delay_nudge: Seconds(0.0),
        time_base: TimeBase::Shared {
            epoch: SessionEpoch(1),
            delay: Seconds(delay),
        },
        freshness: Some(Freshness::from_age(0.1)),
        smoothing: None,
        note: None,
        stored: None,
        selected: false,
        compared: false,
        unresolved: None,
    }
}
