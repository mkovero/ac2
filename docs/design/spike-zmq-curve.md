# Spike: ZeroMQ + CURVE (phase 0)

Feeds PLAN.md §6 (protocol), §4.3 (publishing), open questions Q2 (freshness), Q5 (frame
identity), Q6 (lease binding to identity).

> **Concluded; code promoted** to `crates/ac2-zmq` (build script, typed API,
> `SecureContext` + ZAP handler, authorized-keys store, `drain_latest`,
> `SubscriptionTracker`). Its tests (`cargo test -p ac2-zmq --test it`) cover the claims
> below; the spike's throughput example was not carried over.

## Answer

**Yes, with one caveat.** libzmq 4.3.5 with CURVE (libsodium 1.0.22) builds from cargo with
no system packages on Linux, and the build path for macOS and Windows has no extra
requirements beyond the platform C/C++ toolchain. The `zmq` crate as published can't do it:
its vendored build compiles libzmq **without CURVE**. ac2-zmq builds libzmq itself
(`zeromq-src` + `libsodium-sys-stable`) and binds the C API with a thin hand-written wrapper.
The socket design (ROUTER/DEALER ctrl, XPUB/SUB data, CURVE + ZAP on both, client-side
latest-wins drain) works as intended.

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

The build script: `libsodium-sys-stable` builds/fetches static libsodium and exports
`DEP_SODIUM_INCLUDE` / `DEP_SODIUM_LIB` (Unix); then
`zeromq_src::Build::new().with_libsodium(Some(..)).build()` compiles static libzmq with
`ZMQ_USE_LIBSODIUM` + `ZMQ_HAVE_CURVE`. A unit test asserts `zmq_version() == 4.3.5`,
`zmq_has("curve")` and (Unix) `zmq_has("ipc")`. `zeromq-src` and `libsodium-sys-stable` are
pinned exactly in `crates/ac2-zmq/Cargo.toml`.

| OS | needs | notes |
|---|---|---|
| Linux | C/C++ compiler, `make`, `sh` (build-essential) | Verified (Arch, gcc). Binary depends dynamically on `libstdc++.so.6`, which every desktop has. |
| macOS | Xcode Command Line Tools (clang, make) | libsodium via `configure`; libzmq uses kqueue. Verified on CI (macos-latest). |
| Windows MSVC | Visual Studio Build Tools (already needed by Rust) | Verified on CI (windows-latest, x64, debug). libsodium: `configure` fails, so `libsodium-sys-stable` falls back to the prebuilt zip **downloaded at build time** (minisign-verified), or `SODIUM_LIB_DIR` / `SODIUM_DIST_DIR` / vcpkg. That fallback unpacks to `<its OUT_DIR>/installed/libsodium/{include, x64/{Debug,Release}/v143/{static,dynamic,ltcg}}`, emits `rustc-link-search` + `static=libsodium` for the `static` dir of the current profile, but exports **no `DEP_SODIUM_LIB`** and `DEP_SODIUM_INCLUDE = <OUT_DIR>/installed/include`, which does not exist. Our build script rebuilds both paths from that anchor (`installed/libsodium/include`, `installed/libsodium/<arch>/<Debug or Release>/v143/static`) and asserts `sodium.h` / `libsodium.lib` are there. It also (a) appends `/DSODIUM_STATIC` to `CXXFLAGS` so libzmq does not expect `dllimport` symbols, (b) gives zeromq-src a header shim for the `builds/msvc/version.h` path it copies from a libsodium *source* tree, and (c) links `ws2_32`, `iphlpapi`, `advapi32` explicitly (zeromq-src names only `iphlpapi`; libsodium's RNG uses `RtlGenRandom` from advapi32). libzmq and libsodium are both static. Debug builds link libsodium's Debug archive against Rust's release CRT without errors. libzmq is C++: the binary needs `msvcp140.dll` (VC++ redistributable) unless built with `+crt-static`. IPC on Windows depends on `afunix.h` (Win10+ SDK); ac2 uses tcp://127.0.0.1 there anyway, and the ipc tests are Unix-only. Cold Windows CI build of the workspace is ~15 min (whole job). |
| Windows GNU | mingw toolchain | libsodium prebuilt archive (downloaded); no wepoll. Low priority. |

Measured (Threadripper PRO 3945WX, Linux): cold libsodium build 15.3 s (configure-bound,
sequential before libzmq), libzmq 7.9 s; static `libzmq.a` / `libsodium.a` 2.7 / 1.2 MB;
**~1 MB** added to a stripped release binary.

`cc` builds libzmq at the member crate's opt-level; `[profile.dev.package."*"]` does not
apply to workspace members, so ac2-zmq's C code builds at opt-level 0 in dev unless set
explicitly (it currently is not).

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

Numbers from `cargo test -p ac2-zmq --test it latest:: -- --nocapture`.

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

Publisher + subscriber in one process over tcp://127.0.0.1: 8 topics × 60 fps × 6 KB
(2.9 MB/s) costs 2.1 % of one core with NULL, 2.8 % with CURVE (latency p99 ~1 ms). Burst:
~90 k frames/s NULL, ~34 k frames/s (≈ 209 MB/s) CURVE. The planned load is < 2 % of CURVE's
headroom; encryption cost is not a factor.

## Decisions carried into ac2d / ac2-client

1. **`ac2-zmq` is the only crate linking libzmq.** Minimal typed API (socket-type enum, one
   setter per option).
2. **An I/O thread owns the sockets** (libzmq sockets are single-threaded): in ac2d it owns
   ROUTER + XPUB and polls them with an inproc PULL fed by handlers and the publisher. No
   tokio-integrated `ZMQ_FD` (edge-triggered semantics are a known trap).
3. **Ctrl:** DEALER ↔ ROUTER without an empty delimiter frame; one msgpack `{v, id, cmd}` per
   message; `ROUTER_MANDATORY` so replies to vanished clients fail visibly. The ZAP `User-Id`
   (without CURVE, the routing id) is the client identity for leases (Q6) and request-id
   dedup.
4. **Data:** XPUB with `XPUB_VERBOSE` and a per-topic interest set (`SubscriptionTracker`);
   on a new subscribe re-send the latest slot so late joiners don't wait. Per-peer SNDHWM
   ≈ topics × 2–4; small `SNDBUF` in network mode. Subscribe to `evt` before requesting a
   snapshot (Q5): the XPUB subscribe event proves the subscription live daemon-side, a `ka`
   keepalive received after subscribing proves it client-side.
5. **Client:** `drain_latest` before each render (read until EAGAIN, keep max `seq` per
   topic, count malformed); never render frame by frame. Validate sizes before decoding.
6. **Security:** ZAP handler first, alive for the context lifetime, exit = fatal; CURVE on
   both sockets in network mode.

## Risks / open items

- **Windows build**: verified on CI. Open: libsodium download at build time (decide: allow, vendor the zip via
  `SODIUM_DIST_DIR`, or vcpkg in CI); `msvcp140.dll` runtime dependency (decide:
  `+crt-static` vs redistributable in the installer).
- **pyzmq interop** of the CURVE build is untested (expected fine: same libzmq/libsodium);
  the Python fixtures exercise encoding only.
- rust-zmq's `links = "zmq"` means no dependency may pull in `zmq`/`zmq-sys`; any crate that
  wants `links = "sodium"` must agree on `libsodium-sys-stable`.
