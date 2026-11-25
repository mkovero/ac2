//! The measurement edit token (`edit_rev`), the set `base` of `meas.eq put`, the delay and
//! delay-finding entities, and which retried requests run again, against the real daemon.

use crate::common;

use std::time::Duration;

use ac2_proto::event::{Change, Patch};
use ac2_proto::model::{EqEdit, FinderBand, MeasConfig, Measurement};
use ac2_proto::units::{Db, MeasId, Rev, Seconds};
use ac2_proto::{Command, ErrorCode, ErrorDetail, ProtoError, ReplyBody, Request};
use ac2d::{Daemon, Handle};
use common::*;

const T: Duration = Duration::from_secs(10);

fn start(name: &str) -> (Handle, Client, Sub) {
    init_log();
    let h = Daemon::start(config(manual_rig(), inproc(name))).expect("daemon");
    let (mut c, sub) = connect(&h, &[b"evt", b"ka"]);
    sub.ka(T).expect("subscribed");
    c.ok(Command::StreamOpen {
        config: stream(true),
    });
    (h, c, sub)
}

fn update(m: &Measurement, base: Rev, config: MeasConfig) -> Command {
    Command::MeasUpdate {
        meas: m.id,
        base,
        config,
    }
}

/// The current `edit_rev` a `stale_meas` refusal carries.
#[track_caller]
fn stale(e: &ProtoError, meas: MeasId) -> Rev {
    assert_eq!(e.code, ErrorCode::Conflict, "{e:?}");
    match e.detail {
        Some(ErrorDetail::StaleMeas { meas: m, edit_rev }) if m == meas => edit_rev,
        _ => panic!("{e:?}"),
    }
}

/// A whole-config write on a base another commit has moved is refused with the current
/// token, and leaves the measurement as it was.
#[test]
fn a_stale_base_is_refused_and_changes_nothing() {
    let (h, mut c, _sub) = start("edit-stale");
    let m = c.measurement(Command::MeasCreate {
        config: transfer("main"),
    });
    let moved = c.measurement(Command::MeasOffset {
        meas: m.id,
        offset: Db(2.0),
    });
    assert!(moved.edit_rev > m.edit_rev);
    let rev = c.state_rev();
    let e = c
        .call(update(&m, m.edit_rev, transfer("renamed")))
        .expect_err("a stale base");
    assert_eq!(stale(&e, m.id), moved.edit_rev);
    assert_eq!(c.meas(m.id), moved);
    assert_eq!(c.state_rev(), rev, "a refusal commits nothing");
    h.shutdown();
}

/// An accepted edit is one commit of the measurement, and its token is that commit's rev,
/// in the reply as in the state; a field edit with no base moves it as well.
#[test]
fn an_accepted_edit_raises_the_token_once() {
    let (h, mut c, sub) = start("edit-once");
    let m = c.measurement(Command::MeasCreate {
        config: spl("meter", 0),
    });
    let before = c.state_rev();
    let after = c.measurement(Command::MeasColour {
        meas: m.id,
        colour: None,
    });
    assert_eq!(after.edit_rev, Rev(before.0 + 1));
    assert_eq!(c.state_rev(), after.edit_rev);
    assert_eq!(c.meas(m.id).edit_rev, after.edit_rev);
    let e = sub
        .event(T, |e| e.rev == after.edit_rev)
        .expect("the colour's event");
    assert!(
        matches!(&e.change, Change::Measurement(Patch::Set(x)) if x.edit_rev == e.rev),
        "{e:?}"
    );
    // The whole-config write made on the reply's token goes through.
    let renamed = c.measurement(update(&m, after.edit_rev, spl("renamed", 0)));
    assert_eq!(renamed.config.name, "renamed");
    assert_eq!(renamed.edit_rev, c.state_rev());
    h.shutdown();
}

/// Each measurement has its own token: an edit of one leaves another's base good.
#[test]
fn edits_to_different_measurements_keep_their_tokens() {
    let (h, mut c, _sub) = start("edit-apart");
    let a = c.measurement(Command::MeasCreate {
        config: spl("a", 0),
    });
    let b = c.measurement(Command::MeasCreate {
        config: spl("b", 1),
    });
    c.ok(Command::MeasOffset {
        meas: a.id,
        offset: Db(1.0),
    });
    let base = c.edit_rev(a.id);
    c.measurement(update(&a, base, spl("a2", 0)));
    assert_eq!(c.edit_rev(b.id), b.edit_rev);
    let b2 = c.measurement(update(&b, b.edit_rev, spl("b2", 1)));
    assert_eq!(b2.config.name, "b2");
    h.shutdown();
}

/// Two clients on one measurement: the second write on the token both read is refused,
/// with the first's token, and goes through re-made on it.
#[test]
fn two_clients_editing_one_measurement() {
    let (h, mut a, _sub) = start("edit-two");
    let mut b = Client::connect(h.context(), h.ctrl_endpoint());
    let m = a.measurement(Command::MeasCreate {
        config: spl("shared", 0),
    });
    let read_by_b = b.meas(m.id);
    let by_a = a.measurement(update(&m, m.edit_rev, spl("from a", 0)));
    let e = b
        .call(update(&read_by_b, read_by_b.edit_rev, spl("from b", 0)))
        .expect_err("b's base is a's");
    assert_eq!(stale(&e, m.id), by_a.edit_rev);
    assert_eq!(b.meas(m.id).config.name, "from a");
    let by_b = b.measurement(update(&m, by_a.edit_rev, spl("from b", 0)));
    assert_eq!(by_b.config.name, "from b");
    assert!(by_b.edit_rev > by_a.edit_rev);
    // A field edit has no base: last write wins, whoever read what.
    a.ok(Command::MeasOffset {
        meas: m.id,
        offset: Db(-1.0),
    });
    b.ok(Command::MeasOffset {
        meas: m.id,
        offset: Db(1.5),
    });
    assert_eq!(a.meas(m.id).offset, Db(1.5));
    h.shutdown();
}

/// A `put` on a set another edit has moved is refused with the set's current rev; made on
/// that rev it goes through.
#[test]
fn an_eq_put_on_a_stale_set_rev_is_refused() {
    let (h, mut c, _sub) = start("edit-eq");
    let m = c.measurement(Command::MeasCreate {
        config: transfer("main"),
    });
    let eq = |c: &mut Client, edit| c.call(Command::MeasEq { meas: m.id, edit });
    let Ok(ReplyBody::Measurement(m1)) = eq(
        &mut c,
        EqEdit::New {
            name: None,
            realisation: None,
            peak_q: None,
            sections: Vec::new(),
        },
    ) else {
        panic!("new set");
    };
    let set = m1.eq[0].clone();
    let put = |base: u32, enabled: bool| EqEdit::Put {
        set: set.id,
        base,
        body: ac2_proto::model::EqBody {
            enabled,
            ..set.body.clone()
        },
    };
    let Ok(ReplyBody::Measurement(m2)) = eq(&mut c, put(set.rev, false)) else {
        panic!("put");
    };
    let moved = m2.eq[0].rev;
    assert!(moved > set.rev);
    let e = eq(&mut c, put(set.rev, true)).expect_err("a stale set rev");
    assert_eq!(e.code, ErrorCode::Conflict, "{e:?}");
    assert_eq!(
        e.detail,
        Some(ErrorDetail::EqConflict {
            set: set.id,
            rev: moved
        })
    );
    assert!(
        !c.meas(m.id).eq[0].body.enabled,
        "the refused put changed nothing"
    );
    eq(&mut c, put(moved, true)).expect("made on the current rev");
    assert!(c.meas(m.id).eq[0].body.enabled);
    h.shutdown();
}

/// The delay is its own entity: `set` and `nudge` move its `rev` and leave the
/// measurement's tokens, `track` commits it without moving `rev`, `find` commits a finding
/// and a typed value deletes it.
#[test]
fn delay_commands_commit_the_delay_entity() {
    let (h, mut c, sub) = start("edit-delay");
    let m = c.measurement(Command::MeasCreate {
        config: transfer("main"),
    });
    let created = sub
        .event(
            T,
            |e| matches!(&e.change, Change::Delay(Patch::Set(d)) if d.meas == m.id),
        )
        .expect("the delay follows its measurement");
    assert!(created.rev > m.edit_rev);
    let Change::Delay(Patch::Set(d0)) = created.change else {
        unreachable!()
    };
    assert_eq!(
        d0.rev, m.config_rev,
        "created under its measurement's config_rev"
    );

    let delay_event = |sub: &Sub| {
        let e = sub
            .event(T, |e| matches!(e.change, Change::Delay(_)))
            .expect("a delay event");
        match e.change {
            Change::Delay(Patch::Set(d)) => (e.rev, d),
            other => panic!("{other:?}"),
        }
    };
    let set = c.delay(Command::DelaySet {
        meas: m.id,
        delay: Seconds(0.002),
    });
    let (rev, d) = delay_event(&sub);
    assert_eq!((d.rev, rev, &d), (rev, set.rev, &set));
    let nudged = c.delay(Command::DelayNudge {
        meas: m.id,
        by: Seconds(0.001),
    });
    let (rev, d) = delay_event(&sub);
    assert_eq!((d.rev, &d), (rev, &nudged));
    assert!(nudged.rev > set.rev);
    let tracked = c.delay(Command::DelayTrack {
        meas: m.id,
        enabled: true,
    });
    let (rev, d) = delay_event(&sub);
    assert!(d.tracking && rev > nudged.rev);
    assert_eq!(tracked.rev, nudged.rev, "tracking moves no delay");
    let after = c.meas(m.id);
    assert_eq!(
        (after.edit_rev, after.config_rev),
        (m.edit_rev, m.config_rev),
        "delay commands commit no measurement"
    );

    c.ok(Command::MeasStart { meas: m.id });
    let f = c.delay_finding(Command::DelayFind {
        meas: m.id,
        band: FinderBand::Auto,
        observation: None,
    });
    assert_eq!(f.meas, m.id);
    sub.event(
        T,
        |e| matches!(&e.change, Change::DelayFinding(Patch::Set(x)) if x.meas == m.id),
    )
    .expect("the finding's event");
    assert_eq!(c.state().delay_finding(m.id), Some(&f));
    c.delay(Command::DelaySet {
        meas: m.id,
        delay: Seconds(0.001),
    });
    sub.event(
        T,
        |e| matches!(e.change, Change::DelayFinding(Patch::Deleted(x)) if x == m.id),
    )
    .expect("a typed value drops the finding");
    assert!(c.state().delay_finding(m.id).is_none());

    // Deleted: the delay goes before the measurement.
    c.ok(Command::MeasDelete {
        meas: m.id,
        traces: ac2_proto::model::OwnedTraces::Keep,
    });
    let gone = sub
        .event(T, |e| matches!(e.change, Change::Delay(Patch::Deleted(_))))
        .expect("the delay's delete");
    let meas_gone = sub
        .event(T, |e| {
            matches!(e.change, Change::Measurement(Patch::Deleted(_)))
        })
        .expect("the measurement's delete");
    assert_eq!(meas_gone.rev, Rev(gone.rev.0 + 1));
    h.shutdown();
}

/// A pure read retried under its id runs again and sees the state as it is now; an acting
/// command retried is answered from the stored reply and runs once.
#[test]
fn a_retried_read_runs_again_and_an_acting_command_replays() {
    let (h, mut c, _sub) = start("edit-retry");
    let m = c.measurement(Command::MeasCreate {
        config: spl("meter", 0),
    });
    let read = Request::new(c.next_id(), Command::StateSnapshot);
    let first = c.call_req(read.clone());
    c.ok(Command::MeasOffset {
        meas: m.id,
        offset: Db(3.0),
    });
    let again = c.call_req(read);
    let rev_of = |r: &ac2_proto::Reply| match &r.result {
        Ok(ReplyBody::Snapshot(s)) => s.rev,
        other => panic!("{other:?}"),
    };
    assert!(rev_of(&again) > rev_of(&first), "the read ran again");

    let act = Request::new(
        c.next_id(),
        Command::MeasOffset {
            meas: m.id,
            offset: Db(4.0),
        },
    );
    let first = c.call_req(act.clone());
    let rev = c.state_rev();
    let again = c.call_req(act);
    assert_eq!(first, again);
    assert_eq!(c.state_rev(), rev, "the retry committed nothing");
    h.shutdown();
}
