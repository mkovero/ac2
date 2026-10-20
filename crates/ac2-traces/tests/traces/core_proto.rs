//! ac2-core computes grids and band tables without the protocol crate (it stays pure maths);
//! the protocol's copies are what the scene and the stored traces read. Both must agree bit
//! for bit, or a column would sit at a different frequency on screen than in the DSP.

use ac2_core::grid::{LogGrid, iec_band_centres, iec_band_edges};

use super::*;

fn bits(v: impl IntoIterator<Item = f64>) -> Vec<u64> {
    v.into_iter().map(f64::to_bits).collect()
}

#[test]
fn log_grid_columns_agree() {
    for ppo in [1, 3, 6, 12, 24, 48, 96] {
        for (lo, hi) in [(20.0, 20_000.0), (2.0, 24_000.0), (900.0, 1100.0)] {
            let core = LogGrid::covering(ppo, lo, hi);
            let proto = GridDef::Log {
                ppo,
                k_min: core.k_min,
                k_max: core.k_max,
            };
            assert_eq!(bits(core.frequencies()), bits(proto.centres()), "{ppo}");
            let core_edges = (0..core.len()).flat_map(|i| {
                let (a, b) = core.edges(i);
                [a, b]
            });
            let proto_edges = proto.edges().into_iter().flat_map(|(a, b)| [a, b]);
            assert_eq!(bits(core_edges), bits(proto_edges), "{ppo}");
        }
    }
}

#[test]
fn iec_band_columns_agree() {
    for fraction in [
        BandFraction::Octave,
        BandFraction::Third,
        BandFraction::Sixth,
        BandFraction::Twelfth,
        BandFraction::TwentyFourth,
    ] {
        let b = fraction.b();
        let centres = iec_band_centres(b, 10.0, 24_000.0);
        let proto = GridDef::IecBands {
            fraction,
            centres: centres.iter().map(|&f| Hz(f)).collect(),
        };
        assert_eq!(bits(centres.iter().copied()), bits(proto.centres()));
        let core_edges = centres.iter().flat_map(|&f| {
            let (a, b) = iec_band_edges(f, b);
            [a, b]
        });
        let proto_edges = proto.edges().into_iter().flat_map(|(a, b)| [a, b]);
        assert_eq!(bits(core_edges), bits(proto_edges), "1/{b}");
    }
}

#[test]
fn band_leq_tables_agree() {
    use ac2_core::band_leq::{BANDS, LF_BANDS, NOMINAL_HZ};
    assert_eq!((BANDS, LF_BANDS), (BAND_COUNT, LF_BAND_COUNT));
    assert_eq!(bits(NOMINAL_HZ), bits(BAND_NOMINAL_HZ));
    assert_eq!(BAND_NOMINAL_HZ, THIRD_OCTAVE_NOMINAL_HZ[..BAND_COUNT]);
}
