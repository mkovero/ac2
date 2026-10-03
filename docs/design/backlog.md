# Backlog

Work found in use, not yet scheduled into a phase. Newest first. Move an item to "Done" with
the commit when it lands.

## Flaky tests (seen on CI, passed on rerun)

- Pink-noise level check on Linux and a spectrum-smoothing check on Windows failed once each
  during the sweep-distortion branch runs (2026-10-03), then passed. Find the timing/tolerance
  cause before they mask a real failure.

## Parked

- **Genelec 1083 on pupu: 2–6 kHz dip of −5…−9.5 dB** (docs/rigs/pupu.md). Mic at 102 cm
  and 94 cm (re-centred) gives the same dip within ±1.2 dB, and no distortion above 0.15 % goes
  with it, so mic geometry is ruled out. Left: rear-panel settings, near-field per driver,
  off-axis, 2 m; listen during a burst;
  `ac2 ir capture --ref 2 --mic 1 --out 1,2 --level -50dbfs --duration 6s --repeats 2`.

## From the first rig session on pupu (2026-10-03)

Open:
- **"No audio session" hint covers stored traces** in the transfer pane; it should yield (or move
  to the banner strip) when the pane has data to show.

## Done

Trace features (trace-mic-sweep-csv branch, protocol 7, session format 5):
- **Mic curve on stored traces**: `trace.mic_curve` / `ac2 trace mic <t> <mic|none>` / palette
  **Mic curve on the selected trace…** applies the store's curve for a mic to a captured trace
  as a display edit (columns as measured, points kept with the trace, exports name it); a
  capture that has the curve in its columns refuses a second (`q7-calibration.md` §9). Sweep
  traces no longer claim the input's curve in `mic.curve` (their analysis never applied it).
- **Trace smoothing from the CLI**: `ac2 trace smooth <t> 1/12 | none [--phase |
  --magnitude-only]`.
- **A sweep CSV re-import brings the sweep back**: the export (v2) carries `# sweep_info:` and
  the decimated IR as a second table; `trace import` restores a sweep trace the distortion pane
  draws. Imports keep the export's `delay_ms` (all kinds; it was 0.00 ms). A v1 sweep export
  imports as its transfer function with a note (`sweep_without_analysis`). Sessions keep the
  sweep in its one CSV (no `.sweep.json` sidecar).

- **Top bar: ARMED badge overlapped the session text** ("2ARMED…") at 1290 px. The bar is now
  fitted before it is drawn: lower-priority texts (key help, next-key hint, autosave, the
  device name) shorten or go first, the state badge and level always stay; `top_bar_never_overlaps`
  checks 640–1600 px.

From the first rig session on pupu (2026-10-03):
- **Finder said "AMBIGUOUS · merged arrivals" with a single candidate.** The ambiguity is real.
  A crossover inside the full band (LR4 sum = allpass) smears the one peak, so its centre is
  more than 1 sample after the onset. Two arrivals inside one pulse width do the same. The finder
  lists the peak and never invents a second arrival. The text now says "arrivals merged into one
  peak", explains the single row, offers only pick 1, and points to another band (q1 §8).
- **Clearer refusal messages.** An unpaired client says "not paired with <host>: … run `ac2
  auth pair <host> --server-key …`" (CLI and app). "Not responding" from a CURVE client adds
  "or this client is not authorized on it (this client's fingerprint: …)" and where to
  authorize it. ac2d logs each refused key's fingerprint, key and address, once per key and
  address per 10 s, with the count it suppressed.

Sweep findings from pupu (2026-10-03), fixed on the sweep-fixes branch:
- **Generator re-armed after a sweep** (every run). Not a restore of the pre-sweep state:
  `ir.capture` needs an armed generator, and once the recording was in the daemon only
  cleared `firing` ("generator Set by daemon"), leaving it armed with the sweep; the app
  followed with ARMED and "Enter sweeps again". The daemon now disarms (`Stop` by daemon,
  lease kept) as soon as the sweep has played, and on failure; the app shows STIM OFF, gives
  the lease back once the result (or failure) is in, and ends sweep mode.
- **Sweep dialog steppers** sorted (1 s, 3 s default, 6 s, 12 s) and every dialog stepper
  (choices, inputs, the session dialog's backend and device) stops at the ends.
- **Text fields select their text on focus** (and Ctrl+A), so typing replaces the default
  name or level.
- **Sweep reference** is the loopback input, else "choose the reference": the sweep does not
  arm until one is picked.
- **"output timing jump 0 → 96000 samples"**: not the output record. At 96 kHz the
  acquisition range is 0 … 96 000 samples; a slow sweep's first seconds are a near-tone, and
  GCC-PHAT on a rectangular capture window turned the window's end leakage into confident
  peaks exactly at the range ends (0 and 96 000, also ±64 while tracking). The capture window
  is now tapered: those windows are no measurement (Lost) instead of a false jump.
  Reproduced in `ac2-core` (`a_slow_sweep_start_never_reads_as_a_range_edge`).

Sweep / distortion pane (from the first sweep run on pupu):
- **Caption overlapped the axis title** in a small pane: the caption now shortens to fit
  (CLIPPED always kept), the legend wraps and the cursor readout starts below it; tested at
  300 … 1290 px.
- **Only the H2 floor was shaded**: each order within the noise is drawn dashed at its own
  floor in its colour, the shading is under the lowest order's floor (legend `< floor`,
  `noise`). Lone valid points are drawn across their cell, no longer as dots.
- **dB / % had no visible control**: a `dB | %` toggle in the pane's title (tooltip names U);
  percent is a log axis (0.001 … 100 %, decade labels).

Rig findings fixed on `fix/rig-findings`:
- **Remote generator played silence on JACK.** Arming reopened the stream to change the
  generator routing, which closed and re-created the JACK client: every connection to
  `ac2:out_N` (the recorder, the hand patch to the interface) was gone while the state said
  FIRING. Routing now changes inside the running stream. The lease gate also never mutes a
  source that has not played, and a mute on an expired deadline disarms with an `expiry`
  event.
- **JACK outputs never connected by the daemon.** The outputs chosen for the stimulus (and the
  loopback output) are connected to the playback ports of the same number, and again after a
  reopen; nothing else is connected or disconnected.
- **`ac2 spl watch` read the wrong level.** `--input` reused any running SPL meter with the same
  settings, typically one a killed watch had left behind, whose Leq integrated since long
  before the tone. The watch now always runs its own meter, cleans it up on SIGTERM/SIGHUP too,
  and prints the input and meter id; `--for` ends it after a set time.
- **Numeric measurement names** resolve: id first, then name; a value that is one measurement's
  id and another's name is refused naming both.
- **Daemon network ports vs host firewall.** `ac2d --listen` names its ports and warns when ufw
  or firewalld is active (one rule per port); "not responding" from a remote client hints at
  the firewall.
- **The daemon never answered mDNS.** `Handle::wait` dropped the advert before blocking, so the
  responder said goodbye and shut down right after "advertising". The advert now lives until the
  daemon stops; responder errors are logged.
- **`ac2 discover` asked on some interfaces only.** The browser now also asks on every interface
  address itself (0, 1, 3 s) and lists where it asked when nothing answered.
