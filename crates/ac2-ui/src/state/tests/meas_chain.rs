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
        rev: Rev(before.edit_rev.0 + 1),
        config: sent,
        ..before.clone()
    };
    let r = t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        link: 0,
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
        link: 0,
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

/// The daemon's reply to the `meas.update` in `r`: committed on its base, as it was sent.
fn commit(t: &mut T, r: &[Request]) -> Vec<Request> {
    let Some((Command::MeasUpdate { meas, base, config }, what)) = calls(r)
        .into_iter()
        .find(|(c, _)| matches!(c, Command::MeasUpdate { .. }))
    else {
        panic!("no meas.update in {r:?}")
    };
    let m = Measurement {
        edit_rev: Rev(base.0 + 1),
        rev: Rev(base.0 + 1),
        config: config.clone(),
        ..t.st.rig.meas(*meas).expect("mirrored").clone()
    };
    let (meas, what) = (*meas, what.to_owned());
    t.conn(ConnEvent::MeasEdited {
        meas,
        link: 0,
        what,
        result: Ok(Some(Box::new(m))),
    })
}

fn update_what(r: &[Request]) -> String {
    calls(r)
        .into_iter()
        .find(|(c, _)| matches!(c, Command::MeasUpdate { .. }))
        .map(|(_, w)| w.to_owned())
        .expect("a meas.update")
}

fn stale(t: &mut T, what: &str) -> Vec<Request> {
    t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        link: 0,
        what: what.into(),
        result: Err(MeasRefusal::Stale("conflict: stale measurement".into())),
    })
}

fn toasts(t: &T) -> Vec<String> {
    t.st.toasts.list.iter().map(|t| t.text.clone()).collect()
}

/// K pressed twice before the first step is answered: two steps, the second made on the
/// first, and each in turn on the token the reply before it carries.
#[test]
fn a_step_key_pressed_twice_fast_steps_twice() {
    let mut t = T::new();
    let start = transfer_of(&t.st.meas(MeasId(1)).expect("Main L").config).smoothing;
    let first = t.key("K");
    let one = transfer_of(update(&first).expect("goes at once").1).smoothing;
    assert_ne!(one, start);
    assert!(update(&t.key("K")).is_none(), "waits for the first reply");
    let r = commit(&mut t, &first);
    let (base, config) = update(&r).expect("the second goes");
    assert_eq!(base, Rev(update(&first).expect("first").0.0 + 1));
    let two = transfer_of(config).smoothing;
    assert_ne!(two, one, "a second step, not the first one again");
    assert_ne!(two, start);
}

/// While a step is unanswered, the list of measurements reads it as the lookup does: what
/// lists them (the G cycle's band meter, the tree) never sees an older measurement.
#[test]
fn the_measurement_list_reads_an_unanswered_edit_as_the_lookup_does() {
    let mut t = T::new();
    let sent = t.key("K");
    let stepped = transfer_of(update(&sent).expect("goes at once").1).smoothing;
    let looked_up = transfer_of(&t.st.meas(MeasId(1)).expect("Main L").config).smoothing;
    assert_eq!(looked_up, stepped);
    let listed =
        t.st.measurements()
            .into_iter()
            .find(|m| m.id == MeasId(1))
            .map(|m| transfer_of(&m.config).smoothing);
    assert_eq!(listed, Some(stepped));
}

/// B round its three styles before any reply ends on the style it began with.
#[test]
fn a_cycle_round_before_any_reply_ends_where_it_began() {
    let mut t = T::new();
    t.go(PaneKind::Spectrum);
    let style = |c: &MeasConfig| match &c.kind {
        MeasKind::Spectrum { config } => config.display.style,
        other => panic!("{other:?}"),
    };
    let start = style(&t.st.meas(MeasId(2)).expect("Sub").config);
    let mut r = t.key("B");
    assert!(update(&t.key("B")).is_none());
    assert!(update(&t.key("B")).is_none());
    let mut seen = vec![style(update(&r).expect("goes").1)];
    for _ in 0..2 {
        r = commit(&mut t, &r);
        seen.push(style(update(&r).expect("the next goes").1));
    }
    assert_eq!(seen[2], start, "{seen:?}");
    assert_ne!(seen[0], seen[1]);
}

/// The mirror's copy of measurement 1 replaced by `m`.
fn mirrored(t: &mut T, m: &Measurement) -> Vec<Request> {
    let mut s = t.st.daemon().expect("mirrored").clone();
    let i = (s.measurements.iter().position(|x| x.id == m.id)).expect("listed");
    s.measurements[i] = m.clone();
    t.conn(mirror(s))
}

/// A step made while its measurement's panes are being stored waits for that call, then goes
/// on the token it was made on: storing panes leaves the configuration, so it leaves the
/// token, whether the layout's event comes before its reply or after.
#[test]
fn an_update_made_while_a_layout_is_in_flight_goes_on_its_token() {
    let mut t = T::new();
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    let before = t.st.meas(MeasId(1)).expect("Main L").clone();
    let n = t.key("N");
    let Some((Command::MeasLayout { layout, .. }, what)) = calls(&n)
        .into_iter()
        .find(|(c, _)| matches!(c, Command::MeasLayout { .. }))
    else {
        panic!("no meas.layout in {n:?}")
    };
    let laid_out = Measurement {
        layout: layout.clone(),
        rev: Rev(before.rev.0 + 1),
        ..before.clone()
    };
    let what = what.to_owned();
    assert!(update(&t.key("K")).is_none(), "waits for the layout");
    // The event first, then the reply: the order the local pair often brings them in.
    let mut r = mirrored(&mut t, &laid_out);
    r.extend(t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        link: 0,
        what,
        result: Ok(Some(Box::new(laid_out))),
    }));
    assert_eq!(update(&r).map(|u| u.0), Some(before.edit_rev), "{r:?}");
}

/// A second step waiting on the first, whose event reaches the mirror before its reply: the
/// reply is then no news, and the second goes on the token the mirror shows.
#[test]
fn a_waiting_update_takes_the_token_the_mirror_shows_before_the_reply() {
    let mut t = T::new();
    let before = t.st.meas(MeasId(1)).expect("Main L").clone();
    let first = t.key("K");
    let (_, sent) = update(&first).expect("goes at once");
    let committed = Measurement {
        edit_rev: Rev(before.edit_rev.0 + 1),
        rev: Rev(before.edit_rev.0 + 1),
        config: sent.clone(),
        ..before
    };
    let what = update_what(&first);
    assert!(update(&t.key("Alt+K")).is_none(), "waits for the first");
    let mut r = mirrored(&mut t, &committed);
    r.extend(t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        link: 0,
        what,
        result: Ok(Some(Box::new(committed.clone()))),
    }));
    assert_eq!(update(&r).map(|u| u.0), Some(committed.edit_rev), "{r:?}");
}

/// A layout call dropped behind a stale refusal is answered: the next layout of the
/// measurement is sent, not held behind a call that never comes back.
#[test]
fn a_stale_refusal_answers_the_layout_waiting_behind_it() {
    let mut t = T::new();
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    let first = t.key("K");
    assert!(update(&t.key("Alt+K")).is_none());
    let layouts = |r: &[Request]| {
        calls(r)
            .into_iter()
            .filter(|(c, _)| matches!(c, Command::MeasLayout { .. }))
            .count()
    };
    assert_eq!(layouts(&t.key("N")), 0, "held behind the update in flight");
    let what = update_what(&first);
    stale(&mut t, &what);
    assert!(
        toasts(&t).contains(&ac2_scene::osc::edits_dropped("Main L", 2)),
        "{:?}",
        toasts(&t)
    );
    assert_eq!(layouts(&t.key("N")), 1, "the next layout is stored");
}

/// An EQ editor `put` dropped behind a stale refusal is answered: the editor shows the
/// daemon's set again and its next step goes.
#[test]
fn a_stale_refusal_answers_the_eq_put_waiting_behind_it() {
    let mut t = T::new();
    super::eq_edits::with_set(&mut t);
    t.go(PaneKind::Transfer);
    let first = t.key("K");
    t.key("Alt+E");
    let puts = |r: &[Request]| {
        calls(r)
            .into_iter()
            .filter(|(c, _)| matches!(c, Command::MeasEq { .. }))
            .count()
    };
    assert_eq!(
        puts(&t.key("Shift+B")),
        0,
        "held behind the update in flight"
    );
    let what = update_what(&first);
    stale(&mut t, &what);
    let Overlay::Eq(e) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(e.pending, 0);
    assert!(e.refusal.is_some());
    assert_eq!(puts(&t.key("Shift+B")), 1, "the next step goes");
}

/// The link lost with edits waiting: none of them is sent to the daemon answering next,
/// one warning says how many were not, and the next edit goes at once.
#[test]
fn a_lost_link_drops_the_waiting_edits() {
    let mut t = T::new();
    let state = t.st.daemon().expect("mirrored").clone();
    let first = t.key("K");
    assert!(update(&t.key("Alt+K")).is_none());
    assert!(update(&t.key("Alt+Shift+K")).is_none());
    let mut out = t.conn(ConnEvent::Failed {
        down: ac2_scene::chrome::RigDown::Unreachable,
        target: "local daemon".into(),
        error: "lost".into(),
        retry_in: std::time::Duration::from_secs(1),
    });
    t.connect(state.clone());
    out.extend(t.conn(mirror_of(state, 2, Some("c1"))));
    // The edit in flight ends with the old link.
    out.extend(t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        link: 0,
        what: update_what(&first),
        result: Err(MeasRefusal::Failed("closed".into())),
    }));
    assert!(update(&out).is_none(), "{out:?}");
    assert!(
        toasts(&t).contains(&ac2_scene::osc::edits_unsent_link(2)),
        "{:?}",
        toasts(&t)
    );
    let mirror_rev = t.st.meas(MeasId(1)).expect("Main L").edit_rev;
    assert_eq!(update(&t.key("K")).map(|u| u.0), Some(mirror_rev));
}

/// Answers to edits sent before the link was lost are told from answers to edits sent
/// since by the link they went on, not by their count: a new edit's answer that comes
/// first moves the chain, and the late answer to the old one moves nothing.
#[test]
fn a_late_answer_from_the_lost_link_moves_no_chain() {
    let mut t = T::new();
    let state = t.st.daemon().expect("mirrored").clone();
    let old = t.key("K");
    assert!(update(&old).is_some());
    let mut out = t.conn(ConnEvent::Failed {
        down: ac2_scene::chrome::RigDown::Unreachable,
        target: "local daemon".into(),
        error: "lost".into(),
        retry_in: std::time::Duration::from_secs(1),
    });
    t.connect(state.clone());
    out.extend(t.conn(mirror_of(state, 2, Some("c1"))));
    assert!(
        out.iter().any(|r| matches!(r, Request::EditLink(1))),
        "{out:?}"
    );

    let new = t.key("K");
    assert!(update(&new).is_some(), "nothing in flight on the new link");
    t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        link: 1,
        what: update_what(&new),
        result: Ok(None),
    });
    let third = t.key("Alt+K");
    assert!(update(&third).is_some(), "the new edit was answered");
    t.conn(ConnEvent::MeasEdited {
        meas: MeasId(1),
        link: 0,
        what: update_what(&old),
        result: Err(MeasRefusal::Failed("closed".into())),
    });
    assert!(
        update(&t.key("Alt+K")).is_none(),
        "the third is still in flight"
    );
}

/// The SPL / Leq page applied on the rev it was opened on, which another client has moved
/// since, while an edit of the meter is in flight: it goes on that older rev, for the
/// daemon to refuse, never on the token the reply brings.
#[test]
fn a_leq_apply_on_an_older_rev_goes_as_made() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    t.st.apply(Msg::SelectMeas(MeasId(4)), &t.keys);
    let colour = t.key("Shift+C");
    assert_eq!(calls(&colour).len(), 1);
    t.type_key("Shift+L", "L");
    let opened = t.st.overlay.leq().expect("the page").base_rev;
    // Elsewhere: the meter's configuration moves on.
    let mut s = with_spl();
    s.measurements[2].edit_rev = Rev(opened.0 + 5);
    if let MeasKind::Spl { config } = &mut s.measurements[2].config.kind {
        config.time_weighting = ac2_proto::model::TimeWeighting::Slow;
    }
    t.conn(mirror(s.clone()));
    assert!(update(&t.key("Enter")).is_none(), "waits for the colour");
    let m = s.measurements[2].clone();
    let r = t.conn(ConnEvent::MeasEdited {
        meas: MeasId(4),
        link: 0,
        what: calls(&colour)[0].1.to_owned(),
        result: Ok(Some(Box::new(m))),
    });
    assert_eq!(update(&r).map(|u| u.0), Some(opened), "{r:?}");
}
