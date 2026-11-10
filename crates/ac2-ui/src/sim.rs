//! The simulated rig: an `ac2d` on the fake backend that keeps nothing
//! ([`crate::spawn::spawn_simulated`]), made ready to use by the app as a client.
//!
//! Its session is opened (in 1–2, out 1, the loopback out 1 → in 1 as the rig is wired) and
//! a transfer measurement "demo" (ref 1 → meas 2) is created and started. A daemon on real
//! audio starts with no session: which interface and channels to open is the operator's
//! choice, made in the session dialog.

use ac2_client::ClientConfig;
use ac2_proto::model::{
    DeviceSelector, LoopbackRoute, MeasConfig, MeasKind, SessionConfig, TransferConfig,
};

/// What a simulated rig has when the app (or a test) starts using it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    /// No session: the operator opens one, as on real audio.
    Empty,
    /// The session open and the "demo" measurement running.
    Demo,
}

/// The session the simulated rig opens: inputs 1–2, output 1, loopback out 1 → in 1.
pub fn demo_session() -> SessionConfig {
    SessionConfig {
        backend: Some(ac2_proto::model::BackendKind::Fake),
        input_device: DeviceSelector::Default,
        output_device: DeviceSelector::Default,
        input_channels: vec![0, 1],
        spare_inputs: 0,
        output_channels: 1,
        sample_rate_hz: None,
        buffer_frames: None,
        loopback: Some(LoopbackRoute {
            output: 0,
            input: 0,
        }),
        remote_inputs: Vec::new(),
        link_offset: None,
    }
}

/// The measurement the simulated rig starts: transfer ref 1 → meas 2 named "demo", with the
/// defaults of `ac2 meas new tf`.
pub fn demo_measurement() -> MeasConfig {
    MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: "demo".into(),
        kind: MeasKind::Transfer {
            config: TransferConfig::with_inputs(0, 1),
        },
    }
}

/// Opens the simulated rig's session and starts "demo", as a client of the daemon at
/// `config`. Runs on its own thread and runtime, so it works whether or not the caller is
/// inside one.
pub fn set_up_demo(config: ClientConfig) -> Result<(), String> {
    use ac2_client::{Client, expect_body};
    use ac2_proto::{Command, ReplyBody};
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
    let run = move || -> Result<(), String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async move {
            let work = async {
                let c = Client::connect(config).await?;
                c.call(Command::SessionOpen {
                    config: demo_session(),
                })
                .await?;
                let r = c
                    .call(Command::MeasCreate {
                        config: demo_measurement(),
                    })
                    .await?;
                let m = expect_body!("meas.create", r, ReplyBody::Measurement(m) => m)?;
                c.call(Command::MeasStart { meas: m.id }).await?;
                Ok::<(), ac2_client::ClientError>(())
            };
            match tokio::time::timeout(DEADLINE, work).await {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err("the daemon did not answer".into()),
            }
        })
    };
    std::thread::Builder::new()
        .name("ac2-ui simulated rig setup".into())
        .spawn(run)
        .map_err(|e| format!("simulated rig setup: {e}"))?
        .join()
        .map_err(|_| "simulated rig setup: thread panicked".to_owned())?
        .map_err(|e| format!("simulated rig setup: {e}"))
}
