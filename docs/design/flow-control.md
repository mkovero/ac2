# Data flow control: pacing each client to what it consumes

Status: built (PROTO 83, [`wire-delta.md`](wire-delta.md) stage C, W10); normative in
`docs/protocol.md` §5.8. The publisher before it is described in
[`q2-q5-q6-protocol.md`](q2-q5-q6-protocol.md) (Q2).

## Problem

The daemon's I/O thread keeps a latest slot per topic and sends a dirty slot at most at the
publish rate (60 fps, local and network). The data socket was an XPUB, which queues per
peer up to `SNDHWM` messages and then silently drops the *newest* message for that peer.
For a peer that reads slower than the publish rate (a laptop on Wi-Fi, a Pi's uplink) the
queue is always full:

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

- **Socket.** The data socket is a ROUTER (CURVE + ZAP as on ctrl); each client connects
  a DEALER for data and never sets a routing id, so a reconnect is a new peer. The daemon
  tracks subscriptions itself, per peer (`ac2d/src/peers.rs`); `Interest` is the union
  over peers, counted. Subscribing, unsubscribing and keyframe requests travel on the
  DEALER, not ctrl: the per-peer state is keyed by the data connection, which ctrl's
  connection and CURVE identity cannot name, and on the DEALER they stay ordered with the
  credit and vanish with the peer.
- **Credit in bytes.** A client states a window (default 64 KiB, at least the largest
  frame) and returns, on receipt, the bytes it read (each frame charged at least 64 B)
  and the count of events, keepalives and notices: once per read burst, and mid-burst
  whenever half the window is owed. Bytes, not messages: message credit caps throughput
  at W/RTT messages whatever their size, bytes cap it at window/RTT and bound the delay to
  window / link rate.
- **Send.** A peer with credit is served its dirty topics in rotation from where its last
  round stopped, one frame each, the newest slot, each topic at most at the publish rate.
  A peer without credit gets nothing; its topics' slots keep only the newest frame, so
  when credit returns it receives the newest frame of each topic, never a backlog. A
  frame that met an exhausted window is held, formed, and goes first when credit comes,
  unless its slot moved on.
- **Chains.** Each (peer, topic) keeps an `Arc` to the frame last handed over; its delta
  is taken against exactly that, so a slow peer gets deltas over a longer distance.
  Encodes are cached by (topic, frame, base) within one send round, so peers in step
  share one. Keyframe triggers are per peer.
- **Bound.** At most the window is in flight per peer, so the standing delay is
  `window / link rate` plus at most one rotation round, and fast peers are never held
  back by slow ones. `SNDHWM` is twice what credit and the event bound let be outstanding
  (`256 KiB ÷ 64 B + 1 + 256`): libzmq tells the writer what the connection took only
  every half queue, so the writer may count up to half a queue too many. `EAGAIN` on a
  data send is then a bug, logged as one.
- **Events.** Events, keepalives and notices go outside the window, at most 256
  unacknowledged per peer. Past that the peer's events are dropped and, once it has room,
  it is sent `resync_required` first and resynchronises with a snapshot.
- **Late joiners.** A new subscription has no base on that peer, so its topics' newest
  frames go to the newcomer alone, as keyframes, paced by its credit.
- **Liveness.** `ROUTER_MANDATORY`: a send to a vanished peer fails `EHOSTUNREACH`, which
  drops its state and subscriptions (logged once). A peer whose topics are quiet is still
  sent a keepalive each second, so that send happens. `PEER_LIVENESS` closes idle
  connections; a live peer that holds unacknowledged messages and sends nothing for 60 s
  is dropped as stalled and told to `rejoin` if heard from again.

## Cost and wire impact

- New DEALER messages (`window`, `subscribe`, `unsubscribe`, `keyframe`, `credit`) and
  notices (`resync_required`, `rejoin`), the ROUTER/DEALER framing on the data channel,
  and `data.keyframe` gone from ctrl: a wire change (PROTO 83, fixtures, the Python
  cross-language tests). Ports are unchanged.
- The daemon's I/O thread does per-peer bookkeeping (credit, chain per topic, cursor):
  O(peers × topics), the bases mostly `Arc`s shared with peers in step.
- One small credit message upstream per read burst: a client reading 340 frames/s sends
  a few hundred tens-of-bytes messages a second at most, fewer when its reads batch.
- Measured (`wire_size`, tuning session with the IR view, 60 fps): the counted peer's
  payload 4.66 Mbit/s beside a starved peer, as without one; the starved peer (64 KiB
  window drained at 250 kB/s) is served every topic, a median 0.27 s behind (window ÷ rate
  = 0.26 s), p95 0.33 s, at most ≈ 0.37 s (window plus one rotation round).
