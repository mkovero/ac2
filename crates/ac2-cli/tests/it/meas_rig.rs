//! `ac2 meas new … --start` against a real daemon on the simulated rig: on an input the
//! open session does not capture it creates nothing, so a retry cannot make a duplicate.
#![allow(clippy::unwrap_used)]

use ac2_client::Endpoints;
use serde_json::Value;

use super::common::{self, rig};

async fn ac2(ep: &Endpoints, args: &[&str]) -> (u8, Value) {
    let r = common::ac2(ep, true, args).await;
    (r.code, r.json(args))
}

async fn ok(ep: &Endpoints, args: &[&str]) -> Value {
    let (code, v) = ac2(ep, args).await;
    assert_eq!(code, 0, "{args:?}: {v}");
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn meas_new_start_on_an_uncaptured_input_creates_nothing() {
    let (h, ep) = rig();
    let tf = |name: &'static str, start: bool| {
        let mut a = vec![
            "meas", "new", "tf", "--name", name, "--ref", "5", "--meas", "1",
        ];
        if start {
            a.push("--start");
        }
        a
    };

    // No session: a started measurement waits for one, whatever inputs it names.
    let m = ok(&ep, &tf("early", true)).await;
    assert_eq!(m["running"], true, "{m}");
    ok(&ep, &["meas", "rm", "early"]).await;

    ok(
        &ep,
        &["session", "open", "--backend", "fake", "--in", "1-4"],
    )
    .await;
    let before = ok(&ep, &["meas", "list"]).await;
    let (code, e) = ac2(&ep, &tf("TF 2", true)).await;
    assert_eq!(code, 1, "{e}");
    assert_eq!(
        e["error"]["msg"], "input 5 is not captured by the audio I/O",
        "{e}"
    );
    assert_eq!(ok(&ep, &["meas", "list"]).await, before);

    // Without --start a stopped measurement is made; the daemon refuses its start in the
    // same words, numbering the input as the operator does.
    ok(&ep, &tf("TF 2", false)).await;
    let (code, e) = ac2(&ep, &["meas", "start", "TF 2"]).await;
    assert_eq!(code, 1, "{e}");
    assert_eq!(e["error"]["code"], "invalid", "{e}");
    assert_eq!(
        e["error"]["msg"], "invalid: input 5 is not captured by the session",
        "{e}"
    );
    h.shutdown();
}
