//! Reducer tests of the edits of one measurement: a whole-config write waits for the reply
//! of the edit before it and is made on the measurement that reply carries; a `stale_meas`
//! refusal drops what waits behind it and says so.

use super::*;
use crate::conn::MeasRefusal;

fn update(r: &[Request]) -> Option<(Rev, &MeasConfig)> {
    calls(r).into_iter().find_map(|(c, _)| match c {
        Command::MeasUpdate { base, config, .. } => Some((*base, config)),
        _ => None,
    })
}

fn transfer_of(c: &MeasConfig) -> &ac2_proto::model::TransferConfig {
    match &c.kind {
        MeasKind::Transfer { config } => config,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_second_update_is_made_on_the_first_ones_reply() {
    let mut t = T::new();
    let before = t.st.meas(MeasId(1)).expect("Main L").clone();
    let first = t.key("K");
    let (base, sent) = update(&first).expect("the smoothing goes at once");
    assert_eq!(base, before.edit_rev);
    let sent = sent.clone();
    // Made before the first is answered: it waits.
    assert!(update(&t.key("Alt+K")).is_none());

    // The reply: committed under a newer token, which the mirror does not hold yet.
    let committed = Measurement {
        edit_rev: Rev(before.edit_rev.0 + 1),
        config: sent,
        ..before.clone()
    };
    let r = t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        what: "Main L: smoothing 1/48 oct".into(),
        result: Ok(Some(Box::new(committed.clone()))),
    });
    let (base, config) = update(&r).expect("the waiting one goes");
    assert_eq!(base, committed.edit_rev);
    let (now, was) = (transfer_of(config), transfer_of(&committed.config));
    assert_eq!(
        now.smoothing, was.smoothing,
        "the first edit's change is kept"
    );
    assert_ne!(
        now.resolution, was.resolution,
        "the second edit's change is made"
    );
}

#[test]
fn a_stale_refusal_drops_the_waiting_edits_and_says_so() {
    let mut t = T::new();
    assert!(update(&t.key("K")).is_some());
    assert!(update(&t.key("Alt+K")).is_none());
    assert!(update(&t.key("Alt+K")).is_none());
    let r = t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        what: "Main L: smoothing 1/48 oct".into(),
        result: Err(MeasRefusal::Stale("conflict: stale measurement".into())),
    });
    assert!(update(&r).is_none(), "{r:?}");
    let texts: Vec<_> = t.st.toasts.list.iter().map(|t| t.text.as_str()).collect();
    assert!(
        texts.contains(&"Main L: smoothing 1/48 oct: changed elsewhere — reloaded"),
        "{texts:?}"
    );
    assert!(
        texts.contains(&ac2_scene::osc::edits_dropped("Main L", 2).as_str()),
        "{texts:?}"
    );
    // Nothing outstanding: the next edit goes at once, on the mirror.
    let mirror_rev = t.st.meas(MeasId(1)).expect("Main L").edit_rev;
    assert_eq!(update(&t.key("K")).map(|u| u.0), Some(mirror_rev));
}

#[test]
fn field_edits_are_not_held() {
    let mut t = T::new();
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert_eq!(calls(&t.key("Shift+C")).len(), 1);
    assert_eq!(
        calls(&t.key("Shift+C")).len(),
        1,
        "colour is last write wins"
    );
}
