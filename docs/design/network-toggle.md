# Network on/off, live, from Settings

The operator turns the network on in the app's settings, on any OS, however `ac2d` was
started; the app manages the local daemon completely and at once. Today the transport is
fixed by `ac2d --listen` at start, and a network-mode daemon serves tcp only, so the local
app and CLI cannot reach it.

## Implementation briefs

One agent per stage; each stage lands with its tests on the quick tier.

1. Spike, done: `crates/ac2-zmq/tests/it/listen_toggle.rs`.
2. Dual pair, live attach/detach, retry, non-fatal network: `crates/ac2d/src/{lib.rs,
   io.rs, peers.rs, outbox.rs, control.rs, dedup.rs, config.rs, main.rs}`,
   `control/{request.rs, files.rs, recording.rs, generator.rs, remote.rs, rig.rs,
   execute.rs, file_jobs.rs, meas_jobs.rs}`, `refusals.rs`, `ac2-client/src/endpoint.rs`,
   `ac2-ui/src/{spawn.rs, app/daemons.rs}`, `ac2-cli/src/cmd/daemon.rs`, root `Cargo.toml`
   (`if-addrs` into `[workspace.dependencies]`), ac2d tests; all-OS CI.
3. Wire and setting: `ac2-proto/src/{lib.rs, ctrl/command.rs, model/state.rs, samples/}`,
   `fixtures/protocol/WIRE_LOCK`, `tools/protocol`, `docs/protocol.md`, `ac2-paths/src/lib.rs`
   (`network_settings`), `crates/ac2d/src/{network_store.rs (new), control/rig.rs,
   control/remote.rs, lib.rs}`, `ac2-scene/src/stimulus.rs`, ac2d and ac2-e2e tests;
   all-OS CI.
4. CLI: `ac2-cli/src/cmd/` (`server network`, `server authorize --refused`).
5. App rows: `ac2-ui` Settings › Connection, keytips, `docs/user-guide.md`.
6. Docs: `docs/rigs/pupu.md` (line 86: units keep `--listen`), STATUS.md.

## Behaviour

1. `ac2d` **always serves local** (ipc on Linux/macOS, loopback tcp on Windows). The
   network is an optional listener beside it on a chosen interface or address and port,
   switched on and off **live** by `server.network`: session, audio, jobs and local clients
   keep running. The choice persists in a daemon-side file of its own, so it applies however
   the daemon starts. The mDNS advert follows it and names only the chosen interface.
2. Only a **local** client may change it: a remote client can neither cut itself off nor
   open a rig it reached some other way.
3. `ac2d --listen tcp://IFACE[:PORT]` still works and wins: the network is fixed at that
   address and `server.network` is refused as "fixed by --listen". The daemon serves local
   beside it. pupu and the CM4 meter keep `--listen`.
4. A refused app key can be authorized from Settings › Connection: `server.info` already
   carries each refused key in full (`RefusedKey.key`, Z85); `server.authorize` applies to
   handshakes at once.
5. The server key is generated on the first enable (`keys::load_or_create_server_keys`);
   its fingerprint is shown for pairing from then on, on or off.
6. Outgoing rig links (this rig taking another rig's inputs) work with the network off:
   they dial out with the server key and need no listener. Off removes only the listener,
   the audio tap other rigs follow, and the advert. The peer browse that keeps those links
   on the peers' current addresses runs whenever a server key and rig peers exist.
7. No failure of the network needs an operator: a bind that fails, an address that goes
   away and a ZAP handler that dies all end in `Waiting`/`Failed` and are retried, for a
   network fixed by `--listen` as much as for one from the setting.

## Spike: what libzmq 4.3.5 does

Kept as tests in `crates/ac2-zmq/tests/it/listen_toggle.rs`:

- **Off then on at once:** the old pair (200 ms linger, a client connected, a message
  queued) dropped on a closer thread while a new pair binds the same port: the new bind may
  fail "address in use"; tearing its context down and retrying binds once the closer is
  done.

- **ZAP address** is set by libzmq from the accepted socket: on ipc `"localhost"` on Linux
  and empty on macOS (no peer credentials read, so no uid either way), `"127.0.0.1"` on
  loopback tcp. Loopback is also
  every ssh tunnel, port proxy and other user on the host, so the address never decides
  "local"; the socket a message came in on does.
- **Rebind:** a new `SecureContext` and socket, made after the previous ones were dropped,
  bind the same port at once (20/20 rounds, the old client's connection in TIME_WAIT):
  context term waits for listeners to close and libzmq sets SO_REUSEADDR.
- **Two pairs, one poll:** a plain ROUTER and a CURVE ROUTER of another context poll
  together on one thread; clients with the same routing id on each are kept apart and
  answered on their own socket; only the CURVE one carries a ZAP identity.

Found with a throwaway `zmq_unbind` wrapper, since removed (design B closes sockets and
needs no unbind): unbind ends the connections accepted through that endpoint at once on the
server side and keeps the socket's other endpoints; binding the same endpoint right after
can fail `EADDRINUSE`, because the listener closes on libzmq's I/O thread after unbind
returns.

## Decision: two socket pairs (B), not one CURVE pair (A)

**A** (one CURVE pair, local endpoint always bound, tcp bound and unbound, ZAP waving local
peers through) is rejected: the ZAP address cannot tell local from tunnelled on loopback, so
it is unsound on Windows; every local client (ac2-client, CLI, app, tests) would need CURVE
and the server key from first start; a dead ZAP handler would take local service down; local
data would pay for encryption.

**B:** plain local ROUTERs for the daemon's lifetime, plus a CURVE pair made on enable and
dropped on disable. Local stays as today; off ends every remote connection by closing the
sockets; a dead ZAP handler fails only the network. Cost, counted on main: `io.rs` 9 sites
(poll set, ctrl and data recv, reply send, events, keepalives, audio, `serve`, ZAP check)
and `peers.rs` (map key and the 4 `Peers` functions that take the data socket; the 7 `Peer`
methods take the socket their peer's origin picks).

### Contexts

- The **lifetime context** (plain) owns the local ROUTERs, the internal PULL, every
  `Outbox`, and the outgoing follow links.
- The **per-enable `SecureContext`** owns only the two network ROUTERs and its ZAP handler.
  Dropping it after the sockets is the whole of "off"; nothing else lives in it, so nothing
  else breaks.

### Origin

- `Route { origin: Origin, id: Vec<u8> }`, `Origin = Local | Network(NetEpoch)`, replaces
  the bare `routing_id` the control thread passes around (45 mentions in control.rs,
  request.rs, dedup.rs, outbox.rs, refusals.rs, file/meas jobs; mechanical). A typed field
  rather than a prefix byte: the epoch needs a place too.
- Lease ownership and the audit compare on `Peer` (origin and identity), not on `ClientId`
  alone (control/generator.rs, the lease checks and audit lines): a remote key named like a
  local client can never pass as it.
- `dedup` keys a `Curve` peer on (key, request id), not on its route, so a client that
  retries a request across off/on (new epoch, new routing id) gets the stored reply instead
  of running it twice; a `Local` peer keys on its route as today.
- `ControlMsg::Request.peer` becomes `Peer = Local | Curve(CurvePeer)`, set by the socket.
  Data plane likewise: `Peers::request` gets the origin instead of inferring "local" from a
  missing client (peers.rs 416-440). A message on the network pair without a ZAP identity
  and key is dropped and logged; it cannot happen with our ZAP handler, so it is a bug.
- `NetEpoch` counts attaches (enables and automatic retries). A reply, event or job result
  addressed to an older epoch is dropped by the io thread, so nothing meant for the
  previous pair reaches a client of a new one that happens to get the same routing id.
- "Is this client local" comes from the origin, not from the `local-` name prefix.
  `ClientId::is_local` (a prefix test) has no daemon caller; its readers are the app's
  stimulus owner wording (ac2-scene stimulus.rs) and an e2e test, through the lease owner
  on the wire. It goes: the daemon records the `Peer` with each lease, and the wire's
  lease owner carries `local: bool` from it (part of PROTO 93). Belt and braces:
  `server.authorize` and `auth.admit_rig` refuse a name starting `local-`, so no new
  authorized name looks local in logs or the app; loading the authorized file is not
  touched (an existing `local-x` entry still loads and is still a `Curve` peer).

### What reads the network today, and becomes per-request or live

- **Paths:** `Shared.network` (control.rs) makes `file.*` and `rec.*` refuse absolute paths
  (control/files.rs `session_path`, control/recording.rs `recording_path`). It is deleted;
  both take the request's `Peer`: `Local` may use paths, `Curve` names only. Otherwise a
  toggled-on daemon hands remote clients filesystem paths, and under `--listen` the local
  app is refused.
- **Server setup:** `ServerSetup` becomes mutable control state:
  `Server { local: LocalSetup, network: NetworkSetup }` with
  `NetworkSetup { setting, status, saved, keys: Option<KeyPair>, authorized, refused,
  advertised_as, epoch }`. Readers:
  - the SourceTap (control/remote.rs, session start): installed at **every** session start,
    network or not; it sends only what `Interest` wants, and without network peers nothing
    wants `audio/`. Enabling with a session open needs no session restart.
  - link keys (`respawn_links`): taken from `keys`, present once a key exists, on or off
    (behaviour 6).
  - `RemoteLink::NotNetworked` (remote.rs, state summary) is renamed `NoServerKey`: it
    means "no server key yet", not "network off".
  - `server_info`, `admit`, `server_revoke`: read the live setup; the key lists work off.
    `server.info` to a `Curve` peer leaves out the local endpoints and the interface list
    (host detail a remote client has no use for).
  - **PeerBrowse** (lib.rs, started today only with an advert and a key): started by the
    control thread whenever a key exists and `rig.peers` is non-empty, stopped when either
    goes, independent of the listener and of `--no-mdns` (which only suppresses the advert).

## Interfaces and addresses

- `server.info` lists candidates: `interfaces: Vec<NetInterface { name, addrs: Vec<IpAddr>,
  up: bool }>` from `if-addrs` (already in ac2-discovery), loopback excluded, read at each
  `server.info`.
- The setting is `bind: NetBind = All | Interface(String) | Addr(IpAddr)` plus `port`. The
  daemon resolves an interface name itself with `if-addrs` and binds the address (libzmq's
  own name resolution differs on Windows): the interface's first IPv4 address, else its
  first global IPv6 address (link-local IPv6 needs a scope id clients rarely carry). The
  resolved address is what mDNS gets (`Bind::Addr`) and what `Listening` shows. `All`
  binds `tcp://*:port` (IPv4) as today.
- The interface lister and the retry backoff are injectable in `DaemonConfig` (as
  `lease_expiry` is), so tests drive interfaces appearing and vanishing and run the backoff
  in milliseconds on the fake backend.
- When the chosen interface or address is missing at enable, at start or later: status
  `Waiting { reason }`, retried on a timer (2 s, backing off to 30 s). An address that goes
  away while bound is noticed by the same timer (the bound address is no longer among the
  host's, or the interface now resolves to another): the pair is dropped, `Waiting`, retry;
  when it comes back, bind, `Listening`. A listener on a vanished address would otherwise
  silently accept nothing. `Failed` (ZAP death) retries on the same backoff with a new
  context; the timer runs for `Fixed` and `FromSettings` alike.
- mDNS advertises with `ac2_discovery::Bind` from the setting (`Any`, `Addr`, `Interface`),
  which the responder already honours; it starts at `Listening` and stops on leaving it.

## Wire changes (PROTO_VERSION 92 → 93, `fixtures/protocol/WIRE_LOCK`)

- `server.network { enabled: bool, bind: Option<NetBind>, port: Option<u16> }` (mutation;
  `None` keeps the stored value; defaults `All`, `DEFAULT_PORT`; data on port + 1). Replies
  `ServerInfo`. Refused from a network client, when fixed, and for in-process daemons. One
  port validator, shared with `Listen::validate` at start: not 0, not 65535 (port + 1 must
  exist), and whenever the local transport is tcp (Windows, or `--ctrl/--data` on tcp)
  neither port nor port + 1 equal to a local port. A clash under `--listen` is an argument
  error (exit 2) at start, not a `Waiting`. An enable that cannot bind now (address in use, interface missing) is
  stored and goes `Waiting`, the same as at start; the reply says so.
- `ServerInfo { local: Option<LocalServer>, network: NetworkServer, interfaces, toggle,
  recording_dir }`; `LocalServer = Embedded | Local { ctrl, data }`;
  `NetworkServer { setting: NetSettingView, status: NetStatusView, server_key, fingerprint (both `Option`, once a key
  exists), advertised_as, authorized, refused }`;
  `NetSettingView = Host { setting: NetSetting } | Peer { setting: Off | On | Fixed }`:
  a network client is told only the kind, never the bind (this host's interfaces and
  addresses) nor whether it is kept (the file system's words);
  `NetSetting = Off { kept: Option<NetChoice>, saved } | On { choice: NetChoice, saved } |
  Fixed { listen }`, `NetChoice { bind, port }` (`kept`: where it was last on, where a
  turn-on with neither given goes; PROTO 94) (a fixed
  network keeps nothing: the command line is the setting);
  `saved: Saved | NotSaved { reason: StoreReadOnly | Unreadable { detail } |
  DamagedInPlace | NoStore | WriteFailed { detail } }`;
  `NetStatusView = Host { status: NetStatus } | Peer { status: PeerNetStatus }`, with
  `NetStatus = Off | Listening { ctrl, data } | Waiting { reason } | Failed { reason }` and
  `NetReason = AddressInUse | InterfaceMissing | AddressMissing | ZapFailed | NoServerKey |
  NotHandedOver | Other { detail }`; `PeerNetStatus` carries `NetReasonKind` (the same
  without `Other`'s detail, which is the host OS's words). `ServerMode` goes. To a network client `local` is
  `None` and `interfaces` empty.
- `ServerInfo.toggle: NetToggle = Allowed | Fixed | Unavailable { reason: InProcess |
  NoKeyStore }` (PROTO 94), to every client: whether `server.network` may switch the
  network. One function (`NetCtl::toggle`) gives both this and the command's refusal, so
  the app offers the switch exactly where it works.
- `State.server: ServerSummary { status kind, ctrl endpoint (when listening), port,
  fingerprint }` so every mirrored app
  sees a change made elsewhere; key lists stay in `server.info`.
- Flow notice `network_off` (`FlowNotice::NetworkOff`, on the data socket's `flow`
  topic, outside the window), sent to each network peer of the closing pair only, not a
  numbered state event: it concerns that connection, which is about to end, and is nothing
  a mirror applies (the local clients learn from `State.server`). Remote apps then say
  "network turned off on the rig" instead of "connection lost"; an app that misses it
  falls back to "connection lost". A rig link that hears it reads
  `RemoteLink::NetworkOff` (PROTO 94) until the source welcomes it again; a plain drop
  still reads `Unreachable`.
- `server.authorize`, `server.revoke`, `auth.admit_rig` work with the network off (they
  edit the authorized-clients file).
- Samples, Python fixtures (`tools/protocol`), `docs/protocol.md` follow.

## Configuration and precedence

The setting lives in its own file, `<config>/network.json` (`ac2_paths::network_settings`):
`{ "version": 2, "enabled", "choice": { "bind", "port" } | null }`, written atomically; the
choice is kept while off (version 1 had no way to say "never on", and is set aside). Not in `rig.json`: a
change there that bumps its VERSION would set aside the meters and the max level of every
rig (pupu, the CM4 meter) with it. A missing file, a damaged one or another version means
off; a damaged or other-version file is set aside with a damage notice, as the other stores
do. `DaemonConfig.network: NetworkChoice`:

| how started | choice | `server.network` |
|---|---|---|
| `ac2d --listen tcp://…` | `Fixed(addr)`; `network.json` not read or written | refused: fixed |
| `ac2d` (app, systemd, by hand) | `FromSettings` | local only |
| in-process / ac2-e2e `sim_daemon`, `--sim` | `Off` | refused: unavailable |

- A **read-only** store (unreadable file, a dir it cannot write) or **none** (in-memory):
  the toggle still works live; `saved` says `NotSaved { reason }`.
- `--listen` with `--ctrl/--data` becomes valid: local endpoints from those, network fixed
  (main.rs `listen` stops treating them as alternatives). `--no-mdns` loses its
  `requires = "listen"` and suppresses the advert for either way the network comes on.
- A **network failure never stops the daemon**, fixed or from settings (unattended rigs
  must come up after a power cut with no one there): bind failures `Waiting`, ZAP death
  `Failed`, both retried. A **local** bind failure stays
  fatal at start: without it the daemon has no operator, and the instance lock already
  rules out a second daemon of the same user.
- **Endpoints file and lock holder** (ac2-client endpoint.rs `LockHolder`, ac2-ui spawn.rs):
  every holder serves local, so the `mode=` line and `LockHolder::Network` go; the file
  holds the local endpoints only and the app always reaches the holder. Its consumers
  change with it: ac2-ui `app/daemons.rs` (the two `LockHolder::Network` arms) and ac2-cli
  `cmd/daemon.rs` (the network-holder message). The stdout line
  `ac2d: listening ctrl=… data=…` names the local endpoints only; the network's are in
  `server.info`.

## Live switching in the daemon

- **Enable** (control thread): validate, store the setting, load/create the key and
  authorized file, make the `SecureContext` (audit into the `AuthLog`), make and bind both
  ROUTERs with today's options (`data_socket_options(…, true)`, liveness) and a short
  linger (200 ms), put `Attach { epoch, ctrl, data, secure }` on a channel and queue
  `TAG_ATTACH { epoch }` on the outbox. The io thread takes the `Attach` from the channel
  only when it reads that tag, so attach and detach are ordered with every reply and event
  in the one outbox stream. A failed bind drops its sockets and context at once. Reply `ServerInfo` (`Listening`, or `Waiting` on a bind failure).
- **Disable:** queue `TAG_DETACH { epoch, off }` on the outbox, in band like
  `TAG_REVOKE`/`TAG_CLEAR`; with `off` the io thread sends each of the pair's data peers
  the `network_off` notice before it drops them and closes the pair; reply at once. A change of bind or port while on is `TAG_DETACH { e1 }` then
  an attach of `e2`. The mDNS `Advertiser` is dropped on a helper thread: its goodbye blocks up
  to about 1 s, and the control thread must keep renewing leases (1.5 s expiry).
- **io thread:** never sends an ack the control thread waits for. On `TAG_DETACH` it drops every `Network` peer (their
  `Interest` counts with them, as `drop_key` does) and hands the sockets and context to a
  closer thread, which drops them there (linger lets the event leave; context term waits
  for it), so the io thread never blocks. A re-enable right after can find the port still
  held by the closer: its bind fails, it goes `Waiting`, and the retry timer binds it. `ZAP_CHECK` (100 ms) and the admission closure apply per
  attached pair (io.rs 196-213, 262-266); with no pair the poll is the 1 s `IDLE_POLL`,
  woken by the outbox tag on attach.
- **ZAP handler death:** the io thread drops that pair and reports `Failed { ZapFailed }`
  to control; local keeps serving; the retry timer makes a new context and pair, under
  `--listen` too, so pupu and the CM4 come back unaided.
- **At turn-off:** remote leases are no longer renewed and expire, and a stimulus they held
  fades as on any lease loss; jobs started by network clients finish, their replies are
  dropped (old epoch); recording and the Leq log continue. `firewall::report` and the
  systemd status line run on each `Listening`.

## Security

- **Who may toggle:** requests on the local socket (origin from the io thread, never the
  payload). Remote: refused "change it on the rig's own machine".
- **Confirmation:** none in the daemon (the local user already holds every power, emitting
  sound included); the app asks before enabling, naming the address and port.
- **Local boundary:** Linux/macOS: ipc files in the per-user runtime dir (0700): that user
  and root. **Windows: loopback tcp, kept by decision: any local user of the machine, and
  anything forwarded to its loopback (ssh `-L`, a port proxy), counts as local and can turn
  the network on and authorize a key.** There is no token; that is the same exposure as
  every other command there today (emitting sound included), and the user guide says so.
  Windows local moves off `DEFAULT_PORT` (to 47822/47823) so local and network never clash.
- **Key files on Windows** get no ACL step (`write_private_atomic` sets 0600 on Unix only);
  the user guide says the per-user config dir's default ACL is the protection.
- Off ends every remote connection at once (closed sockets); revocation and ZAP rules are
  unchanged while on.

## App: Settings › Connection

From `server.info`, refreshed on open and on a `State.server` change:

- **Network**: Off / On, an **Interface** choice (All interfaces, then each interface with
  its addresses, or a typed address) and a **Port** (data uses port + 1). Status: "Off",
  "Listening on <ctrl>", "Waiting: <reason>", "Failed: <reason>". Disabled with "Fixed by
  ac2d --listen", and with "Change it on the rig's own machine" when connected remotely.
  Turning on asks first. "Not saved: <why>" when the store cannot write.
- **Server key**: fingerprint, copy (Z85) for pairing; "Created when the network is first
  turned on" before any key exists. Advertised name under it. Status reasons and "not
  saved" come typed (`NetReason`, `saved`) and are worded by the app.
- **Authorized**: name, fingerprint, app or rig; **Revoke** (not the app's own key).
- **Refused**: fingerprint, address, count, last time; **Authorize** shows the full
  fingerprint and address, says "Check that the other device shows this same fingerprint",
  asks a name (1–64 characters, no spaces) and sends
  `server.authorize`. Entries without a key (not CURVE) have no action.

## Stages and tests

1. **Spike** (this commit): the listen_toggle tests.
2. **ac2d dual pair, no wire change:** lifetime/secure contexts, `Route`/`Origin`/`Peer`,
   epoch, `TAG_ATTACH`/`TAG_DETACH`, the retry timer and "a network failure is not fatal"
   (never deployed without them: today ZAP death is fatal and systemd's
   `Restart=on-failure` is what brings pupu and the CM4 back), data-plane origin, `Shared.network` replaced by per-request origin,
   leases and audit on `Peer`, `local-` names refused by authorize/admit, dedup on key for
   `Curve`, tap at every session start, PeerBrowse on key + peers, `NoServerKey`, shared
   port validator, injectable lister and backoff; `--listen` serves local too; endpoints file and
   `LockHolder` simplified. Tests: path refused for a `Curve` client and accepted for a
   `Local` one on one daemon; equal routing ids on both pairs; Fixed still serves local; ZAP
   death under `Fixed` goes `Failed`, local stays up, and it returns to `Listening` unaided;
   with the network off and a rig peer whose address changes, the outgoing link follows it;
   an address vanishing while bound goes `Waiting` then `Listening` when back; `Waiting {
   InterfaceMissing }`; a stale-epoch reply dropped when a routing id is reused; `local-x`
   refused by `server.authorize` while an existing `local-x` file entry loads; a port clash
   under `--listen` exits 2. **All-OS CI run here** (endpoints, Windows ports).
3. **Live toggle + wire:** `server.network`, `ServerInfo`/`State.server`, `network_off`
   event before `TAG_DETACH`, `network.json`, `NetworkChoice`, interfaces and `Waiting`/retry, mDNS on its
   helper, PROTO 93, WIRE_LOCK, samples, fixtures, protocol doc. Tests: enable with a session
   open, a rig subscribes and gets audio; `Interest` back to zero after detach; re-enable
   after ZAP death works; restart with the port busy goes `Waiting`, then `Listening` once
   freed; rapid on/off/on; a remote client receives `network_off` before its connection
   ends; a request retried across off/on is answered once; on, change the port, on again;
   a damaged or other-version `network.json` set aside (off, damage notice); `server.info`
   to a `Curve` peer without local endpoints or interfaces; **all-OS CI run again**; a remote lease's stimulus faded at turn-off; toggle refused from
   remote, when fixed, and for bad ports.
4. **CLI:** `ac2 server network on|off [--interface X] [--port N]`, `ac2 server authorize
   <name> --refused <fingerprint>`.
5. **App rows** above, keytips, user guide (with the Windows notes: key files have no ACL
   step, and any local user or forwarded port can turn the network on and authorize).
6. `docs/rigs/pupu.md` (line 86): units keep `--listen`.
