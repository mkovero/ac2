//! `auth pair|show`: client-side key handling for remote mode.
//!
//! Pairing is two-sided and never automatic: this command pins the daemon key the operator
//! typed in (after comparing fingerprints with what the daemon host shows) and prints this
//! client's public key, which the operator adds to the daemon's `authorized_clients`.

use ac2_client::fingerprint;
use ac2_zmq::PublicKey;
use serde_json::json;

use super::key_dir;
use crate::CliError;
use crate::args::{AuthCmd, Cli};
use crate::output::Out;

/// Default client name: this host's name, reduced to `A–Z a–z 0–9 . _ -`.
pub fn default_name() -> String {
    let raw = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .unwrap_or_default();
    sanitize(&raw)
}

/// Keeps `A–Z a–z 0–9 . _ -`, maps anything else to `-`, at most 64 characters.
pub fn sanitize(s: &str) -> String {
    let n: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    if n.is_empty() { "client".into() } else { n }
}

pub(crate) fn run(cli: &Cli, cmd: &AuthCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let kd = key_dir(cli);
    match cmd {
        AuthCmd::Pair {
            host,
            server_key,
            name,
        } => {
            let server = PublicKey::from_z85(server_key.trim()).map_err(|_| {
                CliError::Usage("--server-key: not a 40-character Z85 CURVE public key".into())
            })?;
            let (kp, created) = kd.ensure_client_keypair()?;
            let replaced = kd.pin_server(&host.host, server)?;
            let name = sanitize(&name.clone().unwrap_or_else(default_name));
            let line = format!("{name} {}", kp.public.to_z85());
            out.emit(
                &json!({
                    "host": host.host,
                    "server_fingerprint": fingerprint(&server),
                    "replaced_pin": replaced.map(|k| fingerprint(&k)),
                    "client_key": kp.public.to_z85(),
                    "client_fingerprint": fingerprint(&kp.public),
                    "client_key_created": created,
                    "authorized_clients_line": line,
                    "key_dir": kd.path(),
                }),
                || {
                    let mut s = format!(
                        "pinned daemon key for {}\n  fingerprint {}   (must match what the daemon host shows)",
                        host.host,
                        fingerprint(&server)
                    );
                    if let Some(old) = replaced {
                        s.push_str(&format!("\n  replaced the earlier pin {}", fingerprint(&old)));
                    }
                    s.push_str(&format!(
                        "\nthis client{}\n  key         {}\n  fingerprint {}\nauthorize it on the daemon host by adding this line to authorized_clients:\n  {line}",
                        if created { " (new keypair)" } else { "" },
                        kp.public.to_z85(),
                        fingerprint(&kp.public)
                    ));
                    s
                },
            )?;
        }
        AuthCmd::Show => {
            let client = kd.client_keypair().ok().map(|k| k.public);
            let servers = kd.known_servers()?;
            out.emit(
                &json!({
                    "key_dir": kd.path(),
                    "client_key": client.map(|k| k.to_z85()),
                    "client_fingerprint": client.map(|k| fingerprint(&k)),
                    "servers": servers.iter().map(|(h, k)| json!({
                        "host": h, "key": k.to_z85(), "fingerprint": fingerprint(k)
                    })).collect::<Vec<_>>(),
                }),
                || {
                    let mut s = format!("key dir  {}\n", kd.path().display());
                    match client {
                        Some(k) => {
                            s.push_str(&format!("client   {}  ({})\n", k.to_z85(), fingerprint(&k)))
                        }
                        None => {
                            s.push_str("client   no keypair yet (`ac2 auth pair` creates one)\n")
                        }
                    }
                    for (h, k) in &servers {
                        s.push_str(&format!("daemon   {h}  {}\n", fingerprint(k)));
                    }
                    s
                },
            )?;
        }
    }
    Ok(())
}
