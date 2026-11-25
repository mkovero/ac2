//! The delay and delay-finding entities of transfer measurements, committed apart from the
//! measurement so a tracking step is one small event, and in the order a client can follow:
//! a transfer measurement comes into being before its delay and goes after it.

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{Delay, DelayFinding, MeasKind, Measurement};
use ac2_proto::units::{MeasId, Rev};
use ac2_proto::{ProtoError, ReplyBody};

use super::Control;
use crate::util::invalid;

impl Control {
    pub(super) fn delay_of(&self, meas: MeasId) -> Option<&Delay> {
        self.store.state().delay(meas)
    }

    /// The delay of transfer measurement `id`; refused for any other measurement.
    pub(super) fn transfer_delay(&self, id: MeasId) -> Result<&Delay, ProtoError> {
        let m = self.meas(id)?;
        let not_transfer = || invalid(format!("measurement {} is not a transfer function", id.0));
        if !matches!(m.config.kind, MeasKind::Transfer { .. }) {
            return Err(not_transfer());
        }
        self.delay_of(id).ok_or_else(not_transfer)
    }

    /// The delay a job of `m` runs with: the stored one, or for a measurement just made a
    /// transfer (its delay is committed after it) none applied under its `config_rev`.
    pub(super) fn job_delay(&self, m: &Measurement) -> Option<Delay> {
        matches!(m.config.kind, MeasKind::Transfer { .. }).then(|| {
            self.delay_of(m.id)
                .cloned()
                .unwrap_or_else(|| Delay::zero(m.id, m.config_rev))
        })
    }

    /// The stamp a job of `m` starts with: the newer of its `config_rev` and its delay's
    /// `rev`, so a frame says in one number which configuration and which delay it shows.
    pub(super) fn job_stamp(&self, m: &Measurement) -> Rev {
        self.job_delay(m)
            .map_or(m.config_rev, |d| d.rev.max(m.config_rev))
    }

    /// Commits `d` and answers with it as stored.
    pub(super) fn reply_delay(&mut self, d: Delay) -> ReplyBody {
        self.commit(Change::Delay(Patch::Set(d.clone())));
        ReplyBody::Delay(d)
    }

    pub(super) fn commit_finding(&mut self, f: DelayFinding) {
        self.commit(Change::DelayFinding(Patch::Set(f)));
    }

    /// Drops the finding of `meas`, if one stands.
    pub(super) fn drop_finding(&mut self, meas: MeasId) {
        if self.store.state().delay_finding(meas).is_some() {
            self.commit(Change::DelayFinding(Patch::Deleted(meas)));
        }
    }

    /// Deletes the finding and the delay of `meas`, in that order, where they stand: ahead
    /// of the measurement's deletion, or of its commit as no longer a transfer.
    pub(super) fn drop_delay(&mut self, meas: MeasId) {
        self.drop_finding(meas);
        if self.delay_of(meas).is_some() {
            self.commit(Change::Delay(Patch::Deleted(meas)));
        }
    }

    /// Commits `m` and then, for a transfer measurement without one, its delay `d` (none
    /// applied unless given) under `m`'s `config_rev`: the stamp of a job started ahead of
    /// both commits then stays the same once both are in. Returns `m` as committed.
    pub(super) fn commit_meas_with_delay(
        &mut self,
        m: Measurement,
        d: Option<Delay>,
    ) -> Result<Measurement, ProtoError> {
        let id = m.id;
        let transfer = matches!(m.config.kind, MeasKind::Transfer { .. });
        if !transfer {
            self.drop_delay(id);
        }
        let config_rev = m.config_rev;
        self.commit(Change::Measurement(Patch::Set(m)));
        if transfer && self.delay_of(id).is_none() {
            let d = d.map_or_else(
                || Delay::zero(id, config_rev),
                |d| Delay {
                    meas: id,
                    rev: config_rev,
                    ..d
                },
            );
            self.commit(Change::Delay(Patch::Set(d)));
        }
        Ok(self.meas(id)?.clone())
    }
}
