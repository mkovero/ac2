//! The commands that commit one measurement, answered with the measurement as committed: its
//! `edit_rev` is the base of the next edit of it, which the mirror may not hold yet.

use ac2_client::{Client, ClientError};
use ac2_proto::model::Measurement;
use ac2_proto::units::MeasId;
use ac2_proto::{Command, ErrorCode, ErrorDetail, ReplyBody};

use super::{ConnEvent, Out};

/// The measurement `cmd` commits, when it is one of the commands that commit a measurement
/// and answer with it.
pub fn target(cmd: &Command) -> Option<MeasId> {
    match cmd {
        Command::MeasUpdate { meas, .. }
        | Command::MeasColour { meas, .. }
        | Command::MeasOffset { meas, .. }
        | Command::MeasLayout { meas, .. }
        | Command::MeasEq { meas, .. }
        | Command::MeasStart { meas }
        | Command::MeasStop { meas } => Some(*meas),
        _ => None,
    }
}

/// Why a measurement edit was not applied.
#[derive(Clone, Debug, PartialEq)]
pub enum MeasRefusal {
    /// Made on an `edit_rev` another commit has moved: the measurement changed meanwhile.
    Stale(String),
    /// Another `conflict` (an EQ set's rev moved).
    Conflict(String),
    /// Any other failure, the daemon's or the transport's words.
    Failed(String),
}

impl MeasRefusal {
    fn of(e: &ClientError) -> Self {
        match e {
            ClientError::Daemon(p) if matches!(p.detail, Some(ErrorDetail::StaleMeas { .. })) => {
                Self::Stale(e.to_string())
            }
            _ if e.code() == Some(ErrorCode::Conflict) => Self::Conflict(e.to_string()),
            _ => Self::Failed(e.to_string()),
        }
    }
}

/// Sends `cmd`, an edit of `meas`, and answers with [`ConnEvent::MeasEdited`].
pub(super) fn send(client: &Client, meas: MeasId, cmd: Command, what: String, out: &Out) {
    let (c, o) = (client.clone(), out.clone());
    tokio::spawn(async move {
        let result = match c.call_warned(cmd).await {
            Ok((body, warnings)) => {
                if !warnings.is_empty() {
                    o.send(ConnEvent::Warnings(warnings));
                }
                Ok(match body {
                    ReplyBody::Measurement(m) => Some(Box::new(m)),
                    _ => None,
                })
            }
            Err(e) => Err(MeasRefusal::of(&e)),
        };
        o.send(ConnEvent::MeasEdited { meas, what, result });
    });
}

/// The answer to an edit that could not be sent.
pub(super) fn not_connected(meas: MeasId, what: String) -> ConnEvent {
    ConnEvent::MeasEdited {
        meas,
        what,
        result: Err(MeasRefusal::Failed("not connected".into())),
    }
}

/// The measurement in an answer, for the reducer.
pub type Edited = Result<Option<Box<Measurement>>, MeasRefusal>;
