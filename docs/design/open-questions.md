# Open design questions

Each question must be answered by a design note in `docs/design/` before the phase that
implements it starts (PLAN.md §9.2). An answer states the decision, the reasoning, and the
tests that prove it. When a question is answered, link the note here and mark it closed.

Origin: two review rounds of PLAN.md (2026-10-01). PLAN.md already fixes the direction for
each; these are the details that are too fine for the plan but too risky to decide in code.

---

## Decision sheet

Round 1 answered 2026-10-01. **Decided** items are settled. Items marked **→ round 2**
were unclear; each now has a plain-language explanation and a proposal. Fill `Answer:`
(empty = proposal accepted).

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

### → round 2

**1c. What happens when the finder sees several similar peaks?**
Example: direct sound at 10.0 ms and a floor bounce at 11.2 ms only 2 dB weaker. The
finder cannot know which one you want to align to.
- Proposal: show both on the IR panel and as a short list (`1: 10.0 ms  2: 11.2 ms`);
  the first-arrival rule pre-selects 10.0 ms, one key accepts it, another picks the other.
  Delay tracking pauses until the ambiguity clears.
- Answer:

**2a. "STALE" — what it means (not about system latency).**
This is not about how much delay your PA chain has — large console/network/processor
delays are normal and are exactly what the reference loopback cancels (see 3a). STALE
only means *the screen stopped receiving fresh data*: WiFi to a remote rig dropped,
daemon hung, audio device vanished. Without it, a frozen trace looks like a perfectly
stable measurement.
- Proposal: if no new frame for a measurement arrives for ~1 s, its trace dims and shows
  STALE with the age; it never decides anything about the measured delay.
- Answer:

**2b. Publish rate.** You said: enough that smoothness is never the bottleneck.
- Proposal: daemon publishes up to 60 fps per measurement locally (frames are small);
  remote clients default to 30 and can raise it; UI interpolates to display refresh.
- Answer:

**3a. Continuous probe/monitor of interface timing — yes, mostly for free.**
With the reference wired as in `ac` (stimulus and reference out through the same
converter, reference looped back into an input), every latency inside the interface,
console, network and processors is common to both legs and cancels. So the loopback
itself *is* the continuous monitor: while any signal plays, the daemon continuously
correlates generator output against the loopback input and watches that offset. If it
jumps (buffer change, device reset, clock slip) the daemon flags it immediately. No
separate probe signal is needed while a stimulus runs; when nothing plays there is
nothing to measure and the last value is shown with its age.
- Proposal: continuous monitoring as above; no start-up probe.
- Answer:

**4b. Which unit the spectrum view shows by default.**
Two honest ways to read an FFT: *tone level* (a −20 dBFS sine reads −20 dBFS; noise
reads lower the finer the FFT) or *band power* (noise reads the same regardless of FFT
size; what an RTA shows). Mixing them up is the classic "why does my pink noise read
15 dB low" confusion.
- Proposal: narrowband spectrum = tone level; RTA = band power; the unit is printed on
  the axis. No setting needed unless you want one.
- Answer:

**4c. Flat-top window.**
A standard FFT window under-reads a pure tone by up to ~1.4 dB when the tone falls
between bins. A flat-top window reads tones exactly, at the cost of blurrier frequency.
Only useful for reading tone levels (e.g. checking a 1 kHz line-up tone).
- Proposal: available as a window option in the spectrum view, not the default.
- Answer:

**6a. Stimulus dead-man timer.**
Safety for remote use: the client that started the noise must tell the daemon "I'm still
here" twice a second. If that stops for 1.5 s (laptop lid closed, WiFi died, app
crashed), the daemon fades the noise out on its own instead of leaving pink noise
running through the PA with nobody in control. `ac` has the same mechanism.
- Proposal: keep as is (heartbeat 0.5 s, timeout 1.5 s), timeout configurable.
- Answer:

**7a/7b. Calibration validity — simplified per your answer.**
Agreed: ac2 can't know the preamp gain, phantom state, or even whether the gain knob is
settable. So it won't pretend to.
- Proposal: a calibration is tied to *device + input channel + mic name* (mic name typed
  once at cal time). If any of those change, the SPL readout says "cal from other mic /
  input". Otherwise it shows the cal's age (e.g. "cal 3 h ago"). No gain or phantom fields,
  no confirmation prompts. Recalibrate when you touch the gain — that's on the operator.
- Answer:

**7c. How the mic correction file is applied (sorry — jargon).**
Measurement mics come with a correction file (e.g. "+1.5 dB at 15 kHz"). Question was
only *how* ac2 applies it internally.
- Proposal: for transfer functions and spectra, subtract the file's dB values from the
  displayed magnitude (simple, what everyone does). For SPL meters, apply it as a filter
  before weighting so dB(A)/dB(C) include it. Phase is never touched. Nothing to choose
  in the UI except on/off per input.
- Answer:

**8b. Shared delay reference for comparing traces — your point taken.**
You said the delay should be continuously measured against the reference loopback and
used in situ. Agreed: every live measurement's delay is measured against the reference
(tracking optional), and every captured trace stores the delay it had. "Shared time
reference" then just means: when overlaying mains and sub traces, phase is drawn relative
to *one* chosen delay, so a 3 ms arrival difference between them stays visible instead
of being aligned away.
- Proposal: the reference delay for an overlay is the selected trace's measured delay
  (pick key to change); every other trace is drawn relative to it; per-trace nudge on top.
- Answer:

---

## Q1 — Delay target and acceptance (before phase 2)

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

**Question.** When can the generator output stand in for a measured reference?

- How each backend exposes input/output latency (JACK port latency ranges, CoreAudio
  device + stream latency, WASAPI) and how far each can be trusted.
- Validation at session open: loopback measurement of the output→input offset; tolerance.
- Re-validation triggers: xrun, device reset, buffer-size change, stream restart.
- Where the reference is tapped: after routing and level, before the DAC.
- What is refused, and what the operator sees, when timing is not validated.

---

## Q4 — Level normalisation (before phase 2)

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

## Q7 — Calibration chain identity (before phase 5)

**Question.** What must match for a calibration to count as verified?

- Chain record fields: device uid, channel, mic id, preamp gain (read or confirmed),
  phantom power state, calibrator id and frequency, timestamp.
- Which backends can read preamp gain (vendor-specific; likely few). Confirmation UX
  when they cannot: at cal time, and again at session open.
- UNVERIFIED display: which readouts carry it (SPL, calibrated RTA, absolute levels).
- Mic curve: FIR design (minimum- vs linear-phase), length, latency, normalisation at the
  calibrator frequency, interaction with LCpeak and Impulse weighting.
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
