//! Remote inputs on the Audio I/O page (`docs/design/remote-inputs.md` §5): the add dialog's
//! model and the pure parts of pairing two rigs (the name one rig is admitted under on the
//! other, the address the session's rig reaches the source at). The reducer side is in
//! `state/remote_impl.rs`; every word is `ac2_scene::remote_inputs`'.

use ac2_proto::model::{MeasKind, RemoteInput};
use ac2_proto::units::RigId;

/// The add dialog (*Add an input from another rig…*), open over the Audio I/O page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemotePick {
    /// The focused input among the ones that can be picked.
    pub focus: usize,
    /// Why the last pick was refused.
    pub error: Option<String>,
}

/// The remote-input state of the Audio I/O page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteRows {
    /// The add dialog, while open.
    pub pick: Option<RemotePick>,
    /// The slot Delete was pressed on once: a second press removes it.
    pub confirm: Option<u16>,
}

/// A line of the add dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickLine {
    /// Words: why a rig's inputs cannot be taken, or why none can.
    Text { text: String, problem: bool },
    /// An input of rig `rig` (its device input `channel`), `added` when the session has it.
    Input {
        rig: RigId,
        channel: u16,
        text: String,
        added: bool,
    },
}

impl PickLine {
    /// The input this line picks, if it can be picked.
    pub fn pick(&self) -> Option<(RigId, u16)> {
        match self {
            Self::Input {
                rig,
                channel,
                added: false,
                ..
            } => Some((*rig, *channel)),
            _ => None,
        }
    }
}

/// A remote input's row on the Audio I/O page, all its words from `ac2-scene`.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteRow {
    pub slot: u16,
    /// Its rig, as the heading above it names it.
    pub rig: String,
    /// The row text its rig gives it (input, kind, mic, curve, calibration).
    pub text: String,
    /// Its link and audio in words.
    pub state: String,
    /// The state is a problem (said in words, and coloured too).
    pub problem: bool,
    pub reading: ac2_scene::meter::MeterReading,
}

/// The name rig `label` is admitted under on another rig: an authorized key's name there
/// (`A–Z a–z 0–9 . _ -`, at most 64), marked as a rig's so it never takes the name of an
/// app the operator authorized from the same computer.
pub fn admit_name(label: &str) -> String {
    let mut s: String = format!("rig-{label}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    s.truncate(64);
    s
}

/// `tcp://host:port` → (`host`, port), brackets of an IPv6 host removed.
fn split_endpoint(e: &str) -> Option<(String, u16)> {
    let rest = e.strip_prefix("tcp://")?;
    let (host, port) = rest.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_owned(), port.parse().ok()?))
}

fn tcp(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("tcp://[{host}]:{port}")
    } else {
        format!("tcp://{host}:{port}")
    }
}

/// Where another rig reaches a source rig whose daemon is bound at `ctrl`/`data`: at the
/// host this app reaches it at over the network (`via`), else at the address it is bound to,
/// else, bound to every interface of this computer, at this computer's name (`local_host`).
/// `None` when it listens on loopback only: no other rig can reach it.
pub fn peer_endpoints(
    ctrl: &str,
    data: &str,
    via: Option<&str>,
    local_host: &str,
) -> Option<(String, String)> {
    let (bound, ctrl_port) = split_endpoint(ctrl)?;
    let (_, data_port) = split_endpoint(data)?;
    let loopback = matches!(bound.as_str(), "127.0.0.1" | "::1" | "localhost");
    let wildcard = matches!(bound.as_str(), "*" | "0.0.0.0" | "::");
    let host = match via {
        Some(h) if !loopback => h.to_owned(),
        _ if loopback => return None,
        _ if wildcard => local_host.to_owned(),
        _ => bound,
    };
    Some((tcp(&host, ctrl_port), tcp(&host, data_port)))
}

/// The session channels measurement kind `kind` names.
pub fn meas_channels(kind: &MeasKind) -> Vec<u16> {
    match kind {
        MeasKind::Spectrum { config } => vec![config.input],
        MeasKind::Rta { config } => vec![config.input],
        MeasKind::Spl { config } => vec![config.input],
        MeasKind::Transfer { config } => vec![config.reference_input, config.measurement_input],
        MeasKind::Sweep { config } => std::iter::once(config.reference_input)
            .chain(config.measurement_inputs.iter().copied())
            .collect(),
        MeasKind::Math { .. } => Vec::new(),
    }
}

/// The slot of a new remote input: past every slot the session has and every remote
/// channel a measurement names (`used`), so a slot is never given to another input while
/// a measurement may still name it.
pub fn next_slot(remote: &[RemoteInput], used: impl IntoIterator<Item = u16>) -> Option<u16> {
    let top = remote
        .iter()
        .map(|r| r.slot)
        .chain(used.into_iter().filter_map(ac2_proto::model::remote_slot))
        .max();
    match top {
        None => Some(0),
        Some(s) => s
            .checked_add(1)
            .filter(|&n| n < ac2_proto::model::REMOTE_BASE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rig_is_admitted_under_a_name_of_its_own_kind() {
        assert_eq!(admit_name("pupu"), "rig-pupu");
        assert_eq!(admit_name("ac2 on pupu"), "rig-ac2-on-pupu");
        assert_eq!(admit_name(&"x".repeat(80)).len(), 64);
    }

    #[test]
    fn the_source_is_reached_where_the_app_reaches_it_else_where_it_listens() {
        let ep = |c: &str, via, local| peer_endpoints(c, &c.replace("7700", "7701"), via, local);
        assert_eq!(
            ep("tcp://0.0.0.0:7700", Some("192.168.9.27"), "app"),
            Some((
                "tcp://192.168.9.27:7700".into(),
                "tcp://192.168.9.27:7701".into()
            ))
        );
        assert_eq!(
            ep("tcp://0.0.0.0:7700", None, "ketunkolo").map(|e| e.0),
            Some("tcp://ketunkolo:7700".into())
        );
        assert_eq!(
            ep("tcp://10.0.0.5:7700", None, "ketunkolo").map(|e| e.0),
            Some("tcp://10.0.0.5:7700".into())
        );
        assert_eq!(ep("tcp://127.0.0.1:7700", None, "ketunkolo"), None);
        assert_eq!(
            ep("tcp://[::]:7700", Some("fe80::1"), "x").map(|e| e.0),
            Some("tcp://[fe80::1]:7700".into())
        );
    }

    #[test]
    fn a_slot_is_never_given_twice() {
        let r = |slot| RemoteInput {
            slot,
            rig: RigId(2),
            rig_name: "k".into(),
            channel: 0,
        };
        assert_eq!(next_slot(&[], []), Some(0));
        assert_eq!(next_slot(&[r(0), r(3)], []), Some(4));
        // A measurement still names slot 5's channel.
        assert_eq!(
            next_slot(&[r(0)], [1, ac2_proto::model::REMOTE_BASE + 5]),
            Some(6)
        );
    }
}
