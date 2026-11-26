//! `ac2 sacn target`: where the rig's universe goes as sACN. The frame, its fade and test
//! frames are `ac2 artnet`'s: both outputs carry the same universe.

use ac2_proto::Command;
use ac2_proto::model::{Rig, SacnDest, SacnPriority, SacnTarget};

use super::connect;
use super::osc::rig_edit;
use crate::CliError;
use crate::args::*;
use crate::output::Out;

pub(crate) async fn sacn(cli: &Cli, cmd: &SacnCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        SacnCmd::Target {
            dest,
            universe,
            via,
            priority,
        } => {
            let target = target(*dest, *universe, *via, *priority)?;
            let c = connect(cli, false).await?;
            let rig = rig_edit(&c, out, |base| Command::SacnTarget { base, target }).await?;
            out.emit(&rig, || match rig.sacn_target {
                Some(_) => ac2_scene::sacn::target_text(rig.sacn_target),
                None => "sACN target off: the stream is ended, no packets are sent".to_owned(),
            })?;
        }
    }
    Ok(())
}

/// The target the arguments name, or why they name none.
fn target(
    dest: SacnDestArg,
    universe: Option<SacnUniverseArg>,
    via: Option<std::net::Ipv4Addr>,
    priority: Option<SacnPriorityArg>,
) -> Result<Option<SacnTarget>, CliError> {
    let usage = |m: &str| Err(CliError::Usage(m.into()));
    let dest = match (dest, via) {
        (SacnDestArg::Off, _) => {
            if universe.is_some() || via.is_some() || priority.is_some() {
                return usage("`off` takes nothing more: ac2 sacn target off");
            }
            return Ok(None);
        }
        (SacnDestArg::Unicast(_), Some(_)) => {
            return usage(
                "--via is for multicast: ac2 sacn target multicast <universe> --via <ip>",
            );
        }
        (SacnDestArg::Unicast(addr), None) => SacnDest::Unicast { addr },
        (SacnDestArg::Multicast, Some(interface)) => SacnDest::Multicast { interface },
        (SacnDestArg::Multicast, None) => {
            return usage(
                "multicast needs the interface: --via <this host's address on the lighting network>",
            );
        }
    };
    let Some(universe) = universe else {
        return usage("give the universe too: ac2 sacn target <ip|multicast> <1–63999>");
    };
    Ok(Some(SacnTarget {
        dest,
        universe: universe.0,
        priority: priority.map_or(SacnPriority::DEFAULT, |p| p.0),
    }))
}

/// The sACN lines of `rig status`.
pub(crate) fn status_text(rig: &Rig) -> String {
    format!(
        "sACN target: {}\nsent: {}\nlast error: {}\n",
        rig.sacn_target.map_or_else(
            || "off".to_owned(),
            |t| ac2_scene::sacn::target_text(Some(t))
                .trim_start_matches("sACN to ")
                .to_owned()
        ),
        rig.sacn.sent,
        rig.sacn.last_error.as_deref().unwrap_or("none"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::SacnUniverse;
    use std::net::Ipv4Addr;

    fn u(n: u16) -> Option<SacnUniverseArg> {
        Some(SacnUniverseArg(SacnUniverse::new(n).expect("test value")))
    }

    #[test]
    fn the_arguments_make_one_target_or_say_what_is_missing() {
        let ip = Ipv4Addr::new(10, 0, 0, 4);
        assert_eq!(target(SacnDestArg::Off, None, None, None).ok(), Some(None));
        let t = target(SacnDestArg::Multicast, u(3), Some(ip), None)
            .expect("test value")
            .expect("test value");
        assert_eq!(t.dest, SacnDest::Multicast { interface: ip });
        assert_eq!(t.priority, SacnPriority::DEFAULT);
        for bad in [
            target(SacnDestArg::Off, u(1), None, None),
            target(SacnDestArg::Unicast(ip), u(1), Some(ip), None),
            target(SacnDestArg::Multicast, u(1), None, None),
            target(SacnDestArg::Unicast(ip), None, None, None),
        ] {
            assert!(matches!(bad, Err(CliError::Usage(_))), "{bad:?}");
        }
    }
}
