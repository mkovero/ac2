# Open design questions

Each question must be answered by a design note in `docs/design/` before the phase that
implements it starts (PLAN.md §9.2). An answer states the decision, the reasoning, and the
tests that prove it. When a question is answered, link the note here and mark it closed.

Origin: two review rounds of PLAN.md (2026-10-01). PLAN.md already fixes the direction for
each; these are the details that are too fine for the plan but too risky to decide in code.

---

## Decision sheet

All sheet items decided 2026-10-01 (two rounds). The Q sections below keep the
remaining *technical* details (formulas, thresholds, tests); those are settled in a design
note at the start of the phase that implements them, within the decisions here.

### Decided

| # | Decision |
|---|---|
| A | Distance readout **in**: plain `delay × c(temperature)` shown next to ms, labelled as acoustic path distance. No correction layers (the uncertainty `ac` worried about is the operator's to judge). |
| B | Toolkit is my call; requirement: cross-platform, sleek, beautiful. egui + custom theme, spike vs iced at phase 4 start. |
| C | Primary performance target: x86-64 with a modern GPU. Use SIMD / CPU features where it measurably matters. ARM/SBC is best effort, not a gate. |
| D | Raw capture: f32 WAV/W64 + JSON sidecar. |
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
| 2b | Up to 60 fps per measurement locally, 30 default remote (raisable); UI interpolates to display refresh. |
| 3a | No start-up probe. Daemon continuously correlates generator output with the reference loopback while a stimulus plays; offset jumps are flagged; when silent, last value shown with age. |
| 4b | Narrowband spectrum = tone level; RTA = band power; unit printed on axis. |
| 4c | Flat-top window available as an option in spectrum view, not default. |
| 6a | Dead-man heartbeat 0.5 s, timeout 1.5 s (configurable); daemon fades stimulus out on timeout. |
| 7a/7b | Calibration tied to device + input channel + mic name. Mismatch → "cal from other mic / input"; otherwise shows cal age. No gain/phantom fields, no prompts. Recalibrating after gain changes is the operator's job. |
| 7c | Mic curve: TF / spectrum / RTA subtract file dB from displayed magnitude; SPL applies it as a filter before weighting; phase never touched; on/off per input. |
| 8b | Overlay reference = selected trace's measured delay (pick key to change); others drawn relative to it; per-trace nudge on top. |
| M1 | MTW averaging: equal statistical confidence across stages is the default; a "fast LF" option caps the deepest stage's settling at ~1 s and shows its higher coherence floor. |
| U1 | Banners live in a strip above the plots, outside the data area; plots shrink while a banner is up. |
| U2 | Coherence in its own pane under phase (3:2:1) by default; toggle to overlay it on top of the magnitude pane. |
| D1 | Delay finder acceptance: full ≤1 sample; mid ≤0.05 ms; sub ≤0.1 ms for arrivals ≥2 pulse widths apart, ≤2 ms for unresolved sub clusters; resolved sub pairs / sub programme ≤2 % wrong, ≤20 % refused at 8 s. |
| D2 | Sub observation 4 s default, operator choice 2/4/8 s. |
| D3 | Sub tracking agreement ±0.1 ms. |
| D4 | GCC-PHAT is a diagnostic option, not a fallback. |
| — | Scene defaults recorded with U1/U2 (banner priority, units, coherence fade, IR dB re peak, spectrum thinning, CHECK ROUTING flag) stand. |

Round 2 answered 2026-10-01: all proposals accepted (rows above the line in the table below).

Round 3 answered 2026-10-01: all proposals accepted (rows M1–D4 in the Decided table above).

### → round 4 (raised by the desktop app, phase 4)

Fill `Answer:` (empty = proposal accepted).

**K1. Esc and other people's stimulus.** Esc sends a universal stop even when another
client (e.g. a laptop at the delay tower) owns the generator.
- Proposal: keep it. Esc is the panic key; any operator can always silence the PA.
- Answer:

**K2. Enter while a prompt or the palette is open** applies the prompt/selection instead of
firing the stimulus.
- Proposal: keep (fails safe; firing needs no overlay open).
- Answer:

**K3. Holding ↑/↓ on level.** Key repeat is off, so holding ↑ doesn't ramp the level.
- Proposal: keep off (no accidental ramps into the PA); Shift+↑/↓ steps 3 dB, `L` types a value.
- Answer:

**K4. Stimulus output default.** Outputs default to channel 1; changed via the palette.
- Proposal: remember the last outputs per device; first run defaults to output 1 and
  shows it in the top bar (as now).
- Answer:

**K5. Spectrum "bars" style for narrowband FFT.** A single-bin tone vanishes as a sub-pixel
bar, so narrowband spectra always draw as a thinned line; bars only for RTA bands.
- Proposal: keep.
- Answer:

**K6. Quitting with the daemon gone** can wait up to ~5 s on lease stop/release timeouts
(the daemon's own 1.5 s expiry is the real safety net).
- Proposal: cap the quit wait at 1 s.
- Answer:

**K7. Slots.** Ctrl+1…9 slots are UI-only and lost on restart until sessions exist (phase 5).
- Proposal: fine for now; slots become part of sessions in phase 5.
- Answer:

**K8. Mic name in SPL calibration.** The meter config has no mic name, so the UI matches a
calibration by device + channel only (decision 7a wants mic name too).
- Proposal: add a mic name per input in the session's input setup (phase 5) and match on it.
- Answer:

---

## Q1 — Delay target and acceptance (before phase 2)

**Design:** [q1-delay-finder.md](q1-delay-finder.md). Decisions D1–D4 accepted provisionally (sheet).

**Question.** What exactly does the finder report, and when do we trust it?

PLAN.md §5.2 sets the target as the first significant arrival (earliest candidate within a
threshold of the strongest) and the estimator as regularised H1 → IFFT on uniform bins. Open:

- Candidate threshold (−12 dB?) and whether it depends on band.
- Candidate detection: local maxima of |h| vs Hilbert envelope; minimum separation between
  candidates; how close arrivals that merge into one lobe are reported.
- Regularisation ε, and when to fall back to GCC-PHAT.
- Observation length per band (sub bands need longer windows), FFT length and padding.
- Confidence: peak-to-sidelobe ratio definition, band SNR estimate, excitation check —
  formulas and thresholds.
- Acceptance numbers (PLAN.md gives starting values): max error when accepted,
  wrong-arrival acceptance rate, refusal rate at a stated SNR, per scenario class.
- Scenario fixture set: synthetic (direct + louder delayed copy at varying separation,
  polarity, amplitude; close interfering arrivals; fractional delays; sub-only) and
  recorded (off-axis, near reflecting surface, different boxes/crossovers).
- How ambiguity is shown in the UI and CLI, and what tracking does with it.
- Minimum excited bandwidth per band before an estimate is allowed; stability of picks
  across excitation spectra (regularisation reshapes weakly excited bins).
- Periodic excitation: default tail allowance T and how a wrapped late reflection is detected.

**Must not:** score "agreement between two selectors" or "repeatability" as correctness.

---

## Q2 — Delivery freshness (before phase 3)

**Design:** [q2-q5-q6-protocol.md](q2-q5-q6-protocol.md) (proposed).

**Question.** What freshness can the data path actually guarantee, end to end?

- Daemon side: per-topic latest slot, PUB HWM value, send policy when HWM is hit.
- Client side: drain loop (read all available, keep newest per topic) before each render.
- Frame age: capture wall-clock time in the header; how clients compare it with their own
  clock (keepalive carries daemon wall time; client keeps an offset estimate).
- STALE deadline per kind (TF vs SPL vs levels), and what the UI shows.
- Behaviour of a stalled subscriber (paused process, slow WiFi) and how it recovers.
- Tests: induced stall, induced packet loss over TCP, bandwidth-limited link.

**Wording:** the guarantee is "bounded freshness with visible age", not "always latest".

---

## Q3 — Duplex timing (before phase 1)

**Design:** [q3-loopback-timing.md](q3-loopback-timing.md) (proposed). Spike input: [spike-audio-duplex.md](spike-audio-duplex.md).

**Question.** How is the generator→loopback offset monitored continuously (decided: 3a, 3b)?

- Correlation method and update rate while a stimulus plays; cost on the daemon.
- Jump detection threshold and what the UI shows (banner, event log).
- Behaviour across xrun, device reset, buffer-size change, stream restart.
- What backends report about latency (JACK port latency, CoreAudio, WASAPI) — used only
  as a plausibility check, never instead of the loopback.

---

## Q4 — Level normalisation (before phase 2)

**Design:** [q4-level-normalisation.md](q4-level-normalisation.md) (proposed).

**Question.** Exact formulas and dB references for every displayed level.

- Amplitude spectrum, PSD (dB re 1 FS²/Hz), band power: one-sided formulas, window
  normalisation, DC/Nyquist handling.
- Scalloping per supported window; whether a flat-top option is offered for tone reading.
- Fractional-octave band power from FFT bins vs IEC filterbank: when each is used, how
  partial bins at band edges are weighted.
- Generator level convention: dBFS RMS, 0 dBFS = full-scale sine; noise crest factor
  limits; behaviour when the requested RMS would clip.
- Test cases: bin-centred and off-bin tones, DC, Nyquist, white/pink noise integrated
  power, per-window coherent and energy gain.

---

## Q5 — Replay, epochs and frame identity (before phase 3)

**Design:** [q2-q5-q6-protocol.md](q2-q5-q6-protocol.md) (proposed).

**Question.** How does a client always know which state and which audio a frame belongs to?

- Replay buffer size (events and/or time), `resync_required` semantics.
- `daemon_incarnation` vs `session_epoch`: when each changes (daemon restart; session
  close/open, device change, sample-rate change).
- Config provenance: each frame carries the `config_rev` the DSP actually used and the
  sample index at which it took effect; how pending vs applied changes are shown.
- Grid lifetime and fetch semantics.
- Tests: missed final patch, expired replay, session reopen mid-subscription, daemon restart.

---

## Q6 — Stimulus lease protocol (before phase 3)

**Design:** [q2-q5-q6-protocol.md](q2-q5-q6-protocol.md) (proposed).

**Question.** Exact lease mechanics so that output is never orphaned or contested.

PLAN.md §6.5 fixes: token, refresh ≥ every 0.5 s, expiry 1.5 s, forced takeover stops and
disarms, stop is universal, CLI holds a lease only while a foreground command runs. Open:

- Token format and binding (per client identity under CURVE).
- Refresh carrier: dedicated message vs piggyback on `gen.set` (full-state drive message).
- Expiry enforcement inside the output path: atomics, fade-out ramp length, click-free stop.
- Behaviour across network hiccups shorter than expiry.
- Interaction with IR capture (ESS): lease held for the whole capture; abort rules.
- Audit trail: who armed, fired, stopped, took over — in the event log.

---

## Q7 — Calibration store (before phase 5)

**Question.** Calibration store details (decided: 7a/7b, 7c — device + channel + mic name,
cal age, no gain/phantom fields).

- Record fields: device uid, channel, mic name, sensitivity, calibrator level and
  frequency, timestamp, optional mic-curve file reference.
- Mismatch display wording and which readouts carry it (SPL, calibrated RTA).
- SPL mic-curve filter: design, length, latency, normalisation at the calibrator
  frequency, interaction with LCpeak and Impulse.
- Storage format, atomic writes, behaviour on unparseable files.

---

## Q8 — Phase comparison time reference (before phase 4)

**Question.** How do overlaid traces show true relative arrival, not just per-trace alignment?

- Shared reference mode: phase of each trace rebuilt from its stored delay against one
  chosen reference delay; how that reference is chosen and shown.
- Independent mode: per-trace alignment, clearly marked so a convincing overlay cannot
  hide relative delay.
- Defaults (shared within a session), behaviour for imported traces without delay metadata.
- Interaction with trace averaging's common delay reference and with A−B math.
- Test: two otherwise identical paths with different physical delays must show their
  relative phase in shared mode.
