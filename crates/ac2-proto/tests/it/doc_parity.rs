//! `docs/protocol.md` names every command, reply body, error code, event kind, frame header
//! field, frame kind, array name, unit and bitmask flag the code defines.
//!
//! The name sets come from serde itself (the "expected one of …" list of an unknown
//! variant / field error), so a new variant or field cannot be missed by this test.

use std::collections::BTreeSet;

use ac2_proto::event::WireEvent;
use ac2_proto::frame::{
    ArrayName, ClipFlags, FrameHeader, FrameKind, LeqFlags, ProtectionFlags, Unit, ValidityMask,
};
use ac2_proto::*;
use serde::de::DeserializeOwned;

fn doc() -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/protocol.md");
    std::fs::read_to_string(p).expect("docs/protocol.md and probes")
}

fn expected_names(err: &str) -> BTreeSet<String> {
    // "expected one of `a`, `b`, …", or "expected `a` or `b`" for two names.
    let tail = err
        .split("expected one of")
        .nth(1)
        .or_else(|| err.split(", expected").nth(1))
        .unwrap_or_else(|| panic!("no name list in {err:?}"));
    tail.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

fn names_of<T: DeserializeOwned + std::fmt::Debug, P: serde::Serialize>(
    probe: &P,
) -> BTreeSet<String> {
    let b = rmp_serde::to_vec_named(probe).expect("docs/protocol.md and probes");
    let err = rmp_serde::from_slice::<T>(&b)
        .expect_err("probe must be refused")
        .to_string();
    expected_names(&err)
}

#[derive(serde::Serialize)]
struct Tagged<'a> {
    #[serde(rename = "type")]
    t: &'a str,
}

#[derive(serde::Serialize)]
struct KindTagged<'a> {
    kind: &'a str,
}

#[derive(serde::Serialize)]
struct OpTagged<'a> {
    op: &'a str,
}

#[derive(serde::Serialize)]
struct Bogus {
    bogus_field: u8,
}

fn check(group: &str, names: &BTreeSet<String>, doc: &str, missing: &mut Vec<String>) {
    assert!(!names.is_empty(), "{group}: no names extracted");
    for n in names {
        if !doc.contains(&format!("`{n}`")) && !doc.contains(&format!("`{n}:")) {
            missing.push(format!("{group}: {n}"));
        }
    }
}

#[test]
fn protocol_doc_names_everything() {
    let doc = doc();
    let mut missing = Vec::new();

    let commands = names_of::<Command, _>(&OpTagged { op: "no.such" });
    assert_eq!(commands.len(), samples::commands().len());
    check("command", &commands, &doc, &mut missing);
    check(
        "reply body",
        &names_of::<ReplyBody, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "error code",
        &names_of::<ErrorCode, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check(
        "error detail",
        &names_of::<ErrorDetail, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "event kind",
        &names_of::<WireEvent, _>(&KindTagged { kind: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "frame header field",
        &names_of::<FrameHeader, _>(&Bogus { bogus_field: 0 }),
        &doc,
        &mut missing,
    );
    check(
        "frame kind",
        &names_of::<FrameKind, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check(
        "array name",
        &names_of::<ArrayName, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check("unit", &names_of::<Unit, _>(&"no_such"), &doc, &mut missing);
    check(
        "cal status",
        &names_of::<model::CalStatus, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "curve choice",
        &names_of::<model::CurveChoice, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "mic-curve file reason",
        &names_of::<ac2_proto::MicCurveFileReason, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check(
        "finder band",
        &names_of::<model::FinderBand, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "delay outcome",
        &names_of::<model::DelayOutcome, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "no-estimate reason",
        &names_of::<model::NoEstimateReason, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "ambiguity reason",
        &names_of::<model::AmbiguityReason, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check(
        "delay pick",
        &names_of::<model::DelayPick, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "leq judgement",
        &names_of::<model::LeqJudgement, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check(
        "spl log which",
        &names_of::<model::SplLogWhich, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check(
        "leq alarm kind",
        &names_of::<model::LeqAlarmKind, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    for (group, fields) in [
        (
            "leq window field",
            names_of::<model::LeqWindow, _>(&Bogus { bogus_field: 0 }),
        ),
        (
            "spl log row field",
            names_of::<model::SplLogRow, _>(&Bogus { bogus_field: 0 }),
        ),
        (
            "leq meta field",
            names_of::<frame::LeqMeta, _>(&Bogus { bogus_field: 0 }),
        ),
        (
            "leq run field",
            names_of::<frame::LeqRun, _>(&Bogus { bogus_field: 0 }),
        ),
    ] {
        check(group, &fields, &doc, &mut missing);
    }
    check(
        "depth policy",
        &names_of::<model::DepthPolicy, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "math operand status",
        &names_of::<frame::OperandStatus, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "math reference",
        &names_of::<model::MathReference, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "math expression",
        &names_of::<model::MathExpr, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "math operand",
        &names_of::<model::Operand, _>(&Tagged { t: "no_such" }),
        &doc,
        &mut missing,
    );
    check(
        "math operator",
        &names_of::<model::MathOp, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check(
        "math domain",
        &names_of::<model::MathDomain, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    check(
        "phase basis",
        &names_of::<model::PhaseBasis, _>(&"no_such"),
        &doc,
        &mut missing,
    );
    for (group, fields) in [
        (
            "delay finding field",
            names_of::<model::DelayFinding, _>(&Bogus { bogus_field: 0 }),
        ),
        (
            "delay arrival field",
            names_of::<model::DelayArrival, _>(&Bogus { bogus_field: 0 }),
        ),
        (
            "delay confidence field",
            names_of::<model::DelayConfidence, _>(&Bogus { bogus_field: 0 }),
        ),
    ] {
        check(group, &fields, &doc, &mut missing);
    }

    for (group, flags) in [
        ("validity bit", ValidityMask::NAMED),
        ("protection bit", ProtectionFlags::NAMED),
        ("clip bit", ClipFlags::NAMED),
        ("leq flag", LeqFlags::NAMED),
    ] {
        for (name, bit) in flags {
            if !doc.contains(&format!("`{name}` {bit}")) {
                missing.push(format!("{group}: `{name}` {bit}"));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "docs/protocol.md lacks:\n{}",
        missing.join("\n")
    );
}

#[test]
fn protocol_doc_states_the_version_and_bounds() {
    let doc = doc();
    assert!(doc.contains(&format!("`PROTO_VERSION = {}`", PROTO_VERSION)));
    assert!(doc.contains(&format!("≤ {} bytes", frame::MAX_HEADER_BYTES)));
    assert!(doc.contains(&format!("≤ {}", frame::MAX_N)));
    assert!(doc.contains(&format!("`0x{}`", samples::log_grid().id())));
}
