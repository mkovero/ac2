//! One daemon reached twice: the rig the app was started with, and a rig remembered from
//! the last run that turns out to be the same daemon under another address (its advert's
//! host name). The app holds it once, on the screen shown, and says so.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ac2_e2e::{Setup, sim_daemon};
use ac2_ui::conn::{Conn, Target};
use ac2_ui::keys::Keymap;
use ac2_ui::state::{AppState, ConnState, Msg};

use crate::harness::{Cli, DEADLINE, NAME, R};

/// The remembered rig carries the daemon's id from the last run, so the rig shown, though
/// it is the one asked for, says hello as a rig already held. It must not stay "connecting"
/// for good with its link running: the held rig takes its place on the screen shown and
/// the second link is closed as any duplicate.
#[test]
fn a_shown_rig_held_under_another_address_is_shown_connected() -> R {
    let daemon = sim_daemon(Setup::Empty)?;
    let id = Cli::connect(daemon.endpoints())?.client().welcome().rig_id;
    let link = |describe: &str| {
        Conn::start(
            Target {
                config: daemon.client_config(NAME),
                describe: describe.into(),
            },
            Arc::new(|| {}),
        )
    };
    let keys = Keymap::default();
    let mut st = AppState::default();
    st.set_prefs(crate::common::grid_ui_prefs());
    let shown = st.rig.slot;
    let held = st.add_rig("pupu".into(), Some((id, "pupu".into())), None);
    let links = [
        (shown, link("daemon at 192.168.9.27")?),
        (held, link("daemon at pupu.local")?),
    ];
    let end = Instant::now() + DEADLINE;
    loop {
        // As the app pumps its links: by slot, each event tagged with its rig's id now.
        for (slot, conn) in &links {
            for e in conn.drain() {
                let Some(rig) = st.rig_in_slot(*slot).map(|r| r.id) else {
                    break;
                };
                st.update(Msg::Conn(rig, Box::new(e)), &keys);
            }
        }
        let connected =
            matches!(&st.rig.conn, ConnState::Connected { rig_id, .. } if *rig_id == id);
        if connected && st.rig.mirror.as_ref().is_some_and(|m| m.state.is_some()) {
            break;
        }
        if Instant::now() > end {
            return Err(format!("the rig shown: {:?}", st.rig.conn).into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(st.rigs().count(), 1, "one daemon is one rig");
    assert_eq!(st.rig.slot, held, "the held link is the one shown");
    let said: Vec<&str> = st.toasts.list.iter().map(|t| t.text.as_str()).collect();
    assert!(
        said.iter().any(|t| t.ends_with("is already connected")),
        "{said:?}"
    );
    Ok(())
}
