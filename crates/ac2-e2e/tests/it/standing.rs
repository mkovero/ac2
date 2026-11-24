//! A paired rig's heading from the clocks and session its levels show: two network-mode
//! daemons paired both ways, both sessions open, no remote input anywhere; the app on one of
//! them only.

use crate::cross_rig::{config, device, net_endpoints, rig, session};
use crate::harness::{Driver, R};
use ac2_client::Client;
use ac2_proto::model::RigPeer;
use ac2_proto::{Command, ReplyBody};
use ac2_scene::standing::FROM_LEVELS;
use ac2_ui::state::AppState;
use ac2_zmq::KeyPair;
use std::time::Duration;

/// Pupu and kettu on one true clock, each pairing the other and admitting it, both sessions
/// open with no remote input; the app connected to pupu alone. The daemons live while the
/// handles do.
fn paired_both_ways(dir: &std::path::Path) -> R<(ac2d::Handle, ac2d::Handle, Driver)> {
    let app = KeyPair::generate()?;
    let pupu_keys = KeyPair::generate()?;
    let kettu_keys = KeyPair::generate()?;
    let kettu = rig(dir, "kettu", device(0.0)?, &app, &kettu_keys)?;
    let pupu = rig(dir, "pupu", device(0.0)?, &app, &pupu_keys)?;
    let rt = tokio::runtime::Runtime::new()?;
    let (b, a) = rt.block_on(async {
        let b = Client::connect(config(&kettu, &app, "e2e setup")?).await?;
        let a = Client::connect(config(&pupu, &app, "e2e setup")?).await?;
        b.wait_synced(Duration::from_secs(10)).await?;
        a.wait_synced(Duration::from_secs(10)).await?;
        Ok::<_, Box<dyn std::error::Error>>((b, a))
    })?;
    let call = |c: &Client, cmd: Command| -> R<ReplyBody> { Ok(rt.block_on(c.call(cmd))?) };
    let pupu_side = (a.welcome().rig_id, "pupu", &pupu, &pupu_keys);
    let kettu_side = (b.welcome().rig_id, "kettu", &kettu, &kettu_keys);
    let sides = [(&a, kettu_side), (&b, pupu_side)];
    // Each admits the other before either lists it: a link to a rig that has not admitted
    // this one waits out a retry before it comes up.
    for (me, (id, name, _, keys)) in sides {
        call(
            me,
            Command::SessionOpen {
                config: session(vec![0, 1], Vec::new()),
            },
        )?;
        call(
            me,
            Command::AuthAdmitRig {
                key: keys.public.to_z85(),
                rig_id: id,
                name: name.into(),
            },
        )?;
    }
    for (me, (id, name, h, _)) in sides {
        call(
            me,
            Command::RigPair {
                peer: RigPeer {
                    rig: id,
                    name: name.into(),
                    ctrl: net_endpoints(h).0,
                    data: net_endpoints(h).1,
                    key: h.server_public_key().ok_or("no server key")?.to_z85(),
                },
            },
        )?;
    }
    let d = Driver::connect(config(&pupu, &app, crate::harness::NAME)?, "pupu")?;
    Ok((pupu, kettu, d))
}

/// Kettu's heading in the list: its narrowest line and its tooltip.
fn kettu_heading(st: &AppState) -> Option<(String, String)> {
    st.list_rows()
        .into_iter()
        .find(|(_, r)| r.depth == 0 && r.name == "kettu")
        .map(|(_, r)| (r.details.last().cloned().unwrap_or_default(), r.describe))
}

/// The rate pupu's daemon hears in kettu's levels, as a heading says it.
fn heard_rate(st: &AppState) -> Option<String> {
    let s = st.daemon()?.remote.standing.first()?;
    Some(ac2_scene::audio::rate_text(
        s.session.as_ref()?.sample_rate_hz,
    ))
}

/// No input from kettu, and the app does not hold it: its heading has the rate its levels
/// carry and the tag they earn, measuring at first, said as measured from the levels.
#[test]
fn a_paired_rig_with_no_input_is_measured_from_its_levels() -> R {
    let dir = tempfile::tempdir()?;
    let (pupu, kettu, mut d) = paired_both_ways(dir.path())?;
    d.until("kettu measuring from its levels", |st| {
        let (Some((line, tip)), Some(rate)) = (kettu_heading(st), heard_rate(st)) else {
            return false;
        };
        line == format!("{rate} UNKNOWN")
            && tip.contains("\nclock UNKNOWN: can't tell: measuring clocks…")
            && tip.ends_with(FROM_LEVELS)
    })?;
    // The rig shown is compared with it too.
    let rows = d.st.list_rows();
    let shown = rows.first().map(|(_, r)| r.details.last().cloned());
    let rate = heard_rate(&d.st).ok_or("kettu's rate")?;
    assert_eq!(shown, Some(Some(format!("{rate} UNKNOWN"))));
    pupu.shutdown();
    kettu.shutdown();
    Ok(())
}

/// A minute of marks between two rigs on one true clock: kettu's heading agrees, with no
/// audio streamed.
#[test]
fn slow_a_paired_rig_agrees_from_its_levels() -> R {
    let dir = tempfile::tempdir()?;
    let (pupu, kettu, mut d) = paired_both_ways(dir.path())?;
    let agreed = |st: &AppState| {
        matches!(
            (kettu_heading(st), heard_rate(st)),
            (Some((line, _)), Some(rate)) if line == format!("{rate} AGREE")
        )
    };
    d.until_within(Duration::from_secs(150), "ketunkolo tagged AGREE", agreed)?;
    let (_, tip) = kettu_heading(&d.st).ok_or("ketunkolo's heading")?;
    assert!(
        tip.contains(&format!(
            "\nclock AGREE: clocks agree with pupu: {FROM_LEVELS}"
        )),
        "{tip}"
    );
    pupu.shutdown();
    kettu.shutdown();
    Ok(())
}
