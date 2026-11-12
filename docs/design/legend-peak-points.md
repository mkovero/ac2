# Peak points from the legend

Status: superseded. **P** (peak hold Track) is the one peak UX in every view; a legend
click only selects its curve, and the legend is one movable plate on every view. The
legend-row pick, `spectrum::points` and `state/peak_points.rs` are gone; the 3D front-row
boxes (`spectrograph_3d::points`) now show the tracked peaks. Kept as the record of the
rejected design.

A click on a curve's legend row shows that curve's **peak points**: its five peaks, each
with frequency and level, on the spectrum pane and in every G view of it, the 3D
spectrograph included. **T** to traces only hides them along with every other peak mark,
so *traces only* means the traces alone again. In the UI, the guide and the toasts this
feature is called "peak points".

## What exists

- The spectrum legend (`ac2-scene/src/spectrum/legend.rs`) is a strip of rows above the
  plot. A click sends `Msg::LegendPick(pane, TraceKey)` (`view/panes/mouse.rs`), and
  `legend_pick` (`state/queries/select.rs`) selects the curve so the keys act on it.
  An unknown id is ignored.
- P steps the display mode (off → hold → track) of the pane's own measurement
  (`display_target`, `state/panes/meas_display.rs`). That mode goes on the wire. The
  tracked peaks belong to each app: `fold_peak_tracks` (`state/link.rs`) runs
  `find_peaks` and a `PeakTracker` per measurement whose mode tracks. `spectrum_pane.rs`
  gives them to one curve only, the pane's own measurement (else the first live curve),
  and only when that curve's own mode tracks.
- `spectrum/draw.rs` draws a 1.5 px box per peak in the curve's colour, dimmed when
  stale, with a rank beside it and a list plate. `PlotChrome::peak_list()` takes the list
  and the ranks away at *traces only*; the boxes stay.
- On the transfer pane, P switches min/max for the whole app (`view.tf.extremes`,
  `commands/tf.rs`). The marks follow the selected curve, else the pane's own, and
  *traces only* keeps them.
- `AppState.view` is app-wide. `own_view` (`state/panes/ops.rs`) copies each pane's own
  values (modes, chrome, plates) into it from `panes::View`. The 3D view keeps per-pane
  state that is not layout in side maps keyed by `PaneId` (`axes3d`). Its legend row
  names the one measurement whose history it draws.

## Where the pick lives

`AppState` gets a side map `peak_points: HashMap<PaneId, PeakPick { rig: RigId, key:
TraceKey }>`, beside `axes3d`. The query is `peak_points_of(pane)`. Putting the pick in
`SpectrumView` is wrong because that view is app-wide. Putting it in `panes::View` is
also wrong: `View` is the pane's layout, rebuilt from stored layouts (`View::of` in
`meas_layout.rs`, `Layout::of`), saved in layout prefs and on the measurement. A pick is
a reading gesture, like the cursor position. It stays in this app, unsaved, and off the
wire, so there is **no PROTO bump**.

- **Layout changes.** The rule is the same after every layout change: only entries whose
  pane is still a leaf are kept. A pane closed, an undo or redo (`restore`), and a layout
  put in place by `install` (which gives fresh ids, so a gone pane's pick cannot pass to a
  new pane) all follow it. `set_prefs` is the one path that builds a layout with stored ids
  (`Layout::of`, `next_id` = max + 1), so a stored id may name different content there, and
  it clears the whole map.
- **Curve no longer on the pane.** "On the pane" means pane membership as the existing
  predicates define it (`state/queries.rs`): `live_on_pane` or `live_compared_on` for a
  live curve, and `on_pane` or `trace_compared_on` for a stored one. It does not mean
  "has a trace in the scene", because a reconnect sets the rig's data to none and a curve
  before its first frame has no legend row. A pick whose curve is no longer a member is
  dropped, and tracking falls back to the pane's own curve. The check runs at the end of
  every reducer message, daemon state events included, because other clients can change
  membership through `kind_shown`, `group_of`, `meas_hidden` and `live_drawn_in`. The
  messages that end membership include:
  - the curve unticked (C) or hidden (A);
  - another row dropped on the pane;
  - an undo or redo that gives the same pane id other content;
  - the rig drop-down switching a `PaneRig::Active` pane to another rig;
  - a mirror event from another client.

  A curve that is still a member but stale or without data keeps its pick. The check is
  not done at draw time, so a pick never comes back by itself.
- **Rig.** The pick stores its rig. It is dropped when the pane is pinned to another
  rig, when that rig's daemon numbers afresh (`keep_listed` with `reborn`), and when its
  measurement or trace is deleted (the ids the mirror lists).
- **Scene cache.** The pane's cached scene compares `peak_points_of(id)`, as it compares
  `axes3d_alpha` (`view/panes.rs`), so a pick redraws the pane even while nothing else
  changes.
- **Undo.** A legend click is not added to the undo stack, just as a selection is not.

## The click

A legend click always selects the curve, as it does today. Peak points toggle on the
click: a click on a row that is not picked picks it, and a click on the picked row clears
the pick. So a click on X just to select it again also clears X's points. That cost is
accepted because the toggle is the only way to remove points without a keyboard, on a
kiosk, and the tap that made them appear is the one that takes them away. Each pane has
at most one pick, and a pane never has two curves with peak marks.

The picked row gets a cue: the point's small box before its name, in the curve's colour.
The spectrographs' legend row gets the same cue, and every row gets the hand cursor. A narrow pane never folds
the picked row into `+N more`: the layout keeps it in the first row, so it can always be
seen and tapped.

## P and the pick

P stays the switch for the list and the ranks. It still steps the mode of the pane's own
measurement, and the list shows only while that mode tracks. The transfer pane works the
same way: P is the switch, and the pick chooses the curve.

- **The pick chooses the tracked curve.** The tracked curve is the picked one, else the
  pane's own as today. If the pane's own mode tracks, the picked curve shows rank numbers
  and the list. Otherwise it shows points with labels. If nothing is picked and the
  pane's own mode tracks, the pane's own curve shows ranks and the list, exactly as today.
- **Points need no P.** A picked curve shows its points whatever the mode. A click changes
  only what this app draws, never another client's mode.
- **Which mode gates the list.** The pane's own measurement's mode. A pane without one
  uses its first live curve's mode, as today. A picked stored trace has no mode, so the
  pane's own (or first live) mode gates its list too.
- **The list names its curve** whenever that curve is not the pane's own, as it already
  names one for several tracked curves. A list of a curve the eye does not expect has to
  say whose it is.
- **P = hold** draws the hold line of the pane's own measurement as today. The picked
  curve's points sit on the live curve beside it.

## Which peaks, and their data

Five, by the tracking rules: 6 dB prominence, peaks within 1/6 octave merged, the
parabolic vertex for narrowband curves, the band centre for RTA, no hopping. One count
and one rule set mean a point from a click and a ranked box from P are the same peak.

- **Live curves.** `Folding` gets `peak_picked: BTreeSet<MeasId>`, built per rig from the
  picks of panes showing that rig. Background rigs with pinned panes are included, the same
  way `spectrograph_shown` reaches them. `fold_peak_tracks` tracks the union of the
  tracking modes and the picked set, and its `retain` keeps the trackers that only a pick
  holds. A newly picked tracker is seeded at once in the `LegendPick` reducer, from the
  last snapshot (`RigState.data`) of the pane's rig (the message is routed to that rig, as
  `msg_rig` routes pane messages), so a stopped or stale curve gets its points from its newest frame
  without waiting for a frame that may never come. If there is no frame, nothing is drawn.
  The row's cue still shows, and the guide says the points come with the first data.
- **Stale and stopped curves** keep today's dimmed boxes and labels.
- **Captures.** A stored trace is fixed, so `find_peaks` runs once. The result is cached
  in ac2-ui state keyed by trace id and data, and invalidated where `keep_trace` and
  `keep_listed` replace or drop the trace. Peak finding is ac2-scene code, so ac2-ui's
  no-DSP rule holds. A stored spectrum carries only its scale and levels, without the
  unresolved ranges or band metadata of a live multi-resolution frame. So a capture of a
  multi-resolution spectrum gets peak points across its whole range, while its live curve
  holds no peak in an unresolved range. The difference is stated in the guide. Carrying
  the ranges would be a wire change, and a peak in a capture is still a level the capture
  really holds.

## Every G view

- **Spectrum** and **Split** (whose top is the spectrum) draw the points on the curve.
- In **Split**, the spectrum's legend above is the pane's only legend; the spectrograph's
  caption under it is inert.
- The **flat Spectrograph** and the **3D spectrograph** have the spectrum's legend strip
  above their caption, built by the same scene code (`spectrum::legend_strip`), over the
  one curve they draw: the pane's measurement's live curve, named and tagged as in the
  spectrum. A click or tap on its row sends `LegendPick(pane, Live(meas))`, as in 2D; the
  caption picks nothing. Without a measurement, or at *traces only*, there is no row. The
  flat spectrograph draws no points, because it has no level axis to put them on. The cue
  on its row shows the pick, and one G to the spectrum shows the points. In 3D they sit on
  the front row (below).
- G keeps the pick, because it belongs to the pane and not to the view.

## Look

**2D.** The existing box is kept, since a second marker for the same peak would read as a
different thing. Without the list, each box carries a label above it, on the faint dark
plate the 3D axis labels use: frequency and level in the cursor readout's forms
(`format::freq_readout`, `format::level`) with the axis unit (`1.00 kHz  −32.0 dBFS`).
The quantity note (`(band)`) is added only when the pane mixes quantities, as the cursor
readout does. When the plot's top would cut a label, it goes below its box. When two
labels overlap, the quieter peak's label is dropped and its box stays, because half a
label reads as another number. The level is the displayed one, offset included.

**Off the plot.** One rule holds in 2D and 3D: a peak outside the frequency span or the
level range gets no point and no label. A label with no point would name a place that
cannot be seen. In 2D this is today's `mark: None`.

**3D.** Points sit on the newest row. Their depth follows that row as it slides with the
rows' phase, and they are projected with the scene's `Projector`. A point is a box in
screen space the size of the 2D one, so it stays findable at any depth. It is drawn in the
theme's text colour, because colour in 3D is the colormap and a curve colour there would
read as a level. Its height is the peak's level, clamped to the top of the cell's spike
on the newest row, because a skyline cell is a coarser grid than the peak's bin. The
label keeps the peak's own value. There is no point when the newest row has no data in
that cell, when the view shows a message instead of a skyline, or when the point is
behind the eye. Labels use the axis plates and the same thinning, and a label that would
be cut is dropped whole. There is no list plate in 3D, because the labels carry the
values.

**At rest in 3D.** The axes hide at rest because nobody asked for them. Points come from a
deliberate click and are five small items, so they show without hover. They read the
pane's `view.chrome`, not the `Bare` that the scene forces while the axes are hidden. An
exported PNG shows them as on screen.

## T on every pane

`PlotChrome::peak_list()` becomes `peak_marks()`, false at *traces only*. The list,
ranks, boxes, labels and the transfer pane's ▲/▼ marks all go together. **This changes
today's behaviour** on both the spectrum and the transfer pane, because the same key on
the same kind of mark must do the same thing. *No grid* keeps peak points, since they are
values like the axis labels. In 3D, *traces only* hides them even while the axes are
revealed. The doc comment on `peak_list` and the guide's "the boxes stay on the curve" and
"keeps the marks" are updated in the same change. The *no grid* toast (`PlotChrome::name`)
becomes "no grid or resolution marks". "Marks" there means the resolution and window marks,
and the old wording would be read as covering peak points.

## Touch

Legend and caption hits are egui `clicked()` responses, and a touch tap produces one. A
kiosk with no keyboard can pick, clear and read points. A Pi has no 3D view, so only the
2D and flat-spectrograph paths apply there.

## Transfer pane

The same side map holds the transfer pane's pick. A transfer legend click selects as
today and picks or clears the curve. Min/max reads the picked curve, else the pane's own,
so on both pane kinds "the marks follow the curve you picked in the legend". The on/off
switch stays P, and it stays app-wide (`view.tf.extremes`). With P off, a transfer pick
shows nothing: ten marks and a comb line would crowd a pane that is clicked for many other
reasons (offset, delay, compare). So a transfer legend click while min/max is off only
selects and leaves the pick alone. The picked row's cue shows only while min/max is on,
because a cue that changes nothing would look like a fault.

## Events

| event | pick | selection | drawn |
|---|---|---|---|
| click row X, nothing picked | X | X | X's points (ranks + list when own mode tracks) |
| click row Y, X picked | Y | Y | Y's points; the tracked curve is Y |
| click picked row X | none | X | no points; the tracked curve is the pane's own |
| click the legend row (flat / 3D) | toggles the pane's measurement | as a legend click | 3D front-row points; flat: cue only |
| G / Shift+G | kept | kept | the same pick in the next view |
| P (own measurement's mode) | kept | — | list and ranks on or off; points otherwise |
| T → traces only | kept | — | no peak mark of any kind |
| X stale or stopped | kept | — | dimmed, from its newest frame |
| X has no data yet (still on the pane) | kept | — | nothing; the cue shows on the flat / 3D legend row only once the curve has one |
| X unticked (C), hidden (A), replaced by a drop, rig drop-down on an Active pane | dropped | as today | the pane's own curve tracked |
| pane closed, `install`, undo / redo | kept only if the pane is still a leaf and X is still on it (membership) | — | — |
| `set_prefs` (stored layout) | cleared | — | — |
| pane pinned to another rig, daemon reborn, X deleted | dropped | as today | — |
| another app's click | unchanged here | — | — |

## Tests

Scene first (`ac2-scene`, headless text asserts):
- `spectrum/tests/tracking.rs`: a picked curve with the mode off gives five labels
  (`1.00 kHz  −32.0 dBFS`). With the mode on track it gives ranks and the list instead.
  *No grid* keeps the points and *traces only* removes every box, rank, label and list.
  Overlapping labels drop the quieter peak's label and keep its box. A label near the top
  moves below. A label carries the quantity note only on a mixed pane. A peak outside the
  level range has no point. The offset is included.
- `spectrograph_3d/tests/scene.rs`: points lie on the sliding front row, inside the plot.
  A point is clamped to its cell's spike top and its label keeps the peak's value. An
  empty cell or a message gives no point. Points show at alpha 0 at *full* and none show
  at *traces only* while revealed. The legend row picks and carries the cue, at rest too;
  there is none at *traces only* or without a measurement.
- `spectrum/tests/` legend: the picked entry stays out of `+N more` and shows the cue.
- Transfer scene: *traces only* hides the ▲/▼ marks (the existing assertion flips).

Reducer (`ac2-ui`: `state/tests/meas_display.rs`, `cam3d.rs`, `tf_group.rs`,
`layout/`):
- A pick selects and picks. Picking the same key again clears the pick and keeps the
  selection. Two panes on one measurement pick independently. A pick sends no request and
  adds nothing to the undo stack.
- `Folding.peak_picked` reaches `fold_peak_tracks`, including for a background rig's
  pinned pane. A pick-only tracker survives `retain`, and a pick on a stopped measurement
  is seeded from `RigState.data`.
- Clearing: pane close, `install`, `set_prefs` with ids reused, re-pinning to another rig,
  a `reborn` daemon and a deleted trace each drop the pick. Undo keeps it while the pane
  still shows the curve. An unknown id is ignored.
- A pick whose curve leaves the pane (C untick, A hide, a drop, an undo giving the id other
  content, the rig drop-down on an Active pane) is dropped, and tracking goes back to the
  pane's own curve. A daemon state event from another client that removes X from the pane
  (hidden, regrouped, kind no longer shown) drops it too. A reconnect that leaves X on
  the pane with no data keeps it.
- The list's gate: the own measurement's mode, else the first live curve's, also for a
  stored-trace pick. The list names a curve that is not the pane's own. The transfer cue
  shows only while min/max is on, and a transfer legend click while min/max is off
  selects without touching the pick.
- A capture's cached peaks are invalidated when its data is replaced.
- `ac2-e2e`: from an empty daemon, put two spectra into one pane, click the second curve's
  legend row and read its point labels, click it again and see none, pick it again, then
  T twice and see none.

## User guide and keys

No new key, so the key tables and hint lines are unchanged.
- *The spectrum legend*: "A click selects the curve and shows its **peak points**, its
  five peaks with frequency and level. A click on the picked row clears them (**A** hides
  the curve)."
- *Peaks: hold and tracking*: P is the switch for the list and ranks, and they go to the
  picked curve, else the pane's own. Peak points need no P and belong to this app.
  *Traces only* hides every mark. A stopped curve shows the points of its newest frame,
  and before any data there are none ("no data yet"). A capture of a multi-resolution
  spectrum gets peak points across its whole range; its live curve skips the unresolved
  ranges.
- *Spectrograph* and *3D spectrograph*: a click or tap on the legend row picks the
  measurement. In 3D the points sit on the front row. Rework the "no cursor" bullet.
- *Min / max tracking*: the picked curve, else the pane's own; *traces only* hides the
  marks.
- Help (`view/help.rs`): a mouse row beside `PLATE_MOUSE`, "Click legend row: peak points;
  again: clear", in `Scope::Spectrograph3d` too.
- `docs/design/spectrograph-3d.md`: its "No cursor" paragraph and P row point here.

## As built

Where the implementation differs from the text above, and why:
- **Cue.** The hollow box is drawn centred on the legend row's swatch, not in extra width
  beside it: a legend
  row keeps its width, so a pick never changes which entries fit before `+N more`.
- **2D plates.** A point's label sits on the plot background colour at the axis plates'
  alpha with the theme's text colour, not on the 3D field's dark plate: in a light theme a
  dark plate with dark text would not read. The 3D view keeps the dark plate and the dark
  theme's text, since its field is dark in every theme.
- **3D points** show only when the pick is the pane's own measurement (its legend row's); the
  3D view draws no compared curve, so another pick has nothing to stand on.
- **Transfer.** The pick chooses min / max's curve only; the mark readout and the cursor
  jumps (Alt+P/N/D) still follow the selection, as before.
- **Daemon run.** A pick stores the daemon incarnation it was made under and is dropped
  when the rig's mirror names another, instead of on `keep_listed`'s `reborn` (which it covers): a
  restarted daemon is usually reached after the link went down, and then the mirror arrives
  with no previous incarnation to compare, so `reborn` is false. While a rig's link is down
  (no mirror) membership is unknown and the pick waits, which is how a reconnect to the same
  daemon keeps it.
- **Help.** One row, "Click legend row: Peak points; again: clear", in the transfer,
  spectrum (which covers the flat spectrograph) and 3D scopes.
- **Spectrographs.** First built with the caption as the click target (no legend); the
  operator missed the legend rows there, so both spectrographs now carry the spectrum's
  legend strip and the caption is text only.
- **Test files.** The scene tests live in `spectrum/tests/points.rs` and
  `spectrograph_3d/tests/points.rs`; the reducer tests in
  `state/tests/meas_display/peak_points.rs` and `state/tests/rigs/rig_peak_points.rs`; the
  e2e test in `ac2-e2e` `panes/peak_points.rs` fires the simulated rig's pink noise so the
  spectra have peaks to label.
