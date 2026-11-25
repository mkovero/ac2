//! `server info|network`: how the daemon serves clients and its network listener switched
//! live (`server.network`); the client keys it admits are `ac2 auth` ([`super::clients`]).
//! Refusals (fixed by `--listen`, asked from the network, an in-process daemon, a bad port
//! or interface) are the daemon's own words, printed as they come.

use ac2_client::{Client, expect_body};
use ac2_proto::model::{
    LocalServer, NetBind, NetChoice, NetReason, NetReasonKind, NetSaved, NetSetting,
    NetSettingKind, NetSettingView, NetStatus, NetStatusView, NetToggle, NetUnavailable,
    NotSavedReason, PeerNetStatus, ServerInfo,
};
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::connect;
use crate::CliError;
use crate::args::{Cli, NetworkCmd, ServerCmd};
use crate::output::Out;

pub(crate) async fn run(cli: &Cli, cmd: &ServerCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    let c = connect(cli, false).await?;
    match cmd {
        ServerCmd::Info => {
            let info = info(&c).await?;
            out.emit(&info, || info_text(&info))?;
        }
        ServerCmd::Network { cmd } => {
            let info = match cmd {
                None => info(&c).await?,
                Some(NetworkCmd::On(a)) => network(&c, true, a.bind(), a.port).await?,
                Some(NetworkCmd::Off) => network(&c, false, None, None).await?,
            };
            let n = &info.network;
            out.emit(
                &json!({
                    "setting": n.setting,
                    "status": n.status,
                    "toggle": info.toggle,
                    "server_key": n.server_key,
                    "fingerprint": n.fingerprint,
                    "advertised_as": n.advertised_as,
                }),
                || network_text(&info),
            )?;
        }
    }
    Ok(())
}

async fn info(c: &Client) -> Result<ServerInfo, CliError> {
    let r = c.call(Command::ServerInfo).await?;
    Ok(expect_body!("server.info", r, ReplyBody::Server(s) => s)?)
}

async fn network(
    c: &Client,
    enabled: bool,
    bind: Option<NetBind>,
    port: Option<u16>,
) -> Result<ServerInfo, CliError> {
    let r = c
        .call(Command::ServerNetwork {
            enabled,
            bind,
            port,
        })
        .await?;
    Ok(expect_body!("server.network", r, ReplyBody::Server(s) => s)?)
}

fn bind_text(b: &NetBind) -> String {
    match b {
        NetBind::All => "every interface".into(),
        NetBind::Interface { name } => format!("interface {name}"),
        NetBind::Addr { addr } => format!("address {addr}"),
    }
}

/// `every interface, port 47820 (data 47821)`.
fn choice_text(c: &NetChoice) -> String {
    format!(
        "{}, port {} (data {})",
        bind_text(&c.bind),
        c.port,
        u32::from(c.port) + 1
    )
}

/// Why `server.network` would be refused; `None` where it may switch the network.
fn toggle_text(t: NetToggle) -> Option<&'static str> {
    match t {
        // The setting line already says it is fixed by --listen.
        NetToggle::Allowed | NetToggle::Fixed => None,
        NetToggle::Unavailable {
            reason: NetUnavailable::InProcess,
        } => Some("unavailable: this daemon serves in-process only"),
        NetToggle::Unavailable {
            reason: NetUnavailable::NoKeyStore,
        } => Some("unavailable: this daemon has no key files"),
        NetToggle::Unavailable {
            reason: NetUnavailable::NotLocal,
        } => Some("only on the rig's own machine"),
    }
}

fn not_saved_text(r: &NotSavedReason) -> String {
    match r {
        NotSavedReason::StoreReadOnly => "the settings file cannot be written".into(),
        NotSavedReason::Unreadable { detail } => {
            format!("the settings file could not be read at start ({detail}); it is left as it is")
        }
        NotSavedReason::DamagedInPlace => "the damaged settings file could not be set aside".into(),
        NotSavedReason::NoStore => "this daemon keeps no settings file".into(),
        NotSavedReason::WriteFailed { detail } => format!("writing the settings failed: {detail}"),
    }
}

fn saved_text(s: &NetSaved) -> String {
    match s {
        NetSaved::Saved => "kept across restarts".into(),
        NetSaved::NotSaved { reason } => {
            format!("NOT KEPT across restarts: {}", not_saved_text(reason))
        }
    }
}

/// The setting in a line: `on: every interface, port 47820; kept across restarts`.
pub fn setting_text(v: &NetSettingView) -> String {
    match v {
        NetSettingView::Host { setting } => match setting {
            NetSetting::Off { kept: None, saved } => format!("off; {}", saved_text(saved)),
            NetSetting::Off {
                kept: Some(c),
                saved,
            } => format!(
                "off (on again at {}); {}",
                choice_text(c),
                saved_text(saved)
            ),
            NetSetting::On { choice, saved } => {
                format!("on: {}; {}", choice_text(choice), saved_text(saved))
            }
            NetSetting::Fixed { listen } => format!("fixed by ac2d --listen {listen}"),
        },
        NetSettingView::Peer { setting } => match setting {
            NetSettingKind::Off => "off".into(),
            NetSettingKind::On => "on".into(),
            NetSettingKind::Fixed => "fixed by ac2d --listen".into(),
        },
    }
}

fn reason_kind_text(k: NetReasonKind) -> &'static str {
    match k {
        NetReasonKind::AddressInUse => "the port is in use",
        NetReasonKind::InterfaceMissing => "no interface of that name on this host",
        NetReasonKind::AddressMissing => "the address is not on this host",
        NetReasonKind::ZapFailed => "the authentication handler stopped",
        NetReasonKind::NoServerKey => "the daemon has no server key",
        NetReasonKind::NotHandedOver => "the daemon's I/O thread did not take the sockets",
        NetReasonKind::Other => "a failure the rig's host reports",
    }
}

fn reason_text(r: &NetReason) -> String {
    match r {
        NetReason::Other { detail } => detail.clone(),
        r => reason_kind_text(r.kind()).into(),
    }
}

/// The status in a line: `listening on tcp://…:47820 (data …:47821)`, `waiting: the port is
/// in use; retried`.
pub fn status_text(v: &NetStatusView) -> String {
    let (word, reason) = match v {
        NetStatusView::Host { status } => match status {
            NetStatus::Off => return "off".into(),
            NetStatus::Listening { ctrl, data } => {
                return format!("listening on {ctrl} (data {data})");
            }
            NetStatus::Waiting { reason } => ("waiting", reason_text(reason)),
            NetStatus::Failed { reason } => ("failed", reason_text(reason)),
        },
        NetStatusView::Peer { status } => match status {
            PeerNetStatus::Off => return "off".into(),
            PeerNetStatus::Listening { ctrl, data } => {
                return format!("listening on {ctrl} (data {data})");
            }
            PeerNetStatus::Waiting { reason } => ("waiting", reason_kind_text(*reason).into()),
            PeerNetStatus::Failed { reason } => ("failed", reason_kind_text(*reason).into()),
        },
    };
    format!("{word}: {reason}; retried")
}

fn network_text(info: &ServerInfo) -> String {
    let n = &info.network;
    let mut s = format!(
        "network      {}\nstatus       {}\n",
        setting_text(&n.setting),
        status_text(&n.status)
    );
    if let Some(t) = toggle_text(info.toggle) {
        s.push_str(&format!("switch       {t}\n"));
    }
    match (&n.server_key, &n.fingerprint) {
        (Some(k), Some(fp)) => s.push_str(&format!("server key   {k}\nfingerprint  {fp}\n")),
        _ => s.push_str("server key   none yet (made when the network is first turned on)\n"),
    }
    match &n.advertised_as {
        Some(a) => s.push_str(&format!("advertised   as {a}\n")),
        None => s.push_str("advertised   no\n"),
    }
    s
}

fn info_text(info: &ServerInfo) -> String {
    let mut s = match &info.local {
        Some(LocalServer::Local { ctrl, data }) => format!("local        {ctrl} (data {data})\n"),
        Some(LocalServer::Embedded) => "local        in this process only\n".into(),
        None => String::new(),
    };
    s.push_str(&network_text(info));
    if let Some(d) = &info.recording_dir {
        s.push_str(&format!("recordings   {d}\n"));
    }
    s.push_str(&super::clients::list_text(info));
    if !info.interfaces.is_empty() {
        s.push_str("interfaces\n");
        for i in &info.interfaces {
            let addrs: Vec<String> = i.addrs.iter().map(ToString::to_string).collect();
            s.push_str(&format!(
                "  {}  {}  {}\n",
                i.name,
                if i.up { "up" } else { "down" },
                addrs.join(", ")
            ));
        }
    }
    s
}
