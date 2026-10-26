//! `levels.set` and `levels.reset`: the programme meters' settings, a rig edit like the cue
//! targets, and the reset of their holds.

use ac2_proto::model::{Ballistic, CorrelationWindow, LevelsSettings};
use ac2_proto::units::{Dbfs, Dbtp, Lufs, Rev};
use ac2_proto::{ErrorCode, ProtoError, ReplyBody};

use super::Control;
use crate::rig::RigSettings;
use crate::util::perr;

impl Control {
    /// `levels.set`: each field given replaces the rig's; the settings file is written
    /// before the state changes, so a restart never shows a ballistic the file lost.
    pub(super) fn levels_set(
        &mut self,
        base: Rev,
        ballistic: Option<Ballistic>,
        correlation_window: Option<CorrelationWindow>,
        over_threshold: Option<Dbtp>,
        correlation_gate: Option<Dbfs>,
        loudness_target: Option<Lufs>,
    ) -> Result<ReplyBody, ProtoError> {
        self.rig_base(base)?;
        let now = self.store.state().rig.levels;
        let levels = LevelsSettings {
            ballistic: ballistic.unwrap_or(now.ballistic),
            correlation_window: correlation_window.unwrap_or(now.correlation_window),
            over_threshold: over_threshold.unwrap_or(now.over_threshold),
            correlation_gate: correlation_gate.unwrap_or(now.correlation_gate),
            loudness_target: loudness_target.unwrap_or(now.loudness_target),
        };
        levels.check().map_err(|e| perr(ErrorCode::Invalid, e))?;
        if levels == now {
            return Ok(ReplyBody::Ack {
                rev: self.store.rev(),
            });
        }
        self.rig.settings.persist(&RigSettings {
            levels,
            ..self.rig_settings_now()
        })?;
        tracing::info!(
            "programme meters: {}, correlation over {}, over above {} dBTP, gate {} dBFS, \
             loudness target {} LUFS",
            levels.ballistic.word(),
            levels.correlation_window.text(),
            levels.over_threshold.0,
            levels.correlation_gate.0,
            levels.loudness_target.0
        );
        let rev = self.commit_rig(|r| r.levels = levels);
        Ok(ReplyBody::Ack { rev })
    }

    /// `levels.reset`: the session meters and the preview clear the max holds and over
    /// counts of `channels` (empty: every input) with their next frame.
    pub(super) fn levels_reset(&mut self, channels: Vec<u16>) -> Result<ReplyBody, ProtoError> {
        if channels.is_empty() {
            tracing::info!("programme meters reset");
        } else {
            tracing::info!("programme meters reset on inputs {channels:?}");
        }
        self.levels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset(channels);
        Ok(ReplyBody::Ack {
            rev: self.store.rev(),
        })
    }

    /// Hands the programme meters what they read of the state.
    pub(super) fn sync_levels_policy(&self) {
        let st = self.store.state();
        let mut p = self.levels.lock().unwrap_or_else(|e| e.into_inner());
        p.settings = st.rig.levels;
        p.declare(&st.inputs, &st.pairs);
        if p.line_calibrations != st.line_calibrations {
            p.line_calibrations = st.line_calibrations.clone();
        }
    }
}
