# ac2 — Project Plan

Open-source live dual-channel FFT analyzer for PA system tuning. Successor to `ac`
(`~/src/ac`). Goal: match the feature set of today's commercial live-tuning analyzers,
then go beyond them on UX, networking and scriptability.

- Platforms: Linux (JACK / PipeWire-JACK), macOS (CoreAudio), Windows (WASAPI, ASIO)
- Language: Rust (stable), single Cargo workspace
- Architecture: `core` (pure math) → `daemon` (owns audio + state) → frontends (`cli`, `ui`) over ZeroMQ, locally or across the network
- UI: keyboard-first, GPU-accelerated, lightweight, smooth
- License: MIT (ASIO handling in §12)

---

## 1. Relation to `ac`

ac2 is a clean-slate rewrite, not a port. `ac` proved the math on a real rig; it also
accumulated Python-era shapes, lab-bench scope, versioning shims and process weight.
ac2 takes **ideas, algorithms and raw test material** from `ac` and re-derives code and
expected results independently. Nothing is copied file-for-file.

### 1.1 Carry forward (re-implement cleanly)
| From `ac` | Why it is worth it |
|---|---|
| MTW ladder (`visualize/mtw/`): decimated stages, independent per-stage decimation, pair decimator, complex crossover blend, comparable averaging depth per frequency | best code in `ac`, rig-proven |
| Display smoothing (log-f Hann, ENBW-widened, phase unwrapped first, coherence never smoothed) | correct and subtle |
| Trace averaging incl. coherence-weighted | parity feature |
| Data protection: clip reject, no-reference pause, weak-reference hold | prevents garbage averages |
| Live IR views: log + ETC (Hilbert), max-bucket decimation | parity feature |
| Delay finder on high-passed IR (2 kHz in `ac`; band becomes selectable); tracking only on agreement of non-overlapping windows | fixes room-mode mispicks |
| ESS deconvolution, harmonic split, Tukey gating | IR mode |
| ISO 3382-1 metrics (per-band trigger, noise truncation + correction) | IR mode |
| IEC 61260-1 filterbank, IEC 61672 A/C weighting | honest RTA / SPL |
| Mic curve parser (.frd/.txt) + inverse FIR; shared with target curves | parity feature |
| Scene/view split ("if a value can be wrong it lives where a test can assert it without a window") | testability of UI |
| Single keybinding table, no dead keys, stimulus arm→fire safety machine, fault banners | UX that worked |
| Typed wire structs shared by producer and consumer, protocol doc parity test | killed schema drift |
| Loopback-only default bind, explicit public mode | safe default |
| Fail-closed backend selection; fake backend only on explicit request, sharing the real ring-drain path | no silent fake data |
| Drive dead-man (full-state `set_drive`, ~1.5 s monotonic timeout), keepalive frames | live safety |
| Rig runbook traps, raw captures from real rigs (as stimulus material, not as oracle) | test realism |

### 1.2 Leave behind
| `ac` piece | Reason |
|---|---|
| Lab bench: THD/THD+N, AES17 noise, BS.468, DMM/SCPI, GPIO, `test_dut`/`test_hardware`, dBu cal prompts, stepped sweeps | out of PA-tuning scope |
| HTML/PDF report renderers, report schema v1–v13, `ir_stats` arrival forensics, citation audit | archival machinery, not live tuning |
| Calibration session/refusal machinery, τ history, loop-gain baselines | overbuilt for PA; replaced by a small cal store with provenance, tied to device + channel + mic name (§5.7) |
| CWT / CQT / reassigned spectrograms | replaced by one spectrograph (later) |
| CWT-based RTA | RTA must be IEC filterbank or plain FFT, honestly labelled |
| Full-rate Welch H1 with phase-rotation-only delay compensation (`transfer.rs:94,174`: both channels windowed at the same offset) | biased for large delays — MTW + time-domain alignment only |
| REQ/REP + `"topic json"` PUB strings, base64 snapshots over ctrl, `serde_json::Value` dispatch, hand-maintained field whitelists | replaced by typed protocol (§6) |
| Per-command worker engines + busy-guard groups | replaced by one duplex audio session with jobs (§4.3) |
| `AudioEngine` trait with no-op defaults and a reference-clone fallback (`audio/mod.rs:158`; latent — streaming transfer now refuses non-routing backends, `plan.rs:172`) | replaced by small trait + explicit capabilities |
| Per-request config re-read from disk | daemon owns state |
| Python leftovers: `sounddevice` alias, `src_mtime` restart, Python-shaped types/config, pytest suite, "for both Python and Rust" protocol doc | legacy |
| Back-compat shims (`meas_channel/ref_channel`, `drive: true`, "absent = v1", snapshot v1/v2, replay optional fields) | pre-1.0, no shims |
| Monolithic `app.rs`, separate spectrum/transfer shells, GPU-required snapshot tests | structure problem |
| Issue-number archaeology in comments | git history holds it |
| Two-model agent approval pipeline, label state machine, out-of-tree handoffs | process overhead; see §10 |

### 1.3 Code rules that keep legacy out
- No compatibility shims before 1.0. Wire/format changes bump the version; old readers fail loudly.
- No stringly-typed modes, commands or keys. Enums with serde.
- No trait default that returns `Ok` for an unsupported operation.
- Comments explain *why the physics/math requires it*, not which issue caused it.
- Every module has a one-line purpose in the crate map; nothing lands without a place in it.

---

## 2. Principles

1. **Core is pure.** No I/O, no threads, no allocation in hot paths. Deterministic, tested against independently derived reference data.
2. **Daemon is the single source of truth.** Frontends are views. Two UIs and a CLI on one daemon always agree.
3. **Network is first-class and authenticated from the first remote use.**
4. **Audio thread is sacred.** Callback only moves sample-indexed blocks into lock-free rings. Zero alloc, locks or syscalls.
5. **Keyboard first, mouse friendly.** Every action has a command and a bindable key.
6. **Display truth is testable.** Every number or string the UI shows is computed in a pure crate and asserted headless.
7. **Measure correctness.** Every DSP feature ships with golden vectors and a loopback test with a known system.
8. **Refuse rather than mislead.** No reference, no estimate, stale data → say so. A blank top end without a banner is a bug.
9. **Stimulus is safe by default.** Load, reconnect and restart always leave outputs disarmed.

---

## 3. Feature parity matrix

Priority: **P0** = needed to tune a PA, **P1** = expected by pros, **P2** = later.
Source: **ac** = idea exists in `ac` and is re-implemented, **new** = not in `ac`.
Phase numbers refer to §9. Phases 0–6 are the **1.0 release** (§9.1); phase 7 is post-1.0.

### 3.1 Audio I/O
| Feature | Pri | Src | Phase |
|---|---|---|---|
| JACK backend (RT-safe, sample-aligned multi-channel capture) | P0 | ac | 1 |
| CoreAudio / WASAPI via cpal, multichannel, RT-safe, with routing (unproven in `ac`) | P0 | new | 1 |
| Device enumeration, rate, buffer size; xrun / discontinuity telemetry | P0 | ac | 1 |
| Input meters with clip indication, always visible | P0 | ac | 4 |
| ASIO | P1 | new | 7 |
| Multiple devices in one clock domain; drift detection | P2 | new | 7 |
| Network audio via OS device (Dante VSC, AES67 via PipeWire) | P1 | — | works via OS |

### 3.2 Signal generator
| Feature | Pri | Src | Phase |
|---|---|---|---|
| Pink, white noise; seeded | P0 | ac (pink only) | 2 |
| Periodic pink (full-rate period ≥ delay search range; see §5.4) | P1 | new | 2 |
| Sine, log sweep as generator types | P0 | ac | 2 |
| Band-limiting, level, multi-output routing | P0 | new | 2 |
| Arm → fire stimulus safety, typed level, global max, drive dead-man, single owner lease (§6.5) | P0 | ac + new | 2–3 |
| Generator as internal reference (only with validated duplex timing, §4.3) | P1 | new | 3 |

### 3.3 Transfer function
| Feature | Pri | Src | Phase |
|---|---|---|---|
| MTW H1 + coherence ladder, 48 ppo grid, complex crossover blend | P0 | ac | 2 |
| Fixed-FFT mode (time-domain aligned) | P1 | new | 2 |
| Averaging of Sxx/Syy/Sxy (FIFO / exponential); reset | P0 | ac + new | 2 |
| Fractional-octave smoothing (1/3 … 1/48), after coherence | P0 | ac | 2 |
| Coherence display, blanking / alpha on traces | P0 | ac | 4 |
| Delay finder: selectable band, confidence, explicit "no estimate"; operator inserts / nudges / types | P0 | ac + new | 2 |
| Delay tracking (non-overlapping window agreement + confidence) | P1 | ac | 2 |
| Polarity invert, dB offset per measurement | P0 | ac | 4 |
| Phase wrapped / unwrapped, group delay | P1 | ac | 4 |
| Live IR panel: linear, log, ETC — regularised H1 → IFFT on uniform bins, time origin = inserted delay | P1 | ac | 4 |
| Multiple measurement pairs live at once | P0 | ac | 3 |
| Fault banners: NO REFERENCE / NO SIGNAL / CHECK ROUTING / CLIP / STALE (frame age) / NO DELAY ESTIMATE | P0 | ac + new | 4 |
| Delay change without full ladder resettle; sub-sample delay | P1 | new | 7 |
| Live spatial average of N transfer functions (done, as a math channel's average: `docs/design/math-channels.md`) | P1 | new | 7 |

### 3.4 Spectrum / RTA
| Feature | Pri | Src | Phase |
|---|---|---|---|
| Narrowband FFT spectrum with defined units (§5.3) | P0 | ac | 2 |
| Fractional-octave RTA 1/1 … 1/24, IEC 61260-1 bank or FFT banding, labelled which | P0 | ac (redo) | 2 |
| A / C / Z weighting | P0 | ac | 2 |
| Peak hold, decay | P1 | new | 4 |
| Spectrograph (one, GPU texture) | P1 | new | 7 |

### 3.5 Traces, comparison & sessions
| Feature | Pri | Src | Phase |
|---|---|---|---|
| Capture live → named, colored trace; slots; mandatory metadata: delay, polarity, offset, smoothing, cal state, mic, time | P0 | ac | 4 |
| Show/hide/lock/reorder, offset, invert | P0 | ac | 4 |
| A measurement owns its traces: one tree of measurements with their captures, math channels and sweep runs (Imported for the rest); move, fold, delete keeping or deleting them; sweep as a measurement kind (done: `docs/design/measurement-tree.md`) | P0 | new | 7 |
| Synchronized comparison cursor across traces and panes | P0 | new | 4 |
| Phase comparison: overlays drawn relative to the selected trace's measured delay (pick key to change), so relative arrival stays visible; imported traces marked independent; per-trace delay nudge | P0 | new | 4 |
| Delay distance readout: delay × c(temperature) next to ms, no correction layers | P1 | new | 4 |
| Trace averaging: power, complex, coherence-weighted; common delay/phase reference stated per average | P0 | ac | 5 |
| Math channels: A ÷ × + − B and the average of N, operands by name (live measurements and stored traces), complex for transfer functions (magnitude and phase on a stated delay reference), levels for spectra / RTA, drawn in their kind's pane, live, editable, captured (done: `docs/design/math-channels.md`) | P1 | new | 5 |
| Target curves (file or drawn) | P1 | ac (file) | 5 |
| CSV export; import common analyzer text exports | P0 | ac (export) | 5 |
| Sessions: whole state save/load (always loads disarmed) | P0 | new | 5 |
| Raw capture files (lossless samples + config timeline) re-analysable within tolerance | P1 | ac (redo) | 7 |

### 3.6 Calibration & SPL
| Feature | Pri | Src | Phase |
|---|---|---|---|
| Mic sensitivity cal against 94/114 dB calibrator; SPL from raw dBFS | P0 | ac | 5 |
| Electrical sensitivity cal without a calibrator: voltmeter at the input + data-sheet sensitivity, stated uncertainty (done: `docs/design/q7-calibration.md` §11) | P1 | new | 5 |
| Mic curve library (several labelled curves per mic) and an explicitly chosen curve per input, with provenance | P0 | ac | 5 |
| Calibration tied to device + input channel + mic name; mismatch → "cal from other mic / input", otherwise cal age shown | P0 | ac (simplified) | 5 |
| Fast / Slow / Impulse; Leq, LAeq, LCeq, LCpeak, Lmax/Lmin | P0 | ac (partly) | 5 |
| Big-number SPL display + history (meter, Leq windows, or both; history strip rebuilt from the log) | P0 | new | 5 |
| Rolling Leq windows, limits and alarms; filling windows judged on their energy budget; informational regulation presets (done: `docs/design/leq.md`) | P1 | new | 7 |
| Continuous crash-safe logging, export (done for the per-second LAeq/LCeq/LZeq log: autosaved, in sessions, CSV export) | P1 | new | 7 |
| LUFS / true peak meter | P2 | ac | later |

### 3.7 IR / room acoustics
| Feature | Pri | Src | Phase |
|---|---|---|---|
| IR capture by ESS, deconvolution, harmonic split, gating; H2…H5 / THD vs f (done: `docs/design/sweep-distortion.md`) | P1 | ac | 7 |
| ETC, Schroeder, T20/T30/EDT, C50/C80/D50 per band (done: `docs/design/room-metrics.md`) | P1 | ac | 7 |
| STI / STIPA | P2 | new | later |

### 3.8 Beyond parity
- Headless daemon on small hardware, UI anywhere on the network
- Several clients on one rig (FOH laptop + second laptop at a delay tower)
- Scriptable: CLI + documented protocol, usable from Python etc. with ZMQ + msgpack
- Later: push EQ/delay suggestions to DSP processors

---

## 4. Architecture

```
┌──────────────┐   ┌──────────────┐   ┌──────────────┐
│   ac2-ui     │   │  ac2 (cli)   │   │ 3rd party    │
│ winit, wgpu  │   │ clap, tables │   │ python etc.  │
└──────┬───────┘   └──────┬───────┘   └──────┬───────┘
       │ ac2-scene (pure: data → display truth)   │
       │ ac2-client (typed, async)                │
       └──────────────┬───────────────────────────┘
                      │  ZeroMQ: ROUTER/DEALER (ctrl) + XPUB/SUB (data, events)
                      │  ipc:// / tcp:// / inproc://  ·  CURVE over network
              ┌───────┴────────┐
              │     ac2d       │  session, jobs, state, logging, discovery
              │  ac2-audio     │  JACK / cpal backends, rtrb rings
              │  ac2-core      │  pure DSP + acoustics math
              └────────────────┘
```

### 4.1 Workspace
One crate per concern: pure DSP (`ac2-core`) and pure display (`ac2-scene`) have no I/O;
only `ac2-zmq` links libzmq; the daemon (`ac2d`) owns audio, jobs and state; the desktop app
(`ac2-ui`) can host an embedded daemon (`--embedded`, local transports). The crate map with
each crate's role is in `CLAUDE.md`; golden vectors come from `tools/refgen`, protocol
fixtures from `tools/protocol`.

### 4.2 Key crates
| Need | Choice | Notes |
|---|---|---|
| FFT | `realfft` / `rustfft` | plans cached |
| Audio | `jack` crate directly (not cpal's JACK host), `cpal` for CoreAudio/WASAPI; Linux is JACK only (JACK2 or pipewire-jack, decision L1) | ASIO feature opt-in; cpal has no duplex API and no exact hardware index — see `docs/design/spike-audio-duplex.md` |
| Rings | `rtrb` | SPSC wait-free |
| ZMQ | own thin binding over libzmq 4.3.5 + libsodium built from source (`zeromq-src`, `libsodium-sys-stable`) | `zmq` crate's vendored build has CURVE off; pure-Rust options lack CURVE or are too young — see `docs/design/spike-zmq-curve.md`. ZAP handler must be bound before CURVE sockets (fails open otherwise) |
| Serialization | `rmp-serde` headers + raw `f32` LE arrays | decode via `bytemuck::try_cast_slice` with copy fallback on misalignment / big-endian |
| Discovery | `mdns-sd` | `_ac2._tcp`; discovery never implies trust |
| Async | `tokio` only in control plane | never on DSP threads |
| CLI | `clap` with typed unit value-parsers (`20hz`, `-12dbfs`, `1.5ms`), no panics | keeps `ac`'s pleasant grammar |
| GPU | `winit` + `wgpu` 30 | Vulkan/Metal/DX12; headless tests on lavapipe / WARP / Metal proven in CI — see `docs/design/spike-gpu-headless.md` |
| UI chrome | `egui` on wgpu (egui-wgpu shares wgpu 30), custom theme | plots via `ac2-plot` prepare/paint callbacks; iced_wgpu lags 3 wgpu majors |
| Text | `glyphon` / `cosmic-text` | |
| Config | TOML | |

### 4.3 Daemon model
One **audio session** owns the duplex stream. Analyses run as **jobs** (transfer pair,
RTA, SPL meter/log, IR capture) attached to it. Replaces `ac`'s one-engine-per-command model.

```
audio callback ──► multichannel blocks {start_sample, frames, flags} ──► rtrb
                                                     │
                                         fan-out to jobs (one consumer per job)
                                                     │ results → per-topic latest slot
                                                     ▼
                                     publisher: coalesce per topic ──► XPUB
control (tokio) ◄─► ROUTER; serialized state commits; jobs controlled via channels
generator ◄── atomics / lock-free param swap ◄── control (owner lease enforced, §6.5)
```
- **Sync contract.** Every block carries its absolute sample index and flags (xrun, overflow, device change). All channels of a block travel together, so channels can never shift relative to each other. A gap is a discontinuity marker: affected jobs reset their averages and report it; nothing silently splices.
- **Reference & timing contract.** A measured reference is required: stimulus and reference leave through the same converter and the reference is looped back into an input, so every latency in interface, console, network and processors cancels. While a stimulus plays, the daemon continuously correlates generator output against the loopback input and flags any offset jump (buffer change, device reset, clock slip); when silent, the last value is shown with its age. No start-up probe. Without a loopback there is no internal reference. Detail: `docs/design/open-questions.md` Q3.
- **Job lifetime** follows explicit commands (`meas.start/stop`; an SPL meter's per-second log and Leq windows run with the meter). Closing every UI never stops measuring, averaging, logging or alarms. Subscriptions only decide what is *published* and which optional display derivations are computed.
- Block grid fixed to the sample stream (push pipeline); never re-segment a sliding buffer.
- **Bounded freshness, not guaranteed latest.** Each topic has one latest-result slot in the daemon; the publisher sends only the newest frame per topic with a small PUB HWM. Frames already queued in ZMQ cannot be replaced, so clients also drain their socket and keep only the newest frame per topic before rendering. Frames carry a capture wall-clock time so age is measurable remotely; past a deadline they are shown STALE. Recoverable state events use a separate path with replay (§6.2). Detail: Q2.

### 4.4 Scene layer
- `ac2-scene` turns frames + view state into a `Scene`: polylines with per-vertex alpha,
  axes, ticks, labels, readouts, banners — all plain data, all unit-tested headless.
- `ac2-plot` paints a `Scene`; it may choose pixel placement, never a value.
- Animation applies to navigation (zoom, pan, layout) only. Measurement values and
  fault transitions are shown as received; interpolation between data frames is
  optional, off for stored/compared traces, and never hides a fault or discontinuity.
- Renderer tests: headless wgpu on a software adapter (lavapipe / WARP) in CI, so
  rendering tests never need the rig GPU (the reason `ac`'s first GPU UI was abandoned).

---

## 5. DSP core

### 5.1 Transfer (MTW)
- Stages at fixed NFFT (4096) on decimated rates (full, ~12 kHz, ~4 kHz targets; factor
  = round(sr/target)), each stage decimated independently from full rate by Kaiser
  polyphase FIR; both channels through one pair decimator with a shared phase counter.
- Overlap deepens per stage (e.g. 50/75/87.5 %). Averaging depth is matched in
  **model-based effective averages** (stationary-noise model of window overlap
  correlation and FIFO/exponential weighting), not raw block counts. This reduces
  coherence-bias steps at crossovers; it does not remove differences from spectral
  resolution, column aggregation or non-stationary signals. Effective N is reported per
  column and labelled as a model value; crossover behaviour is validated by test, not assumed.
- H1 = Gxy/Gxx, γ² = |Gxy|²/(Gxx·Gyy). Time averaging is over spectra only, never H,
  γ² or dB. Within a display column sum cross-spectra, divide once.
- Crossovers: H1 blended complex over 1/3 oct; γ² in the overlap is a labelled display
  blend of per-stage estimates, not a new estimator.
- Stage served band ≤ 0.45 × its rate. Grid 48 ppo base-2; columns thin rather than
  interpolate where resolution runs out.
- Alignment: one signed delay per pair; its whole samples shift the reference at full
  rate, before decimation; its fraction (≤ ½ sample) rotates each block's cross-spectrum.
  Negative delays first-class. A delay change splices the stream and keeps each stage's
  averages (rotated to the new delay) while every held block's window correlation with the
  new alignment stays ≥ 0.995; other stages start over, and only when none can keep does
  the ladder restart (`docs/design/delay-no-resettle.md`).
- Absolute levels never go through the decimated ladder.

### 5.2 Delay finder & tracking
- **Target.** The finder estimates the **first significant arrival** (direct sound): the
  earliest candidate within a band whose level is within a set threshold (e.g. −12 dB,
  tunable) of the strongest candidate. The strongest arrival is reported alongside. When
  candidates are ambiguous (several within threshold, close spacing), the finder returns
  all of them ranked and the operator chooses; tracking never acts on an ambiguous result.
- **Estimator**, separate from the ladder, on the **raw, unaligned** pair with uniform
  bins: regularised H1 = Gxy / (Gxx + ε·mean(Gxx)) → band-limit → IFFT gives an
  approximate band-limited impulse response. The recovery is only approximate where the
  band is sufficiently excited: regularisation shrinks weakly excited bins
  (Ĥ ≈ H·Gxx/(Gxx + ε·mean Gxx)), which reshapes the response and can reorder candidates.
  It is far less excitation-dependent than plain cross-correlation, not independent of it.
  GCC-PHAT is a diagnostic option only: in multipath scenes it accepted wrong arrivals
  10–20 % of the time vs 0 % for regularised H1 (Q1 evidence). Neither recovers frequencies
  the excitation never contained. Inadequate excited bandwidth in the selected band →
  "no estimate". Tests check that arrival picks stay stable across excitation spectra
  (white, pink, band-limited, programme material).
  Zero-padded to ≥ 2× the search span to avoid circular wrap; lag sign: positive = measurement late.
  Search range bounded and signed (default ±1 s, configurable). Result is the **absolute**
  delay; the held delay is never added to it.
- **Band** per measurement: high-pass 2 kHz default (full-range), band-pass for subs
  (e.g. 40–120 Hz) and mid boxes; auto mode picks from measured excitation.
- **Confidence** from peak-to-sidelobe ratio, band SNR and excitation check. Below
  threshold → explicit "no estimate".
- A residual check on the *aligned* stream reports `residual` separately (should be ≈0).
- **Tracking** moves delay only when a window sharing no samples with the last agrees
  within ±1 sample, both pass confidence and neither is ambiguous.
- Delay is operator-owned: finder proposes, operator inserts (or tracking is explicitly on).
- **Acceptance** on scenario fixtures (sub-only, reflection louder than direct, close
  interfering arrivals, fractional and negative delays, recorded noise at varying SNR):
  error ≤ 1 sample (full-range) / ≤ 0.1 ms (sub bands) when accepted; wrong-arrival
  acceptance ≤ 1 %; refusal ≤ 10 % at ≥ 20 dB band SNR on unambiguous scenarios.
  These are provisional targets for identifiable delays, not guarantees for acoustic
  onsets in general. Q1 revised them per band (mid 0.05 ms; sub 0.1 ms only for arrivals
  ≥ 2 pulse widths apart, ≤ 2 ms for unresolved sub clusters; sub tracking agreement
  ±0.1 ms): see `docs/design/q1-delay-finder.md`.

### 5.3 Spectrum, RTA, SPL
- Units defined per display: **amplitude spectrum** (bin-centred sine reads its RMS;
  normalised by coherent gain Σw; off-bin scalloping stated per window), **PSD**
  (dB re 1 FS²/Hz, one-sided; normalised by Σw²·fs), **band power** (Σ PSD × bin width,
  or IEC filter output). DC and Nyquist not doubled. One periodic window convention.
  Generator level is dBFS RMS with 0 dBFS = full-scale sine. Exact formulas and test
  cases (off-bin tones, DC/Nyquist, integrated noise power) in Q4.
- RTA: IEC 61260-1 bank (base-10 G) for banded levels; FFT banding labelled as such.
  Base-2 octaves only for MTW grid and smoothing. Never mix the two constants.
- SPL from raw dBFS + mic sensitivity. Voltage scale never in that path. Mic curve per
  §5.7 (filter before weighting/integration); LCpeak uses the uncorrected path unless the
  filter's latency and pre-ringing are characterised.
- A/C IIR verified per rate (44.1/48/96 kHz); Fast/Slow/Impulse; Leq in f64.
- Claims are "digital response tests per IEC 61672-1 tables, with stated tolerances",
  never implied instrument class compliance.

### 5.4 Generator
- Pink (free-running, filtered), white, periodic pink, sine, ESS. Seeded RNG.
  Band-limit filters. Level always typed by the operator (dBFS RMS); the system max level
  is enforced in the daemon and its output path: any client lowers it at once, a raise is
  confirmed and never passes the `--max-level` bound (`docs/design/settings.md`).
- Periodic pink: period P defined at full rate, a power of two, with
  P ≥ largest stage's full-rate window span **and** P > W + T, where W is the full width
  of the delay search interval (2 s for ±1 s) and T the significant response-tail
  duration (late reflections / decay down to the finder's threshold; configurable,
  default 1 s). Example: ±1 s search, 1 s tail at 48 kHz → 2¹⁸ ≈ 5.5 s. This prevents
  repeated direct-arrival candidates and keeps late energy from wrapping into the search
  interval only as long as the room's tail is actually shorter than T; it is not an
  absolute guarantee. Shorter periods require a correspondingly limited search range,
  shown in the UI. Free-running noise remains the default for unrestricted finding.
  Test: a late reflection placed to wrap into the search interval at a too-short period
  must be flagged or refused, never accepted as an arrival.
- ESS for IR capture and generator noise share one output path. An IR capture needs the
  stimulus lease like any emission (§6.5); it never takes it implicitly.

### 5.5 IR mode
- ESS + inverse filter, harmonic split, Tukey gate. ISO 3382-1 per band with own trigger,
  noise truncation + tail correction, sweep-IR noise floor valid ≤ one sweep length after peak.

### 5.6 Validation
- Expected results derived independently (`tools/refgen`, analytic models), never by
  reproducing `ac` output. Where ac2 intentionally differs from `ac` (window convention,
  Welch removal, finder), the difference is documented.
- Loopback: `x` vs `h * x + noise` with known filters and delays (incl. negative, large).
  Beyond unity loopback: partial coherence (uncorrelated noise added), unequal spectra,
  transients, steep phase slopes. Explicit regression: "large delay doesn't bias coherence".
- Raw `ac` rig captures replayed as stimulus; results judged against refgen, not `ac`.
- Quantified non-DSP gates: xruns/overruns, device disconnect + recovery, daemon
  restart recovery, CPU and memory per job, clock drift detection.
- `proptest` for averaging/smoothing invariants; `criterion` benches in CI.

### 5.7 Calibration store
- Per input: mic sensitivity, mic curve (magnitude-only file), optional voltage scale.
- Each entry records provenance (when, how, which calibrator, calibrator frequency) and
  is tied to **device + input channel + mic name** (typed once at cal time). Any of those
  changed → SPL readout says "cal from other mic / input"; otherwise it shows the cal's
  age. ac2 cannot know preamp gain or phantom state and does not pretend to: recalibrating
  after a gain change is the operator's job.
- Mic curve placement: TF, spectrum and RTA subtract the file's dB values from the
  displayed magnitude; SPL applies it as a filter before weighting and integration,
  normalised at the calibrator frequency (sensitivity cal uses the same convention, so
  nothing double-counts). Phase is never touched. A mic can have several curves (one per
  incidence angle); each input chooses one of its mic's curves or none, explicitly.
- Atomic writes; an unparseable file is never overwritten.

---

## 6. Protocol (ZeroMQ)

### 6.1 Sockets
| Socket | Daemon | Purpose |
|---|---|---|
| ctrl | ROUTER | async request/response with ids; slow handlers never block other clients |
| data | XPUB | frames + state events, subscription-aware |

Transports: `ipc://` (Linux/macOS), `tcp://127.0.0.1` (Windows) — an embedded daemon uses the same (private ipc dir / ephemeral loopback ports), since the client owns its own ZeroMQ context;
`tcp://<iface>` only in network mode, which requires CURVE on **both** sockets.

### 6.2 Messages & state sync
- Ctrl: msgpack `{v, id, cmd}` where `cmd` is a tagged enum with `deny_unknown_fields`.
  Reply `{v, id, result | error{code, msg}}`. Request ids are deduplicated per client
  for a retry window, so a retried command never executes twice.
- Command groups: `session` (devices, open, close, status), `gen` (acquire, release, arm,
  fire, set, stop), `meas` (create, update, delete, start, stop, reset), `delay`
  (find, insert, set, track), `trace` (capture, list, get, update, delete, average,
  import, export, mic curve), `cal` (spl, spl electrical, curve import / rename / delete,
  use, list, delete), `spl` (log get, log new, history get), `ir` (capture), `state` (snapshot, since), `grid` (get), `file` (save,
  load, list). `docs/protocol.md` is the normative list.
- Mutations take an optional `expect_rev` precondition; stale → `conflict` error.
  The daemon commits state changes serially.
- **State sync:** client subscribes to `evt` first, buffers, then requests a snapshot
  (`rev = R`), applies buffered events with rev > R. Daemon keeps a bounded replay buffer;
  `state.since(rev)` fills gaps or answers `resync_required` when the gap has expired, and
  the client re-snapshots. Keepalive carries `{daemon_incarnation, session_epoch, rev, seq}`,
  so a missed final patch, a session reopen or a daemon restart is always detected.
  Frames carry `session_epoch` and the `config_rev` actually applied by DSP, with the
  sample at which it took effect. Detail: Q5.
- Grids are immutable, identified by `grid_id`, fetchable any time via `grid.get`;
  frames reference them. Late subscribers fetch on first unknown id.

### 6.3 Data frames (normative schema in `docs/protocol.md`)
- Topics `d/<meas>/<kind>` (`tf`, `ir`, `rta`, `spec`, `spl`, `levels`), `evt`, `ka`.
- Multipart `[topic][msgpack header][payload arrays…]`. Header fields: `seq`,
  `audio_sample` (index in the session's sample clock, origin = session open),
  `daemon_incarnation`, `config_rev`, `grid_id`, `n`, `kind`, `units` per array,
  `protection` flags, `eff_avg` (effective averages).
- Arrays: little-endian f32, column order = grid order; invalid values = NaN with a
  separate validity/reason bitmask array where meaning matters (thinned column vs
  protected vs below floor).
- Bounds: max frame size and array length validated before decode; malformed frame →
  dropped and counted, never panics.
- TF frame (mag + phase + coh, ~480 cols) ≈ 6 KB. Up to 60 fps per measurement locally, 30 default remote (raisable) ≈ 180 KB/s; UI interpolates to display refresh.
- Large blobs (raw captures) via chunked binary ctrl transfer, never base64.
- Cross-language fixtures: Rust encodes, Python decodes (and back) in CI.

### 6.4 Versioning & security
- One protocol version in `hello`; mismatch = refuse with clear error. No implicit
  defaults for missing versions.
- Default local only. Network mode: CURVE on ctrl and data, ZAP with `authorized_clients`,
  client pins the server key on first pairing (`ac2 auth pair`, short code / QR).
  mDNS only advertises (name, version, key fingerprint); it never establishes trust.
- Tests cover unauthorized connects on both sockets.

### 6.5 Multi-client rules
- **Stimulus lease.** One client holds the stimulus at a time. `gen.acquire` returns a
  lease token; every stimulus command must carry it; the client refreshes it at least
  every 0.5 s (expiry 1.5 s, the drive dead-man). Expiry stops output in the daemon's
  output path and disarms.
- **Takeover** requires `gen.acquire --force`: it stops output and disarms first; the new
  owner must arm and fire explicitly. IR capture follows the same rule.
- **Stop is universal.** Any authorized client can stop and disarm output at any time
  without holding the lease.
- Session load and daemon restart come up disarmed with no owner. A disconnected
  owner's lease simply expires; on reconnect it starts disarmed and must re-acquire.
- The CLI holds a lease only while a foreground command runs (§7).
- Non-stimulus mutations are open to every authorized client, ordered by serial commit,
  optionally guarded with `expect_rev`.

---

## 7. CLI

Thin typed client. `--json` everywhere, `--watch` live terminal view. Unit-suffixed values
like `ac`, parsed with typed parsers.
```
ac2 devices
ac2 daemon start|stop|status
ac2 gen pink --out 1,2 --level -20dbfs        # foreground: acquires + arms, Enter fires,
                                              # Ctrl-C/Esc stops; lease ends with the process
ac2 gen stop                                  # any client, no lease needed
ac2 meas new tf --ref 1 --meas 2 --name main-l
ac2 delay find main-l --band 2khz- --insert
ac2 delay find sub --band 40hz-120hz
ac2 spl watch --input 3 --weight a --json
ac2 cal spl --input 3 --ref 94db
ac2 trace capture main-l --name l-pre-eq
ac2 trace export l-pre-eq --csv out.csv
ac2 --remote foh-rig.local status
```
Daemon auto-spawn locally; staleness detected by build id in `status`, not file mtimes.

---

## 8. UI

### 8.1 Feel
- Render on change; when live, vsync-locked (60–144 Hz).
- Spring animations for zoom, pan, layout. Values and faults are never animated away (§4.4).
- GPU lines: instanced quads, SDF anti-aliasing, per-vertex alpha (coherence), round joins.
  Spectrograph as scrolling texture + colormap LUT.
- Frame age shown; stale data marked, not just frozen.
- Perceptual, colorblind-safe palettes. Dark, light, high-contrast sunlight theme.

### 8.2 Keyboard
- One binding table, scoped (global / transfer / spectrum / SPL / IR), test-enforced: no dead keys, no conflicts.
- Layout-safe defaults: no `[ ] + -` (unreachable on Nordic layouts). Bindings user-configurable in TOML.
- `H` (or `F1`) help overlay (`/` is Shift+7 on Nordic layouts); `Ctrl/Cmd+K` command palette (fuzzy, shows key per command); the focused pane's most used keys on a hint line (`Shift+H` on / off); `Ctrl/Cmd+P` Settings: every setting in one full-window view, a page per area, each marked this app / the rig.
- Stimulus cluster reserved: `Space` arm, `Enter` fire, `Esc` stop, `↑/↓` level — with no
  window open. An open window (help, palette, prompt, dialog, view) owns the keyboard:
  `↑/↓` move or scroll it, `←/→` change a choice, `Enter` confirms, `Esc` closes the topmost
  window only; none of them reaches the stimulus. `Shift+Esc` stops the stimulus from
  anywhere, windows included (fixed, not remappable). No dialog arms anything (the sweep
  dialog makes a sweep measurement that waits); a playing stimulus keeps playing until
  stopped. The focused view decides what `Space` arms: the sweep view a run of the selected
  sweep measurement with its settings (the dialog that makes one when there is none), every
  other view the generator for live measuring; `Enter` fires what is armed, named in the top
  bar (`Enter fires: sweep Genelec 1 m · 3 s −50 dBFS`).
- Full screen (the stage view) shows only the focused pane, also while a stimulus is armed
  or playing or a sweep runs: no top bar, strip or badge comes back and no pane resizes.
  Operator's decision beside principle 9: full screen is the explicit choice to see only
  the pane; the stimulus stays safe by its keys (`Esc`, `Shift+Esc` stop from full screen
  as everywhere) and is shown in the top bar of every other layout.
- The keys that act on "the selected curve" (`A` show / hide, `Delete` / `Backspace`
  delete after a confirmation, the offsets) act on the item selected last: a measurement
  (its row in the measurement tree, its live curve's or a math channel's row, `N`, a pane's
  chip) or a stored trace (the tree, `V`); `Shift+A` the item's whole group (the
  measurement and everything under it); selecting a measurement
  deselects the trace, `Esc` hands the keys back to the measurement. Hiding a measurement
  is this app's display only; it keeps measuring. In an open window `Backspace` edits text
  and never deletes what is behind the window.
- Carry `ac` bindings that operators learned (`X` insert delay, `Y` track, `U` invert, `J` offset, `Z` target, `B` coherence mask, `M` average, `Ctrl+1..9` slots, `Shift+P` group delay) unless a conflict forces change (`H` IR became `Shift+I` when `H` became help).

### 8.3 Lightweight targets
- First frame < 300 ms, RSS < 120 MB with 8 live TFs, binary < 25 MB.
- Frame p99 < 4 ms at 1440p with 16 live traces on an integrated GPU.
- Idle (nothing changing on screen): no repaints, near-zero CPU; the GPU request prefers
  the low-power adapter on hybrid laptops; device limits fit a Pi 4 class GPU (V3D).
- Daemon, SPL meter only: a few % of one Pi 4 core at 48 kHz, no wakeup storms, disk
  writes proportional to new log rows.

---

## 9. Phases

No calendar estimates; each phase ends on a testable exit criterion. Risky integration
(backends on 3 OS, ZMQ + CURVE build) is spiked in phase 0, not discovered later.

Exit criteria are of two kinds: **CI** (hosted runners, software audio, synthetic
fixtures) and **HW** (named hardware acceptance runs, listed in `docs/rigs/`, with stated
device, sample rate, buffer size and job load). Hosted CI never stands in for an HW gate.

| # | Name | Scope | Exit criterion |
|---|---|---|---|
| 0 | Foundations + spikes | workspace, CI (linux/mac/win), lint/fmt, refgen; spikes: cpal multichannel duplex + output→input timing on mac/win, libzmq+CURVE vendored build on 3 OS, wgpu software-adapter test | CI: green on 3 OS; golden-vector harness runs. HW: duplex check (`ac2 selftest duplex`) on a real interface per OS |
| 1 | Audio | backend trait + capabilities, sample-indexed blocks, JACK, cpal, fake (explicit), xrun/discontinuity telemetry, duplex timing validation | CI: forced overflow yields a discontinuity marker, never a channel shift. HW: 8 in / 2 out @ 48 kHz, 128-frame buffer (256 on Windows WASAPI), 4 TF jobs, 1 h, zero self-caused xruns, per OS |
| 2 | Core DSP | MTW ladder + alignment, averaging (reset), smoothing, protection, delay finder (target, bands, confidence, candidates) + tracking, spectrum, RTA, weighting, generator | CI: §5.6 loopback suites pass against refgen; finder meets §5.2 acceptance numbers on scenario fixtures |
| 3 | Daemon + protocol + CLI | session + jobs, ROUTER/XPUB, typed proto, state sync with replay/resync, bounded-freshness publishing, stimulus lease, CURVE + pairing, client, CLI `--watch` | CI: two clients stay in sync through dropped events, expired replay, session reopen and daemon restart; stalled subscriber recovers to fresh frames; unauthorized connect refused on both sockets; lease expiry stops output. HW: CLI drives a live TF remotely over CURVE |
| 4 | Scene + UI | scene layer, wgpu plot, panes, keys, palette, banners, meters, live TF/RTA/IR, traces + metadata + slots + compare cursor, embedded daemon | HW: tune a real speaker end-to-end keyboard-only on Linux, macOS and Windows |
| 5 | Calibration, SPL, sessions | cal store (sensitivity, mic curve, device/channel/mic binding), SPL meter, trace averaging/math/targets, import/export, sessions | HW: **replaces `ac` for PA work** — mains + sub + delay alignment, cal'd SPL, saved/compared traces, session reload disarmed, on all 3 OS |
| 6 | Release 1.0 | packaging + signing, install docs, protocol docs, mDNS polish | HW: clean machine install → first measurement < 2 min per OS; FOH↔stage over WiFi |
| 7 | Post-1.0 extras | ASIO, SPL logging/alarms, ESS IR + ISO 3382, spectrograph, spatial average, raw capture files, delay without resettle, multi-device | per-feature criteria (room metrics vs published values; 24 h log clean; …) |

### 9.0 Status

Last updated 2026-10-05.

| # | CI criteria | HW criteria |
|---|---|---|
| 0 | done | duplex check (`ac2 selftest duplex`) on real mac/win interface — **open** |
| 1 | done (overflow → discontinuity, never a channel shift) | 8 in / 2 out, 1 h, per OS — **done on Linux** (pupu, 48 kHz / 128, 4 TF jobs on electrical loops, 0 xruns in 65 min; `docs/rigs/pupu.md`); macOS, Windows open |
| 2 | done (refgen + Q1 scenario acceptance) | — |
| 3 | done (sync, replay, restart, lease expiry, CURVE refusal) | CLI drives a live TF remotely over CURVE — **done** on Linux (`docs/rigs/pupu.md`, network test) |
| 4 | done (headless UI snapshots on lavapipe/WARP/Metal) | keyboard-only tuning of a real speaker per OS — **open** (Linux: measured from the app on pupu) |
| 5 | done (traces, sessions, calibration — acoustic and electrical, mic library — SPL) | mains + sub + delay workflow per OS — **open** (Linux: electrical SPL calibration on pupu, 2026-10-04) |
| 6 | done (packages, release dry run, mDNS) | clean install → first measurement < 2 min per OS — **open** (macOS: disk image installs, app starts and asks for microphone access, tester 2026-10-05; Windows: MSI install and simulated rig in a VM); signing needs Apple Developer ID + Windows code-signing cert |
| 7 | in progress (post-1.0): done — ESS sweep with H2…H5 / THD and IR (`docs/design/sweep-distortion.md`), rolling Leq windows, limits, alarms and presets with the per-second SPL log, run clock, new log and history, peak limits (LCpeak, LAFmax), measuring-position correction and alarm hysteresis (`docs/design/leq.md`); acoustic calibration dialog in the app (`docs/design/q7-calibration.md` §12); spectrograph under the spectrum (`docs/design/spectrograph.md`); delay change without resettle and sub-sample delay (`docs/design/delay-no-resettle.md`); output-vs-input clock drift detection (`docs/design/multi-device.md`); raw capture files: record to f32 WAV/RF64 + sidecar, replay as a session, replay within stated tolerance (`docs/design/raw-capture.md`); ISO 3382-1 room parameters per band from the sweep IR (`docs/design/room-metrics.md`); live spatial average of transfer functions (`docs/design/spatial-average.md`); audio that stops is reported and reopened by itself (`docs/design/audio-recovery.md`); Settings view, system max level at run time, output names and server features (`docs/design/settings.md`); band Leq per 1/3 octave with a measured band transfer (`docs/design/band-leq.md`); open — ASIO, multi-device support (resampling) and input-vs-input drift | 24 h log clean — **done** on Linux (pupu, 32 h log with 26 h continuous, no discontinuity; `docs/rigs/pupu.md`) |

Hardware so far: Linux on one rig (JACK, RME Fireface 400, 96 kHz / 256 frames:
transfer, delay finder, sweeps, electrical SPL calibration, remote CLI and app over CURVE,
mDNS; `docs/rigs/pupu.md`); Windows only as an MSI install in a VM with the simulated rig
(`docs/design/backlog.md`); macOS: the universal disk image installs and starts on a
tester's Mac (microphone prompt shown), not yet measured with an audio interface. No
GitHub release is published: installers are workflow artifacts of `release.yml` runs,
unsigned. Versions (protocol, session format) live in the code. CI (Linux, macOS, Windows) green again
since 9dc4274 (2026-10-05). The current macOS tester build (dev.9, 9dc4274) and its test
guide are in `testing/macos/` (binaries git-ignored); open items in
`docs/design/backlog.md` → *Performance and platforms*.

Performance pass for the real targets (2026-10-04, `docs/design/flow-control.md` for what
is left): idle wakeups cut (≈1340 → 250/s with a session open), work only for subscribed
and new results, display-sized spectrum frames (≈8–12 MB/s → ≈22 kB/s per client),
append-only SPL log (no whole-log rewrites, SD-card safe), cheaper SPL/RTA/delay-tracking
DSP, UI repaints only on visible change with per-pane rebuilds and a low-power GPU request.
Measured on the fake rig: daemon 10.5 % → 2.4 % of a desktop core with TF + RTA + spectrum +
SPL; UI with idle panes 3.3 → 0.2 cores under a software renderer. On pupu (i5-2415M,
96 kHz): SPL + spectrum ≈ 14 % of a core, of which ≈ 4 % is JACK's own process thread.
Not yet measured: §8.3 frame time on a real laptop iGPU, anything on Pi hardware. Pi 4 class
so far in emulation only (2026-10-05): the aarch64 test suites pass under qemu-user, and a
kiosk image (JACK, network-mode daemon, full-screen UI) boots in QEMU (`docs/design/backlog.md`).

### 9.1 1.0 release
Phases 0–6: one clock domain, reliable dual-channel TF and RTA, delay finder, traces and
comparison, calibration, SPL meter, sessions, recovery, authenticated remote use, signed
installers. Phase 7 items ship after 1.0 unless they fall out cheaply. `ac` stays in use
until phase 5 exit.

### 9.2 Design decisions required before code
`docs/design/open-questions.md` lists questions that must be answered (as design notes)
before the phase that implements them starts: Q1 delay target & acceptance (phase 2),
Q2 delivery freshness (phase 3), Q3 duplex timing (phase 1), Q4 level normalisation
(phase 2), Q5 replay & epochs (phase 3), Q6 stimulus lease protocol (phase 3),
Q7 calibration store (phase 5), Q8 phase comparison time reference (phase 4).

---

## 10. Process

- GitHub issues + PRs, CI gates (test, clippy `-D warnings`, fmt, golden, loopback, finder scenarios, headless render, cross-language frame fixtures).
- One AI review pass is fine; no label state machine, no out-of-tree handoffs. Design notes live in `docs/design/` in-tree.
- Rig testing kept as a runbook (carry `ac`'s traps and emission limits: typed level, ≤ −40 dBFS for unattended runs, bounded commands only).
- Standards references with verified citations kept in the design notes that use them (e.g. `docs/design/leq.md` *Sources*); standard PDFs stay out of the repo.

---

## 11. Packaging
- Linux: tarball + AppImage, Flatpak later; daemon as systemd user unit; PipeWire-JACK works.
- macOS: universal `.app`, signed + notarized, mic entitlement; optional launchd agent.
- Windows: MSI; ASIO per license decision (§12).

---

## 12. Risks & open decisions

| Risk | Mitigation |
|---|---|
| mac/win audio unproven in `ac` | phase 0 spike + phase 1 HW gate on all 3 OS |
| Delay finder picks a reflection | first-arrival target, candidate list, scenario acceptance numbers (§5.2, Q1) |
| GPU UI testability (killed `ac`'s first GPU UI) | scene layer + software-adapter render tests in CI |
| ZMQ PUB queueing defeats latest-wins | daemon slot + client-side drain, frame age, STALE (§4.3); `CONFLATE` unusable with multipart |
| MTW resettle on delay change (~2.4 s) | **resolved** (phase 7): stages keep their averages, rotated, while the change is small next to their window; only larger changes resettle (`docs/design/delay-no-resettle.md`) |
| Clock drift between devices (~600 µs in 6–30 s) | one clock domain required; drift detection warns (`docs/design/multi-device.md`) |
| libzmq / CURVE build on Windows/macOS | phase 0 spike, vendored |
| Scope creep before 1.0 | §9.1 slice; phase 7 is explicitly post-1.0 |

Open decisions (settle before phase 1; technical design questions are in §9.2):
1. ~~License~~ **Decided: MIT.** ASIO stays an opt-in cargo feature, off in default
   builds; the SDK is not vendored. ASIO binaries need their own licensing decision when
   phase 7 starts (Steinberg proprietary terms vs GPLv3 SDK, which would make that binary GPL).
2. ~~Distance readout~~ **Decided:** shown as plain delay × c(temperature), no correction layers.
3. ~~UI chrome~~ **Decided (phase 0 spike):** egui + custom theme; requirement stays cross-platform, sleek, beautiful.
4. ~~Headless hardware~~ **Decided (revised 2026-10-05):** primary targets are laptops with
   integrated GPUs on battery (the UI, often with an embedded daemon) and, in the near
   future, a headless ARM board of Raspberry Pi 4 class running the daemon for SPL metering
   with the UI elsewhere. A desktop with a discrete GPU is the easy case, not the yardstick.
   The Pi is best effort until a Pi rig exists; then it gets an HW gate (SPL + Leq log 24 h,
   CPU budget, SD-card writes). SIMD where it matters, NEON included.
5. ~~Raw capture format~~ **Decided:** f32 WAV/W64 + JSON sidecar (config timeline,
   discontinuities, algorithm version). Exact sample preservation;
   DSP replay judged within tolerance, not bit-exact.
