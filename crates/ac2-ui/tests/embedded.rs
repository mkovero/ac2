//! The embedded daemon (no window, no GPU): `start_embedded` on the fake rig serves a
//! client that syncs, and a transfer measurement driven by the fake generator publishes
//! frames that show the rig's acoustic path.
#![cfg(feature = "embedded")]

use std::time::{Duration, Instant};

use ac2_client::{Client, ClientConfig, OnDrop};
use ac2_proto::model::{
    DepthPolicy, DeviceSelector, GeneratorDesired, GeneratorSettings, LogGridSpec, LoopbackRoute,
    MeasConfig, MeasKind, SessionConfig, Signal, TfAveraging, TransferConfig,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{Dbfs, MeasId, Seconds};
use ac2_proto::{Command, FrameData, ReplyBody, Subscription};
use ac2_ui::embedded::{EmbeddedBackend, start_embedded};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

const DEADLINE: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread")]
async fn embedded_fake_rig_serves_a_transfer_measurement() -> R {
    let daemon = start_embedded(EmbeddedBackend::Fake)?;
    assert_eq!(daemon.describe(), "embedded daemon (fake rig)");
    let ep = daemon.endpoints();
    #[cfg(unix)]
    assert!(ep.ctrl.starts_with("ipc://"), "{ep:?}");
    #[cfg(not(unix))]
    assert!(ep.ctrl.starts_with("tcp://127.0.0.1:"), "{ep:?}");

    let c = Client::connect(ClientConfig::new(ep, "ac2-ui embedded test")).await?;
    let state = c.wait_synced(DEADLINE).await?;
    assert!(state.measurements.is_empty());
    assert_eq!(c.view().client_id.as_ref(), Some(&c.client_id()));

    c.call(Command::SessionOpen {
        config: SessionConfig {
            input_device: DeviceSelector::Default,
            output_device: DeviceSelector::Default,
            input_channels: vec![0, 1],
            output_channels: 2,
            sample_rate_hz: Some(48_000),
            buffer_frames: Some(256),
            loopback: Some(LoopbackRoute {
                output: 0,
                input: 0,
            }),
        },
    })
    .await?;
    let meas = match c
        .call(Command::MeasCreate {
            config: MeasConfig {
                name: "main".into(),
                kind: MeasKind::Transfer {
                    config: TransferConfig {
                        reference_input: 0,
                        measurement_input: 1,
                        averaging: TfAveraging::Fifo { blocks: 4 },
                        grid: LogGridSpec {
                            ppo: 24,
                            k_min: -120,
                            k_max: 119,
                        },
                        smoothing: None,
                        depth: DepthPolicy::FastLf {
                            max_settle_s: Seconds(1.0),
                        },
                    },
                },
            },
        })
        .await?
    {
        ReplyBody::Measurement(m) => m.id,
        other => return Err(format!("meas.create: {other:?}").into()),
    };
    c.subscribe(Subscription::Meas(meas))?;
    c.call(Command::MeasStart { meas }).await?;

    // The fake rig's generator: pink noise into its simulated paths (no real audio).
    let lease = c.acquire_lease(false, OnDrop::StopAndRelease).await?;
    lease
        .set(GeneratorDesired {
            settings: GeneratorSettings {
                signal: Signal::Pink,
                level: Dbfs(-20.0),
                band: None,
                outputs: vec![0],
            },
            armed: true,
            firing: true,
        })
        .await?;

    // Frames arrive, and once averaged, the mid band shows the path's −6 dB.
    let topic = Topic::Data {
        meas: MeasId(meas.0),
        stream: Stream::Tf,
    };
    let end = Instant::now() + DEADLINE;
    let mut frames = 0;
    let mut seen_gain = None;
    while Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let latest = c.latest()?;
        let Some(f) = latest.get(&topic) else {
            continue;
        };
        frames += 1;
        let FrameData::Tf(tf) = &f.frame.data else {
            return Err("tf topic carried another kind".into());
        };
        assert_eq!(tf.mag.len(), 240);
        // Column 120 is 1 kHz on this grid.
        let m = tf.mag[120];
        if m.is_finite() && (m - (-6.02)).abs() < 0.5 {
            seen_gain = Some(m);
            break;
        }
    }
    assert!(frames > 0, "no tf frame from the embedded daemon");
    assert!(
        seen_gain.is_some(),
        "the fake rig's −6 dB path never showed at 1 kHz"
    );
    lease.end().await?;
    drop(c);
    drop(daemon);
    Ok(())
}
