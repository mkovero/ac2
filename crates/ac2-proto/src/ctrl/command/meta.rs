//! A command's wire name, mutation flag and lease token.

use super::Command;

use crate::units::LeaseToken;

impl Command {
    /// Wire name (`op`).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::SessionDevices => "session.devices",
            Self::SessionPreview { .. } => "session.preview",
            Self::SessionPreviewStop => "session.preview_stop",
            Self::SessionControlPanel { .. } => "session.control_panel",
            Self::SessionDetectLoopback { .. } => "session.detect_loopback",
            Self::SessionOpen { .. } => "session.open",
            Self::SessionClose => "session.close",
            Self::SessionStatus => "session.status",
            Self::SessionInputs { .. } => "session.inputs",
            Self::SessionOutputs { .. } => "session.outputs",
            Self::GenAcquire { .. } => "gen.acquire",
            Self::GenSet { .. } => "gen.set",
            Self::GenRefresh { .. } => "gen.refresh",
            Self::GenRelease { .. } => "gen.release",
            Self::GenStop => "gen.stop",
            Self::GenCeiling { .. } => "gen.ceiling",
            Self::OscTarget { .. } => "osc.target",
            Self::OscTest { .. } => "osc.test",
            Self::ArtnetTarget { .. } => "artnet.target",
            Self::ArtnetFade { .. } => "artnet.fade",
            Self::ArtnetTest { .. } => "artnet.test",
            Self::RigMonitorStart { .. } => "rig.monitor_start",
            Self::LevelsSet { .. } => "levels.set",
            Self::LevelsReset { .. } => "levels.reset",
            Self::MeasCreate { .. } => "meas.create",
            Self::MeasUpdate { .. } => "meas.update",
            Self::MeasColour { .. } => "meas.colour",
            Self::MeasCompare { .. } => "meas.compare",
            Self::MeasOffset { .. } => "meas.offset",
            Self::MeasLayout { .. } => "meas.layout",
            Self::MeasDelete { .. } => "meas.delete",
            Self::MeasEq { .. } => "meas.eq",
            Self::MeasStart { .. } => "meas.start",
            Self::MeasStop { .. } => "meas.stop",
            Self::MeasReset { .. } => "meas.reset",
            Self::DelayFind { .. } => "delay.find",
            Self::DelayInsert { .. } => "delay.insert",
            Self::DelaySet { .. } => "delay.set",
            Self::DelayNudge { .. } => "delay.nudge",
            Self::DelayTrack { .. } => "delay.track",
            Self::TraceCapture { .. } => "trace.capture",
            Self::TraceList => "trace.list",
            Self::TraceGet { .. } => "trace.get",
            Self::TraceUpdate { .. } => "trace.update",
            Self::TraceDelete { .. } => "trace.delete",
            Self::TraceAverage { .. } => "trace.average",
            Self::TraceImport { .. } => "trace.import",
            Self::TraceTarget { .. } => "trace.target",
            Self::TraceMicCurve { .. } => "trace.mic_curve",
            Self::TraceExport { .. } => "trace.export",
            Self::CalSpl { .. } => "cal.spl",
            Self::CalSplElectrical { .. } => "cal.spl_electrical",
            Self::CalCurveImport { .. } => "cal.curve_import",
            Self::CalCurveRename { .. } => "cal.curve_rename",
            Self::CalCurveDelete { .. } => "cal.curve_delete",
            Self::CalLine { .. } => "cal.line",
            Self::CalList => "cal.list",
            Self::CalDelete { .. } => "cal.delete",
            Self::SplLogGet { .. } => "spl.log_get",
            Self::SplLogNew { .. } => "spl.log_new",
            Self::SplBandTransfer { .. } => "spl.band_transfer",
            Self::SplBandLogGet { .. } => "spl.band_log_get",
            Self::SplHistoryGet { .. } => "spl.history_get",
            Self::SweepRun { .. } => "sweep.run",
            Self::StateSnapshot => "state.snapshot",
            Self::StateSince { .. } => "state.since",
            Self::GridGet { .. } => "grid.get",
            Self::FileSave { .. } => "file.save",
            Self::FileLoad { .. } => "file.load",
            Self::FileList => "file.list",
            Self::FileNew { .. } => "file.new",
            Self::FileRecover => "file.recover",
            Self::FileSalvage { .. } => "file.salvage",
            Self::FileUploadBegin { .. } => "file.upload_begin",
            Self::FileUploadChunk { .. } => "file.upload_chunk",
            Self::FileImport { .. } => "file.import",
            Self::FileDownloadBegin { .. } => "file.download_begin",
            Self::FileDownloadChunk { .. } => "file.download_chunk",
            Self::FileDownloadEnd { .. } => "file.download_end",
            Self::RecStart { .. } => "rec.start",
            Self::RecStop => "rec.stop",
            Self::RecList => "rec.list",
            Self::SessionReplay { .. } => "session.replay",
            Self::ServerInfo => "server.info",
            Self::ServerAuthorize { .. } => "server.authorize",
            Self::ServerRevoke { .. } => "server.revoke",
            Self::PlanSave { .. } => "plan.save",
            Self::PlanList => "plan.list",
            Self::PlanDelete { .. } => "plan.delete",
            Self::WalkNew { .. } => "walk.new",
            Self::WalkUpdate { .. } => "walk.update",
            Self::WalkGoto { .. } => "walk.goto",
            Self::WalkCapture { .. } => "walk.capture",
            Self::WalkSkip { .. } => "walk.skip",
            Self::WalkRedo { .. } => "walk.redo",
            Self::WalkSwap { .. } => "walk.swap",
            Self::WalkDelete { .. } => "walk.delete",
            Self::SimRun { .. } => "sim.run",
            Self::SimFit { .. } => "sim.fit",
            Self::SimCancel => "sim.cancel",
        }
    }

    /// Whether the command commits a state change (and so honours `expect_rev`).
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Hello { .. }
                | Self::SessionDevices
                | Self::SessionPreview { .. }
                | Self::SessionPreviewStop
                | Self::SessionControlPanel { .. }
                | Self::SessionDetectLoopback { .. }
                | Self::SessionStatus
                | Self::TraceList
                | Self::TraceGet { .. }
                | Self::TraceExport { .. }
                | Self::CalList
                | Self::StateSnapshot
                | Self::StateSince { .. }
                | Self::GridGet { .. }
                | Self::DelayFind { .. }
                | Self::FileUploadBegin { .. }
                | Self::FileUploadChunk { .. }
                | Self::FileImport { .. }
                | Self::FileDownloadBegin { .. }
                | Self::FileDownloadChunk { .. }
                | Self::SplLogGet { .. }
                | Self::SplHistoryGet { .. }
                | Self::SplBandLogGet { .. }
                | Self::FileList
                | Self::RecList
                | Self::ServerInfo
                | Self::PlanList
        )
    }

    /// The lease token, for commands that require the stimulus lease.
    pub fn lease_token(&self) -> Option<LeaseToken> {
        match self {
            Self::GenSet { lease_token, .. }
            | Self::GenRefresh { lease_token }
            | Self::GenRelease { lease_token }
            | Self::SessionDetectLoopback { lease_token, .. }
            | Self::SweepRun { lease_token, .. } => Some(*lease_token),
            _ => None,
        }
    }
}
