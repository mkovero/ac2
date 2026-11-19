# Network on/off, live, from Settings

The operator turns the network on in the app's settings, on any OS, however `ac2d` was
started; the app manages the local daemon completely and at once. Today the transport is
fixed by `ac2d --listen` at start, and a network-mode daemon serves tcp only, so the local
app and CLI cannot reach it over ipc.

## Behaviour

1. `ac2d` **always serves local** (ipc on Linux/macOS, loopback tcp on Windows). The
   network is an optional second listener beside it, switched on and off **live** by
   `server.network`: session, audio, jobs and local clients keep running. The choice
   (on/off, port) persists in the daemon's rig settings, so it applies however the daemon
   starts (app, systemd, by hand). The mDNS advert and the rig-peer browse follow it.
2. Only a **local** client may change it: a remote client can neither cut itself off nor
   open a rig it reached some other way.
3. `ac2d --listen tcp://IFACE[:PORT]` still works and wins: the network is on at that
   address and `server.network` is refused as "fixed by --listen". The daemon now also
   serves local beside it.
4. A refused app key can be authorized from Settings › Connection: `server.info` already
   carries each refused key in full (`RefusedKey.key`, Z85), and `server.authorize` applies
   to handshakes at once.
5. The server key is generated on the first enable (`keys::load_or_create_server_keys`
   as today); its fingerprint is shown for pairing from then on, on or off.

## Spike: what libzmq 4.3.5 does (`crates/ac2-zmq/tests/it/listen_toggle.rs`)

| | question | found |
|---|---|---|
| a | `zmq_unbind` of a tcp endpoint: accepted connections? | **Ended** at once on the server side (a ROUTER_MANDATORY send to the peer fails); the socket's ipc endpoint and its connection are untouched. The client notices at its next send (or its keepalive deadline) and is refused on reconnect. |
| b | ZAP address frame per transport | ipc: `"localhost"` (this libzmq build reads no SO_PEERCRED: no uid); loopback tcp: `"127.0.0.1"`. Set by libzmq from the accepted socket, not by the peer. |
| c | rebind the same tcp port | Same socket right after unbind: **can fail `EADDRINUSE`** (the listener closes on libzmq's I/O thread after unbind returns; seen both ways), so retry briefly. New `SecureContext` + new socket after the old ones are dropped: binds at once, 20/20 rounds, with the old client's connection in TIME_WAIT (libzmq sets SO_REUSEADDR; context term waits for listeners to close). |
| d | unbind in ac2-zmq | Added: `Socket::unbind` (same shape as `bind`). Design B below does not need it (it closes sockets), but it is the primitive for A and is tested. |

## The central decision: one CURVE pair (A) or two pairs (B)

**A** — one CURVE ROUTER pair bound to the local endpoint always and to tcp while on; ZAP
lets local connections through without an authorized key.

- (b) makes the local test possible only on ipc (`"localhost"`). On Windows the local
  transport is loopback tcp, where `"127.0.0.1"` is also every ssh `-L` tunnel, port proxy
  and other user on the host: A would turn "any loopback peer" into "authorized" on the
  very socket the network uses. A Windows-safe A needs ipc there (AF_UNIX, not built in).
- Every local client (ac2-client `ClientConfig::local`, CLI, app, ac2-e2e, ac2d tests)
  must speak CURVE and learn the server public key, which must then exist from the first
  start, not the first enable.
- The ZAP handler becomes a dependency of local service: today its death closes both
  sockets and shuts the daemon down (`io.rs`, `zap_status`), i.e. it would take the
  operator's own app down with it.
- A local peer still needs an identity of its own (a ZAP user id like `local`, kept apart
  from authorized names), and local data pays CURVE encryption (not measured).
- On Windows the local port and the network port collide (`DEFAULT_PORT` is both) unless
  one moves; same as B.
- Turning off is an unbind (a: ends remote peers, keeps local), on is a bind (c: retry).

**B** — plain local ROUTERs (as today's `Listen::Local`) for the daemon's lifetime, plus a
CURVE pair in its own `SecureContext` created on enable and dropped on disable.

- Local stays exactly as today: no keys, no ZAP, the OS boundary; nothing changes in
  ac2-client, the CLI, the app or any test that connects locally.
- Off is "drop the network sockets and their context": every remote connection ends (a
  closed socket ends all its connections), and (c) shows the port is free again at once.
- A dead ZAP handler closes only the network pair and reports the network failed; the
  daemon and local clients keep running (still fail-closed for the network).
- Cost: the io thread and the data peers key clients by routing id and must know which
  socket a client came from. Counted on main:
  - `io.rs` — 9 sites: the poll set (2 → 4 client sockets, dynamic), ctrl recv,
    data recv, `TAG_REPLY` send, `TAG_EVENT` / `TAG_KA` / `TAG_AUDIO` to peers,
    `peers.serve`, the ZAP status check, plus a new message that attaches/detaches the
    network pair.
  - `peers.rs` — the `HashMap<Vec<u8>, Peer>` key, and the 4 `Peers` functions that take
    the data socket (`request`, `outside`, `audio`, `serve`); the 7 `Peer` methods keep
    taking one socket, chosen by the peer's origin. Detaching drops every network peer
    (their `Interest` counts with them), as `drop_key` does per key.
  - Control side: none of the 45 `routing_id` mentions (control.rs, request.rs, dedup.rs,
    outbox.rs, refusals.rs, file/meas jobs) need to change if io.rs prefixes one origin byte to the
    routing id it hands over and strips it when sending. `ControlMsg::Request.peer`
    becomes an enum `Local | Curve(CurvePeer)` set by the socket, not inferred from the
    absence of a ZAP user id (13 `peer` / `CurvePeer` uses in control.rs and request.rs,
    mostly pattern matches).

**Recommendation: B.** It is confined to two daemon files plus startup, keeps the local
trust boundary and every local client untouched, keeps the daemon alive when ZAP dies,
and is the only option that is sound on Windows without new ipc support. A's one gain
(single poll set) does not pay for CURVE in every local client and the loopback hole.

## Wire changes (PROTO_VERSION 92 → 93, `fixtures/protocol/WIRE_LOCK`)

- Command `server.network { enabled: bool, port: Option<u16> }` (mutation; `port: None`
  keeps the stored one, default `DEFAULT_PORT`, data on port + 1; listens on all
  interfaces). Replies `ServerInfo`. Refused (`Refused`) from a network client and when
  fixed by `--listen`; a bind failure is an error reply and nothing is stored.
- `ServerInfo` reshaped, the `ServerMode` enum replaced:
  ```
  ServerInfo { local: LocalServer, network: NetworkServer, recording_dir }
  LocalServer = Embedded | Local { ctrl, data }
  NetworkServer {
    setting: Off | On { port } | Fixed { listen },   // what was asked, and by whom
    status: Off | Listening { ctrl, data } | Waiting { reason } | Failed { reason },
    server_key: Option<String>, fingerprint: Option<String>,   // once a key exists
    advertised_as: Option<String>,
    authorized: Vec<AuthorizedClient>, refused: Vec<RefusedKey>,
  }
  ```
- `State` gains `server: ServerSummary { network: NetworkStatusKind, port, fingerprint }`
  so every mirrored app sees a toggle made elsewhere; the key lists stay in `server.info`.
- `server.authorize`, `server.revoke`, `auth.admit_rig` stop being network-only: with the
  network off they edit the authorized-clients file (pre-authorizing before enable).
- Samples (`ac2-proto/src/samples`), the Python fixtures (`tools/protocol`) and
  `docs/protocol.md` follow.

## Daemon config and precedence

`rig.json` (`RigSettings`, daemon-owned, atomic writes) gains
`network: { enabled: bool, port: u16 }`, absent = off. No new file: it already holds the
other daemon-side network targets (OSC, Art-Net) and travels with the rig, not the session.

`DaemonConfig` gains `network: NetworkChoice`:

| how started | choice | `server.network` |
|---|---|---|
| `ac2d --listen tcp://…` | `Fixed(addr)`; rig.json not read or written for it | refused: fixed by flag |
| `ac2d` (app, systemd, by hand) | `FromSettings` | allowed from local |
| in-process / test daemons (`Listen::Inproc`, ac2-e2e `sim_daemon`), `--sim` | `Off` | refused: not available |

`--no-mdns` keeps applying to whichever way the network came on. At start with the setting
on, a bind failure never stops the daemon (unattended rigs must come up after a power cut
with no one there): local serves, status is `Waiting { reason }`, and the bind is retried
with backoff (a specific interface address may appear after DHCP).

## Live switching in the daemon

- Enable (control thread): load or create the server key and the authorized file, make the
  `SecureContext` (audit into the existing `AuthLog`), make and bind both CURVE ROUTERs
  with today's options (`data_socket_options(…, true)`, liveness), and only then store the
  setting and reply. The bound pair and context go to the io thread over a channel, with an
  outbox tag to wake it. Then start the mDNS `Advertiser` and `PeerBrowse`.
- Disable: tag to the io thread; it drops the network peers, the sockets, then the
  context, and acknowledges; then the control thread drops the `Advertiser` (its goodbye
  blocks up to ~1 s, so never on the io thread), stops `PeerBrowse` and the follow links
  that need the network, stores the setting and replies. A port change is off then on.
- `firewall::report` runs on each enable; the systemd notifier status line names it.
- The local endpoints and the instance/endpoints file never change, so the app's spawn and
  rediscovery logic (`ac2-ui/src/spawn.rs`) is untouched.

## Security

- **Who may toggle:** requests that arrived on the local socket (origin set by the io
  thread, never from the payload). Remote: refused "change it on the rig's own machine".
- **Confirmation:** the daemon needs no second step (the local user already holds every
  power, including emitting sound); the app confirms enabling, naming the port and that
  only authorized keys get in.
- **Local boundary per OS:** Linux/macOS: ipc files in the per-user runtime dir (0700):
  the same user and root. Windows: loopback tcp: any local user and anything forwarded to
  loopback; that is today's exposure for every command, and the toggle adds nothing new to
  it. Windows local moves off `DEFAULT_PORT` (to 47822/47823) so local and network never
  share a port.
- Turning off ends every remote connection at once (spike a, and socket close); revocation
  and ZAP rules are unchanged while on.

## App: Settings › Connection

Rows, from `server.info` (refreshed on open and on a `State.server` change):

- **Network** — Off / On toggle with a **Port** field (data uses port + 1). Status beside
  it: "Off", "Listening on <ctrl>", "Waiting: <reason>", "Failed: <reason>". Disabled with
  "Fixed by ac2d --listen" when `setting = Fixed`, and with "Change it on the rig's own
  machine" when the app is connected remotely. Turning on asks first.
- **Server key** — fingerprint, with copy (Z85) for pairing; "Created when the network is
  first turned on" before any key exists. Advertised name under it when advertised.
- **Authorized** — name, fingerprint, app or rig; **Revoke** (not on the app's own key).
- **Refused** — fingerprint, address, count, last time; **Authorize** asks a name (1–64
  characters, no spaces) and sends `server.authorize { name, key }`; the row moves to
  Authorized in the reply. Entries without a key (not CURVE) have no action.

## Stages

1. **ac2-zmq** `Socket::unbind` + the spike tests (this commit).
2. **ac2d dual pair (B), no wire change:** io thread with a local pair and an optional
   network pair, origin byte on routing ids, `Peer` origin, `Local | Curve` peer enum;
   ZAP death closes the network pair only. `--listen` now serves local beside the
   network. Tests: a local and a CURVE client on one daemon; ZAP death keeps local.
3. **Live toggle + wire:** `server.network`, `ServerInfo` reshape, `State.server`, rig.json
   `network`, `NetworkChoice` precedence, bind retry at start, mDNS/PeerBrowse follow,
   key lists usable while off; PROTO 93, WIRE_LOCK, samples, Python fixtures, protocol doc.
   Tests: on/off/on with a connected remote client (cut, local untouched), toggle refused
   from remote and when fixed.
4. **CLI:** `ac2 server network on|off [--port N]`, `ac2 server authorize <name>
   --refused <fingerprint>`.
5. **App rows** (Settings › Connection, above), keytips and user guide.
6. **Windows** local ports, all-OS CI run; `docs/rigs/pupu.md` notes that units may keep
   `--listen` (fixed) or drop it for the setting.

## Open questions for the operator

- Listen address: all interfaces only, or an interface choice in the setting too?
- Windows local transport: keep loopback tcp on its own port, or build libzmq with AF_UNIX
  ipc on Windows so the local boundary matches Linux/macOS?
- With the network off, should rig follow (taking another rig's inputs) still connect out?
  It needs no listener, only the server key; the plan stops it with the network.
