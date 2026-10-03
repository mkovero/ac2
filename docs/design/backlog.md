# Backlog

Work found in use, not yet scheduled into a phase. Newest first. Move an item to "Done" with
the commit when it lands.

## Leq windows and limits (left after the first version, `docs/design/leq.md`)

- **History strip starts when the app connects**: the app draws what it received; a backfill
  from the daemon's log (`spl.log_get`, windows recomputed from the rows) would show the show
  so far after a reconnect.
- **Peak limits not judged**: DIN 15905-5 also limits LCpeak (135 dB), V-NISSG LAFmax
  (125 dB). The meter shows LCpeak / LAFmax; no limit, state or alarm on them yet.
- **No measuring-position correction**: a limit for the loudest audience position read from a
  FOH mic needs the difference added by hand; a per-meter offset (with its own "corrected"
  label) would do it.
- **No hysteresis on the alarms**: judged at 0.1 dB, a window hovering on its limit toggles
  over / recovered each time the rounded value crosses it.
- **The app has no acoustic calibration flow**: an electrical calibration (DMM in-line or an
  injected generator, `q7-calibration.md` §11) is in the app (**E** in the Calibrations
  view); a calibrator dialog (`cal.spl`) is still CLI only.

## Electrical calibration (left after the first version, `q7-calibration.md` §11)

- **Stored traces do not record the method**: `CalState::Calibrated` names the key and the
  sensitivity, not whether it was electrical or its uncertainty; trace captions say `cal`
  either way. Adding it is a session-format change.
- **Clipping refusal is not exercised end to end**: the fake rig's generator ceiling keeps
  the input below full scale; the clip tracking is covered only by reading.
- **No guided in-line tone**: the operator starts the 1 kHz tone (own source or the
  generator); the dialog could offer to arm a 1 kHz sine under the usual ceiling.
- **Autosave rewrites the whole session once a minute while a meter logs** (trace files
  included); appending to the log file would cut that to the new rows.

## UI focus (2026-10-03)

- **egui focus wanders to the sidebar's measurement chip** after arrow keys in the Leq dialog
  (seen while testing preset cycling): the chip gets egui's focus highlight. Check whether a
  dialog leaves focus on the side panel in the app too, and keep keyboard focus inside the open
  dialog.

## Windows (2026-10-03, operator's Windows 11 VM, release build of 6c122d5)

- MSI installed cleanly (unsigned: SmartScreen "More info → Run anyway"), Start-menu entry and
  PATH fine; the app ran the simulated rig. Real audio (WASAPI) on Windows is still untested.
- **Sluggish without a GPU** (VM, software adapter). When wgpu reports a CPU / software adapter
  (WARP, llvmpipe, lavapipe), lower the redraw rate and skip costly effects (MSAA, blur), and
  say so once ("software rendering: reduced frame rate"); measure frame time before and after.

## Flaky tests

- **ac2d `traces::capture_average_math_export_import`, unaligned capture at 301.6 Hz:
  −6.37 dB (want −6.02 ± 0.3)**, once, in a full `cargo test --workspace` at load ~40 after
  the capture-slot fix. Unloaded the error there is −0.002 ± 0.028 dB (300 runs, max 0.10);
  200 runs of the traces suite at load 40–130 did not repeat it. Not queue overflow (no
  capture-discontinuity warning in the output). Next: log `eff_avg` and the block count the
  low-frequency stage averaged when the band check fails.

## Parked

- **Genelec 1083 on pupu: 2–6 kHz dip of −5…−9.5 dB** (docs/rigs/pupu.md). Mic at 102 cm
  and 94 cm (re-centred) gives the same dip within ±1.2 dB, and no distortion above 0.15 % goes
  with it, so it is not a reflection at the mic, and an independent JACK + numpy capture confirms it in
  the direct sound (docs/rigs/pupu.md). Two-way box: most likely woofer–tweeter interference off
  the summing axis. Left: heights through the reference axis, 15–30° off axis, rear-panel
  settings, listening position; listen during a burst;
  `ac2 ir capture --ref 2 --mic 1 --out 1,2 --level -50dbfs --duration 6s --repeats 2`.

## Stored traces in the app (left after the Traces list)

- **No export of the selected trace from the app**: `ac2 trace export` does it by name; a
  palette entry (export to a typed path) would close it.
- **The level axis ranges are forgotten when the app quits**: kept per pane while it runs;
  `ui.toml` could remember them (a fit made for one show is rarely right for the next).
- **The spectrum pane has no legend**: only offset curves are named (a note in its corner);
  the colours of several live spectra and captures are told apart by the list alone.
- **A − B takes the two lowest shown slots**: an unslotted trace has to be moved to a slot
  first (**Move the selected trace to slot…**); "selected trace minus the next shown one"
  would not need it.
- **The transfer legend does not mark the selected trace**: the pane's title names it and the
  list highlights it; a thicker line or a marker in the legend would show it in the plot.

## Done

Comparing curves (field, 2026-10-03: "offset/change gain of the selected
trace/measurement/spectrum … spread traces a little … spectrum needs to focus on very low
signals"; "remove the selected trace by selecting it and pressing Delete"; "in show-one-pane
mode I would expect the panel to change if I click a different measurement"):
- **No quick offset** → **Alt+↑/↓** ±1 dB, **Alt+Shift+↑/↓** ±3 dB, **Alt+Home** 0 dB on the
  selected stored trace (any kind; `trace.update`, recorded with the trace) or the focused
  pane's live measurement (display only); **J** types it in the spectrum pane too. The plot
  names every offset curve (transfer legend tag, spectrum note and cursor).
- **Fixed level axes** → per pane: **Ctrl+I/O** zoom, **Ctrl+↑/↓** pan, **Shift+Home** fits
  the shown curves (low-percentile floor), **Ctrl+Home** resets; Ctrl+wheel / Shift+wheel
  with the mouse. `ac2_scene::view::level`.
- **No delete in the app** → **Delete** (palette: Delete selected trace…) asks, Delete or
  Enter deletes (`trace.delete`), the selection moves to the next shown trace; locked traces
  refuse; never a live measurement.
- **Maximised pane ignored the list** → picking a measurement brings up the pane that shows
  it (and focuses it in the split layout); a trace picked while maximised brings up its pane.
  No wire change.

Choosing between stored traces (field, 2026-10-03: "in transfer view where there are several
sweep traces, should I be able to choose between them?"):
- **Only slots 1–9 could be selected or hidden** → the sidebar's **Traces** list holds every
  stored trace (name, kind, slot, hidden, its curve's colour; a click selects, a click on
  the dot shows / hides). **V** / **Shift+V** step through every shown trace, **Alt+V** /
  **Alt+Shift+V** the hidden ones too, **A** shows / hides the selected one, **Move the
  selected trace to slot…** slots it. U, J, `,` `.`, E, K and the mic curve act on any
  selected trace on the transfer pane (they used to act on the live measurement only).
- **The sweep pane's choice was its own** → one selection: a sweep selected in the transfer
  pane is what the sweep pane shows, and N there selects. A finished sweep is selected.
- CLI: `ac2 trace display <t> on|off`, `ac2 trace slot <t> <1-9|none>` (`trace.update`; no
  wire change).

Leq run clock and a new log (operator: "should I see a timer somewhere which shows the whole
measurement time?"; `docs/design/leq.md` "Run clock and total", "A new log"; 774e690):
- **No timer for the whole measurement** → every Leq caption (columns, tiles, stage view) and
  `ac2 spl leq watch` (`run` in `--json`) show `running 2:14:05 since 19:02 · LAeq total
  97.8 · gaps 0:12`: the clock from the log's first kept second (it carries on across app
  and daemon restarts with the log), the energy average over the whole log's measured time,
  the unmeasured time; "last 48 h" once the log is at its retention. Shortened, then moved
  to a row of its own, at narrow widths. Protocol 10: `LeqMeta.run`.
- **No way to start the windows afresh** (show start after a loud soundcheck) → `spl.log_new`:
  the windows, their states, the alarms, the clock and the total start over, windows and
  limits stay; the ended log stays readable as the previous log (`spl.log_get` with `log:
  previous`, `ac2 spl leq export --previous`) until the next new log or a daemon restart.
  App: Shift+R in the SPL pane or "Start a new SPL log…", after a confirmation naming the
  run that ends; CLI: `ac2 spl leq new --yes [--export FILE]`.

Calibration visibility (field, 2026-10-03; `docs/design/q7-calibration.md` §10; de17a13):
- **No calibration view in the app** → the Calibrations view (palette **Calibrations…**,
  **Input setup…** on the selected measurement's input): what each input uses (mic, curve,
  sensitivity with calibrator and age), every mic with its curves, every sensitivity
  calibration; ←/→ choose an input's curve, N names the mic, I imports a curve, R renames,
  Delete (twice) deletes. The session dialog's mic rows say the same and ←/→ choose there too.
- **"mic curve: on" with no curve stored was misleading** → there is no on/off any more: an
  input chooses one of its mic's curves (or off), and says why none applies — `no curve
  stored for MM1 34804`, `choose: 0°, 90°`, `90° — not stored for MM1 34804` — in the input
  setup, `ac2 cal list` / `ac2 status`, the sidebar label and the pane captions.
- Several curves per mic (MM1 0° / 90°), labels from the files, the curve in use recorded
  with captured traces (label + hash). Protocol 8, calibration store version 2 (version 1 is
  set aside), session format 6.
- Rig note kept: the MM1 90° curve was only imported on pupu on 2026-10-03 (after all of the
  day's sweeps); earlier ac2 traces are uncorrected (`ac2 trace mic` fixes them). After the
  update the store is set aside: re-import both MM1 files and calibrate again.

Flaky tests (seen on CI and under load, 2026-10-03), by cause (db95852, d8fc428, 927c69c):
- **A capture could return the result before the one a client was shown** (ac2d
  `traces::spectrum_smoothing_live_and_captured` "bin 1: live … vs ac2-core …",
  `live_smoothing_and_resmoothed_captures` "301 Hz: … vs live …",
  `capture_average_math_export_import` NaN average, `session_save_load_round_trip` "no result
  to capture yet"; Windows CI). A job sent its frame to the I/O thread before storing it as the
  capture slot, so a client that had already received the frame could capture the previous
  result, or none. The slot is now filled first; a 20 ms pause in the old order reproduced
  the CI messages exactly.
- **Full-band pink over a 200–300 ms meter span** (ac2d `stimulus::lease_acquire_force_expiry_
  and_universal_stop` "pink at -20 dBFS RMS on the loopback: -21.5"). Content down to 5 Hz
  gives one or two cycles per span: +2.7/−1.6 dB from seed to seed, ~1 % beyond ±1.5 dB, and
  the daemon seeds each generator afresh. The test plays pink high-passed at 50 Hz (±0.75 dB
  over 3000 seeds).
- **UI tests under load** (load 25–50 on 12 cores, lavapipe): `startup_first_frame` timed
  shader compilation on a software rasteriser (3–4 s loaded); it now bounds the app's own
  startup (App::new through its first UI pass, < 1 s; 1–65 ms measured loaded). Snapshots
  caught wall-clock states ("not responding", STALE, a countdown second, a measurement created
  but not yet started); they are taken from a pass laid out while the link was healthy, the
  progress picture pins its clock, the transfer dialog waits for "running". A test process
  starved past the fake daemon's 1.5 s lost its stimulus lease (fake `expiries` 1, 0–10
  refreshes) and the flow stalled: the UI tests' fake grants 60 s. A sweep submitted while the
  previous stop was in flight was dropped by the app (real bug, fixed: it arms after the
  stop). Waits are condition-based with a 60 s ceiling and report state on timeout; the meters
  round test freezes the reducer's clock instead of a 700 ms bound.
- Evidence: ui + embedded suites 15× under 36 CPU burners (load 45–48): 0 failures (before:
  5/12 and 3/12); ui, embedded, connect, conn and ac2d traces + stimulus 8× beside full
  `cargo test --workspace` runs (load 16–62): 0 failures; full workspace 3×: all pass; ac2d traces + stimulus 20× under
  36 burners: 0 failures.

Autosave (561d944):
- **Sweep results lost on a daemon restart** (a `--max-level` change needs one): a stand-alone
  daemon autosaves measurements and traces, sweep distortion and impulse responses included,
  and restores them disarmed on start; the top bar shows the autosave state.

Trace features (protocol 7, session format 5; f3ea323):
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

- **Top bar: ARMED badge overlapped the session text** (076320e) ("2ARMED…") at 1290 px. The bar is now
  fitted before it is drawn: lower-priority texts (key help, next-key hint, autosave, the
  device name) shorten or go first, the state badge and level always stay; `top_bar_never_overlaps`
  checks 640–1600 px.

From the first rig session on pupu (2026-10-03):
- **"No audio session" hint covers stored traces** (927c69c): while the transfer pane draws a
  stored curve the hint is one line in the pane's title strip; over an empty plot it stays
  centred.
- **Finder said "AMBIGUOUS · merged arrivals" with a single candidate** (b01bd87). The ambiguity is real.
  A crossover inside the full band (LR4 sum = allpass) smears the one peak, so its centre is
  more than 1 sample after the onset. Two arrivals inside one pulse width do the same. The finder
  lists the peak and never invents a second arrival. The text now says "arrivals merged into one
  peak", explains the single row, offers only pick 1, and points to another band (q1 §8).
- **Clearer refusal messages** (2565cfd, edf8492). An unpaired client says "not paired with <host>: … run `ac2
  auth pair <host> --server-key …`" (CLI and app). "Not responding" from a CURVE client adds
  "or this client is not authorized on it (this client's fingerprint: …)" and where to
  authorize it. ac2d logs each refused key's fingerprint, key and address, once per key and
  address per 10 s, with the count it suppressed.

Sweep findings from pupu (2026-10-03; 2aa6867, 61e7ea5):
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

Sweep / distortion pane (from the first sweep run on pupu; a9ad7cb):
- **Caption overlapped the axis title** in a small pane: the caption now shortens to fit
  (CLIPPED always kept), the legend wraps and the cursor readout starts below it; tested at
  300 … 1290 px.
- **Only the H2 floor was shaded**: each order within the noise is drawn dashed at its own
  floor in its colour, the shading is under the lowest order's floor (legend `< floor`,
  `noise`). Lone valid points are drawn across their cell, no longer as dots.
- **dB / % had no visible control**: a `dB | %` toggle in the pane's title (tooltip names U);
  percent is a log axis (0.001 … 100 %, decade labels).

Rig findings (19ce861, 27bef5b, cd7d8af):
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
