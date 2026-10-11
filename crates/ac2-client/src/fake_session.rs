//! Session, server and input-setup commands of the fake daemon.

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, Command, ErrorCode, ProtoError, ReplyBody, Welcome};

use super::{Shared, err};

impl Shared {
    pub(super) fn session_cmd(
        &mut self,
        client: &ClientId,
        cmd: Command,
    ) -> Result<ReplyBody, ProtoError> {
        use Command as C;
        Ok(match cmd {
            C::Hello { .. } => ReplyBody::Welcome(Welcome {
                server: self.opts.server.clone(),
                client_id: client.clone(),
                daemon_incarnation: self.incarnation,
                session_epoch: self.state.session.epoch,
                rev: self.rev,
            }),
            C::SessionDevices => ReplyBody::Backends(self.backends.clone()),
            C::SessionPreview { backend, device } => {
                let channels = self
                    .backends
                    .iter()
                    .flat_map(|b| &b.devices)
                    .find(|d| d.backend == backend && d.id == device)
                    .and_then(|d| d.input.as_ref())
                    .map(|i| i.max_channels)
                    .ok_or_else(|| err(ErrorCode::NotFound, "no such device"))?;
                self.preview = Some((backend, device.clone()));
                ReplyBody::Preview(Preview {
                    backend,
                    device,
                    channels,
                    sample_rate_hz: 48_000,
                    expires_in_ms: 5000,
                })
            }
            C::SessionPreviewStop => {
                self.preview = None;
                ReplyBody::Ack { rev: self.rev }
            }
            C::SessionDetectLoopback {
                lease_token,
                backend,
                input_device,
                output_device,
                output,
                level,
            } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                let Some(level) = level else {
                    return Err(err(ErrorCode::Refused, "no level"));
                };
                if level.0 > self.state.generator.ceiling.0 {
                    return Err(err(ErrorCode::Refused, "level above ceiling"));
                }
                self.preview = None;
                ReplyBody::LoopbackDetection(LoopbackDetection {
                    backend,
                    input_device,
                    output_device,
                    output,
                    level,
                    ..self.detection.clone()
                })
            }
            C::SessionOpen { config } => {
                let dev = |s: &DeviceSelector| match s {
                    DeviceSelector::Default => DeviceId("fake:loop".into()),
                    DeviceSelector::Id { id } => id.clone(),
                };
                self.preview = None;
                let s = Session {
                    epoch: SessionEpoch(self.state.session.epoch.0 + 1),
                    open: Some(OpenSession {
                        backend: config.backend.unwrap_or(BackendKind::Fake),
                        input_device: dev(&config.input_device),
                        output_device: dev(&config.output_device),
                        sample_rate_hz: config.sample_rate_hz.unwrap_or(48_000),
                        buffer_frames: config.buffer_frames.unwrap_or(256),
                        clock: ClockRelation::SingleCallback,
                        opened_at: WallNs(self.now_ns()),
                        config,
                        replay: None,
                    }),
                    stopped: None,
                };
                self.commit(Change::Session(s.clone()));
                ReplyBody::Session(s)
            }
            C::SessionClose => {
                let s = Session {
                    epoch: SessionEpoch(self.state.session.epoch.0 + 1),
                    open: None,
                    stopped: None,
                };
                let rev = self.commit(Change::Session(s));
                ReplyBody::Ack { rev }
            }
            C::SessionStatus => ReplyBody::Session(self.state.session.clone()),
            C::SessionOutputs { outputs } => {
                for o in &outputs {
                    if let Some(l) = &o.label {
                        check_output_label(l).map_err(|e| err(ErrorCode::Invalid, e))?;
                    }
                }
                let mut all: Vec<OutputSetup> = self
                    .state
                    .outputs
                    .iter()
                    .filter(|c| outputs.iter().all(|r| r.channel != c.channel))
                    .cloned()
                    .collect();
                all.extend(outputs.into_iter().filter(|o| o.label.is_some()));
                all.sort_by_key(|o| o.channel);
                if all != self.state.outputs {
                    self.commit(Change::Outputs(all));
                }
                ReplyBody::Outputs(self.state.outputs.clone())
            }
            C::ServerInfo => ReplyBody::Server(ServerInfo {
                mode: ServerMode::Embedded,
                recording_dir: None,
            }),
            C::ServerAuthorize { .. } | C::ServerRevoke { .. } => {
                return Err(err(
                    ErrorCode::Unsupported,
                    "the fake daemon is not in network mode",
                ));
            }
            C::SessionReplay { .. } => {
                return Err(err(
                    ErrorCode::Unsupported,
                    "the fake daemon has no recordings to replay",
                ));
            }
            C::SessionInputs { inputs } => {
                for r in &inputs {
                    if let (Some(m), CurveChoice::Curve { label }) = (&r.mic, &r.curve)
                        && !self.state.inputs.contains(r)
                        && ac2_proto::cal::curve(&self.state.mics, m, label).is_none()
                    {
                        return Err(err(
                            ErrorCode::Invalid,
                            format!("no curve {label:?} stored for {m}"),
                        ));
                    }
                }
                let mut all: Vec<InputSetup> = self
                    .state
                    .inputs
                    .iter()
                    .filter(|c| inputs.iter().all(|r| r.channel != c.channel))
                    .cloned()
                    .collect();
                all.extend(inputs);
                self.set_inputs(all);
                ReplyBody::Inputs(self.state.inputs.clone())
            }
            _ => unreachable!("not a session command"),
        })
    }
}
