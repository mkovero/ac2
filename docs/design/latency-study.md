# Latency study: from the sample to the pixel

Status: plan, not run (phase 1: code read, 2026-11-10). Operator request: profile the delay from
a sound to the pixel that shows it, which many stages now lie between. The complaint: lag and
jitter between what is heard and the 3D spectrograph (perhaps the 2D spectrum too) on
ketunkolo (app, RTX 2070, 60 Hz) fed by pupu (daemon, FF400 over JACK at 96 kHz / 256 frames,
wired LAN, RTT under 1 ms), viewing *Spectrum 2* (multi-resolution, 1/12 octave).

Builds on `timing-study.md`: its tools (`ac2 selftest stamps`, `selftest net`) and its phase A/C
results (stamp residual σ ≈ 15 µs; one-way network p99.9 ≤ 1.3 ms, one 250 ms stall an hour
from pupu's tg3 PTP driver) are reused here, not run again.

## 0. What "the sample" means

- **t_ear**: the instant the operator hears the event. It differs from the mic's instant by the
  path difference (≈ 3 ms/m), so it is outside ac2's control and the study's scope.
- **t_s**: the instant sound sample *s* reaches the mic (the ADC input). This is where the study starts.
- A spectrum frame is stamped with `audio_sample` = the **last** sample of the newest band
  frame in it (`crates/ac2d/src/jobs/spectrum.rs:331`, `samples_since_newest` at :489–500).
  Its other bands are older by their `SpecBand.age` (already on the wire, meta of each frame,
  `spectrum.rs:365`).
- What the eye takes as "the event shows" is near the **window centre**: a Hann window weights
  the newest samples near zero, so an onset gets its full level when it reaches the centre,
  `N_b/2` before the window end. The study reports both: *end-to-photon* (system latency, the
  thing we can fix) and *onset-to-visible* (adds the physics of the window).

Multi-res ladder at 96 kHz (`crates/ac2-core/src/multires.rs:112`, hop from
`crates/ac2-proto/src/model/defaults.rs:86`; table in `multires-spectrum.md`):

| band | N | window | N/2 | hop | serves |
|---|---|---|---|---|---|
| 0 | 65 536 | 683 ms | **341 ms** | 85 ms | < 203 Hz alone, to 406 Hz blended |
| 1 | 32 768 | 341 ms | 171 ms | 43 ms | 203–812 Hz |
| 2 | 16 384 | 171 ms | 85 ms | 21 ms | 406 Hz–1.6 kHz |
| 3 | 8 192 | 85 ms | 43 ms | 21 ms | 0.8–3.25 kHz |
| 4 | 4 096 | 43 ms | 21 ms | 21 ms | 1.6–6.5 kHz |
| 5 | 2 048 | 21 ms | 11 ms | 11 ms | > 3.25 kHz |

Onset-to-visible from the DSP alone is then about `N_b/2 + U(0, hop_b)`: **≈ 340–430 ms for a
kick drum or bass line**, ≈ 11–22 ms for hi-hats. No pipeline fix changes this. Averaging, if
it is on for Spectrum 2, adds its time constant on top.

## 1. Stage map

Clocks: **RAW** = CLOCK_MONOTONIC_RAW (jackd's stamp clock on both hosts, phase A), **MONO** =
CLOCK_MONOTONIC (`std::time::Instant`), **WALL** = CLOCK_REALTIME (chrony-disciplined from aika;
`capture_wall_ns`, `Arrival.wall_ns`). Numbers are first-principles expectations for pupu →
ketunkolo; *U(a,b)* is a wait spread evenly between a and b.

| # | Stage | Where | Clock there | Expected | Jitter source |
|---|---|---|---|---|---|
| S1 | ADC + driver + JACK capture period | FF400, `crates/ac2-audio/src/jack_host.rs:792` (stamp = cycle start − capture latency max) | RAW (`BlockStamp.capture_ns`) | 3–6 ms (AD ≈ tens of samples + 2.67 ms period; FireWire may add a period) | FireWire 125 µs iso cycles; stamp residual 17 µs σ (phase A) |
| S2 | Capture ring → fan-out hand-off | `crates/ac2d/src/fanout.rs:551` (sleeps to `next`), period `handoff_period` :259 = min(1/60, 1/60) = **16.7 ms** | MONO; block `wall_ns` = WALL at pop (:176) | U(0, 16.7) for the block holding the frame end | hand-off beats against hop and block period |
| S3 | DSP: band frame formed | `Spectrum::push` `spectrum.rs:477`, result stamped back from the pop instant :496 | — | FFT of up to 6 bands, < 2 ms (measure) | CPU load, 65 k FFT every 85 ms |
| S4 | Job emit cadence | `crates/ac2d/src/jobs/mod.rs:541` (`run`, Cadence 60/s, quarter-period lead), `Pace` :174 | MONO | ≈ 0 (a new result goes on the hand-off that formed it); up to one period when two results land within one | Cadence vs hand-off |
| S5 | Encode + per-peer send | `Emitter::send` `jobs/mod.rs:132` → `crates/ac2d/src/io.rs` (per-topic `min_gap` = period/2 = 8.3 ms, :166) → `peers.rs:214` (credit, rotation) | MONO | < 1 ms; credit waits only with a slow reader | credit stall when the client drains late |
| S6 | Network | CURVE ROUTER/DEALER | — | 0.2–0.5 ms (phase C p50/p99) | rare 250 ms stalls (tg3) |
| S7 | Client drain | `crates/ac2-ui/src/conn/mod.rs:831` (`data_changed`) → `PollClock::arrived` `conn/pacing.rs:174`: drains at `last + period/2`, period = `DISPLAY_PERIOD` **33 ms** (`conn/mod.rs:415`) | MONO + WALL (`Arrival`, `crates/ac2-client/src/data.rs:111`) | U(0, 16.5) | beats the daemon's 60/s against the link's own clock |
| S8 | UI wake + pacer | `link_wake` `crates/ac2-ui/src/app.rs:944` → `Pacer::request` `crates/ac2-ui/src/pace.rs` (frame starts ≥ 16.667 ms apart, `gpu.rs:47`) | MONO | U(0, 16.7) | third free-running 60 Hz clock |
| S9 | Reducer + glide | `ui()` `crates/ac2-ui/src/app/frame.rs:24` → `pump_link` `app.rs:785` → `keep_spec_glide` `state/glide.rs:73`, `keep_row_glide` `state/row_glide.rs:59`; `Pacing::glide_s` `crates/ac2-scene/src/curve_ease.rs:59` | MONO, frame WALL | glide D = clamp(interval + (max lag over 32 frames − lag), **30 ms**, 200 ms); drawn value reaches a frame at the glide's end | the 32-frame max moves with S2/S7/S8 |
| S10 | Scene build + GPU prepare | `scene_for` `crates/ac2-ui/src/view/panes.rs:88` (cache), `PlotCallback::prepare` `crates/ac2-ui/src/plot.rs:66`, skyline/heatmap uploads in `ac2-plot` | MONO | 1–5 ms (3D more; measure) | cache misses, 3D uploads |
| S11 | Submit → present | eframe 0.36 / wgpu 30, `PRESENT_MODE = AutoNoVsync` `crates/ac2-ui/src/gpu.rs:65`, `desired_maximum_frame_latency = 2` :82 | none exposed | Immediate (NVIDIA Vulkan on X11 offers it): ≈ 0 queue, tears; Mailbox/compositor: +≤1 frame; FIFO (GLES, Pi): +1–2 frames | present mode as resolved; compositor |
| S12 | Scanout + panel | display | — | U(0,16.7) by screen row (pane centre ≈ 8 ms) + panel 2–10 ms (monitor) or **30–100 ms (TV without game mode)** | 60.000 Hz pacer vs the real refresh: a tear line or a doubled frame crawling at the beat period |

Sum, end-of-window to photon, glide off, Immediate, monitor: ≈ 4 + 8 + 1 + 0 + 0.5 + 0.3 + 8 +
8 + 0 + 3 + 0 + 8 + 5 ≈ **54 ms mean**, ≈ 95 ms worst. S2, S7 and S8 are three independent
uniform waits of ≈ 16.7 ms: together σ ≈ 8.3 ms and a 0–50 ms spread, i.e. ±½ to 1½ display
frames of jitter from scheduling alone. Glide adds roughly one more interval (≥ 30 ms) to the value the eye reads.
For the 3D/flat spectrograph, rows are placed by `capture_wall_ns` into **30 slots a second**
(`crates/ac2-scene/src/spectrograph.rs:44`, slot = wall / slot_ns): at 60 frames a second each
frame advances the history 0, 1 or occasionally 2 slots depending on where its stamp falls,
and the row glide is paced by those advances.

`capture_wall_ns` itself (`fanout.rs:160` pop, `stamp_arrivals` :746) is the WALL time the
fan-out *popped* the newest block, back-dated by audio after it. It is therefore late against
t_s by S1 plus up to one block (≤ 2.7 ms), and it follows chrony slews. That is good enough for
pacing, but the backend's RAW stamp (`Block.host_ns`, already line-fitted in
`crates/ac2d/src/capture_clock.rs`) is the better clock for this study.

## 2. Instrumentation plan

Principle: **no wire change.** Every frame already carries the join key (`daemon_incarnation`,
topic, `seq`) and `audio_sample` / `capture_wall_ns`. Each side logs its own stamps locally
and the logs are joined offline. No `PROTO_VERSION` bump is needed for the study. A fix that
later wants a send stamp on the wire (e.g. for an in-app latency readout) would bump it, which
is allowed pre-1.0.

**Switch:** `ac2d --latency-log <dir>` and `ac2-ui --latency-log <dir>` (typed flags; off by
default). Off costs one `Option` check per site. On: each site writes a fixed-size record
into a preallocated SPSC ring; a logger thread writes CSV. No stamp in the audio callback:
the callback already stamps blocks (S1). Every daemon site below runs on the fan-out, job or
I/O thread.

Clock rule per host: intervals on MONO; the cross-host join on WALL. RAW stamps are taken onto
MONO by the bracketed pairs the capture clock already reads (`StampClock::pair`,
`crates/ac2-audio/src/stamp.rs:28`). Each log starts with a header of
(MONO, WALL, RAW) triples, and the logger adds one every 10 s.

Daemon (pupu):

| id | Site | Per | Fields |
|---|---|---|---|
| D0 | fan-out after a batch, `fanout.rs` loop (~:620) | hand-off | newest `end_sample`, its `host_ns` (RAW, block start) + frames, pop MONO, pop WALL, batch length |
| D1 | `Spectrum::push` when a band frames (`spectrum.rs:496`), and RTA/TF alike | result | generation, result end sample, formed MONO |
| D2 | `Emitter::send` (`jobs/mod.rs:132`) | frame | topic, seq, audio_sample, capture_wall_ns, max band age, send MONO, channel-full flag |
| D3 | peer send (`peers.rs` serve) | frame × peer | peer, topic, seq, key/delta, bytes, slot-put MONO, sent MONO, waited-for-credit flag, encode µs |

D0 gives the sample→RAW→MONO line, so the instant **t_s of any sample index** is known on
pupu to the stamp residual (tens of µs, far under what matters here). D0−D1−D2−D3 separate
S2–S5.

App (ketunkolo):

| id | Site | Fields |
|---|---|---|
| A0 | client drain (exists: `ClientConfig::record_arrivals`, `crates/ac2-client/src/client.rs:77`; used by `ac2 selftest frames`) | topic, seq, arrival MONO + WALL, bytes |
| A1 | `DataSnapshot` handed to the UI (`drained`) | snapshot id, MONO, seqs per topic |
| A2 | `ui()` pass start/end (`frame.rs:26`) | pass, start MONO, why woken (data / animation / input), end MONO |
| A3 | per visible spectrum/spectrograph pane in that pass | pane, view kind, to_seq, glide k, **effective capture time** (below), row lag (slots), newest slot, scene cache hit, build µs, drawn level at 3 probe columns (63 Hz, 1 kHz, 8 kHz) |
| A4 | GPU done: `queue.on_submitted_work_done` registered in `PlotCallback::prepare` | pass, MONO when it fires (resolution = how often wgpu polls; verify) |
| A5 | vblank | see below |

*Effective capture time* is what the drawn curve stands for: a glide from levels standing for
time t_from to a frame captured at t_to, k of the way, stands for t_from + k(t_to − t_from).
Kept as one f64 beside `SpecGlide.from` (and the row position for `RowGlide`), it turns glide
lag into a number per pass: **photon − effective capture time** is the end-to-photon latency as the
eye gets it.

Present timing. wgpu 30's public API reports nothing after `present()`: no `present_id` /
`present_wait`, no `VK_GOOGLE_display_timing`. egui-wgpu owns the surface, so reaching the
`VkSwapchainKHR` through `as_hal` is impractical; to be confirmed at implementation. In order of
preference:
1. a side thread logging vblank instants from DRM (`drmWaitVBlank` on the nvidia-drm node; needs
   `nvidia-drm.modeset=1`; verify on ketunkolo) or the compositor's frame callbacks if the app
   runs under Wayland/XWayland;
2. the refresh rate read exactly (`xrandr --verbose` / modeline) and the vblank phase taken once
   from a short FIFO run, where `present()` returns on vblank;
3. the photon check (§3c) as the ground truth for the remaining constant.

Photon estimate per pass: Immediate gives the first scan of the pane's centre row after A4. FIFO
or a compositor gives the vblank after A4 plus the queue, plus the row's scan time. The panel
constant is unknown without §3c.

CSV: `daemon/frames.csv` (D2 ⨝ D3 rows), `daemon/handoffs.csv` (D0), `daemon/results.csv` (D1);
`ui/arrivals.csv` (A0), `ui/passes.csv` (A2+A3+A4, one row per pane per pass), `ui/vblank.csv`.
The analysis lives in Python under `tools/experiments/latency/` (like `timing/`), run with `-I`. It
joins on (incarnation, topic, seq), prints per-stage p50/p95/p99/max in ms, and plots latency over
time (beats show as sawtooths). Logs from the rigs stay private.

Cross-host offset (pupu WALL − ketunkolo WALL): needed only to join D2/D3 to A0 (S6). Both
hosts are chrony-disciplined from aika over software PTP. Phase B of the timing study measured
the composite at tens to hundreds of µs, which is 1–2 orders under the ms resolution this study needs. Bound it per
run three ways: `chronyc tracking` on both hosts (logged by the run script); the app's own
keepalive estimator (`ClockEstimator`, `crates/ac2-client/src/mirror.rs:57`, a lower bound biased
by the least one-way delay); and, when it matters, `ac2 selftest net` in both directions for
5 min, giving offset = (d_ab − d_ba)/2 and the error ≤ RTT/2 from the minima. Ratio: pupu runs
+5.6 ppm, i.e. 0.34 ms over a 1 h run against WALL. Intervals within a host use MONO and need no correction.

### 2a. App side as built (`ac2-ui --latency-log FILE`)

One CSV file (not the per-site files above), `#` comment lines first: the log's start on the
wall clock, the adapter, and the present mode, requested (`AutoNoVsync`) and resolved by
wgpu's rule against what the surface offers (read on Vulkan through a second, at once dropped
surface on the window; GL is FIFO only; other backends say `unknown`). Then one row per pane
drawn per pass (A2+A3+A4), or one row with an empty `pane` for a pass that drew none. Off,
each site is one `Option` check; on, rows are fixed-size records in preallocated buffers,
handed about once a second to a writer thread that formats them. A pass's rows are completed
by the next pass, which knows the probe stamps of the one before.

`*_ns` columns are MONO ns since the log started; `*_wall_ns` are ns since the Unix epoch,
this host's wall clock mapped from MONO by a pair read at each pass start, except
`capture_wall_ns` (the daemon's).

| column | what |
|---|---|
| `pass`, `pass_start_ns`, `ui_end_ns` | A2: the `ui()` pass, its start and end |
| `frame_cpu_ms` | eframe's CPU time for that frame (`IntegrationInfo::cpu_usage`: build, tessellate, paint, without the surface waits); empty where eframe gives none |
| `panes_drawn` | every view drawn that pass, `+`-joined (`spectrum`, `spectrograph`, `spectrum_3d`, `transfer`, `spl`, …) |
| `pane`, `view`, `scene`, `build_ms`, `scene_ns` | A3: the pane, what it drew, `built` or `cached` (the cache holds while the picture is the newest frames'), the time taken, when the scene was ready |
| `topic`, `incarnation`, `seq`, `audio_sample`, `capture_wall_ns` | the newest data frame on a spectrum pane (empty on other panes): the join key and the wire stamps |
| `read_ns`, `read_wall_ns` | A0: when the link's drain read it (`TopicFrame::received`) |
| `snapshot_ns`, `apply_ns` | A1: when the link's snapshot carrying it was drained, and when the reducer applied that snapshot (empty for a frame applied before the log started) |
| `prepare_ns`, `submit_ns`, `submit_wall_ns` | A4 as far as the app can see: a probe paint callback on the topmost layer stamps egui-wgpu's prepare (before the surface acquire) and its paint (after the acquire, the frame's last; `queue.submit` and `present` follow with no app code between). Present timing itself is not observable: wgpu 30 reports nothing after `present()`, and eframe owns the surface. A FIFO wait happens in the acquire, between the two stamps; under Immediate `present` returns at once. A5 (vblank) and the GPU's completion are not logged |
| `glide_k` | how far the 2D curve's ease into this frame has got, 0–1 (1: ended; `SpecGlide`, slow curves only); empty with no ease and for spectrographs |
| `drawn_capture_wall_ns`, `glide_lag_ms` | the *effective capture time* the drawn curve or rows stand for, and how far it trails `capture_wall_ns`. 2D: `from + k (to − from)` (`SpecGlide::from_capture_ns`); spectrographs: how far the scroll's time axis stands behind the newest frame's audio, 0 once at or past it |
| `row_lag_slots`, `newest_slot`, `slot_advance` | H5: the spectrograph rows' lag behind their places in slots (minus the scroll's lead: negative while the axis runs past the newest slot), the slot the newest row is in and how many slots it moved since the pane was last drawn |

`tools/latency/summarize.py FILE` (stdlib, run with `-I`) prints p50/p95/max of
capture→read, read→apply, apply→build, build→present (to `submit_ns`), the glide lag,
capture→present and capture→present as seen (plus the glide lag), the pass interval and
CPU, build times per view, and the 3D slot-advance histogram.

## 3. Measurement protocol

No run emits sound on real hardware. Agents never pass `--emit`. Synthetic signals enter as
a recording (replay backend) or through a JACK dummy server.

**L0, DSP alone (agent, offline, minutes).** Feed `MultiResAnalyzer` a burst file (below) in
a test-like harness. Per band, time from the onset sample to the first frame within 3 dB of
the settled level. This is onset-to-visible without any system, and it checks §0's table.

**L1, local synthetic (agent; ketunkolo or a dev host, daemon and app on one host, one clock).**
- Signal: 96 kHz f32 WAV, 10 min. Every 1.37 s (incommensurate with the 16.7 ms hand-off,
  the hops and the 33 ms slots, so phases spread uniformly) a 40 ms Hann-gated burst rotating 63 Hz,
  1 kHz, 8 kHz and a single-sample click. The onset sample indices are written beside it.
- Run 1, replay: `ac2d` with the recording opened as a capture-only device (the replay
  backend at realtime speed, opened through the control request as the app does). It pushes each block at its start rather than its end
  (`crates/ac2-audio/src/replay.rs:403–420`), one block ahead of a real device. Correct this by
  frames/fs.
- Run 2, JACK dummy: `jackd -d dummy -r 96000 -p 256`, the daemon on JACK, the WAV played by
  a JACK file player into its input. This exercises the real backend and stamps. Ground truth
  comes from a raw capture taken alongside (`ac2-traces` raw capture: f32 + sample index):
  find each onset offline, then map its sample to MONO through D0.
- App: the normal build with `--latency-log`, on the same display. Each configuration runs
  3 min (≈ 130 events).

**L2, real rig, passive (agent-run with the operator's OK; the operator plays music as usual).**
Deploy the instrumented daemon to pupu (the flag is off by default, so a normal deploy) and run
it with `--latency-log` for 30 min. The app on ketunkolo runs with `--latency-log`, Spectrum 2
on screen in each view for 5 min. Without events, report per-stage distributions and
end-to-photon (via the effective capture time) per pass. Also log `chronyc tracking` on both
hosts and `selftest net` 5 min before and after. Repeat 5 min with the app on pupu's own kiosk if
there is a display, to take S6 out.

**L3, photon check (optional, the operator's choice).** A phone in slow motion films the
screen and a hand clap or a stick hitting the mic stand within 30 cm of the mic. The contact
is visible, so the phone's audio track, whose sync is not trustworthy, is not needed.
Resolution per video frame: 240 fps → 4.2 ms, 120 fps → 8.3 ms, 960 fps (short bursts) →
1 ms. Use a broadband hit and watch an HF column (band 5), so the DSP adds only ≈ 11–22 ms.
20 hits give the mean within ≈ 1–2 ms. Once with the display's game/low-latency mode on and
once off, if it has one.

**L4, matrix.** For each of: 2D spectrum, flat spectrograph, 3D spectrograph × glide on/off
(Settings › Display) × present mode as resolved (log it at start) × L1/L2. Plus the kiosk Pi
(`ac2pi`, software rasteriser at 30 fps, `gpu.rs` `SOFTWARE_FRAME_INTERVAL`; V3D on GLES =
FIFO) for 2D and flat only, since 3D is off there.

## 4. Hypotheses, ranked by likely size for the complaint

| # | Hypothesis | Size | Confirms / refutes | Candidate fixes |
|---|---|---|---|---|
| H1 | **LF window physics.** Bass and kick show 340–430 ms after they are heard and grow over 683 ms, while hats show in ≈ 15 ms. On music this reads as "the picture lags", most of all in the spectrograph, where a kick's row smears down the time axis. | 100s of ms, LF only | L0 per band; L1 63 Hz vs 8 kHz burst; ask what content lags | A *fast* ladder for live views (N_max 16 384: LF centre 85 ms at 5.9 Hz bins); in history views, place each band's contribution at its window centre (shift by N_b/2) so onsets line up across frequency; state band age in the readout |
| H2 | **Display chain.** A TV without game mode (ketunkolo's screen blanks like a TV, per `ketunkolo-blanking`) or a compositor (screenshots in the tree are from Hyprland, so is the app under XWayland?) adds 1–6 frames. | 15–100 ms | L3 with game mode on/off; log the WM/compositor and the resolved present mode | game mode; bypass the compositor (fullscreen unredirect); nothing in ac2 |
| H3 | **Glide lag.** D ≥ 30 ms at 60 frames/s, plus the 32-frame max-lag margin, so the drawn curve trails the newest frame by about an interval and more when S2/S7/S8 jitter. | 20–50 ms | L1 glide on vs off: Δ(photon − effective capture time) | 2D: draw the newest frame at once and glide only columns whose band updates slower than 30/s (LF); 3D: show the newest row at once and smooth only the scroll; `MIN_S` = one display frame; margin from p90, not max |
| H4 | **Three free-running 60 Hz clocks** (fan-out hand-off, link drain at 33/2 ms, UI pacer) plus present unsynced to vblank. Delivery jitters ±½–1½ frames, with sawtooth beats; under Immediate a tear line crawls at the 60.000 vs real-refresh beat. | jitter σ ≈ 8 ms, span ≈ 50 ms | L1/L2 per-stage holds (D0→D2, A0→A1, A1→A2); beat period in the latency-over-time plot; exact refresh rate | drain on arrival while the UI is idle (no period/2 hold); link period = display period (16.7 ms, not 33 ms); pace on vblank (FIFO, frame latency 1, while keeping the TV-blanking fix 7420be47 intact); align the hand-off to the hop |
| H5 | **Spectrograph slot quantisation.** 30 slots/s by `capture_wall_ns`, frames at 60/s: rows step 0/1/2 slots irregularly and the row glide follows those steps. | 33 ms steps, uneven | A3: slot advance per frame histogram; row lag over time | place rows by `audio_sample` through the rate fit (exact spacing) instead of WALL; 60 slots/s for short histories |
| H6 | **Fan-out hand-off wait.** U(0, 16.7 ms) of real latency by design, so one wakeup per publish period. | mean 8 ms | D0→D1 per result | hand-off at a multiple of the block period phase-locked to the shortest hop; or emit as soon as the HF band frames |
| H7 | **Scene/GPU cost in 3D.** A pass over 16.7 ms drops to 30 frames/s and doubles S8. | 0–17 ms | A2 end − start, A4; cache hit rate | skyline upload only for new slots (check it holds), cache per pane |
| H8 | **Credit stalls / network hitches.** Rare hitches, not the steady lag. | rare 250 ms | D3 credit flag; phase C already | already known (tg3 PTP); nothing for steady lag |
| H9 | **`capture_wall_ns` bias.** Late by S1 + ≤ 1 block, slewed by chrony; it pollutes `Pacing` lags only slightly. | ≤ 6 ms bias, µs jitter | D0: WALL stamp vs RAW-line time of the same sample | stamp from the RAW line mapped to WALL (capture clock fit) |

Expectation: H1 dominates *lag* on music. H2 may dominate the constant if the display is a TV.
H3 + H4 + H5 make up the *jitter* the operator sees in the 3D view.

## 5. Acceptance

Measured from the frame's end-of-window sample; the DSP window (H1) is reported separately per
band and not counted here.

| Quantity | Good | Now (expected) |
|---|---|---|
| daemon-added (t_s of the last sample → peer send, S1–S5) | p50 ≤ 10 ms, p95 ≤ 20 ms | ≈ 13 / 25 ms |
| network S6 | p99 ≤ 2 ms | met (phase C) |
| app-added, glide off (arrival → photon estimate, S7–S12 without panel) | p50 ≤ 1 display frame (16.7 ms) + scanout to the pane, p95 ≤ 2 frames | ≈ 25 / 45 ms |
| glide cost (glide on − off, by the effective capture time) | ≤ 1 display frame p50 | ≈ 30–50 ms |
| end-to-photon jitter | p95 − p5 ≤ 1 display frame (16.7 ms); no visible beat | ≈ 40–50 ms span |
| 3D scroll | row position error vs a straight line ≤ 0.25 slot p95 | steps of 1 slot |
| kiosk Pi (software, 30 fps) | app-added p95 ≤ 2 of its frames (67 ms) | unmeasured |

Total for an 8 kHz event on a monitor: ≤ 60 ms end-to-photon plus ≈ 11 ms of window.

## 6. Open items for the operator

1. ketunkolo's display: a TV or a monitor; game/low-latency mode; X11 or Wayland/XWayland
   (Hyprland?) and which compositor. This decides H2 and how A5 is read.
2. Spectrum 2's settings: averaging on or off (a τ adds directly), and what lags to the ear:
   bass/kick or everything (H1 vs the rest).
3. OK to run the instrumented daemon on pupu and the app on ketunkolo with `--latency-log` for
   L2 (≈ 45 min, passive, the operator's own music)?
4. L3 photon check: optional, only if the operator wants the panel constant.

## 7. Measured so far (2026-11-10)

`ac2 selftest frames` on ketunkolo against pupu, 30 s each, live music, daemon d5562480:

| | Spectrum 2 (multi-res 1/12) | Spectrum 1 (multi-res 1/48) |
|---|---|---|
| frames | 1799, 0 missing, 0 repeats | 1800, 0 missing |
| arrival interval p50 / p95 / p99 / max | 16.7 / 20.6 / 22.0 / 32.4 ms | 16.7 / 21.0 / 22.5 / 28.7 ms |
| age at arrival (capture_wall_ns → read) min / p50 / p95 / max | 2.2 / 8.2 / 13.3 / 26.7 ms | 2.1 / 8.3 / 13.3 / 19.8 ms |
| audio step per frame (`audio_sample`) p50 / min / max | 21.3 / 10.7 / 42.7 ms | 21.3 / 10.7 / 32.0 ms |
| rate | 0.34 Mbit/s | 0.66 Mbit/s |

Clock offset daemon − local ≈ −0.3 ms (keepalive); phase C bounds it to −38…+7 µs.

Read: the daemon's send through the client's read costs 8 ms p50 and 13 ms p95: H6, H8 and
the network are small. Frames leave on a steady 60/s clock, but the audio each one carries
steps 10.7/21.3/32/42.7 ms (multiples of a 1024-sample block at 96 kHz): the content
advances unevenly against a smooth arrival clock. That is H4's beat inside the daemon, and the
3D rows placed by wall time inherit it (H5).

Operator items resolved from the host: ketunkolo is Xorg + xfwm4 with its compositor on
(`use_compositing` true, `vblank_mode` auto, `unredirect_overlays` true) driving a 1600 × 900 mm
HDMI panel (a TV) at 1920 × 1080 60 Hz. Both spectra run with no averaging. H2 stays open on
the TV's game mode and whether the maximised (not full-screen) app window is unredirected.

### App on ketunkolo, 3D view, 180 s (`--latency-log`, 8e3e2381)

RTX 2070 Vulkan, present mode resolved Immediate (AutoNoVsync; Fifo offered), frame latency 2,
xfwm4 compositor on. 10 625 passes, 9 819 data frames shown.

| stage | p50 | p95 | max |
|---|---|---|---|
| capture → app link read | 16.4 | 25.9 | 40.1 ms |
| read → reducer apply | 8.8 | 16.3 | 47.6 ms |
| apply → scene build | 0.2 | 0.3 | 1.1 ms |
| build → submit/present | 0.2 | 0.3 | 2.1 ms |
| capture → present (newest frame) | 25.5 | 38.0 | 52.9 ms |
| glide lag (drawn behind newest) | 25.6 | 45.2 | 151 ms |
| **capture → present, as drawn** | **53.4** | **63.9** | **169 ms** |

Pass interval 16.9 ms p50 (max 114), CPU 0.8 ms per frame, 3D build 0.11 ms. Slot advance per
frame: 0 slots 49.6 %, 1 slot 50.3 %, ≥ 2 rare: the 3D scroll moves every other frame (30
slots/s at 60 frames/s).

Read:
- Rendering is not the cost (< 1 ms build + submit).
- **Glide doubles what is seen** (H3): +26 ms p50, +45 ms p95, 151 ms worst.
- The app reads the link 8 ms later than `selftest frames` does (16.4 vs 8.2 ms p50): the link
  drain period (H4), then another ≈ 9 ms waiting for the next UI pass.
- The 3D scroll steps at 30/s (H5): visible judder at 60 frames/s.
- Not measured: compositor + TV after present (H2), and the DSP window (H1, per band).

## 8. Changes (2026-11-10)

- **Glide (H3).** A curve that updates at the display rate (frames ≤ 33 ms apart) shows each
  frame the pass it arrives; a slower one eases in over one display frame (`curve_ease::EASE_S`,
  1/60 s), no longer over the time to the next frame (30–200 ms). The glide's added lag goes
  from 26 ms p50 / 45 p95 / 151 max to 0 for live curves and at most one display frame for
  slow ones.
- **Spectrograph scroll (H5).** Rows no longer glide frame to frame. The flat and 3D
  spectrographs scroll on the display's clock (`ac2_scene::scroll_clock::ScrollClock`): the
  time axis is `now − offset` in session audio time (`audio_sample ÷ rate`), the offset the
  earliest arrival over the last 120 frames, changing at most 0.05 s per second. The axis is at or past the
  newest frame when it arrives and moves the same distance every display frame, whatever
  audio the frames carry (10.7–42.7 ms steps at 96 kHz). Between frames it runs up to the
  arrivals' spread plus one hop past the newest audio, the newest column stretched over that
  lead (`lead_slots`, the 3D phase and the heatmap shader's `lead`), then waits. Slots are
  placed by audio time, not wall time.
- **Link drain (H4).** The link task reads within 1 ms of a frame's arrival (`PollClock`
  gathers 1 ms after the first arrival, not on a fixed period), and an arriving frame wakes the
  UI (`pace::link_wake`): the pass runs once half a display interval has passed since the last
  one (`pace::data_wait`), so a new frame draws next pass rather than after the idle tick.
- **Present (H2).** Kept Immediate (AutoNoVsync), frame latency 2. Fifo would block the
  acquire for a second when the TV stops vblanks (the blanking freeze fixed in 7420be47).
  Present-to-glass is still unmeasured.

Local check, not ketunkolo: Xvfb + llvmpipe, fake backend at 48 kHz, one multi-res 1/12
spectrum in the flat spectrograph, 60 s each, release builds of 8e3e2381 and this change.

| | before | after |
|---|---|---|
| capture → read p50 / p95 / max | 10.8 / 20.0 / 24.5 ms | 7.7 / 11.8 / 17.7 ms |
| read → apply p50 / p95 / max | 8.1 / 17.3 / 19.8 ms | 5.1 / 15.7 / 22.7 ms |
| capture → present p50 / p95 / max | 20.7 / 33.9 / 41.1 ms | 14.1 / 23.8 / 29.9 ms |
| pass interval p50 | 33.8 ms | 17.0 ms |
| frames shown | 1843 | 3553 |
| CPU per frame p50 | 2.9 ms | 4.4 ms |

The flat spectrograph had no glide before, so this run shows the link and pacing gains only;
on ketunkolo's 3D view the glide lag (25.6 ms p50, 151 max) is gone on top. The rerun on
ketunkolo with `--latency-log` is the acceptance (§5).

### ketunkolo after ceaf5d2d (3D view, 180 s, live music)

| stage | before p50 / p95 / max | after p50 / p95 / max |
|---|---|---|
| capture → app link read | 16.4 / 25.9 / 40.1 | 9.8 / 14.9 / 30.3 ms |
| read → reducer apply | 8.8 / 16.3 / 47.6 | 0.7 / 8.3 / 12.9 ms |
| capture → present (newest) | 25.5 / 38.0 / 52.9 | 12.8 / 21.9 / 38.8 ms |
| glide lag | 25.6 / 45.2 / 151 | 0 / 0 / 0 ms |
| **capture → present, as drawn** | **53.4 / 63.9 / 169** | **14.6 / 29.8 / 50.6 ms** |

Pass interval p50 9.3 ms (data passes plus display passes under Immediate), CPU 0.7 ms per
pass. Left: the daemon's 60 Hz hand-off (≈ 8 ms mean of capture → read) waits for multi-rig
stage 3a's per-period hand-off; present → glass (compositor + TV) is not measured.

## 9. Frames on device periods (2026-11-11, H4 and H6 in the daemon)

A live spectrum now frames every whole number of device periods nearest a display frame
(`SpectrumConfig::frame_hop`: the multiple of the period closest to `fs / 60`, at least one
period and never under `fs / 64`; 6 × 256 = 1536 samples = 16 ms at 96 kHz, 3 × 256 = 768 at
48 kHz). Every hop (`display_hop`) is that frame hop when it lies in n/8 … n/2, a multiple of
it for long windows, a divisor for short ones; spectra end on multiples of their hop
(`SpectrumAnalyzer`), so the multi-resolution bands frame on one grid and a frame goes at
each grid point where the shortest band (or a long window) has a spectrum. While a spectrum
runs the fan-out wakes for the block that ends the next frame (below), and the job sleeps
until that block, then sends it at once. RTA is unchanged (its own
interval); the spectrograph keeps placing rows by `audio_sample`. No wire change; the app
states the new update rate (`fft_resolution` takes the period).

`ac2 selftest frames`, 20 s each, local fake backend at 48 kHz / 256 (release builds of
a32f2711 and this change):

| | multi-res 1/48 before | after | 4096 pt before | after |
|---|---|---|---|---|
| frames | 1200 | 1250 | 938 | 1250 |
| interval p50 / p95 / max | 16.7 / 17.2 / 19.2 | 16.0 / 16.4 / 22.6 | 16.7 / 33.5 / 34.3 | 16.0 / 16.2 / 24.2 ms |
| audio step min / p50 / max | 10.7 / 21.3 / 21.3 | 16.0 / 16.0 / 16.0 | 21.3 (every one) | 16.0 ms |
| age p50 / p95 / max | 5.6 / 6.3 / 8.0 | 0.5 / 0.9 / 8.5 | 5.7 / 11.2 / 16.5 | 0.3 / 0.5 / 8.5 ms |
| ac2d CPU (meas thread) | 4.2 % (1.3) | 3.8 % (1.2) | 2.3 % (0.4) | 2.6 % (0.5) of a core |

The fixed 4096-point spectrum used to skip every fourth publish slot (a 21.3 ms hop on a
16.7 ms clock: 97 gaps in 20 s). FFT work: the multi-res ladder at 96 kHz costs 1.20 % of a
core here against 1.29 % with the old power-of-two hops (`multires_bench`); at 48 kHz the
shorter HF hops add about 9 % in FFT operations.

### Waking for the frame, not every period (2026-11-11)

Handing on every device period cost pupu (i5-2415M, 96 kHz / 256) about 17 % of a core more
(ac2d 57 → 74 %: fan-out 22.6 %, every job woken 375 times a second). Now each spectrum
job states the end sample of the block that completes its next frame (`Queue::due`, from
`frames_needed`), and the fan-out (`hop_wake.rs`) predicts when the device delivers that
block from the blocks seen so far and sleeps until a quarter period after it. A wake that
finds it not in yet looks again a quarter period later, then every period if the stream has
stalled; each new target starts a 64th of a period earlier, so an estimate that drifted late
comes back. Other jobs and the meters still get audio at least every `handoff`. Stage 3a's
cases (an active tap, a remote-input joiner) keep handing on every period. Spectra start
their grid on a multiple of the frame hop in device samples, so all spectra of a session
frame at the same points and share one wake. Simulated (`hop_wake` tests, ±300 ppm, jitter
up to a third of a period): 66 wakes/s for 62.5 hops/s, the frame's block taken 0.3–0.6 ms
p50, at most 1.4 ms (a period is 2.7 ms) after it arrived.

Local, release builds of a9fd17a6 and this change, `jackd -d dummy -r 96000 -p 256`
(not real-time), two multi-res 1/48 spectra and an SPL meter, each followed by
`ac2 selftest frames` for 36 s; CPU and voluntary context switches over 30 s:

| thread | before: % of a core, wakes/s | after |
|---|---|---|
| ac2d-fanout | 1.3 %, 375 | 0.9 %, 66 |
| meas (spectrum, each) | 2.0–2.3 %, 62 | 2.0–2.2 %, 62 |
| meas (SPL) | 0.8 %, 375 | 0.6 %, 70 |
| ac2d-io | 1.5 %, 298 | 1.3 %, 245 |
| ac2d total | 9.1 % | 8.1 % |

| `selftest frames` | spectrum before | after | SPL before | after |
|---|---|---|---|---|
| frames in 36 s | 2250 | 2250 | 720 | 718 |
| interval p50 / p95 / p99 / max | 16.0 / 16.8 / 17.2 / 20.8 | 16.0 / 16.8 / 17.3 / 26.7 | 50.6 / 50.8 / 51.5 / 54.2 | 48.0 / 64.5 / 65.2 / 80.0 ms |
| age p50 / p95 / max | 0.5 / 1.2 / 5.1 | 0.6 / 1.3 / 3.2 | 0.3 / 0.5 / 1.4 | 0.5 / 0.9 / 1.7 ms |

This host is fast enough that the CPU saved is small in percent; the wakes are what cost
pupu, and they fell to the hop rate. Age counts from the hand-off that took the frame's
block, so it does not show how long after its arrival that was; the simulation above does.
The one 27 ms interval came at the same instant on both spectra (one late hand-off of the
fan-out on the non-real-time dummy server). SPL frames go on hand-offs about 16 ms apart
again, as before frames moved to device periods (a 20 Hz meter steps 48 / 64 ms).
