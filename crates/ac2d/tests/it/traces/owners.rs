//! Trace ownership: capture filing, moves and measurement deletion.

use super::*;

/// Who owns what: a capture is filed under its measurement, a math channel's capture under
/// the math channel's owner, an average with its inputs' common owner; a trace moves by
/// `trace.update`, a math channel by `meas.update`; deleting a measurement keeps what it
/// owns (in the imported group) or deletes it, and refuses while a math channel that stays
/// would lose an operand.
#[test]
fn owners_capture_move_and_delete() {
    let mut r = rig("owners");
    let c = &mut r.c;
    let main = TraceOwner::Meas { meas: MeasId(1) };
    let cap = |c: &mut Client, meas: MeasId, name: &str| {
        trace(c.ok(Command::TraceCapture {
            meas,
            name: name.into(),
            slot: None,
        }))
    };
    let pre = cap(c, MeasId(1), "pre-EQ");
    let post = cap(c, MeasId(1), "post-EQ");
    assert_eq!((pre.edit.owner, post.edit.owner), (main, main));

    // A math channel made on the measurement: listed and captured under it.
    let math = match c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "pre ÷ post".into(),
            kind: MeasKind::Math {
                config: MathConfig::of(
                    main,
                    MathDomain::Transfer,
                    MathExpr::Binary {
                        a: Operand::Trace { trace: pre.id },
                        op: MathOp::Divide,
                        b: Operand::Trace { trace: post.id },
                    },
                ),
            },
        },
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    c.ok(Command::MeasStart { meas: math.id });
    for _ in 0..2 {
        run(&mut r.d, 0.5);
        c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let captured = cap(c, math.id, "pre ÷ post 21:04");
    assert_eq!(captured.edit.owner, main);
    let avg = trace(c.ok(Command::TraceAverage {
        traces: vec![pre.id, post.id],
        method: AverageMethod::Power,
        reference: DelayReference::Trace { trace: pre.id },
        name: "avg".into(),
    }));
    assert_eq!(avg.edit.owner, main);

    // Owners must exist and must not be math channels; math cannot own math.
    let mv = |c: &mut Client, t: &TraceMeta, owner: TraceOwner| {
        let mut e = t.edit.clone();
        e.owner = owner;
        c.call(Command::TraceUpdate {
            trace: t.id,
            edit: e,
        })
    };
    let e = mv(c, &avg, TraceOwner::Meas { meas: MeasId(99) }).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "{}", e.msg);
    let e = mv(c, &avg, TraceOwner::Meas { meas: math.id }).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{}", e.msg);
    let avg = trace(mv(c, &avg, TraceOwner::Imported).unwrap());
    assert_eq!(avg.edit.owner, TraceOwner::Imported);
    let mut under_math = MathConfig::of(
        TraceOwner::Meas { meas: math.id },
        MathDomain::Transfer,
        MathExpr::Binary {
            a: Operand::Trace { trace: pre.id },
            op: MathOp::Divide,
            b: Operand::Trace { trace: avg.id },
        },
    );
    let e = c
        .call(Command::MeasCreate {
            config: MeasConfig {
                name: "nested".into(),
                kind: MeasKind::Math {
                    config: under_math.clone(),
                },
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{}", e.msg);

    // A math channel moves without restarting: its config rev stays.
    let MeasKind::Math { config } = &math.config.kind else {
        unreachable!()
    };
    let moved = match c.ok(Command::MeasUpdate {
        meas: math.id,
        config: MeasConfig {
            name: math.config.name.clone(),
            kind: MeasKind::Math {
                config: MathConfig {
                    owner: TraceOwner::Imported,
                    ..config.clone()
                },
            },
        },
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    let before_rev = state(c)
        .measurements
        .iter()
        .find(|m| m.id == math.id)
        .unwrap()
        .config_rev;
    assert_eq!(moved.config_rev, before_rev);
    assert!(moved.running);
    // Back under the measurement for the delete below.
    c.ok(Command::MeasUpdate {
        meas: math.id,
        config: math.config.clone(),
    });

    // A second measurement and a math channel elsewhere computing from one of its traces.
    c.ok(Command::MeasCreate {
        config: transfer("side"),
    });
    let side = state(c).measurements.iter().map(|m| m.id).max().unwrap();
    c.ok(Command::MeasStart { meas: side });
    for _ in 0..2 {
        run(&mut r.d, 0.5);
        c.ok(Command::GenRefresh { lease_token: r.tok });
    }
    let s1 = cap(c, side, "side 1");
    under_math.owner = TraceOwner::Imported;
    under_math.expr = MathExpr::Binary {
        a: Operand::Trace { trace: s1.id },
        op: MathOp::Divide,
        b: Operand::Trace { trace: avg.id },
    };
    under_math.reference = MathReference::Operand {
        operand: Operand::Trace { trace: s1.id },
    };
    let user = match c.ok(Command::MeasCreate {
        config: MeasConfig {
            name: "side ÷ avg".into(),
            kind: MeasKind::Math { config: under_math },
        },
    }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    // Deleting side with its traces would take an operand from a math channel that stays.
    let e = c
        .call(Command::MeasDelete {
            meas: side,
            traces: OwnedTraces::Delete,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Refused, "{}", e.msg);
    assert!(e.msg.contains("side ÷ avg"), "{}", e.msg);
    // Kept, they move to the imported group and the math channel keeps its operand.
    c.ok(Command::MeasDelete {
        meas: side,
        traces: OwnedTraces::Keep,
    });
    let st = state(c);
    assert_eq!(
        st.traces.iter().find(|t| t.id == s1.id).unwrap().edit.owner,
        TraceOwner::Imported
    );
    c.ok(Command::MeasDelete {
        meas: user.id,
        traces: OwnedTraces::Keep,
    });

    // The main measurement with its traces: the math channel under it goes too.
    c.ok(Command::MeasDelete {
        meas: MeasId(1),
        traces: OwnedTraces::Delete,
    });
    let st = state(c);
    assert!(
        st.measurements
            .iter()
            .all(|m| m.id != MeasId(1) && m.id != math.id)
    );
    let left: Vec<&str> = st.traces.iter().map(|t| t.edit.name.as_str()).collect();
    assert_eq!(left, ["avg", "side 1"]);
}
