//! `auth add|role|remove|list`: the client keys the daemon admits and what each may do
//! (`auth.authorize`, `auth.revoke`, `server.info`'s `authorized`). A role is changed by
//! authorizing the same key again with the new role, as the daemon has it.

use ac2_client::{Client, ClientError, expect_body};
use ac2_proto::model::{AuthorizedClient, ClientRole, RefusedKey, ServerInfo};
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::connect;
use crate::CliError;
use crate::args::{Cli, RoleArg};
use crate::output::{Out, utc};

async fn info(c: &Client) -> Result<ServerInfo, CliError> {
    let r = c.call(Command::ServerInfo).await?;
    Ok(expect_body!("server.info", r, ReplyBody::Server(s) => s)?)
}

async fn authorize(
    c: &Client,
    name: &str,
    key: &str,
    role: ClientRole,
) -> Result<AuthorizedClient, CliError> {
    let r = c
        .call(Command::AuthAuthorize {
            name: name.to_owned(),
            key: key.to_owned(),
            role,
        })
        .await?;
    let info = expect_body!("auth.authorize", r, ReplyBody::Server(s) => s)?;
    Ok(info
        .network
        .authorized
        .into_iter()
        .find(|a| a.name == name && a.key == key)
        .ok_or(ClientError::UnexpectedReply {
            op: "auth.authorize",
            got: "server info without the authorized name",
        })?)
}

/// What a role allows, for the line that confirms it.
fn role_says(r: &ClientRole) -> &'static str {
    match r {
        ClientRole::Operator => "it may drive the rig",
        ClientRole::Viewer => "it may only look (and stop the generator)",
        ClientRole::Rig { .. } => "it may take this rig's inputs",
    }
}

/// `auth add`.
pub(crate) async fn add(
    cli: &Cli,
    name: &str,
    key: Option<&str>,
    refused: Option<&str>,
    role: RoleArg,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let before = info(&c).await?;
    let key = match (refused, key) {
        (Some(fp), _) => refused_key(before.network.refused.as_deref(), fp)?,
        (None, Some(k)) => k.trim().to_owned(),
        (None, None) => return Err(CliError::Usage("give KEY or --refused".into())),
    };
    // The daemon replaces the key of a name given again (and drops its connections).
    let replaced = before
        .network
        .authorized
        .iter()
        .find(|a| a.name == name && a.key != key)
        .map(|a| a.fingerprint.clone());
    let entry = authorize(&c, name, &key, role.into()).await?;
    out.emit(
        &json!({ "authorized": entry, "replaced_fingerprint": replaced }),
        || {
            let mut s = format!(
                "authorized {name} as {} (fingerprint {}): {}; its connections are admitted \
                 from now on",
                ac2_scene::rig::key_kind(&entry),
                entry.fingerprint,
                role_says(&entry.role)
            );
            if let Some(old) = &replaced {
                s.push_str(&format!(
                    "\nreplaced its earlier key (fingerprint {old}), whose connections are dropped"
                ));
            }
            s
        },
    )?;
    Ok(())
}

/// `auth role`: the same key authorized again with `role`.
pub(crate) async fn role(
    cli: &Cli,
    name: &str,
    role: RoleArg,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let before = info(&c).await?;
    let Some(had) = before.network.authorized.iter().find(|a| a.name == name) else {
        return Err(CliError::NotFound(format!(
            "no client is authorized as {name} (`ac2 auth list`)"
        )));
    };
    if matches!(had.role, ClientRole::Rig { .. }) {
        return Err(CliError::Usage(format!(
            "{name} is admitted as a rig: remove it (`ac2 auth remove {name}`) before \
             authorizing an app under its name"
        )));
    }
    let entry = authorize(&c, name, &had.key, role.into()).await?;
    let changed = had.role != entry.role;
    out.emit(&json!({ "authorized": entry, "changed": changed }), || {
        if changed {
            format!(
                "{name} is {} now: {}; its connections drop and come back with the new role",
                ac2_scene::rig::key_kind(&entry),
                role_says(&entry.role)
            )
        } else {
            format!("{name} is {} already", ac2_scene::rig::key_kind(&entry))
        }
    })?;
    Ok(())
}

/// `auth remove`.
pub(crate) async fn remove(cli: &Cli, name: &str, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    c.call(Command::AuthRevoke {
        name: name.to_owned(),
    })
    .await?;
    out.emit(&json!({ "removed": name }), || {
        format!("removed {name}: its requests are refused and its connections end now")
    })?;
    Ok(())
}

/// `auth list`.
pub(crate) async fn list(cli: &Cli, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    let info = info(&c).await?;
    let n = &info.network;
    out.emit(
        &json!({ "authorized": n.authorized, "refused": n.refused }),
        || list_text(&info),
    )?;
    Ok(())
}

/// The authorized keys and the refused ones, as `auth list` and `server info` print them.
pub(crate) fn list_text(info: &ServerInfo) -> String {
    let mut s = String::from("authorized\n");
    if info.network.authorized.is_empty() {
        s.push_str("  none\n");
    }
    for a in &info.network.authorized {
        s.push_str(&authorized_line(a));
    }
    s.push_str("refused lately\n");
    match &info.network.refused {
        None => s.push_str("  listed on the rig's own host only\n"),
        Some(r) if r.is_empty() => s.push_str("  none\n"),
        Some(r) => r.iter().for_each(|r| s.push_str(&refused_line(r))),
    }
    s
}

/// `  laptop  operator  <fingerprint>`, `  rig-pupu  rig 00000000000000a1  <fingerprint>`:
/// the role in the app's word for it, a rig's id after it.
fn authorized_line(a: &AuthorizedClient) -> String {
    let kind = ac2_scene::rig::key_kind(a);
    let id = match a.role {
        ClientRole::Rig { rig_id } => format!(" {:016x}", rig_id.0),
        ClientRole::Operator | ClientRole::Viewer => String::new(),
    };
    format!("  {}  {kind}{id}  {}\n", a.name, a.fingerprint)
}

fn refused_line(r: &RefusedKey) -> String {
    let when = format!(
        "from {}, {}×, last {} UTC",
        r.address,
        r.count,
        utc(r.last_at.0)
    );
    match &r.fingerprint {
        Some(fp) => format!(
            "  {fp}  {when}\n    authorize: ac2 auth add <name> --refused {fp} --role viewer|operator\n"
        ),
        None => format!("  (no key: not a CURVE client)  {when}\n"),
    }
}

/// The key of the refused client whose fingerprint is `fp`, as the daemon lists it (`None`:
/// it lists them only to a client on its own host).
fn refused_key(refused: Option<&[RefusedKey]>, fp: &str) -> Result<String, CliError> {
    let Some(refused) = refused else {
        return Err(CliError::Usage(
            "the daemon lists the keys it refused only on its own host: run this there, or \
             give the KEY"
                .into(),
        ));
    };
    let fp = fp.trim();
    if let Some(k) = refused
        .iter()
        .filter(|r| {
            r.fingerprint
                .as_deref()
                .is_some_and(|f| f.eq_ignore_ascii_case(fp))
        })
        .find_map(|r| r.key.clone())
    {
        return Ok(k);
    }
    let mut listed: Vec<&str> = Vec::new();
    for f in refused.iter().filter_map(|r| r.fingerprint.as_deref()) {
        if !listed.contains(&f) {
            listed.push(f);
        }
    }
    let then = "have the client try again, or give its KEY (shown by `ac2 auth show` on the \
                client)";
    Err(CliError::NotFound(if listed.is_empty() {
        format!("no refused key has fingerprint {fp}: the daemon has refused no key lately; {then}")
    } else {
        format!(
            "no refused key has fingerprint {fp} (refused lately: {}); {then}",
            listed.join(", ")
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An authorized key reads with its role, the app's word for it.
    #[test]
    fn an_authorized_key_says_its_role() {
        let op = AuthorizedClient {
            name: "laptop".into(),
            key: String::new(),
            fingerprint: "ab12".into(),
            role: ClientRole::Operator,
        };
        assert_eq!(authorized_line(&op), "  laptop  operator  ab12\n");
        let viewer = AuthorizedClient {
            name: "phone".into(),
            role: ClientRole::Viewer,
            ..op.clone()
        };
        assert_eq!(authorized_line(&viewer), "  phone  viewer  ab12\n");
        let rig = AuthorizedClient {
            name: "rig-pupu".into(),
            role: ClientRole::Rig {
                rig_id: ac2_proto::units::RigId(0xa1),
            },
            ..op
        };
        assert_eq!(
            authorized_line(&rig),
            "  rig-pupu  rig 00000000000000a1  ab12\n"
        );
    }
}
