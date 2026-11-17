//! The generator's sine read under a calibration: the fundamental is what the frame shows
//! for it, and the harmonics carry the mic curve's difference to it.

use std::sync::Arc;

use ac2_audio::BlockFlags;
use ac2_core::mic_curve::MicCurve;
use ac2_proto::frame::{HarmonicReading, SpecHarmonics};
use ac2_proto::model::{CalStatus, SpecAveraging, SpecLevel, SpecResolution, Window};
use ac2_proto::units::{MeasId, Rev};

use super::*;

const FS: u32 = 48_000;
const BLOCK: u32 = 256;
/// A bin centre of a 4096-point FFT and of the multi-resolution spectrum's longest.
const F0: f64 = 85.0 * FS as f64 / 4096.0;
/// dB SPL of 0 dBFS.
const SENSITIVITY: f64 = 120.0;
const LEVEL_DBFS: f64 = -20.0;
const SECOND_DBC: f64 = -40.0;

/// A curve that is not flat between the sine and its second harmonic.
fn cal() -> InputCal {
    let curve =
        MicCurve::from_points(&[(20.0, 0.0), (500.0, -1.0), (4000.0, 5.0), (20_000.0, 5.0)])
            .expect("curve");
    InputCal {
        status: CalStatus::Uncalibrated,
        sensitivity: Some(SENSITIVITY),
        correction: Some(Arc::new(curve.normalised(1000.0))),
        spl_entry: None,
        curve: None,
    }
}

/// A second of the sine with its second harmonic through a spectrum of `resolution`.
fn read(resolution: SpecResolution, level: SpecLevel, cal: InputCal) -> (Spectrum, SpecHarmonics) {
    let cfg = WireConfig {
        input: 0,
        resolution,
        window: Window::Hann,
        averaging: SpecAveraging::Off,
        smoothing: None,
        level,
        display: Default::default(),
    };
    let mut s = Spectrum::new(MeasId(1), cfg, FS, BLOCK, 0, cal, Rev(1)).expect("spectrum");
    s.set_tone(Some(F0));
    let a = 10f64.powf(LEVEL_DBFS / 20.0);
    let w = std::f64::consts::TAU * F0 / f64::from(FS);
    for b in 0..u64::from(FS / BLOCK) {
        let start = b * u64::from(BLOCK);
        let data = (start..start + u64::from(BLOCK))
            .map(|n| {
                let t = n as f64 * w;
                (a * (t.sin() + 10f64.powf(SECOND_DBC / 20.0) * (2.0 * t).sin())) as f32
            })
            .collect();
        s.push(&Block::new(start, BLOCK, 1, BlockFlags::NONE, 1, data));
    }
    let h = *s.harmonics().expect("a sine and a spectrum");
    (s, h)
}

fn parts(h: &SpecHarmonics) -> (f64, f64, f64, f64) {
    let HarmonicReading::Read {
        fundamental,
        harmonics,
        thd,
        thd_n,
        ..
    } = &h.reading
    else {
        panic!("{:?}", h.reading)
    };
    assert!(
        harmonics[0].resolved && harmonics[0].order == 2,
        "{harmonics:?}"
    );
    (
        *fundamental,
        harmonics[0].level.0,
        thd.expect("a resolved harmonic").0,
        thd_n.0,
    )
}

#[test]
fn uncalibrated_reads_dbfs_and_dbc() {
    let (_, h) = read(
        SpecResolution::Fixed { fft_len: 4096 },
        SpecLevel::Tone,
        InputCal::none(),
    );
    assert!(h.on_bin);
    let (fundamental, second, thd, thd_n) = parts(&h);
    assert!((fundamental - LEVEL_DBFS).abs() < 1e-3, "{fundamental}");
    for v in [second, thd, thd_n] {
        assert!((v - SECOND_DBC).abs() < 1e-3, "{v}");
    }
}

#[test]
fn calibrated_reads_what_the_frame_shows() {
    let cal = cal();
    let c = cal.correction.clone().expect("curve");
    let (c1, c2) = (c.db(F0), c.db(2.0 * F0));
    assert!((c2 - c1).abs() > 1.0, "the curve must tilt: {c1} {c2}");
    for (resolution, level) in [
        (SpecResolution::Fixed { fft_len: 4096 }, SpecLevel::Tone),
        // The reading does not depend on how the frame scales its levels.
        (SpecResolution::Fixed { fft_len: 4096 }, SpecLevel::Density),
        (SpecResolution::MULTI, SpecLevel::Tone),
    ] {
        let (mut s, h) = read(resolution, level, cal.clone());
        let (fundamental, second, thd, thd_n) = parts(&h);
        let want = LEVEL_DBFS + SENSITIVITY - c1;
        assert!(
            (fundamental - want).abs() < 1e-3,
            "{resolution:?}: {fundamental} for {want}"
        );
        let want = SECOND_DBC - (c2 - c1);
        for v in [second, thd, thd_n] {
            assert!((v - want).abs() < 1e-3, "{resolution:?}: {v} for {want}");
        }
        // On a bin the frame's tone level at the sine is the same number, but for the
        // curve's slope across the three bins a Hann window spreads the sine over.
        if let (Engine::Fixed(_), SpecLevel::Tone) = (&s.engine, level) {
            assert!(s.fill_power());
            let Engine::Fixed(e) = &s.engine else {
                unreachable!()
            };
            let shown = 10.0 * e.power[85].log10();
            assert!(
                (shown - fundamental).abs() < 1e-3,
                "{shown} against {fundamental}"
            );
        }
    }
}

#[test]
fn no_sine_no_reading() {
    let (mut s, _) = read(
        SpecResolution::Fixed { fft_len: 4096 },
        SpecLevel::Tone,
        InputCal::none(),
    );
    s.set_tone(None);
    assert_eq!(s.harmonics(), None);
}
