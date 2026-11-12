//! `--json` output of commands run in process against a real daemon on the simulated rig.

mod daemon;
mod gen_;
mod input;
mod kinds;
mod levels;
mod meas;
mod trace;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ac2_audio::fake::{FakeDrive, FakePath};
use ac2_audio::{FakeBackend, FakeConfig};
use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::{Client, ClientConfig, Endpoints, OnDrop, StimulusLease};
use ac2_proto::model::{GeneratorDesired, GeneratorSettings, Signal};
use ac2_proto::units::Dbfs;
use ac2d::Handle;
use clap::Parser;
use serde_json::Value;

use super::common::rig_on;

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A daemon on a fake device, its sessions saved in a directory of its own. The device is
/// stepped by a thread of the test as fast as the daemon reads it, so the seconds of audio
/// a steady calibration tone or a delay finding needs take less wall time than they cover.
struct Rig {
    h: Option<Handle>,
    ep: Endpoints,
    dir: tempfile::TempDir,
    stop: Arc<AtomicBool>,
    /// Device frames stepped so far.
    frames: Arc<AtomicU64>,
    rate: u32,
    driver: Option<JoinHandle<()>>,
}

impl Rig {
    /// A daemon on a device made from `config`.
    fn on(config: FakeConfig) -> R<Self> {
        let backend = FakeBackend::new(FakeConfig {
            drive: FakeDrive::Manual,
            ..config
        })?;
        let dir = tempfile::tempdir()?;
        let sessions = dir.path().join("sessions");
        let (h, ep) = rig_on(Arc::new(backend.clone()), |cfg| {
            cfg.session_dir = sessions;
            // A tone's lease is held across CLI runs by a client of the test; a stalled
            // refresher on a loaded machine must not fade it out mid-test.
            cfg.lease_expiry = Duration::from_secs(60);
        });
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let rate = backend.config().sample_rate;
        let driver = std::thread::spawn({
            let (stop, frames) = (stop.clone(), frames.clone());
            move || drive(&backend, &stop, &frames)
        });
        Ok(Self {
            h: Some(h),
            ep,
            dir,
            stop,
            frames,
            rate,
            driver: Some(driver),
        })
    }

    /// Returns once `seconds` more of device time have been stepped. The daemon's 1 s
    /// average of a tone switched on rises as `1 − e^(−t/1 s)`: 8 s on, it reads the tone's
    /// own level to within 0.002 dB.
    async fn run_for(&self, seconds: f64) -> R {
        let want = self.frames.load(Ordering::Relaxed) + (seconds * f64::from(self.rate)) as u64;
        let end = Instant::now() + Duration::from_secs(60);
        while self.frames.load(Ordering::Relaxed) < want {
            if Instant::now() > end {
                return Err(format!("{seconds} s of device time never came").into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }

    fn dir(&self) -> &std::path::Path {
        self.dir.path()
    }

    /// A client of the test that has the daemon's state.
    async fn client(&self) -> R<Client> {
        let c = Client::connect(ClientConfig::new(self.ep.clone(), "json-test")).await?;
        c.wait_synced(Duration::from_secs(5)).await?;
        Ok(c)
    }

    /// `signal` at `level` dBFS on `outputs` (0-based), played until the lease is dropped.
    async fn tone(
        &self,
        signal: Signal,
        level: f64,
        outputs: Vec<u16>,
    ) -> R<(Client, StimulusLease)> {
        let c = self.client().await?;
        let lease = c.acquire_lease(false, OnDrop::StopAndRelease).await?;
        set_tone(&lease, signal, level, outputs).await?;
        Ok((c, lease))
    }
}

/// `lease`'s generator playing `signal` at `level` dBFS on `outputs`.
async fn set_tone(lease: &StimulusLease, signal: Signal, level: f64, outputs: Vec<u16>) -> R {
    lease
        .set(GeneratorDesired {
            settings: GeneratorSettings {
                signal,
                level: Dbfs(level),
                band: None,
                outputs,
            },
            armed: true,
            firing: true,
        })
        .await?;
    Ok(())
}

impl Drop for Rig {
    fn drop(&mut self) {
        // The daemon closes its stream while the device still runs: a manual stream that is
        // never stepped would hold the shutdown up.
        drop(self.h.take());
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.driver.take() {
            let _ = t.join();
        }
    }
}

/// Steps the stream each session opens, a batch of blocks whenever the daemon has read the
/// last: a real device cannot outrun its reader, and an overflow would be audio lost that
/// no test asked for.
fn drive(backend: &FakeBackend, stop: &AtomicBool, frames: &AtomicU64) {
    // 85 ms of audio per batch: the daemon drains its capture ring (2 s) on its own wakeups,
    // so batches of a few blocks would pace the device near real time.
    const BLOCKS: u64 = 16;
    let block = u64::from(backend.config().block_frames);
    let mut current = None;
    while !stop.load(Ordering::Relaxed) {
        if let Some(d) = backend.take_driver() {
            current = Some(d);
        }
        match current.as_mut() {
            Some(d) if d.capture_queued() == 0 => {
                d.run_blocks(BLOCKS);
                frames.fetch_add(BLOCKS * block, Ordering::Relaxed);
            }
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

struct Run {
    code: u8,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> R<Value> {
        serde_json::from_str(&self.stdout)
            .map_err(|e| format!("{e}: stdout {:?} stderr {:?}", self.stdout, self.stderr).into())
    }
}

async fn ac2(f: &Rig, args: &[&str]) -> R<Run> {
    ac2_at(&f.ep, args).await
}

/// `ac2` against the daemon at `ep`, there or not.
async fn ac2_at(ep: &Endpoints, args: &[&str]) -> R<Run> {
    let argv: Vec<&str> = [
        "ac2",
        "--ctrl-endpoint",
        &ep.ctrl,
        "--data-endpoint",
        &ep.data,
    ]
    .into_iter()
    .chain(args.iter().copied())
    .collect();
    let cli = Cli::try_parse_from(argv)?;
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(cli.json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    Ok(Run {
        code,
        stdout: String::from_utf8(so)?,
        stderr: String::from_utf8(se)?,
    })
}

async fn ok_json(f: &Rig, args: &[&str]) -> R<Value> {
    let r = ac2(f, args).await?;
    if r.code != 0 {
        return Err(format!("{args:?} exited {}: {} {}", r.code, r.stdout, r.stderr).into());
    }
    r.json()
}

/// The daemon's own simulated rig (`ac2d::fake_rig`): out 1 loops back to in 1, reaches in
/// 2 6 dB down and in 3 through a hall; in 4 is quiet.
fn fake() -> R<Rig> {
    Rig::on(ac2d::fake_rig()?.config().clone())
}

/// Out 1 looped back to each of the four inputs: a tone reads at its own level on every one.
fn looped() -> R<Rig> {
    Rig::on(FakeConfig {
        paths: (0..4).map(|i| FakePath::loopback(0, i, 32)).collect(),
        ..FakeConfig::default()
    })
}
