# Spike: ZeroMQ + CURVE (phase 0)

Feeds PLAN.md §6 (protocol), §4.3 (publishing), open questions Q2 (freshness), Q5 (frame
identity), Q6 (lease binding to identity).

> **Code promoted.** The spike code (`spikes/zmq-curve`) has been removed; its build script,
> binding and tests were reviewed and promoted to `crates/ac2-zmq` (build, typed API,
> `SecureContext` + ZAP handler, authorized-keys store, `drain_latest`,
> `SubscriptionTracker`). Commands and paths below that name `spike-zmq-curve` describe the
> spike as it was measured; the throughput example was not carried over. The equivalent
> tests now run with `cargo test -p ac2-zmq`.

## Answer

**Yes, with one caveat.** libzmq 4.3.5 with CURVE (libsodium 1.0.22) builds from cargo with
no system packages on Linux, and the build path for macOS and Windows has no extra
requirements beyond the platform C/C++ toolchain. It cannot be done with the `zmq` crate as
published: its vendored build compiles libzmq **without CURVE**. The spike builds libzmq
itself (`zeromq-src` + `libsodium-sys-stable`) and binds the C API with a thin hand-written
wrapper (~600 lines incl. FFI declarations). The planned socket design (ROUTER/DEALER ctrl, XPUB/SUB data, CURVE + ZAP
on both, client-side latest-wins drain) works as intended. All tests pass and stayed green
over 120 runs while 12 test processes ran in parallel.

Caveat: on Windows, `libsodium-sys-stable` **downloads** a prebuilt, minisign-verified
libsodium archive at build time, unless `SODIUM_LIB_DIR` (e.g. vcpkg) or `SODIUM_DIST_DIR`
(a vendored copy of the zip) is set. CI builds and passes all tests on Linux, macOS and
Windows MSVC (windows-latest).

## Crate survey (2026-10-01)

| crate | version (date) | CURVE | notes |
|---|---|---|---|
| `zmq` (rust-zmq) + `zmq-sys` | 0.10.0 / 0.12.0 (2022-11) | **no** in vendored build | zmq-sys always builds libzmq via `zeromq-src` with `with_libsodium(None)` → `ZMQ_HAVE_CURVE` off. No feature or env var turns it on. Last release 2022; master has small fixes (2025-05), 90 open issues. `links = "zmq"`, so another libzmq build can't coexist without `[patch]`. |
| `zeromq-src` | 0.3.6+4.3.5 (2025-07) | yes, via `with_libsodium(Some(..))` | Builds libzmq 4.3.5 with `cc` (no CMake). tweetnacl was removed from its vendored tree; libsodium is the only CURVE backend. Linux/macOS/Windows (MSVC + GNU) handled; Windows IPC enabled when `afunix.h` exists. |
| `libsodium-sys-stable` | 1.24.0 (libsodium 1.0.22) | n/a | Unix: builds from a **bundled**, signature-checked tarball (`configure && make`). Windows MSVC/GNU: **downloads** the prebuilt archive at build time, verifies it with minisign; overridable with `SODIUM_LIB_DIR`/`SODIUM_DIST_DIR`/vcpkg. `links = "sodium"`. Maintained by the libsodium author. |
| `zeromq` (zmq.rs, pure Rust) | 0.6.0 (2026-05) | **no** | Mechanisms: NULL only (PLAIN parsed, not implemented). No inproc. IPC Unix-only. README: "does not implement all of ZeroMQ's feature set". Out. |
| `omq-tokio` / `omq-proto` (pure Rust) | 0.24.0 / 0.28.1 (2026-09) | yes (`curve` feature, RustCrypto `crypto_box`) | Authenticator callback (ZAP equivalent), ROUTER/DEALER/XPUB, inproc/IPC/TCP, Windows. pyzmq CURVE interop tests. **But:** first release 2026-05, 54 releases in 5 months, single author, ~11k downloads, CURVE implementation unaudited. Worth re-evaluating in a year; too young to put at the trust boundary today. |
| `libzmq-sys` (libzmq-rs) | 0.1.8+4.3.2 (2019) | yes (tweetnacl or libsodium) | Abandoned, libzmq 4.3.2. Out. |
| `zmq2`, `tokio-zmq`, `async_zmq`, `futures-zmq` | 2019–2022 | inherit zmq-sys | Unmaintained wrappers over the same zmq-sys. Out. |

### Choice

**libzmq 4.3.5 + libsodium, built by our own build script, with our own thin binding.**

- libzmq + libsodium is the reference ZMTP/CURVE implementation, interoperates with pyzmq
  (the Python cross-language fixtures in PLAN.md §6.3) and is battle-tested. CURVE is the
  network trust boundary, so maturity wins over "pure Rust".
- rust-zmq's API would be nice to keep, but its sys crate can't enable CURVE and it hasn't
  released in four years. Patching it (`[patch.crates-io] zmq-sys = { path = .. }`) works
  but ties us to a stale high-level crate anyway. The surface ac2 needs is small (context,
  socket, set/get option, bind/connect, multipart send/recv, poll, monitor, `User-Id`
  metadata, CURVE keypair, Z85), so a ~600-line wrapper we own is cheaper than a fork.
- Pure-Rust options: zmq.rs has no CURVE; omq is promising but five months old.

## Build notes

What the build does (`spikes/zmq-curve/build.rs`):
1. `libsodium-sys-stable` builds/fetches static libsodium and exports `DEP_SODIUM_INCLUDE`
   / `DEP_SODIUM_LIB` to our build script (Unix). On MSVC it exports only an include dir that
   does not exist; see the Windows row below.
2. `zeromq_src::Build::new().with_libsodium(Some(LibLocation::new(lib, include))).build()`
   compiles static libzmq with `ZMQ_USE_LIBSODIUM` + `ZMQ_HAVE_CURVE`.
3. A unit test asserts `zmq_version() == 4.3.5`, `zmq_has("curve")` and (Unix) `zmq_has("ipc")`.

Per OS:

| OS | needs | notes |
|---|---|---|
| Linux | C/C++ compiler, `make`, `sh` (build-essential) | Verified (Arch, gcc). Binary depends dynamically on `libstdc++.so.6`, which every desktop has. |
| macOS | Xcode Command Line Tools (clang, make) | libsodium via `configure`; libzmq uses kqueue. Verified on CI (macos-latest). |
| Windows MSVC | Visual Studio Build Tools (already needed by Rust) | Verified on CI (windows-latest, x64, debug). libsodium: `configure` fails, so `libsodium-sys-stable` falls back to the prebuilt zip **downloaded at build time** (minisign-verified), or `SODIUM_LIB_DIR` / `SODIUM_DIST_DIR` / vcpkg. That fallback unpacks to `<its OUT_DIR>/installed/libsodium/{include, x64/{Debug,Release}/v143/{static,dynamic,ltcg}}`, emits `rustc-link-search` + `static=libsodium` for the `static` dir of the current profile, but exports **no `DEP_SODIUM_LIB`** and `DEP_SODIUM_INCLUDE = <OUT_DIR>/installed/include`, which does not exist. Our build script rebuilds both paths from that anchor (`installed/libsodium/include`, `installed/libsodium/<arch>/<Debug or Release>/v143/static`) and asserts `sodium.h` / `libsodium.lib` are there. It also (a) appends `/DSODIUM_STATIC` to `CXXFLAGS` so libzmq does not expect `dllimport` symbols, (b) gives zeromq-src a header shim for the `builds/msvc/version.h` path it copies from a libsodium *source* tree, and (c) links `ws2_32`, `iphlpapi`, `advapi32` explicitly (zeromq-src names only `iphlpapi`; libsodium's RNG uses `RtlGenRandom` from advapi32). libzmq and libsodium are both static. Debug builds link libsodium's Debug archive against Rust's release CRT without errors. libzmq is C++: the binary needs `msvcp140.dll` (VC++ redistributable) unless built with `+crt-static`. IPC on Windows depends on `afunix.h` (Win10+ SDK); ac2 uses tcp://127.0.0.1 there anyway, and the ipc tests are Unix-only. Cold Windows CI build of the workspace is ~15 min (whole job). |
| Windows GNU | mingw toolchain | libsodium prebuilt archive (downloaded); no wepoll. Low priority. |

Measurements (Threadripper PRO 3945WX, 12 cores, Linux):

| what | value |
|---|---|
| cold build of libsodium (`configure` + `make`) | 15.3 s (build-script run) |
| cold build of libzmq (`cc`, parallel) | 7.9 s |
| total cold build of the spike crate incl. deps | ~25 s (sodium and zmq are sequential) |
| static `libzmq.a` / `libsodium.a` (release) | 2.7 MB / 1.2 MB |
| binary size impact (stripped release example vs Rust baseline) | 1.42 MB vs 0.37 MB → **~1 MB** for libzmq + libsodium + rmp-serde |
| build-deps pulled in | `cc`, `zip`, `tar`, `libflate`, `ureq` (download path), `minisign-verify`, `vcpkg`, `pkg-config`, `dircpy` — 69 crates in the normal+build tree |

Note: in a workspace member, `cc` builds libzmq at the member's opt-level (0 in dev). The
production sys crate gets `opt-level = 2` from `[profile.dev.package."*"]` only if it is not
a workspace member — or set it explicitly for that package.

## What the tests prove

`cargo test -p spike-zmq-curve` (18 tests, < 1 s, localhost only; every wait is a poll with a
10 s deadline, never a fixed sleep):

| test | claim |
|---|---|
| `ctrl::slow_handler_does_not_block_other_client` | ROUTER/DEALER with request ids: A's request parks on a test-controlled gate (deterministic "slow"); B gets 10 replies meanwhile; A's later request overtakes its own parked one (replies matched by id); opening the gate delivers A's slow reply. |
| `ctrl::bad_version_and_garbage_get_typed_errors` | version mismatch and undecodable msgpack produce typed errors, not silence. |
| `pubsub::xpub_sees_subscriptions_and_frames_roundtrip` | `[topic][msgpack header][f32 LE]` frames roundtrip bit-exact (6144-byte payload); publisher-side topic filtering; XPUB_VERBOSE reports every subscribe including duplicates; unsubscribe reported only when the last subscriber of a prefix leaves, **including by disconnect**. |
| `curve::authorized_client_works_on_ctrl_and_data` | CURVE + ZAP on both sockets; the ZAP `User-Id` ("alice") arrives with every ctrl message. |
| `curve::unauthorized_client_refused_on_ctrl_and_data` | Valid keypair not on the list: client and server monitors both report `HANDSHAKE_FAILED_AUTH` (status 400) on ctrl and data; the daemon never sees its request; XPUB never sees its subscription; after an authorized client received all 20 frames (barrier), the refused SUB and DEALER hold nothing. |
| `curve::plaintext_and_wrong_server_key_refused` | NULL-mechanism client and a client pinned to the wrong server key are refused (`HANDSHAKE_FAILED_PROTOCOL`) on both sockets and receive nothing. |
| `curve::without_zap_handler_any_curve_client_is_accepted` | Documents the fail-open gotcha below. |
| `latest::*` | Latest-wins behaviour and CONFLATE (below). |
| `transports::*` | Same ctrl + data roundtrip on tcp://127.0.0.1, ipc:// (Unix), inproc://; CURVE on tcp and ipc. |

## CURVE / ZAP pattern

- Server sockets: `CURVE_SERVER=1`, `CURVE_SECRETKEY`, `ZAP_DOMAIN="ac2"`, set **before** bind.
  Client sockets: `CURVE_SERVERKEY` (pinned), `CURVE_PUBLICKEY`, `CURVE_SECRETKEY`, before connect.
- ZAP handler: a REP socket bound to `inproc://zeromq.zap.01` **in the same context**, on its
  own thread. Request frames: version, request id, domain, address, routing id, mechanism,
  credentials (the 32-byte client long-term key for CURVE). Reply: version, request id,
  `200`/`400`, text, **user id**, metadata. Accept only `mechanism == "CURVE"`, the expected
  domain, and a key on the authorized list.
- The user id comes back on every received message as `zmq_msg_gets(msg, "User-Id")`. That
  is the authenticated client identity for Q6: bind the stimulus lease token and audit-log
  entries to it, not to anything the client says about itself.
- **Fail-open gotcha:** if no handler is bound, libzmq skips ZAP and a CURVE server accepts
  every client that knows the server public key (`ZMQ_ZAP_ENFORCE_DOMAIN` would refuse, but it
  is draft API, not compiled in). So ac2d must: bind ZAP before creating CURVE sockets, keep
  the handler alive for the whole context lifetime, and treat its exit as fatal (shut the
  network sockets). A test must cover this ordering.
- Refusals are observable without timing guesses: `zmq_socket_monitor` events
  (`HANDSHAKE_FAILED_AUTH` value = ZAP status, `HANDSHAKE_FAILED_PROTOCOL` for mechanism
  mismatch / bad key, `HANDSHAKE_SUCCEEDED`). Useful for the daemon log ("refused key
  <fingerprint> from <addr>") and for tests.
- A refused client keeps reconnecting (default reconnect interval); each attempt costs a
  ZAP round trip. Rate-limit/log per key+address in ac2d.
- inproc never runs a security mechanism; CURVE applies to tcp and ipc only.

## Latest-wins findings (Q2)

Run `cargo test -p spike-zmq-curve --test latest -- --nocapture` for the numbers.

- **XPUB drops the newest, not the oldest.** When a peer's pipe is at HWM, XPUB discards new
  messages for that peer (multipart-atomic). A stalled subscriber's queue therefore holds the
  *start* of the stall. inproc, SNDHWM = RCVHWM = 4, 2 topics × 1000 frames published during
  the stall: 8 queued (SNDHWM + RCVHWM), newest queued seq 3, 1992 dropped; a naive reader's
  first frame is seq 0.
- **HWM counts whole messages per peer, across all topics.** One small HWM shared by 8 topics
  lets one chatty topic crowd out the others during a hiccup. Size it per peer ≈ topics × 2–4.
- **The client drain recovers in one pass.** `drain_latest` (read until EAGAIN, keep max
  `seq` per topic) discards the backlog; inproc: the next published round is fresh and no
  stale frame appears afterwards. The publisher learns about freed space asynchronously, so
  the first round right after a drain may still be dropped — fine for a stream at frame rate.
- **Over TCP the kernel buffers add backlog.** With `SNDBUF = RCVBUF = 16 KiB` (Linux
  doubles this): 18–20 frames read before both topics were fresh again (2–3 drain passes),
  i.e. ~14 stale frames ≈ 85 KB beyond the HWM. With default autotuned buffers this can be
  MBs — at 180 KB/s remote rate that is seconds of stale data in flight. The drain still
  discards it quickly on a fast link; on a slow link (WiFi) the backlog has to cross the link
  first. Hence: frames carry capture wall time, clients show STALE/age, and ac2d sets a small
  `ZMQ_SNDBUF` on the data socket in network mode.
- **XPUB is always writable.** `POLLOUT` does not reflect a slow peer, so the daemon can't
  pace per subscriber. Freshness = small daemon-side latest slot + small HWM + client drain +
  visible age. Never set `XPUB_NODROP` (it would turn a stalled client into backpressure on
  the daemon).
- **CONFLATE does not support multipart — confirmed.** SUB with `ZMQ_CONFLATE=1`, 10 × 3-part
  frames sent: one single-part message arrives, the 16-byte payload of the last frame; topic
  and header are gone. It cannot replace the drain.

## Throughput / CPU (sanity)

`cargo run --release -p spike-zmq-curve --example throughput` (publisher + subscriber +
libzmq I/O threads in one process, tcp://127.0.0.1):

| run | result |
|---|---|
| paced 8 topics × 60 fps × 6 KB (2.9 MB/s), NULL, 5 s | 2400/2400 frames, **2.1 % of one core**, latency p50 167 µs / p99 1.05 ms |
| same, CURVE | 2400/2400 frames, **2.8 % of one core**, latency p50 275 µs / p99 0.85 ms |
| burst, NULL | ~90 k frames/s (≈ 556 MB/s) |
| burst, CURVE | ~34 k frames/s (≈ 209 MB/s) |

The planned load is < 2 % of CURVE's headroom; encryption cost is not a factor.

## Recommendations for ac2-proto / ac2d / ac2-client

1. **`ac2-zmq` crate** (or `zmq-sys`-style crate under `crates/`): the spike's `build.rs`,
   `ffi.rs`, `zmq.rs`, promoted and reviewed. Keep the API minimal and typed (socket-type
   enum, option setters per option rather than raw ints). Pin `zeromq-src` exactly. No other
   crate links libzmq. Set its opt-level explicitly.
2. **I/O thread owns the sockets.** libzmq sockets are single-threaded. In ac2d one I/O thread
   owns ROUTER + XPUB and `zmq_poll`s them together with an inproc PULL. Tokio handler tasks
   and the publisher coalescer feed it through a `Mutex`-guarded (or per-thread) inproc PUSH;
   no tokio-integrated ZMQ_FD tricks (edge-triggered semantics are a known trap). The spike's
   thread-per-request + PUSH-per-reply is spike-only.
3. **Ctrl:** DEALER ↔ ROUTER without an empty delimiter frame; frame = one msgpack
   `{v, id, cmd}`. Set `ROUTER_MANDATORY` so replies to vanished clients fail visibly. The
   ZAP `User-Id` (or, without CURVE, the routing id) is the client identity for leases (Q6)
   and for per-client request-id dedup.
4. **Data:** XPUB with `XPUB_VERBOSE`; keep a per-topic interest set (subscribe adds, the
   last-unsubscribe removes; prefix matching like ZMQ's). On a new subscribe, re-send the
   latest slot for that topic so late joiners don't wait for the next frame. Per-peer
   `SNDHWM` ≈ topics × 2–4. Subscribe to `evt` before requesting a snapshot (Q5): the XPUB
   subscribe event is the daemon-side proof that the subscription is live, and on the client
   side a keepalive on `ka` received after subscribing proves the same.
5. **Client:** before each render, `drain_latest` (read until EAGAIN, keep max `seq` per
   topic, count malformed); never render frame by frame from the socket. Validate sizes
   before decoding (the spike checks header ≤ 1 KiB, `n` bound, payload == `n × 4`).
6. **Security:** ZAP handler first, alive for the context lifetime, exit = fatal; CURVE on
   both sockets in network mode; tests for authorized, unauthorized, plaintext, wrong-server-
   key, and "no ZAP handler" ordering on both sockets.

## Risks / open items

- **Windows build**: verified on CI. Open: libsodium download at build time (decide: allow, vendor the zip via
  `SODIUM_DIST_DIR`, or vcpkg in CI); `msvcp140.dll` runtime dependency (decide:
  `+crt-static` vs redistributable in the installer).
- **macOS build**: verified on CI (macos-latest, arm64), no changes needed.
- **pyzmq interop** of our CURVE build not tested yet (expected fine: same libzmq/libsodium);
  add to the cross-language fixture job.
- **libsodium build time** (15 s, configure-bound) is paid on every clean CI build; cache
  `target/` or provide `SODIUM_LIB_DIR`.
- rust-zmq's `links = "zmq"` means no dependency may pull in `zmq`/`zmq-sys`; any crate that
  wants `links = "sodium"` must agree on `libsodium-sys-stable`.
