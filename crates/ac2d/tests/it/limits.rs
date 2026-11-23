//! Size bounds the daemon holds at its sockets.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::time::Duration;

use ac2_proto::units::{Blob, TransferId};
use ac2_proto::{Command, ErrorCode, MAX_CTRL_BYTES, Request, TRANSPORT_SLACK_BYTES};
use ac2_zmq::Context;
use ac2d::Daemon;
use common::*;

fn chunk(c: &mut Client, bytes: usize) -> Request {
    Request::new(
        c.next_id(),
        Command::FileUploadChunk {
            transfer: TransferId(1),
            offset: 0,
            data: Blob(vec![7; bytes]),
        },
    )
}

/// A request a little over the ctrl bound is read and answered `too_large`; one past the
/// transport's slack never reaches the daemon (its connection closes), and the daemon goes
/// on answering others.
#[test]
fn a_ctrl_part_past_the_bound_closes_its_connection() {
    init_log();
    let dir = tempfile::tempdir().unwrap();
    let h = Daemon::start(config(manual_rig(), local(dir.path()))).unwrap();
    let ctx = Context::new().unwrap();

    let mut c = Client::connect(&ctx, h.ctrl_endpoint());
    let near = chunk(&mut c, MAX_CTRL_BYTES);
    let r = c.call_req(near);
    assert_eq!(r.result.unwrap_err().code, ErrorCode::TooLarge);

    let far = chunk(&mut c, MAX_CTRL_BYTES + TRANSPORT_SLACK_BYTES);
    c.send(&far);
    assert!(
        c.recv(Duration::from_secs(1)).is_none(),
        "a part past the transport bound was read"
    );

    let mut other = Client::connect(&ctx, h.ctrl_endpoint());
    other.ok(Command::Hello {
        client: "limits test".into(),
    });
}
