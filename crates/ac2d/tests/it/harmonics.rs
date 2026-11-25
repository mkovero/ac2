//! The generator's sine read from spectrum measurements on the fake rig: a `spec` frame
//! carries its harmonics while the sine fires, from every spectrum, with whatever window
//! can read it.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig, FakeDriver};
use ac2_proto::Command;
use ac2_proto::frame::{FrameData, HarmonicReading, HarmonicUnreadable, SpecHarmonics};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2d::Daemon;

use crate::common::*;

const T: Duration = Duration::from_secs(10);
const FFT: u32 = 4096;
const LEVEL: f64 = -20.0;
/// A bin centre of the 4096-point FFT and of the multi-resolution spectrum's longest
/// (32 768 points at 48 kHz) alike.
const ON_BIN: f64 = 85.0 * FS as f64 / FFT as f64;
const OFF_BIN: f64 = 1000.0;

struct Rig {
    h: ac2d::Handle,
    c: Client,
    sub: Sub,
    tok: LeaseToken,
    d: FakeDriver,
    _dir: tempfile::TempDir,
}

fn start(name: &str, backend: FakeBackend) -> Rig {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = steady_config(backend.clone(), inproc(name));
    cfg.session_dir = Some(dir.path().join("sessions"));
    let h = Daemon::start(cfg).unwrap();
    let (mut c, sub) = connect(&h, &[b"d/1/spec", b"d/2/spec", b"d/3/spec"]);
    c.ok(Command::StreamOpen {
        config: stream(false),
    });
    let tok = c.acquire();
    let d = driver(&backend);
    Rig {
        h,
        c,
        sub,
        tok,
        d,
        _dir: dir,
    }
}

impl Rig {
    /// A spectrum of the loopback input, started.
    fn spectrum(&mut self, resolution: SpecResolution, window: Window) -> MeasId {
        let m = self.c.measurement(Command::MeasCreate {
            config: MeasConfig {
                scope: MeasScope::Session,
                name: format!("{window:?}"),
                kind: MeasKind::Spectrum {
                    config: SpectrumConfig {
                        resolution,
                        window,
                        ..SpectrumConfig::on_input(0)
                    },
                },
            },
        });
        self.c.ok(Command::MeasStart { meas: m.id });
        m.id
    }

    fn sine(&mut self, freq: f64, firing: bool) {
        self.c.ok(Command::GenSet {
            lease_token: self.tok,
            desired: GeneratorDesired {
                settings: GeneratorSettings {
                    signal: Signal::Sine { freq: Hz(freq) },
                    level: Dbfs(LEVEL),
                    band: None,
                    outputs: vec![0],
                },
                armed: true,
                firing,
            },
        });
    }

    /// Runs a second of audio (the longest window, 0.68 s, filled with what plays now) and
    /// returns what the newest frame of `meas` says of the sine.
    fn reading(&mut self, meas: MeasId) -> Option<SpecHarmonics> {
        run(&mut self.d, 1.0);
        let end = end_sample(&self.d);
        let f = self
            .sub
            .frame(T, |f| {
                matches!(&f.data, FrameData::Spec(s) if s.meas == meas)
                    && f.stamp.audio_sample.0 + u64::from(FS) / 4 >= end
            })
            .expect("spec frame");
        let FrameData::Spec(s) = f.data else {
            unreachable!()
        };
        s.meta.harmonics.map(|h| *h)
    }
}

fn fixed() -> SpecResolution {
    SpecResolution::Fixed { fft_len: FFT }
}

#[test]
fn spectra_read_the_firing_sine_and_stop_with_it() {
    let mut r = start("harmonics", manual_rig());
    let rect = r.spectrum(fixed(), Window::Rectangular);
    assert_eq!(r.reading(rect), None, "nothing fires");
    // Armed is not firing.
    r.sine(ON_BIN, false);
    assert_eq!(r.reading(rect), None, "armed only");

    r.sine(ON_BIN, true);
    let h = r.reading(rect).expect("the sine is read");
    assert_eq!((h.f0, h.on_bin), (Hz(ON_BIN), true));
    let HarmonicReading::Read {
        fundamental,
        harmonics,
        thd,
        thd_bound,
        thd_n,
        leakage,
    } = h.reading
    else {
        panic!("on a bin a rectangular window reads: {:?}", h.reading)
    };
    // The loopback is a cable: the sine comes back at the level it was sent.
    assert!((fundamental - LEVEL).abs() < 0.01, "{fundamental}");
    assert_eq!(
        harmonics.iter().map(|x| x.order).collect::<Vec<_>>(),
        (2..=10).collect::<Vec<u8>>()
    );
    // A clean sine: whatever stands above the floor is the f32 samples' rounding.
    assert!(thd.is_none_or(|t| t.0 < -120.0), "{thd:?}");
    assert!(thd_bound.0 < -120.0, "{thd_bound:?}");
    assert!(thd_n.0 < -120.0, "{thd_n:?}");
    assert!(
        harmonics.iter().all(|x| x.level.0 < -120.0),
        "{harmonics:?}"
    );
    assert_eq!(leakage, None, "nothing leaks on a bin");

    // Spectra made while the sine fires read it from their first frames.
    let bh = r.spectrum(fixed(), Window::BlackmanHarris4);
    let multi = r.spectrum(SpecResolution::MULTI, Window::Hann);
    for m in [bh, multi] {
        let h = r.reading(m).expect("read from the start");
        assert_eq!((h.f0, h.on_bin), (Hz(ON_BIN), true), "{m:?}");
        let HarmonicReading::Read { fundamental, .. } = h.reading else {
            panic!("{m:?}: {:?}", h.reading)
        };
        assert!((fundamental - LEVEL).abs() < 0.01, "{m:?}: {fundamental}");
    }

    // The frequency changes under the firing generator, to between two bins.
    r.sine(OFF_BIN, true);
    let h = r.reading(rect).expect("still a sine");
    assert_eq!(
        h,
        SpecHarmonics {
            f0: Hz(OFF_BIN),
            on_bin: false,
            reading: HarmonicReading::Unreadable {
                reason: HarmonicUnreadable::RectangularOffBin
            },
        }
    );
    let h = r.reading(bh).expect("still a sine");
    assert_eq!((h.f0, h.on_bin), (Hz(OFF_BIN), false));
    let HarmonicReading::Read {
        fundamental,
        thd_n,
        leakage,
        ..
    } = h.reading
    else {
        panic!(
            "a Blackman-Harris window reads between bins: {:?}",
            h.reading
        )
    };
    assert!((fundamental - LEVEL).abs() < 0.01, "{fundamental}");
    let leakage = leakage.expect("between bins the window leaks");
    assert!(leakage.0 < -80.0, "{leakage:?}");
    assert!(thd_n.0 < leakage.0 + 3.0, "{thd_n:?} against {leakage:?}");

    // Noise is no sine.
    r.c.ok(Command::GenSet {
        lease_token: r.tok,
        desired: GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(LEVEL),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        },
    });
    assert_eq!(r.reading(bh), None, "pink noise");
    r.sine(OFF_BIN, true);
    assert!(r.reading(bh).is_some());
    r.c.ok(Command::GenStop);
    for m in [rect, bh, multi] {
        assert_eq!(r.reading(m), None, "stopped: {m:?}");
    }
    shutdown_driving(r.h, Some(r.d));
}

/// A path with y = x + c2·x² puts a second harmonic c2·A/2 below a sine of peak A
/// (`FakePath::nonlinearity`): the frame states it, and a distortion of that alone.
#[test]
fn a_known_second_harmonic_is_read_at_its_level() {
    const C2: f64 = 0.1;
    let backend = FakeBackend::new(FakeConfig {
        sample_rate: FS,
        block_frames: BLOCK,
        inputs: 2,
        outputs: 2,
        drive: FakeDrive::Manual,
        paths: vec![FakePath {
            nonlinearity: vec![C2],
            ..FakePath::loopback(0, 0, LOOP_DELAY)
        }],
        ..FakeConfig::default()
    })
    .unwrap();
    let mut r = start("harmonics-h2", backend);
    let bh = r.spectrum(fixed(), Window::BlackmanHarris4);
    r.sine(OFF_BIN, true);
    let h = r.reading(bh).expect("the sine is read");
    let HarmonicReading::Read {
        fundamental,
        harmonics,
        thd,
        thd_bound,
        thd_n,
        ..
    } = h.reading
    else {
        panic!("{:?}", h.reading)
    };
    assert!((fundamental - LEVEL).abs() < 0.01, "{fundamental}");
    // 0 dBFS is a sine of peak 1.
    let peak = 10f64.powf(LEVEL / 20.0);
    let want = 20.0 * (C2 * peak / 2.0).log10();
    let second = harmonics[0];
    assert_eq!((second.order, second.resolved), (2, true));
    assert!(
        (second.level.0 - want).abs() < 0.01,
        "{second:?} for {want}"
    );
    assert!(second.floor.0 < want - 60.0, "{second:?}");
    assert!(
        harmonics[1..].iter().all(|x| x.level.0 < want - 60.0),
        "{harmonics:?}"
    );
    let thd = thd.expect("a resolved harmonic");
    assert!((thd.0 - want).abs() < 0.01, "{thd:?}");
    assert!(
        thd_bound.0 >= thd.0 && thd_bound.0 < want + 0.01,
        "{thd_bound:?}"
    );
    assert!((thd_n.0 - want).abs() < 0.01, "{thd_n:?}");
    shutdown_driving(r.h, Some(r.d));
}
