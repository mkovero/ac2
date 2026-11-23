# Bin streams: resolution `bins`, the analysis's own bins

The grid stream (`d/<meas>/tf`) stays what captures, stored traces, EQ and export use. A
transfer's finest resolution step, `bins`, computes the grid as at 1/96 and also offers each
MTW stage's own bins, `bins/<meas>/<stage>`, which the plot digs down to with zoom. All
stages ship together; each is subscribed by what the view shows. See `mixed-grid.md`.

## Design in brief

- A transfer's resolution gains `bins` (typed, section 0), reached by the resolution keys
  and dialog. Any other resolution: no bins formed, subscribed or drawn, except an
  eligible math (section 10).
- Stage frames: mag, phase, γ² per bin, drawn as they come (no DSP in the scene,
  `ac2-scene/src/trace.rs:3-4`), with the `tf` stamp; ranges in `TfMeta`; blends draw grid.
- Bins drawn where finer than the grid and each has a pixel; final frames at `Stop`.
- One `PROTO_VERSION` bump: 104 (the worktree has 101; origin/main dadc1730 has 102, where
  `TfMeta.unresolved` is `Option<Vec<BinRange{lo,hi,bin_hz}>>`; 103 is claimed elsewhere).

## The ladder and where bins are drawn

From `ac2-core/src/mtw/layout.rs`: NFFT 4096 (`:14`), targets 12 k / 4 k / 1 k (`:16`,
`:20`), served band 0.45·rate (`:24`), hop `NFFT >> (b+1)` (`:296`), crossover `lo` at
`κ(48)·Δf` of the shallow stage (`:302`), `hi = lo·2^(1/3)` (`:26`). Stage `b` is drawn
from crossover `b`'s `hi` (first bin `⌈hi/Δf⌉`) to crossover `b−1`'s `lo` (last bin
`⌊lo/Δf⌋`); stage 0 up to `⌊served_hi/Δf⌋` (as `mtw/mod.rs:469`); the deepest from bin 1.
Computed from these formulas, not from a run.

| rate | stage | M | Δf Hz | hop | drawn Hz | bins | frame (6 B/bin) |
|---|---|---|---|---|---|---|---|
| 48 k | 0 | 1 | 11.72 | 42.7 ms | 1 022–21 600 | 88…1843: 1756 | 10.5 kB |
| | 1 | 4 | 2.93 | 85.3 ms | 256–811 | 88…276: 189 | 1.1 kB |
| | 2 Std | 12 | 0.977 | 128 ms | 1–203 | 1…207: 207 | 1.2 kB |
| | 2 LongLf | 12 | 0.977 | 128 ms | 85–203 | 88…207: 120 | 0.7 kB |
| | 3 LongLf | 48 | 0.244 | 256 ms | 0.2–68 | 1…276: 276 | 1.7 kB |
| 96 k | 0 / 1 | 1 / 8 | 23.4 / 2.93 | 21.3 / 85.3 ms | 2 045–43 200 / 256–1 623 | 1756 / 466 | 10.5 / 2.8 kB |
| 192 k | 0 / 1 | 1 / 16 | 46.9 / 2.93 | 10.7 / 85.3 ms | 4 090–86 400 / 256–3 246 | 1756 / 1020 | 10.5 / 6.1 kB |

Stages 2, 3 are alike at every rate; a fifth appears above about 260 kHz (`layout.rs:263`).
Blends (grid drawn): 68–85 Hz (LongLf), 203–256 Hz, 811 Hz–1.02 kHz (48 k; 96 k ×2, 192 k ×4).
A 1/96 column holds more than one bin above `κ(96)·Δf = 138.5·Δf` (`layout.rs:32`): stage 0
from 1.62 / 3.24 / 6.49 kHz (48 / 96 / 192 k), stage 1 406 Hz, stage 2 135 Hz, stage 3 34 Hz.

## 0. `TransferResolution`

`Resolution` (`ac2-proto/src/model/dsp.rs:79-89`) stays the four column grids; sweeps
(`model/sweep.rs:40`, `:190`), multi-resolution spectra (`SpecResolution::Multi`,
`dsp.rs:129-131`), IR import (`model/trace.rs:56`) and `Unresolved` (`dsp.rs:238`) keep it
and cannot hold bins. `TransferConfig.resolution` (`model/measurement.rs:38`) becomes
`enum TransferResolution { Grid { grid: Resolution }, Bins }`, tagged as `SpecResolution`
(`dsp.rs:117-119`): every transfer config on the wire (JSON, msgpack) changes to
`{"type":"grid","grid":"forty_eighth"}` or `{"type":"bins"}`; default `Grid { FortyEighth }`.
`grid()` gives the column grid (`Bins` → `NinetySixth`); `ALL` lists four grids, then `Bins`.

Touch list (every non-test read of a transfer's resolution):
- `model/defaults.rs:39`: `TransferConfig::grid` uses `resolution.grid().ppo()`, so math's
  grid check (`control/maths.rs:229-236`) treats `Bins` and 1/96 operands as one grid.
- `jobs/transfer.rs:281`: `Unresolved` gets `cfg.resolution.grid()` (marker `1/96 oct`);
  after the rebase onto dadc1730 `unresolved` holds no resolution and this touch goes.
- `ac2-ui/src/state/commands.rs:118-131`: `resolution_step` walks `TransferResolution::ALL`,
  so Alt+Shift+K reaches `Bins`; `said` uses the scene's label. Toasts: `resolution bins
  (averaging starts over)`, `bins is the finest resolution`. The job restarts
  (`control/execute.rs:555-563`) even from 1/96, so the toast stays true.
- `ac2-ui/src/forms/options.rs:30-35`, `build.rs:215`, `:293`, `:497-500`, `:626-631`, `:702`: the
  transfer dialog gets its own five-choice list and `TransferResolution` reader; sweep
  (`:413`), IR import (`ir_import_dialog.rs:161`) and `spec_resolutions` (`options.rs:109`)
  keep the four. `RESOLUTION_HINT` (`:39`) names `bins`.
- `ac2-scene/src/format.rs:328`: new `transfer_resolution` gives `1/96 oct` or `bins`;
  `meas_list.rs:131-137` splits transfer from sweep and reads `· resolution bins`.
- CLI: `ResolutionArg { grid, multi }` (`args/trace.rs:229-234`) becomes an enum
  `Grid(Resolution) | Multi(Resolution) | Bins`, parsing `--resolution bins` (`:252-280`).
  `cmd/meas_config.rs:178-186` calls `columns()` before the kind dispatch: dispatch first,
  `tf` takes `transfer()` (accepts `bins`), sweep and `ac2 ir` (`cmd/ir.rs:143`) take
  `columns()` (refuses it), `spec_resolution` (`:48-58`) refuses it; `:528-529` likewise.
  Help texts `args/meas.rs:271`, `:421` add `bins` (tf only). JSON output is serde.
- Session `VERSION` 49 (`ac2-traces/src/session.rs:62`; manifest holds `MeasConfig`, `:78`);
  `docs/protocol.md:1893`; `docs/user-guide.md:1904`; samples
  (`ac2-proto/src/samples/builders.rs:108`; `:185` is a sweep, unchanged) and the Python
  fixtures (`tools/protocol/fixture_messages.py:108`) in the new shape, plus a `Bins` one;
  `fixtures/protocol/` regenerated.

**A capture at `Bins`** is a 1/96 grid trace: `trace.capture` takes the `tf`
(`jobs/transfer.rs:857-866`) and `TraceMeta` (`model/trace.rs:434-470`) records only
`grid_id`, the 1/96 grid. It reads `1/96 oct`; nothing records `Bins`; it keeps no bins.

## 1. The stage frame

| array | element | code | value |
|---|---|---|---|
| `mag` | i16 | dB, hundredths (`ac2-proto/src/packing.rs:7`) | `20·log10|H1ₖ|`, mic curve taken off |
| `phase` | i16 | degrees, hundredths | `arg H1ₖ`, delay applied |
| `coh` | u16 | γ², 1e-4 (`packing.rs:13`) | `|Gxyₖ|²/(Gxxₖ·Gyyₖ)`, clamped to 1 as `engine.rs:577-580` |

**Accessor.** New `Mtw::stage_bins_into(stage, lo, hi, h1, coh)` fills reused buffers, NaN
where `Gxx = 0` (`stage_h1_into` gives 0 and no γ², `ac2-core/src/mtw/engine.rs:437-443`;
`stage_spectra` allocates, `:449-466`). Per-bin mic gain is rebuilt in `set_correction`
(`jobs/transfer.rs:463`); quantising is on the job thread (`jobs/mod.rs:164-169`).

**Stamp.** The `tf`'s `StampArgs`, from the same `emit` (`transfer.rs:814`, built at
`:606-615` from the input end), `grid_id: None`. The stage's newest block end
(`engine.rs:417`) only paces it. Stage dues join the returned `Flush` (`transfer.rs:854`).
A stamp of its own would stall while protection rejects blocks (`:667-676`).

**Static meta** (change forces a keyframe, `frame/wire.rs:160`): stage, `M`, stage rate,
`Δf`, first bin, hop in full-rate samples, delay, `nudged`, `mic_curve`, `mic_range`.
**Per-frame** (`PerFrame::Bins`, `wire.rs:75`): `blocks`, `eff_avg` (`engine.rs:590`).

**No average to show.** `set_delay` keeps some stages and resets others
(`engine.rs:227-236`; full restart `:204`, `:209`). A stage with no held block sends
`blocks: 0` and NaN arrays; its range draws the grid. A stopped cross-rig job, while
`stopped_frame` is `Some` (`transfer.rs:429-436`, `jobs/cross_rig.rs:134`), sends
`blocks: 0`: `tf` then shows an older kept snapshot.

## 2. Layout in `TfMeta`

`TfMeta` gets `bins: Option<Box<BinLayout>>` beside `unresolved` (`frame/meta.rs:42`): per
stage its drawn range, per crossover its blend `lo`/`hi`; keyframes only. `Some` for a
transfer at `Bins` and an eligible math (section 10), whether or not anyone subscribes; a
change (LongLf → Std) drops the stage on the next keyframe.

## 3. Topics and plumbing

- **Name** `bins/<meas>/<stage>`, top-level like `audio/`, so `d/` subscribers
  (`Subscription::AllData`, `ac2-proto/src/topic.rs:237`) miss it. `Topic::parse` gets a
  `bins/` branch (`topic.rs:143`; `d/` takes three segments, `:155-158`; `parse_prefix`
  `:261-278` falls through). One digit: `Interest::wants` matches byte prefixes
  (`ac2d/src/io.rs:59`). Apps only: `may_subscribe` refuses a rig (`peers.rs:193-197`).
- **Clearing.** `remove_meas` (`control/owners.rs:327-332`) clears `bins/<m>/` (`io.rs:221`).
  Session stop clears `bins/` beside `d/` at both places: `wind_down`
  (`control/session_ctl.rs:89-93`) and session close (`:149`, `:172-174`), each after
  `stop_all_jobs()`, so final frames formed there are dropped. Final frames matter for
  `meas.stop` (`control/execute.rs:199-204`) only: a reconfigure's (`:555-563`) are
  superseded, and its old-revision slots linger harmlessly until `remove_meas`.

## 4. Freshness and stop

A slot is not cleared when its last subscriber leaves (`io.rs:221` runs only from
`remove_meas`); a re-subscriber is handed it (`peers.rs:316-323`). The rule:

> Live curve: draw a stage frame only if its `config_rev`, delay and `nudged` equal the
> shown `tf`'s, and its `audio_sample` is at most its hop (meta) plus 250 ms (`REFRESH`,
> `jobs/mod.rs:190`) behind the `tf`'s. Stopped curve: its stamp equals the shown `tf`'s.

The client discards other epochs and incarnations (`ac2-client/src/data.rs:211`,
`:502-504`); a rate or device change starts a new epoch (`session_ctl.rs:193-196`). The delay
compare is for slips, which move the delay without a new revision (`transfer.rs:376-379`).

**Stop.** `JobMsg::Stop` returns with no emit today (`jobs/mod.rs:672`). New: a transfer at
`Bins`, on `Stop`, forms a final `tf` and a frame for every stage from the same averages and
one stamp, before it returns; other resolutions never do. A new
`Analysis::final_frames(&Emitter)` at `:672` bypasses `Pace`, whose `due` would `Skip` an
unchanged `audio_sample` (`jobs/mod.rs:243-262`), and calls `Emitter::send`, which returns
`false` on a full channel (`:164-177`): each frame is retried up to 20 times 1 ms apart, then
dropped with a `warn`, which bounds the join in `stop_inner` (`:477-483`). Frames go into
their slot unconditionally (`io.rs:206-216`), the slot lives until `remove_meas`, and a new
subscriber's chain is dirty for it (`chain.rs:122-124`): a stage first zoomed into after the
stop shows the same averages as the curve. `meas.stop` also takes a math (`is_job`,
`model/measurement.rs:443`); math forms no final frames: stages subscribed at its last emit
hold frames of that emit's stamp and draw; others draw the grid.

## 5. What is applied where

| | where | note |
|---|---|---|
| Delay | daemon, in the bins | aligner splice and block rotation (`engine.rs:178-237`) |
| Mic curve | daemon, per bin | at the bin's frequency; the grid at column centres (`transfer.rs:540-545`) |
| Reference-delay rotation, nudge, offset, polarity, coherence blanking | scene, per bin | the per-column rules of `trace.rs:6-10`, `:20-52` |
| Unwrapped phase, group delay | grid only | the unwrap runs over valid points in order (`trace.rs:536-558`) |

## 6. Drawing, cursor, peaks

**Where drawn.** A transfer at `Bins` or an eligible math, the measurement's own curve
(running or stopped, not a stored trace), unsmoothed,
magnitude, wrapped phase or coherence view, no peak hold or P in the pane, stage frame fresh,
and both of:
- **Finer than the grid**: above `138.5·Δf`; the pixel rule does not imply it.
- **A pixel per bin**, in logical px: bin spacing on screen
  `s = (px per octave) · Δf / (f · ln 2)`, with `f` the view's top (unwidened) clamped into
  the stage's finer-than-grid range (where `s` is smallest), is ≥ 1 px. Narrow and wide
  plots behave alike; zooming in reveals bins.
One vertex per bin at `k·Δf`, including the first bin beyond each view edge; bins and the
adjacent grid points form one connected line, with no minimum drawn span. The join at
`138.5·Δf` is seamless: the grid there already has one point per bin. Everything else: grid.

Widest view (oct) at which each 48 k stage draws, `W·Δf/(f_top·ln 2)`; `s` in 20 Hz–20 kHz:

| width px | stage 0 (to 21.6 k) | stage 1 (to 811) | stage 2 (to 203) | stage 3 LongLf (to 68) | default view: `s` for 0 / 1 / 2 / 3 → result |
|---|---|---|---|---|---|
| 800 | 0.63 (14.0–21.6 kHz) | 4.2 (45–811 Hz) | 5.6 | 4.1 | 0.07 / 0.42 / 0.56 / 0.42 → nothing |
| 1280 | 1.00 (10.8–21.6 kHz) | 6.7 | 8.9 | 6.6 | 0.11 / 0.67 / 0.89 / 0.67 → stage 2 subscribed |
| 1900 | 1.49 (7.7–21.6 kHz) | 9.9 | 13.2 | 9.8 | 0.16 / 0.99 / 1.32 / 0.99 → 2 drawn; 1, 3 subscribed |
| 2500 | 1.96 (5.6–21.6 kHz) | 13.0 | 17.4 | 13.0 | 0.21 / 1.31 / 1.74 / 1.30 → 1, 2, 3 drawn |

Default view draws only finer-than-grid parts (406–811, 135–203, 34–68 Hz); stage 0 needs ≲ 2 oct.

**Blend spans** add no shade: each lies inside an existing unresolved mark (the 1/96
ranges start at each crossover `lo`, `ac2-core/src/mtw/layout.rs:487-497`;
`ac2-scene/src/unresolved.rs:62`). Inside that mark a `811 Hz – 1.02 kHz: grid` text shows
only while an adjacent stage is drawn as bins. **Readout** names its form: nearest drawn
bin, `1 234.4 Hz · 11.7 Hz bin · −3.2 dB`; in an unresolved grid part the existing
unresolved note's bin width (`2.9 Hz bins`); `1/96 oct` only on resolved columns
(`tf/mark.rs:13`). Below, "grid" means whichever of the two applies.

**Peak hold and P force the grid.** While either is on in a pane, that pane draws the grid
everywhere: the hold folds `tf` frames (`ac2-ui/src/link_wants.rs:148`) and P picks grid
landmarks. **A math average's range band** (`jobs/math.rs:596-611`) is per column; it is
hidden where bins are drawn, with a legend note. A per-bin band is not on the wire and would
need a PROTO bump.

**Legend.** A transfer's legend reads its config (`· resolution bins`); a math's, which has
no resolution, reads `· bins` only while a stage is drawn in that pane.

| case | legend | readout |
|---|---|---|
| below `138.5·Δf` (unresolved grid part) | `· resolution bins` | `2.9 Hz bins` (unresolved note) |
| `s` < 1 px; before the first frame; cross-rig blank (`TfMeta.bins` `None`); smoothed (adds `· 1/6 oct smoothing`) | `· resolution bins` | grid |
| peak hold or P on | `· resolution bins (grid: peak hold)` / `(grid: peaks)` | grid |
| unwrapped phase, group delay; Alignment view; stale or settling stage (`blocks: 0`); just stepped 1/96 → `bins` (job restarted) | `· resolution bins` | grid |
| blend span | `· resolution bins` | grid; `…: grid` text while a neighbour draws bins |
| stored curve captured at `Bins` | `1/96 oct` (its grid) | grid |
| eligible math, a stage drawn (an average adds `· range band on grid parts`) | `· bins` | bin |
| eligible math, peak hold or P on | `· bins (grid: peak hold)` / `(grid: peaks)` | grid |
| math with no stage drawn (zoom, smoothing, stale) or not eligible (stored operand, other ladders, operand not at `Bins` or stopped) | no `bins` | grid |

## 7. Subscription from the view

`wanted_topics`/`topics_of` are pure (`link_wants.rs:40-59`, `:131-175`); the range is
app-wide (`ac2-scene/src/view.rs:842`); each transfer pane's plot width is reported by the
view layer and kept per pane, as the Leq strip's (`state/leq.rs:43`, `leq/strip.rs:349`). `topics_of` gets range, widths
and each measurement's latest `TfMeta.bins`. Stage `b` of a `MeasKind::Transfer` at `Bins`,
or of a `MeasKind::Math` with `MathDomain::Transfer` (`:166-169`) whose `TfMeta.bins` is
`Some`, is wanted when a shown Transfer pane (not Walk, `:48`; not hold-only, `:148`; no
peak hold or P on) draws the measurement's own curve (running or stopped, not a stored
trace) unsmoothed, the view widened by 1/3 octave each side meets the stage's finer-than-grid
range (this only decides whether the stage is considered), and `s` at `f` = the unwidened
view top clamped into that range is ≥ 0.8 px; drawing needs 1 px.
Stateless; edge jitter neither subscribes nor drops; a new stage comes next emit.

## 8. Requirement: idle rigs and viewers neither form nor receive bins

`Interest` follows `DataUp::Subscribe` (`peers.rs:488-505`); a job forms only topics
`Emitter::wants` (`jobs/mod.rs:129-144`). SPL-only rig or kiosk: no transfer at `Bins`, no
frame ever. An unviewed transfer at `Bins` forms stage frames once, at `Stop` (13 kB).

## 9. Cost

Per measurement at `Bins`. One stage frame per stage hop, at most 60/s
(`ac2d/src/config.rs:214`, `io.rs:16`); `tf` once per stage-0 block (`transfer.rs:676`,
paced at `jobs/mod.rs:254-258`). Raw codes, before delta and deflate.

| kB/s per measurement | 48 k | 96 k | 192 k |
|---|---|---|---|
| `tf` at 1/96 (1440 col × 10 B, `frame/encode.rs:60`) | 337 (23.4/s) | 675 (46.9/s) | 864 (60/s) |
| stage 0 | 247 | 494 | 632 |
| stage 1 | 13 | 33 | 72 |
| stage 2 Std / LongLf; stage 3 LongLf | 10 / 6; 6 | same | same |
| nobody views (SPL-only rig, kiosk, remote rig, Android, CLI) | 0 | 0 | 0 |

N curves cost N times: eight at 48 k over stages 0 and 1 ≈ 2 MB/s raw plus `tf`.

## 10. Math over `Bins` operands (step 3)

The math job asks each live operand on that operand's thread (`Probe::request` →
`JobMsg::Capture`, `jobs/mod.rs:504-509`, `:659-662`) for a `tf` `Frame`, waiting ≤ 250 ms
(`jobs/math.rs:173-205`); `value` takes columns, delay, validity, mic (`:363-425`);
`math::transfer` combines `Columns` at any `freqs` (`ac2-traces/src/math.rs:266-289`; the
`GridDef` only labels an average, `ops.rs:411`). So per-bin operand data is reachable:

- **Layout without bins.** The plain `Capture` answer is the operand's full `tf` frame
  (`jobs/transfer.rs:857-866`), whose meta carries `bins`. From those metas alone the math
  job knows eligibility and the layout, so its `tf` lists `bins: Some` before anyone
  subscribes, and a subscription can then follow.
- **Request.** New `JobMsg::CaptureBins` / `Probe::request_bins`: the operand answers on its
  own thread with its `tf` and, from the same averages and stamp, per stage the unquantised
  mag dB, phase, γ² (mic curve off, delay applied) and `blocks`. The math job sends it only
  when `Emitter::wants` one of its `bins/<math>/<s>`, so an unviewed math costs no more.
- **Combine** on the math job's thread, per stage: each operand's stage bins as `Columns`,
  `freqs = k·Δf`, a `GridDef::Linear { fs: stage rate, n: 4096 }` (`ac2-proto/src/grid.rs:42`),
  and the same `CoreReference` the grid result used (`math.rs:511-523`), so bins and grid
  refer to one delay. `math::transfer` rotates each operand's phase from its own delay to the
  reference at each bin frequency, so delay differences are exact per bin. Each operand's
  mic curve is already off its bins at the bin frequency, as on its columns; the result's mic
  meta is the grid result's. Stage meta: `blocks` and `eff_avg` the minimum over operands
  (`blocks: 0` if any has none), hop the slowest operand's, `mic_range` the intersection.
- **Eligible:** every operand live, included, at `Bins`, all `BinLayout`s in their `tf` metas
  equal (same rate and LF window, so the same stages and bins). Then the math `tf` carries `bins: Some`
  and stage frames go out on `bins/<math>/<s>`, stamped as the math `tf` from its `emit`
  (`math.rs:801-823`). Otherwise `bins: None`: a stored operand (`Held` columns only,
  `:54-66`), other ladders, an operand not at `Bins` or stopped. `Align` math stays on the
  grid (its view is the Alignment scene); level math (spectrum, RTA) has no bins.
- **Stop.** A math forms no final frames (section 4); a stopped operand makes it ineligible
  (`Answer::Stopped`, `math.rs:304`).

Viewed math bins cost each operand an f32 stage set per math frame (250 ms fallback stays).

## 11. Staging

Split by layer; each step leaves main shippable.
1. **Config, wire and daemon streams.** All of section 0, since the type of
   `TransferConfig.resolution` changes: stepper, five-choice dialog (else a `Bins`
   measurement opened in it saves as 1/48), toasts, `format::transfer_resolution`, legend,
   CLI, session `VERSION` 49. `Topic::Bins`, `FrameKind::Bins`, `PerFrame::Bins`,
   `TfMeta.bins` (math ids too); `Mtw::stage_bins_into`; stage frames in `emit`; `blocks: 0`
   rules; `final_frames`; `bins/` clears. PROTO 104, `WIRE_LOCK`, `docs/protocol.md` §5.1–5.2
   (`:2202`, `:2245`; `tests/it/doc_parity.rs`), fixtures, samples (`samples/frames.rs`);
   `tools/protocol/ac2proto.py`: `STREAMS` (`:24`), `parse_topic` (`:132`), the tf meta table
   (`:252-264`), per-frame `DELTA_META` (`:309`). With step 1 alone the app reads
   `· resolution bins` with the readout unchanged (the form label is step 2). Tests:
   - proto round trip of frame, topic, one-digit cap, `TfMeta.bins`, both
     `TransferResolution` shapes (JSON, msgpack); CLI `--resolution bins` for `tf`, usage
     error for sweep, spectrum, `ac2 ir`; `ui::` Alt+Shift+K 1/96 → `bins` and its toast,
     the dialog keeps `bins` on save, legend text;
   - real `ac2d` on the fake backend (`ac2-e2e`), Std and LongLf: subscribed stages arrive
     with their range; unsubscribed never formed; 1/96 forms none and lists no layout; a
     rig is refused; SPL-only forms none; partial `set_delay` restart sends `blocks: 0` for
     reset stages only; stage stamp keeps pace with `tf` while blocks are rejected; stopped
     cross-rig sends `blocks: 0`; after `meas.stop` a late subscriber gets frames stamped as
     the final `tf`; session stop leaves no `bins/` slot;
   - equality at `Bins`, 48 k, mic curve and smoothing off: stage 0 bin 100 (1 171.9 Hz,
     alone in its 8.5 Hz-wide 1/96 column, outside any blend) equals its `tf` column within
     quantisation; likewise stage 1 bin 110 (322 Hz), stage 2 bin 100 (97.7 Hz), LongLf
     stage 3 bin 100 (24.4 Hz).
2. **Drawing.** Drawing, blend marks, readout form, forced grid, freshness, widths,
   subscription. No wire change. Tests: scene per stage range, grid in each blend with its
   mark, every row of the section 6 table; `ui::` wanted stages against zoom, pan, width,
   peak hold and P, jitter around 1 px and the margin edge (no change), a layout change.
3. **Math.** Eligibility from `tf` metas, `CaptureBins`, per-stage combine. No wire change.
   Tests: an unviewed eligible math lists the layout, and a stage arrives after subscribing;
   A−B of two live `Bins` operands with different delays equals the per-bin difference
   rotated to the reference; a stored operand, a 1/96 operand, Std vs LongLf: `bins: None`.

## 12. Risks; not determined from the code

Risks: blend joins; bandwidth per curve (credit takes topics in turn, `io.rs:12-18`); a full
channel stretches the stop. Unknown: real bytes after delta + deflate; daemon CPU per stage
frame and per math combine, on x86 and the CM4.
