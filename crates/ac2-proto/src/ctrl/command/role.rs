//! Which role may send which command.

use super::Command;

use crate::model::ClientRole;

/// Who may send a command. A viewer is given exactly what reads the rig without acting on
/// it, plus `gen.stop`: anyone who hears sound may stop it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    /// Every role: `hello` only.
    Anyone,
    /// Operators and viewers.
    Viewer,
    /// Operators only.
    Operator,
    /// Rigs only: `source.state`.
    Rig,
}

impl Command {
    /// Whether a client of `role` may send this command. Checked before anything else
    /// runs, so a refused command stores and changes nothing.
    pub fn allowed(&self, role: &ClientRole) -> bool {
        match (self.access(), role) {
            (Access::Anyone, _) => true,
            (Access::Viewer, ClientRole::Operator | ClientRole::Viewer) => true,
            (Access::Viewer, ClientRole::Rig { .. }) => false,
            (Access::Operator, ClientRole::Operator) => true,
            (Access::Operator, ClientRole::Viewer | ClientRole::Rig { .. }) => false,
            (Access::Rig, ClientRole::Rig { .. }) => true,
            (Access::Rig, ClientRole::Operator | ClientRole::Viewer) => false,
        }
    }

    /// Exhaustive without a wildcard, so a new command decides who may send it where it is
    /// added. Reads that act on the rig all the same (a driver loads, a burst plays, a file
    /// lands on the host) are an operator's.
    fn access(&self) -> Access {
        match self {
            Self::Hello { .. } => Access::Anyone,
            Self::SourceState => Access::Rig,
            Self::StateSnapshot
            | Self::StateSince { .. }
            | Self::GridGet { .. }
            | Self::StreamStatus
            | Self::TraceList
            | Self::TraceGet { .. }
            | Self::TraceExport { .. }
            | Self::DynamicsTable { .. }
            | Self::DynamicsEnvelope(_)
            | Self::CalList
            | Self::SplLogGet { .. }
            | Self::SplHistoryGet { .. }
            | Self::SplBandLogGet { .. }
            | Self::FileList
            | Self::RecList
            | Self::PlanList
            | Self::ServerInfo
            | Self::GenStop
            | Self::FileDownloadChunk { .. } => Access::Viewer,
            // Only the working state: a named session is a file on the host a viewer was
            // not given, and a path is the local pair's alone.
            Self::FileDownloadBegin { session } => match session {
                None => Access::Viewer,
                Some(_) => Access::Operator,
            },
            // `saved_as` records the client's file as the working state's origin: a change
            // of state a viewer may not make.
            Self::FileDownloadEnd { saved_as, .. } => match saved_as {
                None => Access::Viewer,
                Some(_) => Access::Operator,
            },
            Self::StreamDevices
            | Self::StreamPreview { .. }
            | Self::StreamPreviewStop
            | Self::StreamControlPanel { .. }
            | Self::StreamDetectLoopback { .. }
            | Self::StreamOpen { .. }
            | Self::StreamClose
            | Self::StreamInputs { .. }
            | Self::StreamOutputs { .. }
            | Self::GenAcquire { .. }
            | Self::GenSet { .. }
            | Self::GenRefresh { .. }
            | Self::GenRelease { .. }
            | Self::GenCeiling { .. }
            | Self::OscTarget { .. }
            | Self::OscTest { .. }
            | Self::ArtnetTarget { .. }
            | Self::ArtnetFade { .. }
            | Self::ArtnetTest { .. }
            | Self::RigMonitorAtStart { .. }
            | Self::LevelsSet { .. }
            | Self::LevelsReset { .. }
            | Self::MeasCreate { .. }
            | Self::MeasUpdate { .. }
            | Self::MeasColour { .. }
            | Self::MeasOffset { .. }
            | Self::MeasLayout { .. }
            | Self::MeasDelete { .. }
            | Self::MeasEq { .. }
            | Self::MeasStart { .. }
            | Self::MeasStop { .. }
            | Self::MeasReset { .. }
            | Self::DelayFind { .. }
            | Self::DelayInsert { .. }
            | Self::DelaySet { .. }
            | Self::DelayNudge { .. }
            | Self::DelayTrack { .. }
            | Self::TraceCapture { .. }
            | Self::TraceUpdate { .. }
            | Self::TraceDelete { .. }
            | Self::TraceAverage { .. }
            | Self::TraceImport(..)
            | Self::TraceTarget { .. }
            | Self::TraceMicCurve { .. }
            | Self::CalSpl { .. }
            | Self::CalSplElectrical(..)
            | Self::CalCurveImport { .. }
            | Self::CalCurveRename { .. }
            | Self::CalCurveDelete { .. }
            | Self::CalLine { .. }
            | Self::CalDelete { .. }
            | Self::SplLogNew { .. }
            | Self::SplBandTransfer { .. }
            | Self::SweepRun(_)
            | Self::DynamicsRun(_)
            | Self::FileSave { .. }
            | Self::FileLoad { .. }
            | Self::FileNew { .. }
            | Self::FileRecover
            | Self::FileSalvage { .. }
            | Self::FileUploadBegin { .. }
            | Self::FileUploadChunk { .. }
            | Self::FileImport { .. }
            | Self::RecStart { .. }
            | Self::RecStop
            | Self::StreamReplay { .. }
            | Self::ServerNetwork { .. }
            | Self::AuthAuthorize { .. }
            | Self::AuthRevoke { .. }
            | Self::AuthAdmitRig { .. }
            | Self::AuthRevokeRig { .. }
            | Self::RigPair { .. }
            | Self::RigUnpair { .. }
            | Self::PlanSave { .. }
            | Self::PlanDelete { .. }
            | Self::WalkNew { .. }
            | Self::WalkUpdate { .. }
            | Self::WalkGoto { .. }
            | Self::WalkCapture { .. }
            | Self::WalkSkip { .. }
            | Self::WalkRedo { .. }
            | Self::WalkSwap { .. }
            | Self::WalkDelete { .. }
            | Self::SimRun { .. }
            | Self::SimFit { .. }
            | Self::SimMap { .. }
            | Self::SimCancel => Access::Operator,
        }
    }
}
