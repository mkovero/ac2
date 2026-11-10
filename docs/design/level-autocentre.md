# Level axis follows the content

Status: implemented (`ac2-scene` `view/follow.rs`, `ac2-ui` `state/panes/follow.rs`). Operator request: the spectrum pane's level axis
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
states; following is armed in the first only:

- **Auto**: not in `level_set`; framed by the app. Following armed. Not saved, not sent.
- **Operator**: in `level_set`; set by hand. Following off. Saved in `ui.toml` and sent in
  the measurement's layout (`operator_levels`; `meas_layout.rs:97`, `queries/layout.rs:156`).

A *followed* range is an Auto range the follower has moved: same persistence as Auto.

Today two flags say "the operator set this": `level_set` per slot (persistence) and
`SpectrumHold.operator` per pane (`rearm_started_spectra` keeps such panes, levels.rs:438).
They already disagree: a received layout without a range clears `level_set`
(`set_operator_levels`, levels.rs:216-226) and leaves `operator: true`. The follower would
make that worse, so **`SpectrumHold.operator` goes: the hold asks `level_set` for its
(pane, scale) slot.** One flag, one truth; the columns below are that flag and what follows
from it.

| event | today | with following (`level_set` → state) |
|---|---|---|
| first show, no saved range | cleared → Auto | same; ring starts empty |
| first show on a saved range (`ui.toml`, own layout) | set → Operator | same; following off |
| run restarted (`rearm_started_spectra`) | Auto panes refit; Operator kept | same, keyed on `level_set`; Auto rings cleared |
| follow move | — | untouched → Auto: `operator_levels` and the layout unchanged, nothing sent, nothing written |
| zoom, pan, wheel, Ctrl+Home | set → Operator, layout sent | same |
| **Shift+Home** | set → Operator (`hold_level`), layout sent | **changed:** fit, then cleared → **Auto**; the layout is sent once *without* that range. With following off: as today |
| double-tap (touch) | — | as Shift+Home, no toast. With following off: as today's Shift+Home |
| layout restored / own layout on row choice | per range: set if present, else cleared | same |
| layout from another client | applied only while this app chose that row (`meas_layout.rs` header); set if it carries a range, else cleared | same, and now a rearm agrees with it. A kiosk's pinned pane never takes another client's layout, so it is never paused this way |

The behaviour change, with following on: Shift+Home no longer stores the fitted range with
the measurement. A fit means "frame what is shown", which following keeps doing; storing a
snapshot of it would freeze the very thing the operator asked to track. With following off
Shift+Home stores it as today: no regression for a desk that turned following off. Ctrl+Home
(reset to the default) stays Operator: a chosen range, not a framing.

**Kiosk line** (for the ac2pi README in the sys repo): a range set by hand over VNC is
Operator and stays until a double-tap on the plot; the double-tap makes it Auto, which
`ui.toml` keeps, so the kiosk follows from then on, across reboots.

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
  rigs. A window decides only when no gap between consecutive frames in it is longer than
  2 s (a multi-resolution spectrum or a slow RTA sends a few frames a second, so a frame
  count would starve them), and a stalled stream counts no time. A curve's rig changing `session_epoch` or `daemon_incarnation` clears the
  ring: a restarted daemon is a new signal history.

## Trigger, target, motion

Asymmetric, because a clipped curve is wrong now and a low one is only less legible:

- **Up, fast:** the 80th percentile of the pane top over the last **3 s** above `hi`
  (a fifth of the frames clipped). No lockout beyond needing 3 s of frames since the
  previous move.
- **Down, slow:** the maximum of the pane top over the last **240 s** below
  `lo + 0.35·span`, and at least **60 s** since the previous move. A song gap, a speech
  break or a quiet verse never lasts that long; an idle mic does.
- **Span (Auto only), independent of clipping:** the slow-window median of the per-frame
  extent (top minus the frame's 5th-percentile column, its floor) above **0.8·span**: the
  content no longer fits, whether it clips or is cut at the bottom. (Testing "top above
  `hi`" over the slow window could never hold: the up rule fires within 3 s.) The new
  range is `level::fit` of the window's median floor and maximum top, widened downward until
  the extent fills at most **0.6** of it, so it does not re-trigger at once. Lockout as for
  down. The follower never narrows a span (idle-mic result below).
- **Target:** up puts the window's *maximum* at `hi − 0.2·span`, down puts it at or below
  that; the shift is the axis's major tick step (`nice(span / 8)`, as `level::fit` rounds)
  times the whole number nearest the target, rounded away from zero when that is zero, then
  clamped to the level bounds. Labels end on the same grid lines; only the numbers change.
- **Ease:** 0.8 s, cosine in-out, on the range itself: curves, grid, spectrograph colours
  and 3D heights move together, every frame on one consistent range.
- **Hands off:** no move while a pointer button is down or a drag is in progress, nor within
  30 s of a key or pointer input on that pane (on any member of its group). The decision
  waits; it is not dropped.

## Validation (constants provisional)

A scratch simulation (not committed; the music clips are private) runs the rule on column
spectra close to the daemon's: Hann 8192, tone level, max of bins per column, 10 frames/s.
Where it differs: **1/24-octave** columns, not the live grid's 1/96 (four times fewer
columns, so a slightly lower per-frame max), and frames either unaveraged or with a 1 s
exponential power average (ac2's averaging is its own setting). Step `nice(span / 8)` as
`level::fit`. Six 50 s commercial clips (−5 … −14 dBFS rms) loop into 3-minute songs at
±4 dB, gaps of pink room noise about 70 dB below. The pane starts on `level::fit` of its
first frame. Constants as written above (up 80th percentile over 3 s, down 240 s,
lockout 60 s, pad 0.2, squash 0.35, widen 0.8 → 0.6). Ten seeds each; "below" is the mean
share of columns under `lo`, the content cut off at the bottom:

| scenario | moves, mean (max) | clipped s, mean (max) | longest clipped run | below |
|---|---|---|---|---|
| show: 1 h of 3-min songs, 30–60 s gaps | 1.1 (2), never down | 0.8 (5.0) | 0.4 s | 13 % |
| same, 1 s average | 0.8 (2) | 0.1 (0.8) | 0.7 s | 7 % |
| show, pane fitted on the silent room first | 3.1 (5): up, then widens | 2.5 (10.9) | 0.7 s | 23 % |
| same, 1 s average | 3.6 (6) | 1.6 (3.0) | 0.8 s | 15 % |
| fitted on a flat −140 floor, then music | 2.2 (3): up, one widen | 1.2 (2.1) | 0.7 s | 18 % |
| tone 40 / 60 dB over a pink floor, 10 min after a fit | 0 | 0 | 0 | 0 % |
| tone appearing after a fit on the floor | 2.0 (2): up, widen | 0.8 (0.9) | 0.8 s | 36 % |
| music, +20 dB for 10 min, then −30 dB for 10 min | 1.5 (4) | 1.3 (3.8) | 2.2 s | 11 % |
| idle mic 20 min between music | 2.4 (4): down, up, widen | 1.1 (2.5) | 1.4 s | 14 % |
| pink noise rising 1 dB/min for 20 min | 1.9 (2) | 11.3 (16.5) | 0.3 s | 6 % |
| same, 1 s average | 2.0 (2) | 2.7 (3.7) | 0.8 s | 0 % |

Tuning on the same runs: the up quantile at the median left 25 s an hour clipped (max 70 s,
in runs under a second) on the show; 0.67 left 3.4 s; 0.8 the figures above, with no more
moves. A 5 s up window clipped more for the same moves. A 0.15 target pad clipped the noise
drift twice as long as 0.2. Without the 0.6 headroom a widen re-triggered at once. The
"below" share stays 7–23 % on music after a widen: music's top octave sits far under its
bass, and the fit's median floor leaves the quietest frames' lowest columns under `lo`.
A show fitted on silence can widen a second time an hour later, when a long gap of room
noise lowers the window's floor.

The first draft's rule (95th percentile top, 20 s squash window, 30 s lockout) made 31–36
moves an hour on the same show: a down in every gap, an up at every song.

All numbers are provisional: confirm on pupu with the live grid and averaging, a music
programme and a level step, before the constants are frozen in `ac2-scene`.

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

**Every view of the spectrum pane follows**, the 3D spectrograph included: its heights are
the pane's range, so a move eases the skyline's heights with it. A double-tap on the 3D view
is a tap, not an orbit (an orbit needs a drag), and makes the slot Auto; an orbit, tilt or
dolly is pointer or key input on the pane and starts the hands-off time like any other.

## Panes on one scale

Same-unit Auto spectrum panes on screen follow as one group with **one range**, so
side-by-side panes never end up a step apart. Once a frame, before drawing, as
`refresh_spl_scales` does for the Leq columns (`scenes/spl_meters.rs`; a pane of another rig
read with its rig swapped in):

- **The group range** is `level::fit` over the union of the members' values. It is fitted
  again when a member joins (a pane shows a curve, or turns Auto), on Shift+Home or a
  double-tap on any member, and on a rearm of any member's measurement; a member leaving
  keeps the range.
- **Following:** the group's top per frame is the maximum of its members' tops (each the
  max over its curves); one ring, one decision, one range written to every member.
- **Hands off** on one member holds the whole group.
- Operator panes are out of the group. The group lives under the Display toggle of the
  shared Leq scale (`UiPrefs::shared_scale`), widened to same-kind panes; off, each pane
  follows alone.

## dBFS and dB SPL

Separate slots: separate rings, states and last-move times. A scale switch carries nothing.

## On/off and default

Settings → Display: **"Level axis follows the content"**, `ui.toml` (`UiPrefs::level_follow`),
**on** by default; app-wide (a kiosk wants it, a desk may not). Off, everything is as today,
Shift+Home included (it stores an Operator range). The key hint stays the static
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
- **Widen:** fitted on a flat −110 dBFS floor, then music: one up, then the span widens
  once (no second widen in 10 min); a follow move never narrows.
- **Coverage gap:** frames 0.3 s apart decide; a 3 s hole in the window holds the decision.

`ac2-ui` reducer (`state/tests/display/follow.rs`):

- **Manual override:** a pan makes the slot Operator; a later step does not move it;
  Shift+Home fits, makes it Auto, the next step moves it.
- **A follow move sends no layout and writes nothing** (`operator_levels` unchanged, no
  request out, prefs not dirty).
- **Start/stop rearm:** refit once, no counter-move from the old ring.
- **Shift+Home, then a run restart:** the slot refits (it is Auto, one flag says so).
- **Received layout without a range:** the slot is Auto for following and for a rearm alike.
- **Hands off:** a step during a drag moves only 30 s after the drag ends.
- **Two panes** of one unit stay on one range through a step.
- **Second pane joins** showing content 30 dB higher: the group refits once over both, both
  panes on the new range.
- **Shift+Home on one member** refits the whole group; hands off on one member holds the
  other's move too.
- **Double-tap** fits level only, leaves the cursor as before the first tap, sends the layout
  without the range, marks `prefs_dirty` (ui.toml drops the range); a tap pair on a legend
  row does nothing of the kind. The 0.4 s / 24 px rule is checked by hand on the CM4 kiosk
  panel before it ships.
- **Setting off:** nothing moves; Shift+Home and double-tap make the slot Operator as today.
- **Frequency zoom:** the same move as unzoomed.
- **Daemon incarnation change:** the old history moves nothing.
- **3D view:** a step moves the range and the skyline's heights span the moved range.

## Open questions (with recommendations)

1. **Idle mic:** after 240 s the axis drops onto the mic's floor; music back costs one up
   move and under 2 s clipped. *Recommend: accept; the room is what there is.*
2. **Narrowing the span** when content fills under a third of it: with a 300 s window it
   zoomed onto an idle mic's noise floor. *Recommend: follower widens only; a fit narrows.*
3. **Other kinds** (transfer, sweep): *Recommend: spectrum only.*
4. **Ctrl+Home re-arm?** *Recommend: no; it is a chosen range.*
5. **Shared-scale toggle** covering spectrum groups: *Recommend: widen the existing one, one
   control per concept.*
