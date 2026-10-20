use super::*;
use crate::axis::Range;
use crate::view::{PlotChrome, SpectrumView};
use ac2_proto::GridDef;
use ac2_proto::units::{Hz, MeasId};

mod captions;
mod drawing;
mod tracking;

const SIZE: Viewport = Viewport {
    width: 800.0,
    height: 400.0,
};

fn third_octaves() -> GridDef {
    let centres = (-17..=13)
        .map(|x| Hz(1000.0 * 10f64.powf(f64::from(x) / 10.0)))
        .collect();
    GridDef::IecBands {
        fraction: BandFraction::Third,
        centres,
    }
}

fn rta_trace<'a>(
    f: &'a [f64],
    e: &'a [(f64, f64)],
    level: &'a [f32],
    scale: LevelScale,
) -> SpectrumTrace<'a> {
    SpectrumTrace {
        key: TraceKey::Live(MeasId(1)),
        name: "RTA".into(),
        color: Color::WHITE,
        freqs: f,
        edges: e,
        level,
        drawn: None,
        validity: None,
        peak: None,
        tracked: None,
        scale,
        quantity: Quantity::Band,
        bin_hz: None,
        caption: format!(
            "{} · {}",
            fraction_label(BandFraction::Third),
            weighting_label(Weighting::A)
        ),
        freshness: None,
        offset_db: 0.0,
        selected: false,
        compared: false,
        bands: None,
        style: SpectrumStyle::Bars,
        layer: 0,
    }
}
