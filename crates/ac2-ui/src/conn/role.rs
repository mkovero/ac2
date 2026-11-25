//! Which requests a viewer's app may send: what only reads the rig, and stopping the
//! generator. The daemon refuses the rest; the app refuses them first, with words.

use ac2_proto::Command;
use ac2_proto::model::ClientRole;

use super::Request;
use crate::sessions::SessionStep;

fn viewer_may(cmd: &Command) -> bool {
    cmd.allowed(&ClientRole::Viewer)
}

impl Request {
    /// Whether an app the rig authorized as a viewer may send this. Exhaustive without a
    /// wildcard, so a new request decides where it is added.
    pub fn viewer_may_send(&self) -> bool {
        match self {
            Self::Call { cmd, .. }
            | Self::Act { cmd, .. }
            | Self::ServerCall { cmd, .. }
            | Self::NewWalk { cmd, .. } => viewer_may(cmd),
            // Stopping is everyone's; a viewer holds no lease, so it is `gen.stop`.
            Self::StimStop => true,
            // Reads, subscriptions and the link itself.
            Self::ExportTrace { .. }
            | Self::Meters(_)
            | Self::LeqBackfill { .. }
            | Self::LeqSpan { .. }
            | Self::ExportLeqLog { .. }
            | Self::BandLogGet(_)
            | Self::DynamicsEnvelope { .. }
            | Self::Topics(_)
            | Self::DisplayPeriod(_)
            | Self::Reconnect
            | Self::ServerInfo
            | Self::SessionList => true,
            // A session job saves, loads or uploads: only quitting and a copy of the
            // working state written here are a viewer's (the daemon does not make the
            // viewer's file the working state's).
            Self::Sessions(job) => job.steps.iter().all(|s| {
                matches!(
                    s,
                    SessionStep::Quit | SessionStep::Export { session: None, .. }
                )
            }),
            Self::StimArm { .. }
            | Self::StimSet(_)
            | Self::Sweep { .. }
            | Self::DynamicsRun { .. }
            | Self::FindDelay { .. }
            | Self::Capture { .. }
            | Self::Import { .. }
            | Self::ImportCurve { .. }
            | Self::Devices
            | Self::OpenStream { .. }
            | Self::RemoteStream { .. }
            | Self::Preview { .. }
            | Self::PreviewStop
            | Self::DetectLoopback(_)
            | Self::CreateMeas { .. }
            | Self::BandTransfer { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionJob;

    /// What a viewer's app sends: reads, the link and stopping; nothing that changes the rig.
    #[test]
    fn a_viewer_sends_reads_and_stop_only() {
        let call = |cmd| Request::Call {
            cmd,
            what: String::new(),
        };
        assert!(call(Command::StateSnapshot).viewer_may_send());
        assert!(call(Command::GenStop).viewer_may_send());
        assert!(Request::StimStop.viewer_may_send());
        assert!(Request::ServerInfo.viewer_may_send());
        assert!(!call(Command::StreamClose).viewer_may_send());
        assert!(!call(Command::GenAcquire { force: false }).viewer_may_send());
        assert!(!Request::Devices.viewer_may_send());
        let job = |steps| Request::Sessions(SessionJob { steps });
        assert!(job(vec![SessionStep::Quit]).viewer_may_send());
        assert!(!job(vec![SessionStep::Recover, SessionStep::Quit]).viewer_may_send());
        let export = |session| SessionStep::Export {
            session,
            path: "/home/foh/show.ac2session".into(),
        };
        assert!(job(vec![export(None)]).viewer_may_send());
        let named = ac2_proto::model::SessionRef::Name {
            name: "show".into(),
        };
        assert!(!job(vec![export(Some(named))]).viewer_may_send());
    }
}
