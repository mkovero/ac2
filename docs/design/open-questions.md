# Open design questions

Each question must be answered by a design note in `docs/design/` before the phase that
implements it starts (PLAN.md §9.2). An answer states the decision, the reasoning, and the
tests that prove it. When a question is answered, link the note here and mark it closed.

Origin: two review rounds of PLAN.md (2026-10-01). PLAN.md already fixes the direction for
each; these are the details that are too fine for the plan but too risky to decide in code.

---

## Decision sheet

Fill in `Answer:` lines. Empty answer = proposed default is accepted. Free text welcome —
reasons help the design note. Full context for each Q is in the sections below the sheet.

### Product decisions (PLAN.md §12)

**A. Delay distance readout (ms → m/ft).** `ac` removed it deliberately (reason in `ac` README).
- Default: leave out of 1.0; revisit after re-reading the `ac` reason.
- Answer:

**B. UI chrome toolkit.**
- Default: egui on wgpu, confirmed by a one-week spike at phase 4 start (vs iced).
- Answer:

**C. Minimum headless hardware.**
- Default: Raspberry Pi 5 class (ARM64), 4 TF jobs @ 48 kHz.
- Answer:

**D. Raw capture format.**
- Default: f32 WAV/W64 + JSON sidecar (config timeline, discontinuities, algorithm version).
- Answer:

### Q1 — Delay finder (before phase 2)

**1a. Target.**
- Default: first arrival within −12 dB of the strongest; strongest shown too.
- Answer:

**1b. Threshold per band or global?**
- Default: one global threshold, operator-adjustable.
- Answer:

**1c. Ambiguous result.**
- Default: list ≤ 3 candidates, operator picks; tracking pauses.
- Answer:

**1d. Default search range.**
- Default: ±1 s.
- Answer:

**1e. Default sub band.**
- Default: 40–120 Hz; auto mode chooses from measured excitation.
- Answer:

**1f. Acceptance targets.**
- Default: error ≤ 1 sample full-range / ≤ 0.1 ms sub; wrong arrivals ≤ 1 %; refusals ≤ 10 % at ≥ 20 dB band SNR.
- Answer:

### Q2 — Delivery freshness (before phase 3)

**2a. STALE deadlines.**
- Default: TF 500 ms, SPL 300 ms, input meters 200 ms.
- Answer:

**2b. Publish rate.**
- Default: 30 fps per topic; client may request lower.
- Answer:

### Q3 — Duplex timing (before phase 1)

**3a. Loopback timing validation at session open?**
- Default: only when internal reference is used; short probe at −40 dBFS; operator confirms the first time.
- Answer:

**3b. No loopback cable available.**
- Default: internal reference refused; measured reference channel required.
- Answer:

### Q4 — Level normalisation (before phase 2)

**4a. Generator 0 dBFS.**
- Default: RMS of a full-scale sine.
- Answer:

**4b. Default spectrum unit.**
- Default: amplitude spectrum for narrowband view; band power for RTA.
- Answer:

**4c. Flat-top window option for tone reading?**
- Default: yes.
- Answer:

### Q5 — Replay & epochs (before phase 3)

**5a. Replay buffer size.**
- Default: 1024 events or 60 s, whichever is smaller.
- Answer:

**5b. Device or sample-rate change starts a new session epoch?**
- Default: yes.
- Answer:

### Q6 — Stimulus lease (before phase 3)

**6a. Refresh / expiry.**
- Default: refresh every 0.5 s, expiry 1.5 s.
- Answer:

**6b. Stop behaviour.**
- Default: 20 ms fade-out, never a hard cut.
- Answer:

**6c. Forced takeover.**
- Default: any authorized client may force; output stops and disarms first.
- Answer:

**6d. Network hiccup shorter than expiry.**
- Default: output continues; nothing special.
- Answer:

### Q7 — Calibration chain (before phase 5)

**7a. Fields that must match for VERIFIED.**
- Default: device, channel, mic id, preamp gain, phantom power state.
- Answer:

**7b. Re-confirming gain that cannot be read from hardware.**
- Default: at session open; one confirmation covers all inputs.
- Answer:

**7c. Mic curve filter.**
- Default: minimum-phase FIR for SPL/RTA; magnitude-only correction for TF.
- Answer:

### Q8 — Phase comparison time reference (before phase 4)

**8a. Default overlay mode.**
- Default: shared time reference within a session; imported traces marked independent.
- Answer:

**8b. Shared reference delay.**
- Default: delay of the first selected trace; pick key to change.
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
