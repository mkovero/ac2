# Open design questions

Each question must be answered by a design note in `docs/design/` before the phase that
implements it starts (PLAN.md §9.2). An answer states the decision, the reasoning, and the
tests that prove it. When a question is answered, link the note here and mark it closed.

Origin: two review rounds of PLAN.md (2026-10-01). PLAN.md already fixes the direction for
each; these are the details that are too fine for the plan but too risky to decide in code.

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
