# Q2 / Q5 / Q6 — Delivery, state sync and stimulus lease

Status: proposed for phase 3. Answers Q2, Q5 and Q6 in `open-questions.md` within the
decisions table (2a, 2b, 5a, 5b, 6a–6d). Transport facts come from `spike-zmq-curve.md`.

## Identities and counters

| Name | Scope | Changes when |
|---|---|---|
| `daemon_incarnation` | u64, random at daemon start | daemon process restarts |
| `session_epoch` | u32, per incarnation | session open/close, device change, sample-rate or buffer change (5b) |
| `rev` | u64, per incarnation | every committed state change (one counter, serial commits) |
| `seq` | u64, per topic per incarnation | every published frame on that topic |
| `client_id` | string | under CURVE, the ZAP User-Id (key name from `authorized_clients`); local transports use a daemon-assigned id bound to the ROUTER routing id |

## Q5 — State sync, replay, frame identity

**Sync procedure (client):**

1. Subscribe to `evt` and `ka`.
2. Wait for the first `ka`. That proves the subscription is live.
3. Call `state.snapshot`, which returns `{state, rev = R, daemon_incarnation, session_epoch}`.
4. Apply buffered `evt` with `rev > R` in order, then live events.
5. If an event arrives with `rev > last + 1`, call `state.since(last)`.
6. If a `ka` shows another `daemon_incarnation`, drop all mirrored state and go back to step 1.
7. If a `ka` shows `rev` ahead of the last applied event for more than 1 s, call `state.since`.
   This catches a missed final patch.

**Replay buffer** (5a): the last 1024 events or 60 s, whichever holds fewer.
`state.since(r)` returns the events after r, or the error `resync_required` if r has been
evicted. The client then takes a fresh snapshot.

**Events** are typed and patch-shaped: `{rev, kind, payload}`, where `kind` names the
changed entity (`measurement`, `trace`, `generator`, `session`, `calibration`, `timing`, …)
and the payload is that entity's full new value (or `deleted`). Full entity values keep
client application trivial and make missed-ordering bugs impossible within an entity.

**Frame identity** (header fields beyond PLAN §6.3):

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
  dirty slot at most at the topic's rate: 60 fps for local transports, 30 for network,
  configurable per subscriber class (2b).
- XPUB: `XPUB_VERBOSE`, per-peer `SNDHWM` = 3 × subscribed topics (minimum 16).
  `XPUB_NODROP` is never set.
- On a new subscription, the daemon re-sends that topic's latest slot so late joiners
  don't wait.
- Network mode sets `ZMQ_SNDBUF` = 64 KiB on the data socket to keep kernel backlog small.
- `ka` keepalive every 250 ms carries:
  `{daemon_incarnation, session_epoch, rev, daemon_wall_ns, timing summary, generator owner/state}`.

Client:
- Before each render, drain the socket: read until EAGAIN, keep the max `seq` per topic,
  and count any malformed frames.
- Clock offset: each `ka` gives a sample `daemon_wall_ns − local_receive_ns` = true offset
  minus that message's delivery delay. Take the **maximum** over the last 10 s (the
  least-delayed sample); it under-estimates the offset by the minimum delivery delay only.
  Frame age = `now_local + offset − capture_wall_ns`.
- **STALE** (2a): no new frame on a topic for 1 s, or frame age > 1 s. The trace dims and
  shows its age. STALE says nothing about measured delay.
- Daemon unreachable: if no `ka` arrives for 1.5 s, every trace is STALE and the banner
  reads "DAEMON NOT RESPONDING".

Tests (phase 3): a stalled subscriber recovers to fresh frames within one drain plus one
publish period on inproc and tcp; bandwidth-limited link (tc/netem or a throttled proxy in
tests) shows bounded age; late joiner gets latest slot immediately.

## Q6 — Stimulus lease

**Ownership** is bound to `client_id` (the ZAP User-Id under CURVE). Request content can
never claim an identity.

**Commands:**

| Command | Lease needed | Effect |
|---|---|---|
| `gen.acquire {force: bool}` | none | Returns `{lease_token: u128 random, expires_in_ms}`. Refused if another client holds the lease, unless `force`. `force` first stops output (fade) and disarms. |
| `gen.set {lease_token, desired}` | yes | Sets the full desired generator state: signal, level, routing, `armed`, `firing`. Also refreshes the lease. |
| `gen.refresh {lease_token}` | yes | Refreshes the lease without changing state. |
| `gen.release {lease_token}` | yes | Stops output, disarms, and releases the lease. |
| `gen.stop` | **no** | Any authorized client: fade out, disarm, keep the owner (6c/decision: stop is universal). |

- Refresh at least every 0.5 s; the lease expires 1.5 s after the last refresh (6a,
  configurable). On expiry the daemon fades the output path out over 20 ms (6b), disarms
  and clears the owner. A network hiccup shorter than the expiry changes nothing (6d).
- Enforcement lives in the output path: the control thread sets an atomic deadline and the
  generator port checks it per callback. If the control thread is stuck, output still fades.
- `firing` requires `armed`. Arming does not emit. Loading a session, reconnecting or
  restarting always comes up disarmed with no owner.
- **IR capture** (ESS) needs the lease and holds it for the whole capture. Expiry or
  `gen.stop` aborts the capture, and the result is discarded with reason `aborted`.
- **Audit:** every acquire, force, arm, fire, stop, release and expiry is an `evt` (kind
  `generator`) carrying `client_id`, and is written to the daemon log.

## Ctrl channel details

- DEALER ↔ ROUTER, one msgpack frame `{v, id, cmd}` per request, `ROUTER_MANDATORY` on.
- Dedup: per `client_id`, the daemon remembers the last 256 request ids and their replies
  for 30 s. A retried id returns the stored reply and never runs twice.
- Every mutation accepts an optional `expect_rev`. If state has moved past it, the reply is
  `conflict` with the current `rev`.
- Errors are a typed `{code, msg}` with `code` an enum: `invalid`, `not_found`,
  `conflict`, `lease_required`, `lease_held`, `refused` (safety), `resync_required`,
  `unsupported`, `internal`.

## Out of scope for phase 3

- Rate-limiting refused CURVE reconnects beyond logging (phase 6).
- pyzmq interop tests (phase 3 adds Rust↔Python frame-codec fixtures; CURVE interop later).
