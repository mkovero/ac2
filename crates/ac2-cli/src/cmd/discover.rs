//! `discover`: list network-mode daemons that advertise `_ac2._tcp` over mDNS.
//!
//! The list is informational. Each rig's pairing status compares the advertised key
//! fingerprint with the keys pinned by `ac2 auth pair`; connecting still goes through
//! `--remote` with a pinned key, so a forged advert can at worst show up in this list.

use std::time::Duration;

use ac2_client::PinStatus;
use ac2_discovery::{Options, Rig};
use serde_json::json;

use super::key_dir;
use crate::CliError;
use crate::args::{Cli, DiscoverArgs};
use crate::output::{self, Out};

/// The `--remote` value for a rig: the host its key is pinned under when paired (the pin is
/// looked up by that name), else its best address.
fn remote_arg(r: &Rig, s: &PinStatus) -> String {
    let host = match s {
        PinStatus::Paired { host, .. } => host.clone(),
        _ => r.connect_host(),
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    if r.port == ac2_client::endpoint::DEFAULT_PORT {
        host
    } else {
        format!("{host}:{}", r.port)
    }
}

fn status_text(s: &PinStatus) -> String {
    match s {
        PinStatus::Paired { host, .. } => format!("paired ({host})"),
        PinStatus::Mismatch { host, pinned } => {
            format!("KEY MISMATCH: {host} is pinned to {pinned}")
        }
        PinStatus::Unpaired => "not paired".into(),
    }
}

pub(crate) fn run(cli: &Cli, a: &DiscoverArgs, out: &mut Out<'_>) -> Result<(), CliError> {
    let opts = Options {
        mdns_port: a.mdns_port.unwrap_or(ac2_discovery::MDNS_PORT),
        loopback_only: a.loopback,
    };
    let wait = Duration::from_secs_f64(a.wait.0.0.clamp(0.1, 60.0));
    let found = ac2_discovery::discover(wait, &opts).map_err(|e| CliError::Usage(e.to_string()))?;
    let table = &found.table;
    let kd = key_dir(cli);
    let mut rows = Vec::new();
    for r in table.rigs() {
        let host = r.connect_host();
        let status = kd.pin_status(&r.advert.fingerprint, &[r.host.as_str(), host.as_str()])?;
        rows.push((r, status));
    }
    let doc: Vec<_> = rows
        .iter()
        .map(|(r, s)| {
            json!({
                "name": r.advert.name,
                "instance": r.instance,
                "host": r.host,
                "addresses": r.addresses,
                "port": r.port,
                "remote": remote_arg(r, s),
                "version": r.advert.version,
                "proto": r.advert.proto,
                "proto_compatible": r.advert.proto == ac2_proto::PROTO_VERSION,
                "fingerprint": r.advert.fingerprint,
                "pairing": match s {
                    PinStatus::Paired { host, .. } => json!({"status": "paired", "host": host}),
                    PinStatus::Mismatch { host, pinned } => json!({"status": "mismatch", "host": host, "pinned_fingerprint": pinned}),
                    PinStatus::Unpaired => json!({"status": "unpaired"}),
                },
            })
        })
        .collect();
    out.emit(&doc, || {
        if rows.is_empty() {
            let asked: Vec<String> = found
                .queried
                .iter()
                .map(|q| match &q.error {
                    None => format!("{} ({})", q.name, q.addr),
                    Some(e) => format!("{} ({}: not sent, {e})", q.name, q.addr),
                })
                .collect();
            return format!(
                "no ac2 daemons answered within {:.1} s (asked on {}).\n\
                 A daemon advertises only in network mode (`ac2d --listen tcp://0.0.0.0`), and \
                 mDNS does not cross routers or VPNs; connect with `--remote <address>` instead.",
                wait.as_secs_f64(),
                if asked.is_empty() {
                    "no interface".to_owned()
                } else {
                    asked.join(", ")
                }
            );
        }
        let mut t = output::table(&["name", "--remote", "version", "fingerprint", "pairing"]);
        for (r, s) in &rows {
            let version = if r.advert.proto == ac2_proto::PROTO_VERSION {
                r.advert.version.clone()
            } else {
                format!(
                    "{} (protocol {}, this client {})",
                    r.advert.version,
                    r.advert.proto,
                    ac2_proto::PROTO_VERSION
                )
            };
            t.add_row(vec![
                r.advert.name.clone(),
                remote_arg(r, s),
                version,
                r.advert.fingerprint.clone(),
                status_text(s),
            ]);
        }
        let mut s = format!("{t}\n");
        if rows
            .iter()
            .any(|(_, s)| !matches!(s, PinStatus::Paired { .. }))
        {
            s.push_str(
                "Discovery is not trust. To pair, get the daemon's key on the daemon host (its \
                 log prints it with the fingerprint), check the fingerprint matches, then run\n  \
                 ac2 auth pair <--remote value> --server-key <key>\n",
            );
        }
        s
    })?;
    Ok(())
}
