//! Generator commands of the fake daemon: bookkeeping and the stimulus lease, never audio.

use std::time::Instant;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ErrorDetail, ProtoError, ReplyBody};

use super::{LeaseSlot, Shared, err};

impl Shared {
    pub(super) fn generator_cmd(
        &mut self,
        client: &ClientId,
        cmd: Command,
    ) -> Result<ReplyBody, ProtoError> {
        use Command as C;
        Ok(match cmd {
            C::GenAcquire { force } => {
                self.expire_lease();
                if let Some(l) = &self.lease
                    && &l.owner != client
                    && !force
                {
                    return Err(ProtoError {
                        code: ErrorCode::LeaseHeld,
                        msg: format!("held by {}", l.owner.0),
                        detail: Some(ErrorDetail::LeaseHeld {
                            owner: l.owner.clone(),
                        }),
                    });
                }
                let forced = self.lease.is_some();
                self.next_token += 1;
                let token = LeaseToken(self.next_token);
                self.lease = Some(LeaseSlot {
                    token,
                    owner: client.clone(),
                    deadline: Instant::now() + self.opts.lease_expiry,
                });
                self.state.generator.owner = Some(client.clone());
                self.state.generator.armed = false;
                self.state.generator.firing = false;
                let action = if forced {
                    GenAction::Force
                } else {
                    GenAction::Acquire
                };
                self.generator_changed(action, Some(client.clone()));
                ReplyBody::Lease(Lease {
                    lease_token: token,
                    expires_in_ms: self.opts.lease_expiry.as_millis() as u32,
                })
            }
            C::GenSet {
                lease_token,
                desired,
            } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                if desired.firing && !desired.armed {
                    return Err(err(ErrorCode::Refused, "firing requires armed"));
                }
                if desired.settings.level.0 > self.state.generator.ceiling.0 {
                    return Err(err(ErrorCode::Refused, "level above ceiling"));
                }
                self.refresh();
                let action = if desired.firing {
                    GenAction::Fire
                } else if desired.armed {
                    GenAction::Arm
                } else {
                    GenAction::Set
                };
                self.state.generator.armed = desired.armed;
                self.state.generator.firing = desired.firing;
                self.state.generator.settings = Some(desired.settings);
                self.generator_changed(action, Some(client.clone()));
                ReplyBody::Generator(self.state.generator.clone())
            }
            C::GenRefresh { lease_token } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                self.refresh();
                self.refreshes += 1;
                ReplyBody::Lease(Lease {
                    lease_token,
                    expires_in_ms: self.opts.lease_expiry.as_millis() as u32,
                })
            }
            C::GenRelease { lease_token } => {
                self.expire_lease();
                self.check_lease(lease_token)?;
                self.lease = None;
                self.state.generator.owner = None;
                self.state.generator.armed = false;
                self.state.generator.firing = false;
                self.generator_changed(GenAction::Release, Some(client.clone()));
                ReplyBody::Ack { rev: self.rev }
            }
            C::GenStop => {
                self.state.generator.armed = false;
                self.state.generator.firing = false;
                self.generator_changed(GenAction::Stop, Some(client.clone()));
                ReplyBody::Ack { rev: self.rev }
            }
            C::GenCeiling {
                ceiling,
                confirm_raise,
            } => {
                let g = &self.state.generator;
                if !ceiling.0.is_finite() || ceiling.0 > g.ceiling_bound.0 {
                    return Err(err(ErrorCode::Invalid, "above the bound"));
                }
                let action = if ceiling.0 > g.ceiling.0 {
                    if !confirm_raise {
                        return Err(err(ErrorCode::Refused, "raising needs a confirmation"));
                    }
                    if g.armed || g.firing {
                        return Err(err(ErrorCode::Refused, "armed: stop first"));
                    }
                    GenAction::CeilingRaised
                } else {
                    let above = g.settings.as_ref().is_some_and(|s| s.level.0 > ceiling.0);
                    if above && (g.armed || g.firing) {
                        self.state.generator.armed = false;
                        self.state.generator.firing = false;
                    }
                    GenAction::CeilingLowered
                };
                self.state.generator.ceiling = ceiling;
                self.generator_changed(action, Some(client.clone()));
                ReplyBody::Generator(self.state.generator.clone())
            }
            _ => unreachable!("not a generator command"),
        })
    }
}
