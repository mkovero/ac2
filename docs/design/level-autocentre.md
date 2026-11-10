# Level axis follows the content

Status: draft, review round 1 applied. Operator request: the spectrum pane's level axis
re-centres on what it shows, gently and only when the picture needs it, because a kiosk
(touch, no keyboard) cannot fix a range that no longer fits.

## Purpose

A spectrum pane frames its level axis on its first curve and again when a stopped run starts
(`fit_first_shows`, `rearm_started_spectra`, `ac2-ui/src/state/panes/levels.rs`); after that
the range stands until the operator acts: Ctrl+I/O, Ctrl+↑/↓, Ctrl/Shift+wheel, Shift+Home
(fit), Ctrl+Home (reset). A kiosk that fitted on a silent room, or a source whose gain
changed after soundcheck, keeps a range that shows the curve off the top or crushed at the
bottom, and the kiosk has no Shift+Home. Following fixes that and never touches a range the
operator set.

## Slot states and what each event does

A slot is one pane's range for one scale (`LevelSlot::SpectrumDbfs`, `SpectrumSpl`). Two
states, as the code has them today; following is armed in the first only:

- **Auto**: not in `level_set`; framed by the app. Following armed. Not saved, not sent.
- **Operator**: in `level_set`; set by hand. Following off. Saved in `ui.toml` and sent in
  the measurement's layout (`operator_levels`; `meas_layout.rs:97`, `queries/layout.rs:156`).

A *followed* range is an Auto range the follower has moved: same persistence as Auto.

| event | today | with following |
|---|---|---|
| first show, no saved range | Auto (`level_set` cleared, hold `operator: false`) | same; ring starts empty |
| first show on a saved range (`ui.toml`, own layout) | Operator (hold `operator: true`) | same; following off |
| run restarted (`rearm_started_spectra`) | Auto slots refit; Operator kept | same; Auto rings cleared |
| follow move | — | Auto only. `level_set` untouched, so `operator_levels` and the layout are unchanged: nothing sent, nothing written |
| zoom, pan, wheel, Ctrl+Home | Operator, layout sent | same |
| **Shift+Home** | Operator (`hold_level`), layout sent | **changed:** fit, then **Auto**: `level_set` cleared, so the layout is sent once *without* that range |
| double-tap (touch) | — | as Shift+Home, no toast |
| layout restored / own layout on row choice | per range: Operator if present, else Auto | same |
| layout from another client | applied only while this app chose that row (`meas_layout.rs` header); its ranges are Operator | same. A kiosk's pinned pane never takes another client's layout, so it is never paused this way |

The behaviour change: Shift+Home no longer stores the fitted range with the measurement. A
fit means "frame what is shown", which following keeps doing; storing a snapshot of it would
freeze the very thing the operator asked to track. Ctrl+Home (reset to the default) stays an
Operator range: it is a chosen range, not a framing.

A kiosk booting on a range set by hand in its `ui.toml` stays on it until a double-tap; the
double-tap makes the slot Auto, which `ui.toml` then keeps, so the kiosk follows from the
next boot on.

## The content statistic

Pure, in `ac2-scene` (a `Follow` state beside `view::level`, the module inside `view.rs`).
The app gathers its input with a new function beside `level_values`, which also takes stored
and compared traces and only the shown band, both wrong here:

- **Curves:** the live curves the pane draws, its own and compared ones, offsets included,
  in the drawn unit. Stored traces never drift and the peak hold only rises: not counted.
- **Per curve, per frame:** the highest valid column (NaN, ±∞, unresolved multi-resolution
  columns dropped) between 20 Hz and 20 kHz, the frequency axis's limits, *not* the shown
  zoom: the frequency axis is shared across panes, and zooming one must not move levels.
  No minimum column count: a 1/1-octave RTA has ten.
- **Per pane:** the maximum over its curves, each on its own grid. This is the "top"
  `level::fit` frames to (its upper end is the maximum), so a fit and the follower never
  disagree about where the content is. A feedback ring or a test tone *is* the top.
- **Grids:** a fixed-FFT spectrum arrives on `LogBins` (single bins at the bottom, then
  1/96 octave, tone level the maximum of a column's bins); multi-resolution on `Log`; RTA on
  `IecBands`. The maximum over many noise bins reads a few dB above their mean, so a
  narrowband noise curve's top sits above its look; RTA band power is its own, higher,
  quantity. Each is placed on its own drawn values, so each lands within a step of the same
  relative position.
- **Clock:** the app's clock at each frame's arrival (`now_s`), one clock for a pane mixing
  rigs. A window decides only when frames cover at least 80 % of it, so a stalled stream
  counts no time. A curve's rig changing `session_epoch` or `daemon_incarnation` clears the
  ring: a restarted daemon is a new signal history.

## Trigger, target, motion

Asymmetric, because a clipped curve is wrong now and a low one is only less legible:

- **Up, fast:** the 80th percentile of the pane top over the last **3 s** above `hi`
  (a fifth of the frames clipped). No lockout beyond needing 3 s of frames since the
  previous move.
- **Down, slow:** the maximum of the pane top over the last **240 s** below
  `lo + 0.35·span`, and at least **60 s** since the previous move. A song gap, a speech
  break or a quiet verse never lasts that long; an idle mic does.
- **Span (Auto only):** over the slow window, the top above `hi` and the median of the
  per-frame 5th percentile below `lo` (content overflowing both ends): refit the span as
  `level::fit` does. The follower never narrows a span (idle-mic result below).
- **Target:** up puts the window's *maximum* at `hi − 0.2·span`, down puts it at or below
  that; the shift is a whole number of the axis's major tick steps, never zero, clamped to
  the level bounds. Labels end on the same grid lines; only the numbers change.
- **Ease:** 0.8 s, cosine in-out, on the range itself: curves, grid, spectrograph colours
  and 3D heights move together, every frame on one consistent range.
- **Hands off:** no move while a pointer button is down or a drag is in progress, nor within
  30 s of a key or pointer input on that pane. The decision waits; it is not dropped.

## Validation

A scratch simulation (not committed; the music clips are private) runs the rule on column
spectra built as the daemon builds them (Hann 8192, tone level, max of bins per 1/24-octave
column, 10 frames/s, no averaging, so noisier than ac2's curves). Six 50 s commercial clips
(−5 … −14 dBFS rms) loop into 3-minute songs at ±4 dB, gaps of pink room noise about 70 dB
below. The pane starts on `level::fit` of its first frame. Ten seeds each:

| scenario (ten seeds) | moves, mean (max) | clipped s, mean (max) | longest clipped run |
|---|---|---|---|
| show: 1 h of 3-min songs, 30–60 s gaps | 0.6 (1), never down | 0.8 (3.3) | 0.6 s |
| show, pane fitted on the silent room first | 1.5 (3), all up in the first minute | 1.2 (2.2) | 0.8 s |
| tone 40 / 60 dB over a pink floor, 10 min after a fit | 0 | 0 | 0 |
| tone appearing after a fit on the floor | 1.2 (2) | 1.1 (2.7) | 1.1 s |
| music, +20 dB for 10 min, then −30 dB for 10 min | 1.3 (2) | 1.7 (5.6) | 0.8 s |
| idle mic 20 min between music | 2.2 (3): one down, one up | 1.1 (2.9) | 0.8 s |
| pink noise rising 1 dB/min for 20 min | 1.7 (2) | 8.1 (8.4) | 0.4 s |

Tuning on the same runs: the up quantile at the median left 25 s an hour clipped (max 70 s,
in runs under a second) on the show; 0.67 left 3.4 s; 0.8 the figures above, with no more
moves. A 5 s up window clipped more for the same moves. A 0.15 target pad clipped the noise
drift twice as long as 0.2. The pink-drift clipped seconds are single noise columns of
unaveraged frames above the top for under half a second; averaged curves do less.

The first draft's rule (95th percentile top, 20 s squash window, 30 s lockout) made 31–36
moves an hour on the same show: a down in every gap, an up at every song.

## Manual control, kiosk gesture

An Operator slot never moves until Shift+Home or a double-tap makes it Auto. No timeout: a
range that jumps back half an hour after a zoom on a detail is a surprise mid-tune.

**Double-tap** on the plot, outside the legend (`row_at` hits excluded, as is the plate's own
double-click): fits the level only (frequency untouched), makes the slot Auto, no toast.
egui 0.36 counts a double click within 0.3 s and 6 px of the first
(`InputOptions::max_double_click_delay`, `max_click_dist`): mouse-sized; a fingertip on a Pi
touch panel drifts further. The pane keeps its own detector: two clicks within 0.4 s and
24 logical px. The first tap already focused the pane and placed the cursor; the cursor state
from before it is restored on the second, so a kiosk is left with no stray cursor. The 3D
view the same; its camera keeps Home.

## Panes on one scale

Same-unit spectrum panes on screen follow as one group, so side-by-side panes never end up a
step apart. Once a frame, before drawing, as `refresh_spl_scales` does for the Leq columns
(`scenes/spl_meters.rs`; a pane of another rig read with its rig swapped in), gather each
Auto pane's top; the group's top is their maximum: one decision, one range written to every
Auto member. Operator panes are out of the group. The group lives under the Display toggle
of the shared Leq scale (`UiPrefs::shared_scale`), widened to same-kind panes.

## dBFS and dB SPL

Separate slots: separate rings, states and last-move times. A scale switch carries nothing.

## On/off and default

Settings → Display: **"Level axis follows the content"**, `ui.toml` (`UiPrefs::level_follow`),
**on** by default; app-wide (a kiosk wants it, a desk may not). The key hint stays the static
"fit level"; the help and the user guide say a fit also re-arms following. **No wire
change**: the toggle is `ui.toml`, rings are run state, followed ranges are Auto.

## How a move shows

The axis numbers are the truth and they change. During the ease and 1.5 s after, the
level-axis gutter carries a faint grey shade that fades out (the gentle plot-hint style): no
accent colour, nothing near the clip/alarm colours, no toast, no words.

## The 2D spectrograph's colours

The spectrograph colours by the pane's range (a uniform), so a move recolours the history and
the colour bar together: every cell still reads its level on the bar. Moves are rare and mark
a real level change. A range of its own would be a second control for one concept.
**Recommend: share.** The 3D view takes the same range (`spectrograph_3d/`, `range` from the
pane's axis for the history's scale), its height post and ceiling line labelled.

## Tests

`ac2-scene` (pure `Follow`, scripted frames with arrival times):

- **Tone over floor:** 1 kHz 40 dB over pink, fitted: no move in 10 min.
- **Show:** an hour of 3-min songs, 30–60 s gaps: no down move, at most one up, no clipped
  run over 2 s, under 10 s clipped in all.
- **Level step** +20 dB: one move up within 3 s, top within one step of the target.
- **Idle mic** 20 min: one down after 240 s; music back: one up.
- **Narrowband vs RTA** on one pink noise: each lands within one step of its target.
- **Compared curve on another grid:** the pane top is the max of both.
- **Frequency zoom:** changes no decision.
- **Epoch / incarnation change:** ring cleared, no move on the old history.
- **Coverage:** a stream that stops counts no time.
- **Ease:** monotonic, final range on the grid, span unchanged.

`ac2-ui` reducer (`state/tests/display/axis.rs`):

- **Manual override:** a pan makes the slot Operator; a later step does not move it;
  Shift+Home fits, makes it Auto, the next step moves it.
- **A follow move sends no layout and writes nothing** (`operator_levels` unchanged, no
  request out, prefs not dirty).
- **Start/stop rearm:** refit once, no counter-move from the old ring.
- **Hands off:** a step during a drag moves only 30 s after the drag ends.
- **Two panes** of one unit stay on one range through a step.
- **Double-tap** fits level only, leaves the cursor as before the first tap, sends the layout
  without the range; a tap pair on a legend row does nothing of the kind.
- **Setting off:** nothing moves.

## Open questions (with recommendations)

1. **Idle mic:** after 240 s the axis drops onto the mic's floor; music back costs one up
   move and under 2 s clipped. *Recommend: accept; the room is what there is.*
2. **Narrowing the span** when content fills under a third of it: with a 300 s window it
   zoomed onto an idle mic's noise floor. *Recommend: follower widens only; a fit narrows.*
3. **Other kinds** (transfer, sweep): *Recommend: spectrum only.*
4. **Ctrl+Home re-arm?** *Recommend: no; it is a chosen range.*
5. **Shared-scale toggle** covering spectrum groups: *Recommend: widen the existing one, one
   control per concept.*
