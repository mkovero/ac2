//! Texts of the rig's settings, shared by the app's Settings view and the CLI: the system
//! max level and who changed it, output names, and how the daemon serves clients (its mode,
//! the authorized client keys and the keys it refused).

use ac2_proto::model::{
    AuthorizedClient, GenAction, Generator, OutputSetup, RefusedKey, ServerInfo, ServerMode,
};
use ac2_proto::units::{Dbfs, WallNs};

use crate::format;
use crate::time::{ClockOffset, age_s};

/// The word the operator types to confirm a raise of the system max level: a whole word,
/// so neither a stray key nor a repeated Enter can raise it.
pub const RAISE_WORD: &str = "raise";

/// `−40.0 dBFS`.
pub fn dbfs(v: f64) -> String {
    format!("{} dBFS", format::level(v))
}

/// The system max level with its bound: `−40.0 dBFS · bound −10.0 dBFS (ac2d --max-level)`;
/// at the bound, `−10.0 dBFS · the rig's bound (ac2d --max-level)`.
pub fn ceiling_line(g: &Generator) -> String {
    if g.ceiling.0 >= g.ceiling_bound.0 {
        format!("{} · the rig's bound (ac2d --max-level)", dbfs(g.ceiling.0))
    } else {
        format!(
            "{} · bound {} (ac2d --max-level)",
            dbfs(g.ceiling.0),
            dbfs(g.ceiling_bound.0)
        )
    }
}

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
        format!("system max level raised to {}", dbfs(to.0))
    } else if to.0 < from.0 {
        format!("system max level lowered to {}", dbfs(to.0))
    } else {
        format!("system max level stays {}", dbfs(to.0))
    }
}

/// The confirmation a raise asks for.
pub fn raise_prompt(from: Dbfs, to: Dbfs) -> String {
    format!(
        "Raise the system max level from {} to {}? Every client can then play louder. Type \
         {RAISE_WORD} and press Enter.",
        dbfs(from.0),
        dbfs(to.0)
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

/// How the daemon serves clients, in lines.
pub fn server_lines(s: &ServerInfo) -> Vec<String> {
    let mut v = match &s.mode {
        ServerMode::Embedded => vec![
            "Embedded: the daemon runs inside this app; no other client can connect.".to_owned(),
        ],
        ServerMode::Local { ctrl } => vec![
            format!("Local only: {ctrl}"),
            "Clients on this computer under the same user connect without keys.".to_owned(),
        ],
        ServerMode::Network {
            ctrl,
            fingerprint,
            advertised_as,
            ..
        } => vec![
            format!("Network mode: {ctrl} (CURVE; paired clients only)"),
            format!("Server key fingerprint: {fingerprint}"),
            match advertised_as {
                Some(n) => format!("Advertised over mDNS as {n:?}"),
                None => "Not advertised over mDNS (clients need the address)".to_owned(),
            },
        ],
    };
    v.push(match &s.recording_dir {
        Some(d) => format!("Recordings: {d}"),
        None => "Recordings: this daemon does not record".to_owned(),
    });
    v
}

/// One authorized client: `laptop · fingerprint 1a2b-…`.
pub fn authorized_row(a: &AuthorizedClient) -> String {
    format!("{} · fingerprint {}", a.name, a.fingerprint)
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
            ceiling_line(&generator(-40.0, -10.0, None)),
            "\u{2212}40.0 dBFS · bound \u{2212}10.0 dBFS (ac2d --max-level)"
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
        let lines = server_lines(&s);
        assert_eq!(
            lines,
            vec![
                "Network mode: tcp://0.0.0.0:47820 (CURVE; paired clients only)",
                "Server key fingerprint: SHA256:3f1c 9a2e 77b0 51d4",
                "Advertised over mDNS as \"foh-rig\"",
                "Recordings: /home/fohtech/.local/share/ac2/recordings",
            ]
        );
        let ServerMode::Network {
            authorized,
            refused,
            ..
        } = &s.mode
        else {
            unreachable!()
        };
        assert_eq!(
            authorized_row(&authorized[0]),
            "laptop · fingerprint SHA256:b2aa 0c3d 9e41 7f60"
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
            server_lines(&ServerInfo {
                mode: ServerMode::Embedded,
                recording_dir: None,
            }),
            vec![
                "Embedded: the daemon runs inside this app; no other client can connect.",
                "Recordings: this daemon does not record",
            ]
        );
    }
}
