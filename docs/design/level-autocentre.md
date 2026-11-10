# Level axis follows the content

Status: draft. Operator request: the spectrum pane's level axis re-centres on what it shows,
gently and only when the picture needs it, because a kiosk (touch, no keyboard) cannot fix a
range that no longer fits.

## Purpose

A spectrum pane frames its level axis once: on its first curve, and again when a stopped
run starts (`fit_first_shows`, `rearm_started_spectra` in `ac2-ui/src/state/panes/levels.rs`).
After that the range stands until the operator acts: Ctrl+I/O zoom, Ctrl+↑/↓ pan, Ctrl+wheel
zoom, Shift+wheel pan, Shift+Home fit, Ctrl+Home reset (`keys/table.rs`). A gain change, a
new source or a band starting after soundcheck leaves the curve against the top or crushed
at the bottom, and a full-screen kiosk has no way to press Shift+Home. Following the content
fixes that without touching what the operator chose on purpose.

## What moves: the centre, not the span

Only the range's position moves; its span stays. The span is the resolution the operator
(or the first fit) chose: how many dB a centimetre is. Changing it changes the shape every
curve appears to have, so a 3 dB bump would read differently after an automatic step; a pan
keeps every shape and only slides the numbers. The span still comes from the first-show fit
and from Shift+Home, as today.

## The content statistic

Computed in `ac2-scene` (`view::level::Follow`, pure, no I/O) from what `level_values`
already gathers: the live curves the pane draws, offsets included, within the shown
frequency range, in the pane's drawn unit.

- **Per frame:** the 95th percentile of the valid columns (`top`). NaN, ±∞, unresolved
  columns (multi-resolution bands without data yet) and columns past their frame age are
  dropped; a frame with fewer than 8 valid columns gives no value. The 95th percentile, not the
  maximum, so one tonal spike or a 50 Hz hum peak does not steer the axis.
- **Over time:** a ring of `(frame time, top)`, using the frames' own time, so a stalled
  stream or a disconnect counts no time and the tests need no wall clock.
- **Spectrum and RTA alike:** RTA bands are columns; the percentile over 31 bands is
  coarse but the same rule.
- **Spectrograph (2D, split, 3D):** the same live frames feed the history, so the same
  statistic. The history itself is never scanned: it would answer for seconds ago.
- **Not counted:** stored traces (they never drift, and a reference trace far below must
  not hold the live curve down), the peak-hold curve (it only rises), compared panes'
  curves when the pane follows only its own measurement (see match scales).

## The trigger: only when the picture needs it

Two conditions, deliberately asymmetric, both on the frame-time ring:

| condition | test | window |
|---|---|---|
| **clipped** | the *median* of `top` over the window is above `hi` | 10 s |
| **squashed** | the *maximum* of `top` over the window is below `lo + 0.3·span` | 20 s |

The median means the curve's top sits above the plot for more than half the time, not
once; transients and loud bars of music are ignored. For the squash test even the loudest
moment of 20 s must sit in the bottom third: a quiet verse between choruses never pulls the
axis down. Neither test fires within **30 s** of the previous move or of the first-show fit,
so the axis never walks.

Hysteresis comes from the target (below): after a move the content top sits at
`hi − 0.2·span`; going up again needs a further rise of 0.2·span (12 dB on a 60 dB
axis) held for the median window, going down a fall of 0.5·span held for 20 s.

## The target and the motion

- **Where:** the window's median `top` lands near `hi − 0.2·span`: the curve's top
  a fifth below the frame, its floor allowed to run off the bottom (noise floor is the
  least read part of a spectrum).
- **Snap:** the shift is a whole number of the axis's major tick steps (the step the
  labels are drawn at, 10 dB on a usual spectrum axis), the multiple that lands nearest the
  target, never zero, clamped to the level bounds. Labels end on the same values at the same
  grid lines, so nothing crawls; only the numbers change.
- **Ease:** 0.8 s, cosine in-out, on the range itself: curves, grid, spectrograph colours and
  3D heights move together, every frame drawn on one consistent range.
- **Kept or not:** a followed range is framed automatically, as the first fit is: it is not
  an operator range, so it is not saved with the measurement's panes nor in `ui.toml`
  (`level_set` cleared for that slot).

## Manual control wins

Any operator range change on a pane's slot (zoom, pan, wheel, Ctrl+Home reset) pauses
following for that slot. It resumes when:

- the pane shows another measurement or scale (the existing `SpectrumHold` rule), or
- the operator asks: **Shift+Home** (fit) frames the content *and* re-arms following. Fit
  and follow are one concept, "frame what is shown", so they share one control; no new key.
- **No timeout.** A range that jumps back half an hour after the operator zoomed in on a
  detail is a surprise mid-tune; a kiosk gets its own gesture instead.

**Touch:** a double-tap on the plot (outside the legend plate, whose double-click already
puts it back) is Shift+Home. The pane's single click focuses and places the cursor; a double
click on the plot is unused today (`view/panes/mouse.rs`, `navigate`). In the 3D view a
double-tap does the same; the camera keeps Home.

A range restored from a saved layout is a starting point, not a pause: following stays armed
on it. The pause is this run's state only, so a kiosk that boots on a layout the operator
once zoomed on the desktop still follows.

## Match scales

Spectrum panes keep a range each today; only SPL panes share a union (`shared_scale`). When
spectrum panes join the shared scale, the group follows as one: the statistic is the maximum
of the panes' window medians in that unit, one decision, one shift, every pane eased
together; an operator change on any pane of the group pauses the group. Until then each pane
follows its own measurement.

## dBFS and dB SPL

Each scale is its own slot (`PaneLevels::spectrum_dbfs`, `spectrum_spl`), so each has its
own ring, pause and last-move time. Switching scale never carries a decision across: the
other slot starts from its own state, with its first-show fit if it has none.

## On/off and default

One app setting, Settings → Display: **"Level axis follows the content"**, `ui.toml`
(`UiPrefs::level_follow`), **on** by default. App, not per pane or per measurement: it
describes this screen's behaviour (a kiosk wants it, a desk may not), and the per-pane state
that matters, the pause, is already implicit in the operator's own range changes.

**No wire change.** The toggle is `ui.toml`, the pause and ring are run state, followed
ranges are not saved. PROTO 86 is untouched by this feature.

## How the UI shows a move

The level axis labels are the truth and they change; nothing else claims anything. During the
ease and 1.5 s after, the axis's tick labels fade from the accent colour back to their usual
grey: enough for an eye on the curve to notice the numbers changed, without a word that could
read as a verdict about the signal. No toast (a kiosk would collect them). The hint bar's
Shift+Home tip reads "fit level" while following and "fit + follow" while paused, so the
desk operator sees the pause without a badge on the plot.

## The 2D spectrograph's colours

The spectrograph colours by the pane's level range (a uniform, no re-upload), so a move
recolours the whole history at once, and the colour bar with it: bar and picture cannot
disagree, and every cell still reads its true level on the bar. Moves are rare (a real,
sustained level change, at most one per 30 s), so the recolour marks the moment the level
changed rather than hiding it. A spectrograph range of its own would be a second control for
the same concept. **Recommend: share the range.** The 3D view takes the same range
(`spectrograph_3d.rs`, `range` from the pane's axis for the history's scale): its height
scale post and ceiling line move with it, labelled.

## Tests

`ac2-scene` (pure `Follow`, frames scripted with frame times):

- **Pink-noise drift:** −1 dB/min over 20 min, inside the range: no move until the squash
  test holds, then exactly one, snapped to the tick step.
- **Music dynamics:** sections ±12 dB every 4–8 s, 50 ms bursts +20 dB, rests: no move in
  10 min.
- **Level step:** +20 dB held: one move after 10 s, content top within half a step of the
  target, no second move; −30 dB: one move after 20 s.
- **Validity:** NaN / unresolved / stale columns ignored; a stream that stops counts no time.
- **Ease:** intermediate ranges monotonic, final range exactly on the grid, span unchanged.

`ac2-ui` reducer (`state/tests/display/axis.rs`):

- **Manual override:** a pan pauses; a later step does not move; Shift+Home fits and
  re-arms; the next step moves.
- **Scale switch:** dBFS pause leaves SPL following.
- **Setting off:** nothing moves; a followed range never lands in `operator_levels`.
- **Double-tap** dispatches the Shift+Home command (key table, hint and user-guide row
  in the same change).

## Open questions (with recommendations)

1. **Windows and thresholds** (10 s / 20 s / 30 s, 0.3·span, target 0.2·span): start here,
   tune on pupu with a music programme and a level step. *Recommend: constants in
   `ac2-scene`, not settings.*
2. **Other kinds** (transfer, sweep response): a transfer magnitude is relative and rarely
   drifts; sweeps already fit per result. *Recommend: spectrum only.*
3. **Should Ctrl+Home (reset to default) also re-arm?** It sets a range by hand. *Recommend:
   no, it pauses; Shift+Home is the one way back to following.*
4. **Double-tap in the 3D view: fit level or reset camera?** *Recommend: fit level, as the
   2D panes do; the camera keeps Home and its drag.*
5. **Follow the floor too** (content top fine, floor off the bottom by a lot)? *Recommend:
   no; the span is the operator's.*
