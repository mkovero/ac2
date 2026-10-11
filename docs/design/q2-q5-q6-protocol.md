# Q2 / Q5 / Q6 — Delivery, state sync and stimulus lease

Status: implemented (`ac2d`, `ac2-client`). Answers Q2, Q5 and Q6 in `open-questions.md`
within decisions 2a, 2b, 5a, 5b, 6a–6d. The wire itself (commands, events, errors, header
fields) is specified in [`docs/protocol.md`](../protocol.md); this note keeps the guarantees
and why they are built this way. Transport facts: `spike-zmq-curve.md`.

## Identities and counters

| Name | Scope | Changes when |
|---|---|---|
| `daemon_incarnation` | u64, random at daemon start | daemon process restarts |
| `session_epoch` | u32, per incarnation | session open/close, device change, sample-rate or buffer change (5b) |
| `rev` | u64, per incarnation | every committed state change (one counter, serial commits) |
| `seq` | u64, per topic per incarnation | every published frame on that topic |
| `client_id` | string | under CURVE, the ZAP User-Id (key name from `authorized_clients`); local transports use a daemon-assigned id bound to the ROUTER routing id |

## Q5 — State sync, replay, frame identity

**Sync** (procedure: protocol §4.3; replay buffer 5a: the last 1024 events or 60 s). The
first `ka` proves the subscription is live before the snapshot is taken, so no event between
snapshot and subscription can be missed; buffered events with `rev > R` are then applied in
order. A gap in `rev` asks `state.since`; an evicted `rev` (`resync_required`) or another
`daemon_incarnation` starts over from a snapshot. A `ka` whose `rev` stays ahead of the last
applied event for 1 s catches a missed final patch.

**Events** carry the changed entity's full new value (protocol §4.2): applying one is
assignment, which keeps clients trivial and makes missed-ordering bugs impossible within an
entity.

**Frame identity** (header fields beyond PLAN §6.3; protocol §5.3):

- `session_epoch`: frames from an older epoch are discarded by clients.
- `config_rev`: the `rev` of the measurement config the DSP actually used.
- `config_applied_at`: the capture sample index from which that config took effect.
- `capture_wall_ns`: daemon wall-clock time (Unix ns) of the newest sample in the frame.

A config change shows as "pending" in clients until a frame arrives whose `config_rev` is at
or above the change's `rev`.

**Grids**: immutable, keyed by `grid_id` (a hash of their parameters). `grid.get(id)` works
for the life of the incarnation. Frames reference a grid only by id; a client fetches any
id it doesn't know.

## Q2 — Delivery freshness

**Guarantee:** bounded freshness with visible age, not "always the latest frame".

Daemon:
- Each job writes its newest result to a per-topic latest slot. The publisher sends each
  dirty slot at most at the topic's rate: 60 fps for every transport since packed frames
  (`wire-slim.md`), configurable per daemon (2b).
- XPUB: `XPUB_VERBOSE`, per-peer `SNDHWM` = 48 (one burst: a full desk's ~40 topics plus
  events and keepalives; libzmq does not see a peer's reads during a burst, so a smaller
  queue drops part of every round; a larger one only lengthens the standing delay in front
  of a slow link, which is queue length / link rate), set once before binding. It is not
  resized with the subscriptions: libzmq 4.3.5 applies a
  changed SNDHWM to live pipes with commands that race with a leaving peer's pipe teardown
  (a use-after-free in its I/O thread, reproduced at daemon shutdown).
  `XPUB_NODROP` is never set.
- On a new subscription, the daemon re-sends that topic's latest slot so late joiners
  don't wait.
- Network mode sets `ZMQ_SNDBUF` = 64 KiB on the data socket to keep kernel backlog small.
- Kernel dead-peer detection on every TCP connection, daemon and client side: keepalive
  probes after 5 s idle (3 probes, 1 s apart) and a 15 s limit on unacknowledged data
  (`TCP_USER_TIMEOUT` / `TCP_MAXRT`). A peer that vanished without closing (lid closed,
  power cut) is disconnected within seconds instead of after the kernel's ~15 min
  retransmission timeout, which releases its queue and its subscriptions (and the optional
  work done for them), and lets a client reconnect to a daemon that came back. libzmq's
  ZMTP heartbeats are not used: in 4.3.5 they trip stream-engine assertions (a SUB whose
  reads pause on a full queue; a heartbeat timer after a failed connection) that abort the
  process.
- Messages pass from the jobs through the internal pipe and the latest slot to the data
  socket as zero-copy libzmq parts: the encoded buffers are never copied in the daemon.
- Client sockets reconnect after 100 ms, backing off to 2 s.
- Pacing each peer to what it has consumed (instead of to the publish rate) is designed in
  [`flow-control.md`](flow-control.md), not implemented.
- `ka` keepalive every 250 ms carries the rev, daemon wall clock, timing state and generator
  summary (protocol §5.4).

Client:
- Before each render, drain the socket: read until EAGAIN (without copying), keep the
  newest message per topic (frames of one topic arrive in publish order over the one
  connection, so that is the last one read), decode only those, count malformed frames and
  discard frames of an older epoch or another incarnation. Superseded messages are never
  decoded. The data SUB's `RCVHWM` (64) holds one publish round between drains.
- Consumers sleep until data is queued (`Client::data_changed`, which on Unix registers
  the SUB's `ZMQ_FD` with the runtime; `wait_data` for blocking threads) instead of
  polling on a timer.
- Clock offset: each `ka` gives a sample `daemon_wall_ns − local_receive_ns` = true offset
  minus that message's delivery delay. Take the **maximum** over the last 10 s (the
  least-delayed sample); it under-estimates the offset by the minimum delivery delay only.
  Frame age = `now_local + offset − capture_wall_ns`.
- **STALE** (2a): no new frame on a topic for 1 s (3 s for the once-a-second Leq streams), or frame age > 1 s. The trace dims and
  shows its age. STALE says nothing about measured delay.
- Daemon unreachable: if no `ka` arrives for 1.5 s, every trace is STALE and the banner
  reads "DAEMON NOT RESPONDING".

Tests: a stalled subscriber recovers to fresh frames within one drain plus one
publish period on inproc and tcp; bandwidth-limited link (tc/netem or a throttled proxy in
tests) shows bounded age; late joiner gets latest slot immediately.

## Q6 — Stimulus lease

**Ownership** is bound to `client_id` (the ZAP User-Id under CURVE). Request content can
never claim an identity.

Commands and rules: protocol §3.2 (`gen.acquire {force}`, `gen.set`, `gen.refresh`,
`gen.release` need the lease token; `gen.stop` is universal: any authorized client fades out
and disarms, keeping the owner, because stopping must never wait on whoever holds the lease).

- Refresh at least every 0.5 s; the lease expires 1.5 s after the last refresh (6a,
  configurable). On expiry the daemon fades the output path out over 20 ms (6b), disarms
  and clears the owner. A network hiccup shorter than the expiry changes nothing (6d).
- Enforcement lives in the output path: the control thread sets an atomic deadline and the
  generator port checks it per callback. If the control thread is stuck, output still fades.
- `firing` requires `armed`. Arming does not emit. Loading a session, reconnecting or
  restarting always comes up disarmed with no owner.
- **Sweep runs** (`sweep.run`) need the lease and hold it for the whole run. Expiry or
  `gen.stop` aborts the run, and its result is discarded.
- **Audit:** every acquire, force, arm, fire, stop, release and expiry is a `generator` event
  naming the client, and is written to the daemon log.

## Ctrl channel

DEALER ↔ ROUTER with `ROUTER_MANDATORY`; request ids are deduplicated per client (a retried
id returns the stored reply and never runs twice), mutations take an optional `expect_rev`,
errors are typed codes: protocol §3.1, §3.4.
