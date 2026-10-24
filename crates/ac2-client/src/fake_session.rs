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
            C::SessionPreview {
                backend,
                device,
                inputs,
                pairs,
            } => {
                let channels = self
                    .backends
                    .iter()
                    .flat_map(|b| &b.devices)
                    .find(|d| d.backend == backend && d.id == device)
                    .and_then(|d| d.input.as_ref())
                    .map(|i| i.max_channels)
                    .ok_or_else(|| err(ErrorCode::NotFound, "no such device"))?;
                let rows: Vec<InputSetup> = inputs
                    .iter()
                    .map(|d| InputSetup {
                        channel: d.channel,
                        kind: d.kind.clone(),
                        pair: None,
                    })
                    .collect();
                let captured = |ch: u16| ch < channels;
                ac2_proto::cal::check_pairs(&rows, &pairs, Some(&captured))
                    .map_err(|e| err(ErrorCode::Invalid, e))?;
                self.preview = Some((backend, device.clone()));
                self.preview_decl = Some(super::levels::PreviewDecl {
                    backend,
                    device: device.clone(),
                    channels,
                    inputs,
                    pairs,
                });
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
                self.preview_decl = None;
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
                self.preview_decl = None;
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
                self.preview_decl = None;
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
                        clock_source: None,
                        driver_change: None,
                        driver_reset: None,
                        stated_latency: None,
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
            C::SessionControlPanel { backend, .. } => {
                if backend != BackendKind::Asio {
                    return Err(err(
                        ErrorCode::Unsupported,
                        format!("{backend:?} has no driver control panel"),
                    ));
                }
                ReplyBody::Ack { rev: self.rev }
            }
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
            C::SessionInputs { inputs, pairs } => {
                for r in &inputs {
                    if let InputKind::Mic {
                        mic: Some(m),
                        curve: CurveChoice::Curve { label },
                    } = &r.kind
                        && !self.state.inputs.iter().any(|x| x.kind == r.kind)
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
                let captured = self
                    .state
                    .session
                    .open
                    .as_ref()
                    .map(|o| o.config.input_channels.clone());
                let captured_fn = |ch: u16| captured.as_ref().is_some_and(|c| c.contains(&ch));
                ac2_proto::cal::check_pairs(
                    &all,
                    &pairs,
                    captured
                        .is_some()
                        .then_some(&captured_fn as &dyn Fn(u16) -> bool),
                )
                .map_err(|e| err(ErrorCode::Invalid, e))?;
                let mut pairs = pairs;
                pairs.sort_by_key(|p| p.left);
                let all = ac2_proto::cal::apply_pair_sides(&all, &pairs);
                self.set_inputs_and_pairs(all, pairs);
                ReplyBody::Inputs(InputSetups {
                    inputs: self.state.inputs.clone(),
                    pairs: self.state.pairs.clone(),
                })
            }
            _ => unreachable!("not a session command"),
        })
    }
}
