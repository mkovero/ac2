# Data flow control: pacing each client to what it consumes

Status: design, not implemented. The current publisher is described in
[`q2-q5-q6-protocol.md`](q2-q5-q6-protocol.md) (Q2).

## Problem

The daemon's I/O thread keeps a latest slot per topic and sends a dirty slot at most at the
publish rate (60 fps local, 30 network). The data socket is an XPUB, which queues per peer
up to `SNDHWM` messages and then silently drops the *newest* message for that peer. For a
peer that reads slower than the publish rate (a laptop on Wi-Fi, a Pi's uplink) the queue
is always full:

- every frame it receives waited behind a full queue: the standing delay is
  `SNDHWM × frame size / link rate` (48 frames of 20–500 KiB at 2–5 MB/s: 0.2 s to
  several seconds);
- which frames get in is decided by the moment space frees up, not by need: a burst of
  large frames can keep small SPL frames, events and keepalives out for a while;
- the latest-slot conflation does nothing for that peer, because a send to the XPUB
  "succeeds" whether or not the slow peer's queue took the message.

The XPUB gives no per-peer feedback (`try_send` succeeds when any matching peer took the
message; `XPUB_NODROP` turns one full peer into back-pressure on all of them), so the
remedy needs a socket that addresses peers individually.

## Design

Credit-based pacing on a ROUTER data socket.

- **Socket.** The data socket becomes a ROUTER (CURVE + ZAP as today); each client
  connects a DEALER for data. The daemon tracks subscriptions itself (the prefix matching
  `SubscriptionTracker` already does, per peer) instead of relying on XPUB filtering.
  `Interest` becomes the union over peers.
- **Credit.** A client grants credit in messages: `credit(n)` on its data DEALER. It
  starts with a window of W (≈ 4) and returns one credit per message it has *consumed*
  (taken by `latest()`, or by its I/O thread for `evt`/`ka`). The daemon keeps per peer:
  remaining credit, the set of dirty topics for that peer, and the rotation position.
- **Send.** When a slot changes it is marked dirty for every subscribed peer. A peer with
  credit gets its dirty topics in rotation (events and keepalives first, then frames
  oldest-dirty first), one credit each, each topic at most at the publish rate. A peer
  without credit gets nothing; its dirty marks just keep pointing at the newest slot, so
  when credit returns it receives the newest frame of each topic, never a backlog.
- **Bound.** At most W messages are in flight per peer, so the standing delay is
  `W × frame size / link rate` regardless of the publish rate, and fast peers are never
  held back by slow ones. `SNDHWM` stays as a safety net above W.
- **Events.** Events must not be lost. They are queued per peer (bounded; a peer that
  falls further behind than the bound is told `resync_required` through its next
  keepalive and resynchronises with a snapshot, as after a gap today).
- **Late joiners.** A new subscription marks the matching slots dirty for that peer only,
  so a burst goes to the newcomer alone and is paced by its credit.
- **Liveness.** A peer whose credit stays at zero is still disconnected by the kernel
  dead-peer detection (keepalive, unacknowledged-data limit) if it is gone; a live but
  stalled reader simply stops receiving.

## Cost and wire impact

- New messages `credit(n)` (client → daemon) and the ROUTER/DEALER framing on the data
  channel: a wire change (bumps `PROTO_VERSION`, fixtures, the Python cross-language
  tests).
- The daemon's I/O thread does per-peer bookkeeping (credit, dirty set per peer): O(peers ×
  topics) bits, negligible next to encoding.
- One extra small message upstream per consumed frame; at 60 fps × 40 topics this is
  ~2400 small messages/s locally, so credits are batched (a client returns credit at most
  every few milliseconds, as one `credit(n)`).

## Embedded daemon

In-process the transport is `inproc://` in the daemon's context (no socket, no copy of the
encoded parts). A further step would hand the UI the `Arc<Frame>` the job produced and
skip encoding and decoding altogether: the I/O thread would keep a typed latest slot
beside the encoded one and an in-process subscriber would take frames from it directly.
That needs a typed sink beside the client protocol (the UI's link and mirror unchanged,
only `latest()` fed from the sink); it is left until profiling on the embedded path shows
encode/decode, rather than DSP or rendering, to matter.
