//! `sacn.target`: where the rig's DMX universe goes as sACN (E1.31), beside Art-Net. The
//! frame, its fade, snaps and test frames are the cue engine's (`super::artnet`); this sets
//! the destination only.

use std::time::Instant;

use ac2_proto::model::{SacnDest, SacnTarget};
use ac2_proto::units::Rev;
use ac2_proto::{ProtoError, ReplyBody};

use super::Control;
use crate::dmx::sacn::check_target;
use crate::rig::RigSettings;
use crate::util::invalid;

impl Control {
    /// `sacn.target`. A changed or removed target ends the old stream first
    /// (Stream_Terminated), so its receivers let go of the rig at once.
    pub(super) fn sacn_target(
        &mut self,
        base: Rev,
        target: Option<SacnTarget>,
    ) -> Result<ReplyBody, ProtoError> {
        self.rig_base(base)?;
        let old = self.store.state().rig.sacn_target;
        if old == target {
            return Ok(ReplyBody::Ack {
                rev: self.store.rev(),
            });
        }
        if let Some(t) = &target {
            check_target(t).map_err(invalid)?;
        }
        // Asked before anything is kept: an interface the host does not have is the
        // operator's to correct now, not a send error found later.
        if let Some(t) = &target {
            self.rig.artnet.sacn.check_dest(t.dest).map_err(invalid)?;
        }
        // Kept first: a target that would come back different after a restart would send
        // the lights' levels somewhere the operator did not choose.
        self.rig.settings.persist(&RigSettings {
            sacn_target: target,
            ..self.rig_settings_now()
        })?;
        tracing::info!(
            "sACN target {}",
            target.map_or_else(|| "off".to_owned(), |t| text(&t))
        );
        if let Some(o) = old {
            let last = self.rig.artnet.output_now(Instant::now());
            self.rig.artnet.sacn.terminate(&o, &last);
        }
        let others = self.dmx_targets().artnet.is_some();
        let rev = self.commit_rig(|r| r.sacn_target = target);
        if target.is_some() {
            self.dmx_target_added(others);
        } else {
            self.artnet_sync(Instant::now());
        }
        self.commit_dmx_status(true);
        Ok(ReplyBody::Ack { rev })
    }
}

/// The target as the log says it.
fn text(t: &SacnTarget) -> String {
    let to = match t.dest {
        SacnDest::Unicast { addr } => addr.to_string(),
        SacnDest::Multicast { interface } => {
            format!("multicast {} via {interface}", t.universe.multicast_group())
        }
    };
    format!("{to} universe {} priority {}", t.universe, t.priority)
}
