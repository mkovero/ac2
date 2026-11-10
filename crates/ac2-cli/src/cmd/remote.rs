//! Remote inputs from the CLI: admit another rig (`auth admit-rig`), pair with one
//! (`auth peer-add` / `peer-remove`), and the remote inputs of `session open --remote-in`.

use ac2_client::Client;
use ac2_client::endpoint::{Endpoints, RemoteAddr};
use ac2_proto::Command;
use ac2_proto::model::{RemoteInput, RigPeer, State};
use ac2_proto::units::RigId;
use ac2_zmq::PublicKey;
use serde_json::json;

use super::connect;
use crate::CliError;
use crate::args::{Cli, RemoteIn};
use crate::output::Out;

fn rig_id(s: &str) -> Result<RigId, CliError> {
    u64::from_str_radix(s.trim(), 16)
        .map(RigId)
        .map_err(|_| CliError::Usage(format!("--rig-id {s:?}: expected 16 hex digits")))
}

fn key(s: &str) -> Result<String, CliError> {
    let s = s.trim();
    PublicKey::from_z85(s)
        .map(|_| s.to_owned())
        .map_err(|_| CliError::Usage("--key: not a 40-character Z85 CURVE public key".into()))
}

/// `auth admit-rig`.
pub(crate) async fn admit_rig(
    cli: &Cli,
    name: &str,
    k: &str,
    id: &str,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let cmd = Command::AuthAdmitRig {
        key: key(k)?,
        rig_id: rig_id(id)?,
        name: name.to_owned(),
    };
    let c = connect(cli, false).await?;
    c.call(cmd).await?;
    out.emit(&json!({ "admitted": name, "rig_id": id }), || {
        format!("admitted rig {name} ({id}): it may take this rig's inputs")
    })?;
    Ok(())
}

async fn set_peers(c: &Client, peers: Vec<RigPeer>) -> Result<(), CliError> {
    c.call(Command::RigPeers { peers }).await?;
    Ok(())
}

/// `auth peer-add`.
pub(crate) async fn peer_add(
    cli: &Cli,
    name: &str,
    id: &str,
    at: &RemoteAddr,
    k: &str,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let ep = Endpoints::remote(at);
    let peer = RigPeer {
        rig: rig_id(id)?,
        name: name.to_owned(),
        ctrl: ep.ctrl,
        data: ep.data,
        key: key(k)?,
    };
    let c = connect(cli, false).await?;
    let mut peers = super::state(&c).await?.remote.peers;
    peers.retain(|p| p.rig != peer.rig && p.name != peer.name);
    peers.push(peer.clone());
    set_peers(&c, peers).await?;
    out.emit(&peer, || {
        format!(
            "paired with {name} at {}: admit this rig there (`ac2 auth admit-rig`)",
            peer.ctrl
        )
    })?;
    Ok(())
}

/// `auth peer-remove`.
pub(crate) async fn peer_remove(cli: &Cli, name: &str, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let mut peers = super::state(&c).await?.remote.peers;
    let before = peers.len();
    peers.retain(|p| p.name != name);
    if peers.len() == before {
        return Err(CliError::Usage(format!("not paired with {name:?}")));
    }
    set_peers(&c, peers).await?;
    out.emit(&json!({ "removed": name }), || {
        format!("unpaired from {name}")
    })?;
    Ok(())
}

/// The remote inputs of `--remote-in`, slot by slot in the order given, each against a
/// paired rig.
pub(crate) fn remote_inputs(st: &State, args: &[RemoteIn]) -> Result<Vec<RemoteInput>, CliError> {
    args.iter()
        .enumerate()
        .map(|(slot, a)| {
            let peer = st
                .remote
                .peers
                .iter()
                .find(|p| p.name == a.rig)
                .ok_or_else(|| {
                    CliError::Usage(format!(
                        "--remote-in {}: not paired with {:?} (`ac2 auth peer-add`)",
                        a.rig, a.rig
                    ))
                })?;
            Ok(RemoteInput {
                slot: u16::try_from(slot)
                    .map_err(|_| CliError::Usage("too many remote inputs".into()))?,
                rig: peer.rig,
                rig_name: peer.name.clone(),
                channel: a.input.0,
            })
        })
        .collect()
}
