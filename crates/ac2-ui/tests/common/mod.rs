//! A fake rig: the client crate's `FakeDaemon` with an open session, two transfer
//! measurements, a spectrum and an SPL meter, and a thread publishing synthetic frames.
//! The values are display test material only (shapes chosen to be recognisable), never a
//! reference for any measurement.
#![allow(dead_code, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use ac2_client::fake::{FakeDaemon, FakeOptions};
use ac2_proto::frame::{IrFrame, SpecFrame, SplFrame, TfFrame};
use ac2_proto::frame::{IrMeta, SpecMeta, SplMeta, TfMeta, ValidityMask};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, Frame, FrameData, GridDef, Patch};

pub struct Rig {
    pub fake: Arc<FakeDaemon>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

const TF_GRID: GridDef = GridDef::Log {
    ppo: 24,
    k_min: -135,
    k_max: 103,
};
const SPEC_GRID: GridDef = GridDef::Linear {
    fs: Hz(48_000.0),
    n: 4096,
};

fn wrap(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

fn tf_frame(meas: u32, gain: f64, tau: f64, bump_hz: f64) -> TfFrame {
    let freqs = ac2_scene::grid::column_frequencies(&TF_GRID);
    let mag = freqs
        .iter()
        .map(|f| {
            let x = (f / bump_hz).log2();
            (gain + 5.0 * (-x * x * 2.0).exp()
                - 12.0 * (60.0 / f).powi(4).min(1.0)
                - 6.0 * (f / 16_000.0).powi(2).min(1.0)) as f32
        })
        .collect();
    let phase = freqs
        .iter()
        .map(|f| wrap(-360.0 * f * tau - 40.0 * (bump_hz / f).min(3.0)) as f32)
        .collect();
    let coh = freqs
        .iter()
        .map(|f| (1.0 - (35.0 / f).powi(2)).clamp(0.05, 0.99) as f32)
        .collect();
    TfFrame {
        meas: MeasId(meas),
        meta: TfMeta {
            delay: Seconds(0.0125),
            frozen: false,
            smoothing: Some(Smoothing {
                fraction: SmoothingFraction::Sixth,
                mode: SmoothingMode::Power,
            }),
            mic_curve: false,
        },
        mag,
        phase,
        coh,
        eff_avg: None,
        validity: vec![ValidityMask::NONE; freqs.len()],
    }
}

fn ir_frame(meas: u32) -> IrFrame {
    let dt = 1.0 / 48_000.0;
    let n = 960;
    let linear = (0..n)
        .map(|i| {
            let t = (i as f64 - 96.0) * dt;
            if t < 0.0 {
                0.0
            } else {
                (0.8 * (-t / 0.004).exp() * (2.0 * std::f64::consts::PI * 900.0 * t).cos()
                    + 0.25 * (-(t - 0.006).powi(2) / 2e-8).exp()) as f32
            }
        })
        .collect();
    IrFrame {
        meas: MeasId(meas),
        meta: IrMeta {
            sample_rate: Hz(48_000.0),
            t0: Seconds(-96.0 * dt),
            dt: Seconds(dt),
            inserted_delay: Seconds(0.0125),
        },
        linear,
        etc: None,
    }
}

fn spec_frame(meas: u32) -> SpecFrame {
    let freqs = ac2_scene::grid::column_frequencies(&SPEC_GRID);
    let level = freqs
        .iter()
        .map(|f| {
            let f = f.max(5.0);
            let tone = if (f - 1000.0).abs() < 6.0 { 30.0 } else { 0.0 };
            (-62.0 - 3.0 * (f / 1000.0).log2() + tone) as f32
        })
        .collect();
    SpecFrame {
        meas: MeasId(meas),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
        },
        level,
        validity: vec![ValidityMask::NONE; freqs.len()],
    }
}

fn spl_frame(meas: u32) -> SplFrame {
    SplFrame {
        meas: MeasId(meas),
        meta: SplMeta {
            scale: LevelScale::Dbfs,
            weighting: Weighting::A,
            time_weighting: TimeWeighting::Fast,
            peak_weighting: PeakWeighting::C,
            level: -23.4,
            lmax: -18.2,
            lmin: -31.0,
            leq: -24.1,
            lpeak: -9.6,
            duration: Seconds(83.0),
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
        },
    }
}

fn measurement(
    id: u32,
    name: &str,
    kind: MeasKind,
    delay: Option<DelayState>,
    grid: Option<&GridDef>,
) -> Measurement {
    Measurement {
        id: MeasId(id),
        config: MeasConfig {
            name: name.into(),
            kind,
        },
        config_rev: Rev(1),
        running: true,
        frozen: false,
        delay,
        grid_id: grid.map(GridDef::id),
    }
}

fn transfer(meas_in: u16) -> MeasKind {
    MeasKind::Transfer {
        config: TransferConfig {
            reference_input: 0,
            measurement_input: meas_in,
            averaging: TfAveraging::Exponential {
                time_constant: Seconds(1.0),
            },
            grid: LogGridSpec {
                ppo: 24,
                k_min: -135,
                k_max: 103,
            },
            smoothing: None,
            depth: ac2_proto::model::DepthPolicy::EqualConfidence,
        },
    }
}

impl Rig {
    pub fn start() -> Self {
        let fake = Arc::new(FakeDaemon::start(FakeOptions::default()).unwrap());
        {
            let mut s = fake.lock();
            s.grids.insert(TF_GRID.id(), TF_GRID);
            s.grids.insert(SPEC_GRID.id(), SPEC_GRID);
            let session = Session {
                epoch: SessionEpoch(2),
                open: Some(OpenSession {
                    config: SessionConfig {
                        backend: Some(BackendKind::Fake),
                        input_device: DeviceSelector::Default,
                        output_device: DeviceSelector::Default,
                        input_channels: vec![0, 1, 2, 3],
                        output_channels: 2,
                        sample_rate_hz: Some(48_000),
                        buffer_frames: Some(256),
                        loopback: Some(LoopbackRoute {
                            output: 1,
                            input: 0,
                        }),
                    },
                    backend: BackendKind::Fake,
                    input_device: DeviceId("fake:loop".into()),
                    output_device: DeviceId("fake:loop".into()),
                    sample_rate_hz: 48_000,
                    buffer_frames: 256,
                    clock: ClockRelation::SingleCallback,
                    opened_at: WallNs(0),
                }),
            };
            s.commit(Change::Session(session));
            // Main L tracks without a current finding: NO DELAY ESTIMATE is up.
            let tracking = DelayState {
                applied: Seconds(0.0125),
                applied_samples: Samples(600),
                tracking: true,
                awaiting_pick: false,
                last_finding: None,
            };
            let fixed = DelayState {
                applied: Seconds(0.0141),
                applied_samples: Samples(677),
                tracking: false,
                awaiting_pick: false,
                last_finding: None,
            };
            for m in [
                measurement(1, "Main L", transfer(1), Some(tracking), Some(&TF_GRID)),
                measurement(2, "Delay tower", transfer(2), Some(fixed), Some(&TF_GRID)),
                measurement(
                    3,
                    "Mic 1 FFT",
                    MeasKind::Spectrum {
                        config: SpectrumConfig {
                            input: 1,
                            fft_len: 4096,
                            window: Window::Hann,
                            averaging: SpecAveraging::Off,
                        },
                    },
                    None,
                    Some(&SPEC_GRID),
                ),
                measurement(
                    4,
                    "FOH SPL",
                    MeasKind::Spl {
                        config: SplConfig {
                            input: 1,
                            weighting: Weighting::A,
                            time_weighting: TimeWeighting::Fast,
                            peak_weighting: PeakWeighting::C,
                        },
                    },
                    None,
                    None,
                ),
            ] {
                s.commit(Change::Measurement(Patch::Set(m)));
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (f, st) = (fake.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let frames = [
                (
                    FrameData::Tf(tf_frame(1, 0.0, 0.0, 2000.0)),
                    Some(TF_GRID.id()),
                ),
                (
                    FrameData::Tf(tf_frame(2, -4.0, 0.000_35, 900.0)),
                    Some(TF_GRID.id()),
                ),
                (FrameData::Ir(ir_frame(1)), None),
                (FrameData::Spec(spec_frame(3)), Some(SPEC_GRID.id())),
                (FrameData::Spl(spl_frame(4)), None),
            ];
            let mut seq = 1;
            while !st.load(Ordering::Acquire) {
                {
                    let mut s = f.lock();
                    for (data, grid) in &frames {
                        let frame = Frame {
                            stamp: s.stamp(seq, *grid),
                            data: data.clone(),
                        };
                        s.publish(&frame);
                    }
                }
                seq += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Self {
            fake,
            stop,
            thread: Some(thread),
        }
    }
}
