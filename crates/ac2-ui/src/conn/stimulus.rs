//! The stimulus task: owns the generator lease and runs its ops in order.

use super::*;

pub(super) enum StimOp {
    Arm {
        settings: GeneratorSettings,
        force: bool,
    },
    Set(GeneratorDesired),
    Stop,
    Detect(crate::session_dialog::DetectRequest),
    Sweep {
        meas: MeasId,
        /// With the `config_rev` it was made on.
        update: Option<(ac2_proto::units::Rev, Box<ac2_proto::model::MeasConfig>)>,
    },
}

/// Owns the lease. Ops run strictly in order, so a stop can never overtake an arm.
pub(super) async fn stimulus_task(
    client: Client,
    mut ops: mpsc::UnboundedReceiver<StimOp>,
    out: Out,
) {
    let mut lease: Option<StimulusLease> = None;
    let mut check = tokio::time::interval(Duration::from_millis(100));
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            op = ops.recv() => {
                let Some(op) = op else { break };
                let ev = match op {
                    StimOp::Arm { settings, force } => arm(&client, &mut lease, settings, force).await,
                    StimOp::Set(d) => match &lease {
                        Some(l) => {
                            let firing = d.firing;
                            match l.set(d).await {
                                Ok(_) => StimEvent::Set { firing },
                                Err(e) => StimEvent::Failed(e.to_string()),
                            }
                        }
                        None => StimEvent::Failed("no stimulus lease held".into()),
                    },
                    StimOp::Stop => stop(&client, &mut lease).await,
                    StimOp::Sweep { meas, update } => match &lease {
                        Some(l) => match async {
                            if let Some((base, config)) = update {
                                client
                                    .call(Command::MeasUpdate {
                                        meas,
                                        base,
                                        config: *config,
                                    })
                                    .await?;
                            }
                            client
                                .call(Command::SweepRun {
                                    lease_token: l.token(),
                                    meas,
                                    name: None,
                                })
                                .await
                                .and_then(|r| expect_body!("sweep.run", r, ReplyBody::Sweep(s) => s))
                        }
                        .await
                        {
                            Ok(run) => StimEvent::SweepStarted(Box::new(run)),
                            Err(e) => StimEvent::Failed(e.to_string()),
                        },
                        None => StimEvent::Failed("no stimulus lease held".into()),
                    },
                    StimOp::Detect(d) => {
                        let r = detect(&client, &mut lease, d).await;
                        out.send(ConnEvent::LoopbackDetected(r));
                        continue;
                    }
                };
                out.send(ConnEvent::Stimulus(ev));
            }
            // Only a held lease can be lost.
            _ = check.tick(), if lease.is_some() => {
                if let Some(lost) = lease.as_ref().and_then(StimulusLease::lost) {
                    lease = None;
                    let ac2_client::LeaseLost::Refused { msg, .. } = lost;
                    out.send(ConnEvent::Stimulus(StimEvent::Lost(msg)));
                }
            }
        }
    }
    if let Some(l) = lease.take() {
        let _ = l.end().await;
    }
}

pub(super) async fn arm(
    client: &Client,
    lease: &mut Option<StimulusLease>,
    settings: GeneratorSettings,
    force: bool,
) -> StimEvent {
    if lease.is_none() {
        match client.acquire_lease(force, OnDrop::StopAndRelease).await {
            Ok(l) => *lease = Some(l),
            Err(e) => return StimEvent::Failed(e.to_string()),
        }
    }
    let Some(l) = lease.as_ref() else {
        return StimEvent::Failed("no stimulus lease held".into());
    };
    match l
        .set(GeneratorDesired {
            settings,
            armed: true,
            firing: false,
        })
        .await
    {
        Ok(_) => StimEvent::Armed,
        Err(e) => StimEvent::Failed(e.to_string()),
    }
}

/// Runs a loopback detection under the lease: the held one, else one taken for it (never by
/// force) and released after.
pub(super) async fn detect(
    client: &Client,
    lease: &mut Option<StimulusLease>,
    d: crate::session_dialog::DetectRequest,
) -> Result<LoopbackDetection, String> {
    let taken = if lease.is_none() {
        Some(
            client
                .acquire_lease(false, OnDrop::StopAndRelease)
                .await
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let token = match (&taken, lease.as_ref()) {
        (Some(l), _) | (None, Some(l)) => l.token(),
        (None, None) => return Err("no stimulus lease".into()),
    };
    let r = client
        .call(Command::SessionDetectLoopback {
            lease_token: token,
            backend: d.backend,
            input_device: d.device,
            output_device: d.output_device,
            output: d.output,
            level: Some(d.level),
        })
        .await
        .and_then(
            |r| expect_body!("session.detect_loopback", r, ReplyBody::LoopbackDetection(x) => x),
        )
        .map_err(|e| e.to_string());
    if let Some(l) = taken {
        let _ = l.end().await;
    }
    r
}

pub(super) async fn stop(client: &Client, lease: &mut Option<StimulusLease>) -> StimEvent {
    let r = match lease.take() {
        Some(l) => l.end().await,
        None => client.call(Command::GenStop).await.map(drop),
    };
    match r {
        Ok(()) => StimEvent::Stopped,
        Err(e) => StimEvent::Failed(format!("stop: {e}")),
    }
}
