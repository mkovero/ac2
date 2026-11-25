//! `state.snapshot` whatever the state's size: inline in the reply, or, when it would not
//! fit a ctrl message, its bytes fetched as a snapshot transfer and decoded here.

use ac2_proto::model::Transfer;
use ac2_proto::{Command, ReplyBody, StateSnapshot};

use crate::client::CallCore;
use crate::{ClientError, Retry};

/// The daemon's state now. `answered` runs after every reply of a transfer's fetch: a slow
/// link's fetch holds the caller from its keepalives, and the replies say the daemon is
/// there all the same.
pub(crate) async fn take(
    core: &CallCore,
    retry: Retry,
    answered: &mut impl FnMut(),
) -> Result<StateSnapshot, ClientError> {
    let op = "state.snapshot";
    let r = core.call(Command::StateSnapshot, retry).await?;
    let t = match r {
        ReplyBody::Snapshot(s) => return Ok(*s),
        ReplyBody::SnapshotTransfer(t) => t,
        other => {
            return Err(ClientError::UnexpectedReply {
                op,
                got: crate::body_name(&other),
            });
        }
    };
    answered();
    let data = fetch(core, retry, &t.transfer, answered).await;
    // Ended whether the fetch succeeded or not: the daemon holds the bytes until then.
    let end = core
        .call(
            Command::FileDownloadEnd {
                transfer: t.transfer.transfer,
                saved_as: None,
            },
            retry,
        )
        .await;
    let data = data?;
    answered();
    ac2_archive::verify(&data, &t.transfer.sha256).map_err(ClientError::Archive)?;
    if let Err(e) = end {
        // Every byte is in and matches its hash: the snapshot stands whatever became of the
        // daemon's copy, which it drops when the transfer expires.
        tracing::warn!(
            "{}: ending the download failed ({e}); the snapshot is whole and used",
            t.transfer.file_name
        );
    }
    let snap = ac2_proto::decode_snapshot(&data)
        .map_err(|e| ClientError::Snapshot(format!("{}: {e}", t.transfer.file_name)))?;
    if snap.rev != t.rev {
        return Err(ClientError::Snapshot(format!(
            "{} holds rev {}, the reply said {}",
            t.transfer.file_name, snap.rev.0, t.rev.0
        )));
    }
    Ok(snap)
}

/// The bytes of snapshot transfer `t`, chunk by chunk, every one of them or an error.
async fn fetch(
    core: &CallCore,
    retry: Retry,
    t: &Transfer,
    answered: &mut impl FnMut(),
) -> Result<Vec<u8>, ClientError> {
    let mut data = Vec::new();
    while (data.len() as u64) < t.bytes {
        let r = core
            .call(
                Command::FileDownloadChunk {
                    transfer: t.transfer,
                    offset: data.len() as u64,
                },
                retry,
            )
            .await?;
        let chunk = crate::expect_body!("file.download_chunk", r, ReplyBody::Chunk(b) => b)?;
        answered();
        if chunk.0.is_empty() || (data.len() + chunk.0.len()) as u64 > t.bytes {
            break;
        }
        data.extend_from_slice(&chunk.0);
    }
    if data.len() as u64 != t.bytes {
        return Err(ClientError::Snapshot(format!(
            "{}: fetch ended at {} of {} bytes",
            t.file_name,
            data.len(),
            t.bytes
        )));
    }
    Ok(data)
}
