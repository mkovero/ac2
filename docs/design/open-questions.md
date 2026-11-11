# Open design questions

Settled decisions (table) and the technical questions each phase answered in a design note
in `docs/design/` before it started (PLAN.md §9.2). An answer states the decision, the
reasoning and the tests that prove it.

## Decision sheet

### Decided

| # | Decision |
|---|---|
| A | Distance readout **in**: plain `delay × c(temperature)` shown next to ms, labelled as acoustic path distance. No correction layers (the uncertainty `ac` worried about is the operator's to judge). |
| B | UI toolkit: egui + custom theme (phase 0 spike); requirement: cross-platform, sleek, beautiful. |
| C | Performance targets: laptops with integrated GPUs on battery, and a Pi 4 class daemon for SPL (PLAN §12 decision 4). SIMD where it measurably matters. |
| D | Raw capture: f32 WAV (RF64 past 4 GiB) + JSON sidecar ([raw-capture.md](raw-capture.md)). |
| 1a | First arrival within −12 dB of strongest; strongest shown too. |
| 1b | One global threshold, operator-adjustable. |
| 1d | ±1 s default search range (configurable up to several seconds for long digital/network chains). |
| 1e | Sub band default **20–120 Hz** (subs go below 40 Hz). Auto mode picks from measured excitation. |
| 1f | Provisional targets stand; tuned on the rig for "useful vs too strict". |
| 3b | Reference is required. No loopback → no internal reference. |
| 4a | Generator 0 dBFS = RMS of a full-scale sine. |
| 5a | 1024 events or 60 s replay (my call). |
| 5b | Device or sample-rate change starts a new session epoch. |
| 6b | 20 ms fade-out on stop, never a hard cut. |
| 6c | Any authorized client may force takeover; output stops and disarms first. |
| 6d | Short network hiccup: output continues. |
| 8a | Shared time reference within a session; imported traces marked independent; **each trace's delay can be nudged individually**. |
| 1c | Near-equal peaks: both shown on IR panel + short list; first-arrival rule pre-selects; one key accepts, another picks; tracking pauses until resolved. |
| 2a | STALE = no fresh frame for ~1 s: trace dims and shows age. Says nothing about measured delay. |
| 2b | Up to 60 fps per topic, local and remote alike; UI interpolates at up to 60 frames a second, paced by the app, not the display's vblank (`ac2-ui` `pace`, `gpu::PRESENT_MODE`). Remote was 30 until frames were packed ([wire-slim.md](wire-slim.md)): a heavy tuning session went 9.3 → 3.2 Mbit/s at 60 fps (2.7 at 30), and transfer functions come at their analysis hop (≈ 26/s) whatever the rate, so 60 costs remote clients ≈ 0.5 Mbit/s more than 30. |
| 3a | No start-up probe. Daemon continuously correlates generator output with the reference loopback while a stimulus plays; offset jumps are flagged; when silent, last value shown with age. |
| 4b | Narrowband spectrum = tone level; RTA = band power; unit printed on axis. |
| 4c | Flat-top window available as an option in spectrum view, not default. |
| 6a | Dead-man heartbeat 0.5 s, timeout 1.5 s (configurable); daemon fades stimulus out on timeout. |
| 7a/7b | Calibration tied to device + input channel + mic name. Mismatch → "cal from other mic / input"; otherwise shows cal age. No gain/phantom fields, no prompts. Recalibrating after gain changes is the operator's job. |
| 7c | Mic curve: TF / spectrum / RTA subtract file dB from displayed magnitude; SPL applies it as a filter before weighting; phase never touched; on/off per input (refined in [q7-calibration.md](q7-calibration.md) §10: a mic has several curves and each input chooses one, or none). |
| 8b | Overlay reference = selected trace's measured delay (pick key to change); others drawn relative to it; per-trace nudge on top. |
| M1 | MTW averaging: equal statistical confidence across stages is the default; a "fast LF" option caps the deepest stage's settling at ~1 s and shows its higher coherence floor. |
| U1 | Banners live in a strip above the plots, outside the data area; plots shrink while a banner is up. |
| U2 | Magnitude, phase and coherence are each a transfer view of their own (G); the transfer stack lays them out as three panes (2:1:1), which a new transfer measurement opens in. Coherence can be overlaid on the magnitude view instead (Settings). |
| D1 | Delay finder acceptance: full ≤1 sample; mid ≤0.05 ms; sub ≤0.1 ms for arrivals ≥2 pulse widths apart, ≤2 ms for unresolved sub clusters; resolved sub pairs / sub programme ≤2 % wrong, ≤20 % refused at 8 s. |
| D2 | Sub observation 4 s default, operator choice 2/4/8 s. |
| D3 | Sub tracking agreement ±0.1 ms. |
| D4 | GCC-PHAT is a diagnostic option, not a fallback. |
| K1 | The stop is universal, even when another client owns the generator: Esc with no window open, Shift+Esc from anywhere (see K9). |
| K2 | Enter applies an open prompt/palette selection instead of firing (fails safe). |
| K3 | No key repeat on level; Shift+↑/↓ steps 3 dB; `L` types a value. |
| K4 | Remember last stimulus outputs per device; first run defaults to output 1, shown in the top bar. |
| K5 | Narrowband spectra always draw as a thinned line; bars only for RTA bands. |
| K6 | Quit waits at most 1 s for lease stop/release; daemon expiry is the safety net. |
| K7 | Slots are UI-only until sessions; they persist in sessions from phase 5. |
| K8 | Mic name per input in the session's input setup; calibration matches device + channel + mic name. Form superseded by V1 (the name and curve live in `InputKind::Mic`); the calibration key is unchanged. |
| K9 | An open window owns the keyboard (as desktop apps do): ↑/↓ move or scroll it, ←/→ change a choice, Enter confirms, Esc closes the topmost window only (a dialog over a view back to the view); no key routed to a window reaches the stimulus, and the wheel scrolls the window, not the plot behind it. Shift+Esc stops from anywhere (handled before any window; the stop key operators know, made deliberate by Shift; same keys on every layout; not taken by any desktop) and shows in the top bar while armed or playing. Closing the sweep dialog disarms a stimulus armed and not playing; a playing one keeps playing. Replaces "Esc always stops": a window's Esc that also stopped the noise surprised operators. |
| T1 | Digits 1–9 show/hide trace slots (Ctrl+1–9 captures); pane focus on Alt+1–9; Tab / Shift+Tab step the measurement tree's rows. (provisional) |
| T2 | Calibrations stay in the per-machine store, not in sessions; traces record which calibration they used. (provisional) |
| T3 | Spectrum/RTA trace math needs identical grids; refused with a clear error otherwise. (provisional) |
| C1 | Data-socket send queue fixed per client at twice what the largest credit window and the event bound let be outstanding (`DATA_SNDHWM`; resizing live crashes libzmq 4.3.5). (provisional) |
| C2 | Platform-native dirs via ac2-paths: config dir for calibrations/keys/prefs, data dir for sessions. (provisional) |
| C3 | `cal.delete` / `ac2 cal rm`; calibration age in spectrum/RTA captions. (provisional) |
| L1 | Linux: JACK only (JACK2 or PipeWire via pipewire-jack); no ALSA. macOS: Core Audio through the HAL with a short default buffer (~20 ms) (H1); Windows: ASIO (W1). |
| P1 | Pane layouts live in the session, per measurement (`Measurement.layout`, `meas.layout`): choosing a measurement's row brings its own panes on every client; one without keeps the panes on screen; editing panes while a measurement's row is chosen stores them on it, and on no other measurement. The tree, pane kinds, views, what each pane draws (its required `content`, each reference with its rig: a graph pane's lead and the entries C ticked behind it, an SPL pane's meter, a Programme pane's inputs or pair, a Walk pane's walk; a deleted id leaves the pane, which is `nothing` once empty, and is never stood in for — [pane-curves.md](pane-curves.md), stage 2) and the level ranges the operator set travel; focus, W/F and the window stay in `ui.toml`. A chip pick or a dropped row changes only that pane, and neither it nor a click in a pane moves the tree selection: pane keys act on what their pane shows, selection keys on the chosen row. A child row of the list (live curve, math channel, stored trace) only selects; only the chip and a row dragged onto a pane put something in a pane. (operator, 2026-10-11; revised 2026-10-17: "save picks in session", "saved to session yes" for level ranges, "leave it on row" for the selection; "I would not use clicking traces from measurement list to choose what to show in pane, we already have drag n drop and picker") |
| P2 | **Dragging a row onto a pane is the one way to change what it shows** ([pane-curves.md](pane-curves.md)). Panes have no chip and no pane list; the title is plain text; palette *Choose pane measurement…* and Shift+drop compare are gone; P on an Inputs row and E on a Programme pane change no pane. A middle drop replaces the pane's content and turns it into the row's kind (the drag label says `becomes SPL`); a drop on the outer fifth splits. The measurement list shows every connected rig under its heading and any rig's row drags (the pane is set on that rig); clicking another rig's row (or Enter/Tab onto it) switches rig first, refused with a window open. Inputs list rows make Programme panes (a pair on its Goniometer). A measurement row click selects: one with an own layout brings its panes (P1), one without changes no pane. A pane nothing was dropped on reads *nothing picked — drag a row of the list here* and follows no selection; nothing stands in for a missing pick (a kind command uses the selected measurement of that kind, else none, and says so). A new measurement made in the app opens on a screen of its own (one pane of its kind; a transfer's stack, an alignment's view), stored as its own panes once listed; the panes before stay the chosen row's own and none becomes the new one's; panes that show nothing and are nobody's (a fresh app, an empty grid) are taken as before, the focused empty pane showing it (operator 2026-11-04: "doing new measurement should open new pane view too", over stage 1's split beside the focused pane). Landed in stage 1 (app only). To come (stages 2–3): graph panes hold an ordered list of entries from any rig (a group, a live curve, a stored trace; the first the lead), `PaneContent::Nothing` on the wire, compare per pane on **C** replacing the per-measurement list, mixed-rig panes stored whole in a layout, and P1's pane-content clauses and N2's `RigRef::Active` superseded. Rigs on this host read `<host>` (N7 supersedes the `· ac2d` / `· in this app` suffixes). Supersedes P1's chip wording. (operator, 2026-11-01: "that 2026-10-17 rule obviously dont fit if you can choose from any of the groups"; 2026-11-02: "remove it as whole from all panes to avoid confusion, lets not do billions of different ways changing the pane view") |
| S1 | Spectrum resolution is a typed choice (`SpecResolution`: `fixed {fft_len}` \| `multi {grid}`), editable on a running spectrum. `multi` = six full-rate Hann/… FFTs of halving length (longest: bin ≤ 96 000/65 536 Hz; shortest ≈ 21 ms), no decimation or zoom FFT; tone level per log-grid column, neighbouring bands blended in power over an octave so noise reads in an exact constant-Q bandwidth with no steps; per-band frame ages on the wire, columns never extrapolated; window changes drawn as faint dashed lines under the resolution-marker toggle. New spectra stay `fixed 65536` by default. Note: `multires-spectrum.md`. |
| S2 | Spectrum level is a typed choice (`SpecLevel`: `tone` \| `density`), per spectrum, editable (forms, `--level` on `meas new spectrum` / `meas set`). Default `density` for multi-resolution (its tone level reads noise in a bandwidth growing with f, ≈ 15 dB above a 65 536-point FFT at 3 kHz), `tone` for one FFT; crossing fixed↔multi without a level takes the new default. Density = power / noise bandwidth in Hz (fixed `ENBW·fs/N`; multi `Σ w_b·ENBW·Δf_b`, no step at crossovers), dBFS reference kept (`dBFS/Hz`, `dB SPL/Hz`), columns the mean of their bins. Tone and density traces are different kinds: math and averaging across them refused, not converted. Sessions before format 25 come back as tone (what they showed and stored). |
| W1 | Windows: **ASIO only** (decided 2026-10-11). It is the professional default, and with no Windows machine to test on, two Windows audio APIs cannot both be kept honest. Devices without an ASIO driver (USB measurement mics, class-compliant interfaces, built-in audio) go through FlexASIO (MIT, ASIO over WASAPI). **Done** 2026-10-12: ASIO is in every Windows build (no cargo feature), the WASAPI backend is removed, the Windows CI audio pass is FlexASIO on VB-CABLE, and with no ASIO driver installed device listing and session open name FlexASIO. Binding: azo, pinned to the fork `mkovero/azo` so ac2 controls it. `docs/design/asio.md`. |
| W2 | Windows builds are **not code-signed** (decided 2026-10-12): no budget for a certificate. The MSI and executables install with SmartScreen's "More info → Run anyway", which the tester guide shows. The release workflow keeps its optional signing steps, so a certificate later (for instance free open-source signing through SignPath Foundation, which needs an application) needs secrets only. |
| W3 | "ASIO" in plain text in the UI and docs (decided 2026-10-12): nominative use, no logo; the README carries "ASIO is a trademark of Steinberg Media Technologies GmbH". The SDK's trademark guidelines bind SDK licensees, and ac2 uses no SDK (`docs/design/asio.md` §1). |
| H1 | macOS: a direct Core Audio HAL backend replaces cpal (designed 2026-10-12; **done** 2026-10-12: stage (a), the backend beside cpal, (b), the macOS default, and (c), cpal removed (PROTO 48)). One IOProc per device: input and output in one callback on one `mSampleTime` line (`SingleCallback`, exact index, stated latency in frames); separate input and output devices run two IOProcs (`Unknown`, drift measured by the loopback monitor), no aggregate created by ac2. The HAL binding is its own MIT crate `cahal` (no ac2 deps, its own repository github.com/mkovero/cahal, taken from crates.io); `ac2-audio` holds a thin adapter. Microphone consent is checked up front, since a missing grant hangs capture. Staged: beside cpal, switch, remove cpal (PROTO bumps at the first and last). (provisional) `docs/design/coreaudio.md`. |
| R1 | Rig-scoped SPL meters (`MeasScope::Rig`) live in the daemon's `rig.json`, not the session: they survive session loads and closes, and a session file never carries them. |
| R2 | OSC cues are state-based (ok / near / over / fault), sent when a state begins and resent each 1 s so a receiver that missed one catches up. The meter-wide cue is the worst state of its limits, ordered Over > Fault > Near > Ok: a limit known exceeded outranks a meter that cannot read. |
| R3 | Rig edits (OSC target, monitor at start, a meter's config) name the rev they were made on and are refused with `conflict` when another client changed it first; the UI keeps the operator's edits on the newer rev, and Apply again overwrites. |
| R4 | Monitor at start reopens the recorded device **capture-only**: an unattended daemon start never emits sound. |
| V1 | Every input has an operator-declared kind (`InputKind`: `mic {mic, curve}` \| `line` \| `digital`) in `InputSetup`, replacing the mic role + mic name (K8's name and curve move inside `mic`), remembered per device with the roles: it fixes the dB reference (dB SPL via mic calibration; a line input reads dBFS and says so, dBu once a line calibration exists; digital is dBFS with no calibration ever offered) and which calibrations are offered. Acoustic measurement kinds (SPL, Leq, band Leq) take `mic` only and refuse others by name; transfer, spectrum, sweeps take any. Backends cannot tell AES from analogue, so nothing is inferred. The line calibration is its own store kind (`cal.line`, `v_fs`) with the scale `dBu_FS − 60 … dBu_FS` (designed 2026-10-23, revised after review, [programme-meter.md](programme-meter.md) §1) |
| V2 | Two inputs of one kind (mics included; a mic pair gets no loudness) may be declared a left/right pair; the Inputs list shows a pair as one row with both bars and a correlation; one input is in at most one pair. Setup keys exist only on the Audio & I/O page, meter keys (R = reset) only on Inputs-list rows and Programme panes ([programme-meter.md](programme-meter.md) §2) |
| V3 | Input meters are peak programme meters; the ballistic (`Ballistic`: `true_peak` (default) \| `type1` \| `type2` \| `vu`, and the loudness ballistics `momentary` \| `short_term` of §6.2) is a **rig setting** the daemon owns under R3 (`levels.set`, with the correlator window, over threshold and gate); the daemon runs every ballistic, holds a max per ballistic and reports the chosen one; VU is offered on a calibrated line input only and falls back to true peak elsewhere with the row saying so (a rig setting never blanks a bar); the frame carries true peak, the max and an overs count reset by `levels.reset`, the app a 1 s flying peak; CLIP (samples at full scale) and overs (true peak over the threshold) are different words; acceptance is IEC 60268-10's per-row burst tolerance, vectors frozen after the text is read; no scale library, bars read dBFS or dBu; preview carries kinds and pairs ([programme-meter.md](programme-meter.md) §3) |
| V4 | Pair correlation is the zero-lag normalised cross-correlation over an exponential window (0.3 s or 1 s, shown in seconds, never *Fast/Slow*), gated **per side**: a quiet side gives no reading and the chip names it (*right quiet*) instead of 0 ([programme-meter.md](programme-meter.md) §4) |
| V5 | The goniometer is 45°-rotated Lissajous (`x = (r − l)/√2`, left leans left) at auto gain (shape, never level), 256 points per frame picked one per bin at a random position (a fixed stride strobes tones), drawn as embers: additive points cooling along a blackbody ramp on an exponential fade (the one app-side setting), truth in the scene, ramp and blending in the plot ([programme-meter.md](programme-meter.md) §5) |
| V6 | `PaneKind::Programme` shows bars + correlation + goniometer (later loudness) for a pair or up to eight inputs; pane content is typed `PaneContent`: `meas` \| `inputs` \| `pair` (now `nothing` \| `curves` \| `meter` \| `inputs` \| `pair` \| `walk`, N2), since a Programme pane is not a measurement; everything else comes from the I/O page and the rig's settings ([programme-meter.md](programme-meter.md) §6) |
| N1 | The app holds any number of rigs (daemons), each a full `RigState` under a tab, one pane layout, one undo history and one Settings for the app; `RigId` is a persisted random id the hello reply (`Welcome`) carries with the rig's name (not the CURVE key: a local daemon has none; the simulated rig's is per start; the embedded daemon's own id file is superseded by N7, the app hosts no daemon). Stage 1 (parts 1a–1d) is the `AppState` split, one link per rig, namespacing every per-daemon id (`MeasId`, `TraceId`, topics, input numbers) by rig, and Shift+Escape stopping every rig's stimulus (K1). A command that resolves to a pane acts on the pane's rig, every other rig key (Inputs scope, slots 1…9 / Ctrl+1…9 included) on the active tab's rig; app toggles (T, W, F) are not rig keys; a window opens for the active rig and the drop-down waits while it is open; with more than one rig the arm line and banner name the rig. Rigs are switched only by a top-bar drop-down (hidden in the stage view); no tab-switch key (designed 2026-10-23, revised after review; slots per rig 2026-10-27, [multi-rig.md](multi-rig.md) §1) |
| N2 | A pane's content names the rig of each reference: `RigRef::{ThisRig, Rig {id: RigId, name}}` on every `PaneContent` reference (each entry of a graph pane's `Curves`, a `Meter`, `Inputs`, `Pair` or `Walk`), `LayoutPane.rig` gone ([pane-curves.md](pane-curves.md), stage 2). `ThisRig` is the daemon storing the layout; `Rig` names another by the daemon id of N1 (never the CURVE key), its ids stored opaque. It lives in `Measurement.layout` and the rig file's meter layouts (PROTO 80, session format 41, rig file 9). `content` is required: `Nothing` on any pane kind is a pane with nothing picked; `check()` refuses content of another kind than the pane's, more than 33 entries, an entry twice, and a Live/Trace that a Group in the same pane already draws. There is no following the tab: a pane shows what it names. A measurement's layout stores the panes of its own rig (a pane mixing rigs whole, other rigs' references opaque); a pane all of another rig stays in the app's prefs (D5) |
| N3 | (stage 3, built 2026-11-11, [remote-inputs.md](remote-inputs.md)) A session may include another rig's input, as measurement or reference, through a daemon-to-daemon capture stream anchored on backend capture stamps put on system time, joined at a fixed link offset, with daemon↔daemon pairing and marked gaps on loss. A cross-rig transfer function runs only while stamp evidence says the two clocks agree (both rigs locked to one reference); otherwise it holds and says why (operator 2026-11-10: *can't tell* is refused). An asynchronous resampler for unlocked rigs is a later stage. The remote input's setup and calibration are the owning rig's, read at open, never copied ([multi-rig.md](multi-rig.md) §3) |
| N4 | Each daemon reports its sample clock's deviation against its system clock (ppm, from block wall stamps) and the system clock's status as the OS states it (Linux `adjtimex` status + `maxerror` as a bound; elsewhere *not stated*), never inferring PTP vs NTP. A cross-rig delay states its basis in words and reads *offset unknown* until validated backend timestamps and stated latencies exist; an unsynced host marks the phase as independent-epoch (8b) ([multi-rig.md](multi-rig.md) §4) |
| N5 | Browse lists every rig; each connects and pairs on its own; the app remembers rigs by `RigId` (name, last address, key fingerprint in network mode) and reconnects at start ([multi-rig.md](multi-rig.md) §5). |
| N6 | A session stays one daemon's; Settings are the app's, rig settings each rig's; undo stays the app's pane history (daemon edits are not undone); a lost block of a remote input is a marked gap, never interpolated ([multi-rig.md](multi-rig.md) §6–7). |
| N7 | **ui is ui, daemon is daemon** ([ui-spawns-ac2d.md](ui-spawns-ac2d.md)). The app hosts no daemon: at start it spawns the `ac2d` beside its executable (never one on `PATH`) and connects at once; one `ac2d` per user is a lock every `ac2d` but `--ephemeral` takes; a loser exits 3 and the app reaches the winner named in `ac2d.endpoints` without owning it (a network-mode winner: the connect dialog says to pair with it, its address typed in). `--remote` / `--ctrl` never spawn. Settings › Connection: *Start ac2d when none is running* (default on; off on service hosts; `ac2d.service` has `SuccessExitStatus=3`) and *Stop ac2d when the app quits* (default on, forced on in the AppImage): the child then holds an inherited stdin pipe and stops in order (fade, autosave flush) when the app quits or dies; off, it is spawned detached and keeps running. An `ac2d` the app did not start is never stopped; `ac2 daemon stop` stops the app's child too. *Simulated rig* is a spawned `ac2d --backend fake --ephemeral` on loopback port 0, always stopped with the app. A local rig reads the host name (`ketunkolo`), superseding pane-curves D8's `· ac2d` suffix. `reach = "embedded"` rigs are dropped from `ui.toml` on load with a notice; `app-rig-id` / `autosave-app` unused. No wire change. Supersedes N1's embedded clause. (operator, 2026-11-03: "ui is ui and daemon is daemon") |
| G1 | *Spatial average* row in the New measurement chooser: the math form prefilled (average of several, power, every transfer of the session, `Spatial average`), own pane; dimmed with its reason under two transfers ([spatial-walk.md](spatial-walk.md) §1) |
| G2 | Plans and position patterns in-tree (`crates/ac2-room`, wire types owned by `ac2-proto`); the simulator in its own repository `ac2-roomsim`, no ac2 dependency ([spatial-walk.md](spatial-walk.md) §2) |
| G3 | A room plan is a floor polygon (any simple shape) extruded to an optional ceiling, with zones and markers; positions read in metres from FOH facing the stage ([spatial-walk.md](spatial-walk.md) §3) |
| G4 | Suggested positions: *Tuning* (default) and *ISO 3382-1* presets, no mirrored pairs, zone quotas then Lloyd relaxation, relaxed rules named; area pattern first, seat cluster later ([spatial-walk.md](spatial-walk.md) §4) |
| G5 | A walk is daemon state shown in its own pane; a move resets its measurements and turns delay tracking on; ready = `TfMeta.filled` = 1 with no tracking move and no fault; the average is an ordinary math channel the daemon updates ([spatial-walk.md](spatial-walk.md) §5) |
| — | Scene defaults recorded with U1/U2 (banner priority, units, coherence fade, IR dB re peak, spectrum thinning, CHECK ROUTING flag) stand. |

Round 2 answered 2026-10-01: all proposals accepted (rows above the line in the table below).

---

## Q1 — Delay target and acceptance (before phase 2)

**Design:** [q1-delay-finder.md](q1-delay-finder.md) (implemented). Decisions D1–D4 accepted
provisionally (sheet). Correctness is never scored as "agreement between two selectors" or
"repeatability".

## Q2 — Delivery freshness (before phase 3)

**Design:** [q2-q5-q6-protocol.md](q2-q5-q6-protocol.md) (implemented). The guarantee is
"bounded freshness with visible age", not "always latest".

## Q3 — Duplex timing (before phase 1)

**Design:** [q3-loopback-timing.md](q3-loopback-timing.md) (implemented; decisions 3a, 3b).
Spike input: [spike-audio-duplex.md](spike-audio-duplex.md). Backend latency reports are a
plausibility check only, never instead of the loopback.

## Q4 — Level normalisation (before phase 2)

**Design:** [q4-level-normalisation.md](q4-level-normalisation.md) (implemented): formulas
and dB references for every displayed level, window normalisation, generator convention,
test cases.

## Q5 — Replay, epochs and frame identity (before phase 3)

**Design:** [q2-q5-q6-protocol.md](q2-q5-q6-protocol.md) (implemented).

## Q6 — Stimulus lease protocol (before phase 3)

**Design:** [q2-q5-q6-protocol.md](q2-q5-q6-protocol.md) (implemented).

## Q7 — Calibration store (before phase 5)

**Design:** [q7-calibration.md](q7-calibration.md) (implemented: K8's mic name per input;
several curves per mic and an active curve per input, §10; electrical calibration, §11).

## Q8 — Phase comparison time reference (before phase 4)

**Answered by decisions 8a/8b** (no separate note); implemented in `ac2-scene`: overlays
relative to the selected trace's measured delay, **E** picks the phase reference, per-trace
nudge, traces from another session epoch or imported marked independent.
