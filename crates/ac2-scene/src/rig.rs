//! Texts of the rig's settings, shared by the app's Settings view and the CLI: the system
//! max level and who changed it, output names, and how the daemon serves clients (its mode,
//! the authorized client keys and the keys it refused).

use ac2_proto::model::{
    AuthorizedClient, ClientRole, GenAction, Generator, LocalServer, NetReason, NetReasonKind,
    OutputSetup, Recordings, RefusedKey, ServerInfo,
};
use ac2_proto::units::{Dbfs, WallNs};

use crate::format;
use crate::time::{ClockOffset, age_s};

/// The word the operator types to confirm a raise of the system max level: a whole word,
/// so neither a stray key nor a repeated Enter can raise it.
pub const RAISE_WORD: &str = "raise";

/// The system max level with its bound: at the bound, `−10.0 dBFS · the rig's bound (ac2d
/// --max-level)`; below it, the bound and where to raise it, since a rig with no level
/// stored starts well below its bound and the operator must find the way up:
/// `−50.0 dBFS · below the rig's bound −10.0 dBFS (ac2d --max-level) · raise: Settings ›
/// Audio I/O, or ac2 gen ceiling <dBFS> --yes`. Whether a level below the bound was set by
/// an operator or is the quiet start is not in the state, so the line does not say.
pub fn ceiling_line(g: &Generator) -> String {
    if g.ceiling.0 >= g.ceiling_bound.0 {
        format!(
            "{} · the rig's bound (ac2d --max-level)",
            format::dbfs(g.ceiling.0)
        )
    } else {
        format!(
            "{} · below the rig's bound {} (ac2d --max-level) · raise: {RAISE_WHERE}",
            format::dbfs(g.ceiling.0),
            format::dbfs(g.ceiling_bound.0)
        )
    }
}

/// Where the system max level is raised, in the app and from the CLI.
pub const RAISE_WHERE: &str = "Settings › Audio I/O, or ac2 gen ceiling <dBFS> --yes";

/// Who changed the system max level last, if that is the generator's latest action:
/// `lowered by laptop`, `raised by foh`.
pub fn ceiling_change(g: &Generator) -> Option<String> {
    let a = g.last_action.as_ref()?;
    let verb = match a.action {
        GenAction::CeilingLowered => "lowered",
        GenAction::CeilingRaised => "raised",
        _ => return None,
    };
    Some(match &a.client {
        Some(c) => format!("{verb} by {}", c.0),
        None => verb.to_owned(),
    })
}

/// What a change did, for a toast or the CLI: `system max level lowered to −40.0 dBFS`.
pub fn ceiling_changed(from: Dbfs, to: Dbfs) -> String {
    if to.0 > from.0 {
        format!("system max level raised to {}", format::dbfs(to.0))
    } else if to.0 < from.0 {
        format!("system max level lowered to {}", format::dbfs(to.0))
    } else {
        format!("system max level stays {}", format::dbfs(to.0))
    }
}

/// The confirmation a raise asks for.
pub fn raise_prompt(from: Dbfs, to: Dbfs) -> String {
    format!(
        "Raise the system max level from {} to {}? Every client can then play louder. Type \
         {RAISE_WORD} and press Enter.",
        format::dbfs(from.0),
        format::dbfs(to.0)
    )
}

/// Why a raise cannot happen now.
pub const RAISE_WHILE_LIVE: &str = "stop the stimulus first: the system max level is never raised while anything is armed or playing";

/// What lowering does to a stimulus above the new maximum.
pub const LOWER_STOPS: &str =
    "lowering applies at once: a stimulus armed or playing above the new maximum is stopped";

/// An output as the Settings view and the stimulus name it: the rig's label, else the
/// device's channel name, else `Output N`.
pub fn output_name(channel: u16, labels: &[OutputSetup], device_name: Option<&str>) -> String {
    labels
        .iter()
        .find(|o| o.channel == channel)
        .and_then(|o| o.label.as_deref())
        .map_or_else(
            || crate::meter::output_name(channel, device_name),
            str::to_owned,
        )
}

/// The outputs a stimulus plays on, by name: `1 · Main L, 2 · Main R`.
pub fn outputs_text(
    channels: &[u16],
    labels: &[OutputSetup],
    device_name: impl Fn(u16) -> Option<String>,
) -> String {
    channels
        .iter()
        .map(|&c| {
            crate::meter::channel_choice(c, &output_name(c, labels, device_name(c).as_deref()))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The stimulus outputs as the top bar says them: by the rig's labels where there are
/// some (`Main L, out 3`), else by number (`out 1, 2`).
pub fn stimulus_outputs(channels: &[u16], labels: &[OutputSetup]) -> String {
    let label = |c: u16| {
        labels
            .iter()
            .find(|o| o.channel == c)
            .and_then(|o| o.label.clone())
    };
    if channels.iter().any(|c| label(*c).is_some()) {
        channels
            .iter()
            .map(|&c| label(c).unwrap_or_else(|| format!("out {}", u32::from(c) + 1)))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        format!(
            "out {}",
            channels
                .iter()
                .map(|c| (u32::from(*c) + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// How the daemon serves clients on its own computer, in lines (the network's rows come
/// after them: [`crate::network`]).
pub fn local_lines(s: &ServerInfo) -> Vec<String> {
    match &s.local {
        Some(LocalServer::Embedded) => {
            vec!["In-process: no client outside its program can connect.".to_owned()]
        }
        Some(LocalServer::Local { ctrl, .. }) => vec![
            format!("Local: {ctrl}"),
            "Clients on this computer under the same user connect without keys.".to_owned(),
        ],
        None => Vec::new(),
    }
}

/// What follows the network's rows: the server key, the advert, the recordings.
pub fn server_lines(s: &ServerInfo) -> Vec<String> {
    let n = &s.network;
    let mut v = vec![match &n.fingerprint {
        Some(f) => format!("Server key fingerprint: {f}"),
        None => "Server key: created when the network is first turned on".to_owned(),
    }];
    if n.status.endpoints().is_some() {
        v.push(match &n.advertised_as {
            Some(a) => format!("Advertised over mDNS as {a:?}"),
            None => "Not advertised over mDNS (clients need the address)".to_owned(),
        });
    }
    v.push(match &s.recordings {
        Recordings::Dir { dir } => format!("Recordings: {dir}"),
        Recordings::Kept => "Recordings: on the rig's host".to_owned(),
        Recordings::Off => "Recordings: this daemon does not record".to_owned(),
    });
    v
}

/// Why the network is not listening, in words.
pub fn reason_text(r: &NetReason) -> String {
    match r {
        NetReason::Other { detail } => detail.clone(),
        r => reason_kind_text(r.kind()).to_owned(),
    }
}

/// [`reason_text`] as a network client is told it: by kind.
pub fn reason_kind_text(k: NetReasonKind) -> &'static str {
    match k {
        NetReasonKind::AddressInUse => "the port is in use",
        NetReasonKind::InterfaceMissing => "the interface is not there",
        NetReasonKind::AddressMissing => "the address is not on this computer",
        NetReasonKind::ZapFailed => "key checking stopped",
        NetReasonKind::NoServerKey => "no server key",
        NetReasonKind::NotHandedOver => "the daemon is too busy",
        NetReasonKind::Other => "an error on the rig",
    }
}

/// What an authorized key may do, in one word every list of keys says it with: an
/// `operator` drives the rig, a `viewer` only looks, a `rig` was admitted to take this rig's
/// inputs.
pub fn key_kind(a: &AuthorizedClient) -> &'static str {
    role_word(&a.role)
}

/// [`key_kind`] of a role.
pub fn role_word(r: &ClientRole) -> &'static str {
    match r {
        ClientRole::Operator => "operator",
        ClientRole::Viewer => "viewer",
        ClientRole::Rig { .. } => "rig",
    }
}

/// One authorized client: `laptop · operator · fingerprint 1a2b-…`, `phone · viewer · may
/// only look · fingerprint …`, `rig-pupu · rig · may take this rig's inputs · fingerprint …`.
pub fn authorized_row(a: &AuthorizedClient) -> String {
    // What a viewer or a rig may do, not only who it is: neither drives this rig.
    let may = match a.role {
        ClientRole::Operator => "",
        ClientRole::Viewer => " · may only look",
        ClientRole::Rig { .. } => " · may take this rig's inputs",
    };
    format!(
        "{} · {}{may} · fingerprint {}",
        a.name,
        key_kind(a),
        a.fingerprint
    )
}

/// One refused key: `fingerprint 1a2b-… from 192.168.1.40 · 12 times · 3 min ago`.
pub fn refused_row(r: &RefusedKey, now: WallNs, offset: ClockOffset) -> String {
    let who = match &r.fingerprint {
        Some(f) => format!("fingerprint {f}"),
        None => "a client without CURVE".to_owned(),
    };
    let times = if r.count == 1 {
        "once".to_owned()
    } else {
        format!("{} times", r.count)
    };
    format!(
        "{who} from {} · {times} · {}",
        r.address,
        format::ago(age_s(r.last_at, now, offset))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::GenAudit;
    use ac2_proto::units::ClientId;

    fn generator(ceiling: f64, bound: f64, action: Option<GenAction>) -> Generator {
        Generator {
            owner: None,
            owner_local: false,
            armed: false,
            firing: false,
            settings: None,
            ceiling: Dbfs(ceiling),
            ceiling_bound: Dbfs(bound),
            last_action: action.map(|action| GenAudit {
                action,
                client: Some(ClientId("laptop".into())),
                at: WallNs(0),
            }),
        }
    }

    #[test]
    fn ceiling_texts() {
        assert_eq!(
            ceiling_line(&generator(-50.0, -10.0, None)),
            "\u{2212}50.0 dBFS · below the rig's bound \u{2212}10.0 dBFS (ac2d --max-level) · \
             raise: Settings › Audio I/O, or ac2 gen ceiling <dBFS> --yes"
        );
        assert_eq!(
            ceiling_line(&generator(-10.0, -10.0, None)),
            "\u{2212}10.0 dBFS · the rig's bound (ac2d --max-level)"
        );
        assert_eq!(
            ceiling_change(&generator(-40.0, -10.0, Some(GenAction::CeilingLowered))),
            Some("lowered by laptop".into())
        );
        assert_eq!(
            ceiling_change(&generator(-40.0, -10.0, Some(GenAction::CeilingRaised))),
            Some("raised by laptop".into())
        );
        assert_eq!(
            ceiling_change(&generator(-40.0, -10.0, Some(GenAction::Fire))),
            None
        );
        assert_eq!(
            ceiling_changed(Dbfs(-50.0), Dbfs(-40.0)),
            "system max level raised to \u{2212}40.0 dBFS"
        );
        assert_eq!(
            ceiling_changed(Dbfs(-40.0), Dbfs(-50.0)),
            "system max level lowered to \u{2212}50.0 dBFS"
        );
        assert!(raise_prompt(Dbfs(-50.0), Dbfs(-40.0)).starts_with(
            "Raise the system max level from \u{2212}50.0 dBFS to \u{2212}40.0 dBFS?"
        ));
        assert!(raise_prompt(Dbfs(-50.0), Dbfs(-40.0)).contains("Type raise and press Enter"));
    }

    #[test]
    fn output_names_prefer_the_rig_label() {
        let labels = vec![OutputSetup {
            channel: 0,
            label: Some("Main L".into()),
        }];
        assert_eq!(output_name(0, &labels, Some("system:playback_1")), "Main L");
        assert_eq!(
            output_name(1, &labels, Some("system:playback_2")),
            "system:playback_2"
        );
        assert_eq!(output_name(2, &labels, None), "Output 3");
        assert_eq!(
            outputs_text(&[0, 2], &labels, |_| None),
            "1 · Main L, 3 · Output 3"
        );
        assert_eq!(stimulus_outputs(&[0, 2], &labels), "Main L, out 3");
        assert_eq!(stimulus_outputs(&[1, 2], &labels), "out 2, 3");
    }

    #[test]
    fn server_texts() {
        let s = ac2_proto::samples::server_info();
        assert_eq!(
            local_lines(&s),
            vec![
                "Local: ipc:///run/user/1000/ac2/ctrl.sock",
                "Clients on this computer under the same user connect without keys.",
            ]
        );
        assert_eq!(
            server_lines(&s),
            vec![
                "Server key fingerprint: SHA256:3f1c 9a2e 77b0 51d4",
                "Advertised over mDNS as \"foh-rig\"",
                "Recordings: /home/fohtech/.local/share/ac2/recordings",
            ]
        );
        let authorized = &s.network.authorized;
        let Some(refused) = &s.network.refused else {
            panic!("the host sample lists refusals")
        };
        assert_eq!(
            authorized_row(&authorized[0]),
            "laptop · operator · fingerprint SHA256:b2aa 0c3d 9e41 7f60"
        );
        assert_eq!(
            authorized_row(&authorized[1]),
            "phone · viewer · may only look · fingerprint SHA256:51d4 77b0 9a2e 3f1c"
        );
        let rig = AuthorizedClient {
            name: "rig-pupu".into(),
            role: ClientRole::Rig {
                rig_id: ac2_proto::units::RigId(7),
            },
            ..authorized[0].clone()
        };
        assert_eq!(
            authorized_row(&rig),
            "rig-pupu · rig · may take this rig's inputs · fingerprint SHA256:b2aa 0c3d 9e41 7f60"
        );
        let now = WallNs(1_790_000_000_000_000_000 + 180_000_000_000);
        assert_eq!(
            refused_row(&refused[0], now, ClockOffset(0)),
            "fingerprint SHA256:51e0 c2b9 aa13 0d77 from 192.168.1.40 · 12 times · 3 min ago"
        );
        assert_eq!(
            refused_row(&refused[1], now, ClockOffset(0)),
            "a client without CURVE from 192.168.1.41 · once · 3 min ago"
        );
        assert_eq!(
            server_lines(&ac2_proto::samples::server_info_fixed()),
            vec![
                "Server key fingerprint: SHA256:3f1c 9a2e 77b0 51d4",
                "Recordings: on the rig's host",
            ]
        );
        let off = ac2_proto::samples::server_info_off();
        assert!(local_lines(&ac2_proto::samples::server_info_fixed()).is_empty());
        assert_eq!(
            server_lines(&off),
            vec![
                "Server key: created when the network is first turned on",
                "Recordings: this daemon does not record",
            ]
        );
    }
}
