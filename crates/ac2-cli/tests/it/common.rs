//! What the rig tests share: a daemon on loopback ports of its own, and the CLI run in
//! process against it with its output captured.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use ac2_audio::Backend;
use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::{Client, ClientConfig, Endpoints};
use ac2_proto::model::{DeviceSelector, SessionConfig};
use ac2d::{BackendChoice, Daemon, DaemonConfig, Handle, Listen};
use clap::Parser;
use serde_json::Value;

/// Port 0: the OS picks, so tests running side by side never share a daemon.
pub fn local() -> Listen {
    Listen::Local {
        ctrl: "tcp://127.0.0.1:0".into(),
        data: "tcp://127.0.0.1:0".into(),
    }
}

pub fn fake_backend() -> Arc<dyn Backend> {
    ac2d::backend(BackendChoice::Fake).unwrap()
}

/// A daemon on `backend`, its config adjusted by `cfg` before the start.
pub fn rig_on(
    backend: Arc<dyn Backend>,
    cfg: impl FnOnce(&mut DaemonConfig),
) -> (Handle, Endpoints) {
    let mut config = DaemonConfig::new(backend, local(), -10.0);
    cfg(&mut config);
    let h = Daemon::start(config).unwrap();
    let ep = Endpoints {
        ctrl: h.ctrl_endpoint().to_owned(),
        data: h.data_endpoint().to_owned(),
    };
    (h, ep)
}

/// A daemon on the fake backend with the default config.
pub fn rig() -> (Handle, Endpoints) {
    rig_on(fake_backend(), |_| {})
}

/// A client that has the daemon's state.
pub async fn client(ep: &Endpoints, name: &str) -> Client {
    let c = Client::connect(ClientConfig::new(ep.clone(), name))
        .await
        .unwrap();
    c.wait_synced(Duration::from_secs(5)).await.unwrap();
    c
}

/// The default devices with `inputs` captured and two outputs, nothing else asked for.
pub fn session(inputs: Vec<u16>) -> SessionConfig {
    SessionConfig {
        backend: None,
        input_device: DeviceSelector::Default,
        output_device: DeviceSelector::Default,
        input_channels: inputs,
        spare_inputs: 0,
        output_channels: 2,
        sample_rate_hz: None,
        buffer_frames: None,
        loopback: None,
        remote_inputs: Vec::new(),
        link_offset: None,
    }
}

/// One finished `ac2` run.
pub struct Run {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    /// The exit code, and stdout followed by stderr.
    pub fn text(self) -> (i32, String) {
        (i32::from(self.code), self.stdout + &self.stderr)
    }

    /// stdout as the one JSON value a `--json` command prints.
    pub fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: {} {}", self.stdout, self.stderr))
    }
}

/// `ac2` with the arguments after the program name.
pub async fn cli(args: &[&str]) -> Run {
    let cli = Cli::try_parse_from(std::iter::once("ac2").chain(args.iter().copied())).unwrap();
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(cli.json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    Run {
        code,
        stdout: String::from_utf8(so).unwrap(),
        stderr: String::from_utf8(se).unwrap(),
    }
}

/// `ac2 [--json] <args>` against the daemon at `ep`.
pub async fn ac2(ep: &Endpoints, json: bool, args: &[&str]) -> Run {
    let to = ["--ctrl-endpoint", &ep.ctrl, "--data-endpoint", &ep.data];
    let argv: Vec<&str> = json
        .then_some("--json")
        .into_iter()
        .chain(to)
        .chain(args.iter().copied())
        .collect();
    cli(&argv).await
}

/// `ac2 --json <args>`: the exit code, and stdout followed by stderr.
pub async fn run_json(ep: &Endpoints, args: &[&str]) -> (i32, String) {
    ac2(ep, true, args).await.text()
}

/// `ac2 <args>` as text: the exit code, and stdout followed by stderr.
pub async fn run_plain(ep: &Endpoints, args: &[&str]) -> (i32, String) {
    ac2(ep, false, args).await.text()
}

/// The one (pretty) JSON document a command printed before any stderr text.
pub fn document(text: &str) -> Value {
    let mut de = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    de.next().expect("a document").expect("json")
}
