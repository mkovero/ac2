//! Reducer tests of spare inputs on the Audio I/O page: a JACK device offers them, ← → on
//! their row add or remove input rows after the physical ones, and the opened session
//! carries the count and the spare channels as ordinary inputs.

use super::*;

/// A JACK server with four physical capture ports and room for four spare ones.
fn jack_with_spares() -> Vec<BackendInfo> {
    let b = backends(false);
    let mut dev = b[0].devices[0].clone();
    dev.backend = BackendKind::Jack;
    dev.id = DeviceId("jack".into());
    dev.name = "JACK server".into();
    if let Some(i) = dev.input.as_mut() {
        i.spare_channels = 4;
    }
    vec![BackendInfo {
        kind: BackendKind::Jack,
        description: "JACK".into(),
        availability: Availability::Available,
        devices: vec![dev],
    }]
}

fn opened_config(r: &[Request]) -> Option<&SessionConfig> {
    r.iter().find_map(|r| match r {
        Request::OpenSession { config, .. } => Some(config),
        _ => None,
    })
}

#[test]
fn spare_inputs_are_rows_after_the_physical_ones() {
    let mut t = T::new();
    open_dialog(&mut t, jack_with_spares());
    let d = dialog(&t);
    assert!(d.rows().contains(&Row::Spares));
    assert_eq!((d.spares, d.inputs.len()), (0, 4));
    assert_eq!(
        d.spares_text(),
        "none · ← → add up to 4 inputs that you connect other JACK sources to"
    );

    focus(&mut t, Row::Spares);
    t.key("ArrowRight");
    t.key("ArrowRight");
    let d = dialog(&t);
    assert_eq!((d.spares, d.inputs.len()), (2, 6));
    let spare = &d.inputs[4];
    assert_eq!(spare.channel, 4);
    assert!(spare.in_session, "a spare is there to be metered");
    assert_eq!(spare.device_name.as_deref(), Some("spare 1"));
    assert_eq!(
        d.spares_text(),
        "inputs 5–6 · connect sources to them with your JACK tools (qjackctl, jack_connect); \
         a patchbay keeps them when JACK restarts"
    );
    // The device's meters open its physical inputs only.
    assert!(d.preview_setup().0.iter().all(|p| p.channel < 4));

    // Never more than the device opens; ← removes the last.
    for _ in 0..8 {
        t.key("ArrowRight");
    }
    assert_eq!(dialog(&t).spares, 4);
    for _ in 0..3 {
        t.key("ArrowLeft");
    }
    let d = dialog(&t);
    assert_eq!((d.spares, d.inputs.len()), (1, 5));
    assert_eq!(d.spares_text().split(" · ").next(), Some("input 5"));

    let r = t.key("Enter");
    let c = opened_config(&r).expect("session.open");
    assert_eq!(c.spare_inputs, 1);
    assert!(c.input_channels.contains(&4), "{:?}", c.input_channels);
}

/// A removed spare takes its pair with it; a device without spares shows no row.
#[test]
fn a_removed_spare_leaves_its_pair_and_other_backends_have_none() {
    let mut t = T::new();
    open_dialog(&mut t, jack_with_spares());
    focus(&mut t, Row::Spares);
    t.key("ArrowRight");
    t.key("ArrowRight");
    focus(&mut t, Row::Input(4));
    t.type_key("Shift+P", "P");
    focus(&mut t, Row::Input(5));
    t.type_key("Shift+P", "P");
    assert_eq!(dialog(&t).pairs.len(), 1);
    focus(&mut t, Row::Spares);
    t.key("ArrowLeft");
    let d = dialog(&t);
    assert!(d.pairs.is_empty(), "{:?}", d.pairs);
    assert_eq!(d.inputs.len(), 5);

    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    assert!(!dialog(&t).rows().contains(&Row::Spares));
}
