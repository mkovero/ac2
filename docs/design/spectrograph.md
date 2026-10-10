# Spectrograph

Status: implemented (PLAN §3.4 "Spectrograph (one, GPU texture)", §8.1 "scrolling texture +
colormap LUT"). User-facing description: `docs/user-guide.md` → *Spectrograph*.

## What the operator sees

**G** steps the spectrum pane's views, as it steps the SPL pane's: spectrum → spectrum +
spectrograph → spectrograph alone → spectrum (remembered in `ui.toml` as `spectrum_view`).
The split puts the spectrum in the top 40 %, the spectrograph of the pane's measurement under
it; alone, the spectrograph takes the whole pane under the banners (with **W** / F11, the
whole screen), its caption carrying the spectrum's window and calibration and its
freshness tag. The order adds the spectrograph beside the curve it is made of first, so its
history (kept only while it is shown) builds while the spectrum is still in view; it then
takes the pane with that history, and the last step hides it. Frequency runs across on the spectrum's own log axis — the
same plot left and right edges, so a frequency is at the same pixel in both, and zoom and
pan move both. Time runs down, the newest frame at the top, over the history length
(**Shift+B**: 10 → 30 → 60 → 120 s, default 30 s). Level is colour, through the theme's
colormap (viridis: perceptually uniform, readable with the common colour-vision
deficiencies), over the pane's level range for the measurement's scale — the range the level
keys (Ctrl+I/O, Ctrl+↑/↓, Shift+Home, Ctrl+Home) and the mouse already move. A colour bar on
the right shows that range with its dB labels. A click in the spectrograph sets the cursor's
frequency and time; the line above the plot reads `1.00 kHz · 4.2 s ago · −32.0 dBFS`.

Why the split comes first, not a replacement: the curve is what the level axis and the cursor were
built around, and an operator reading a spectrograph wants the instantaneous curve beside it
(Smaart does the same). The colour bar sits beside the spectrograph, and the spectrum above
keeps the same right margin, so the frequency axes line up.

## Data: no wire change

The source is the spectrum / RTA frames the pane already receives (live spectrum columns are
display-sized log columns since PROTO 14, `docs/design/flow-control.md`). No new DSP, no new
topic, no `PROTO_VERSION` bump, nothing in the session format: the history is display state
of the UI, like peak hold, kept only while the spectrograph is shown.

`ac2_scene::spectrograph::SpectrographHistory` (pure, unit-tested):

- **Frequency rows.** On the first frame of a grid the columns' band edges are mapped once
  onto rows evenly spaced in log frequency, 96 per octave (the daemon's display columns per
  octave), from the lowest positive edge to the highest, at most 2048. Each row is the
  highest level among the columns overlapping it: a tone in one narrow column keeps its
  level, a wide column (a third-octave band, a low single-bin FFT column) fills every row it
  covers. Invalid RTA bands and NaN bins are no value.
- **Time slots.** 900 slots over the history length (33 ms at 30 s), placed by the frame's
  capture time on the daemon clock — not by arrival, so link jitter does not stretch the
  picture. Two frames in one slot keep the higher level per row (what a pixel over several
  slots shows too).
- **A frame stands for the time since the previous one.** A long FFT updates a few times a
  second; and the link does not deliver a frame that draws the same as the previous one (a
  steady spectrum, digital silence). Both leave slots without a frame that are not missing
  data, so they hold the previous frame — the same `Arc`, no copy.
- **Gaps.** A stream the client marks STALE, or a measurement that is not running, marks a
  break: the slots until the next frame stay empty and draw as the plot background. More than
  a history between frames, or the capture clock going back, starts over. A new grid (FFT
  length, band fraction) or level scale (calibration on or off) starts over: their rows or
  units cannot share a colour field. A change of history length starts over.
- **Memory.** One `Arc<[f32]>` per distinct frame, at most 900 per measurement: about
  4–6 MB for a 65 536-point spectrum at 30 frames a second, less for an RTA or a long FFT.

## Rendering

`ac2-plot` already had a ring heatmap (R32Float texture, colormap LUT texture,
`textureLoad` only — R32Float is not filterable without an optional feature). What changed:

- **Columns by identity instead of upload lists.** A `Heatmap` carries its whole ring as
  `Vec<Option<Arc<[f32]>>>`. The renderer keeps the `Arc` it last uploaded at each ring
  position and uploads a position only when the scene holds another one there (pointer
  comparison is sound because the renderer's own reference keeps the allocation alive). A
  scene can be built, dropped (egui discards passes), rebuilt at another size or painted by
  two renderers without losing or repeating an upload; a new frame costs the slots it wrote,
  one `write_texture` per run of adjacent changed slots. The earlier "uploads since the
  previous frame" list would have lost columns whenever a built scene was not prepared.
- **Time up.** `HeatmapAxes::TimeUp` draws columns bottom to top and rows left to right, so
  frequency is horizontal under the spectrum.
- **Max over a pixel.** Where a pixel covers several cells the shader shows the highest
  (up to 4 taps per axis, spread evenly beyond that), like `max_per_pixel` for the curve:
  30 s of history in 300 pixel rows is 3 slots a pixel, and nearest sampling would let short
  events flicker in and out as the picture scrolls. Magnified cells stay nearest-neighbour.
- **Zoom and pan move the rect, not the data.** The heatmap's rect is where the rows' full
  frequency span lies on the current axis (often wider than the plot), clipped to the plot.
  The level range is a uniform. Neither re-uploads.
- **Idle.** The pane's scene is rebuilt only when its frames change (as before), so nothing
  is uploaded or drawn while nothing new arrives.
- **Pi 4 class limits.** The ring texture is rows × slots ≤ 2048 × 900 texels (V3D allows
  4096 per side); the LUT is 256 × 1.

The colour bar is a one-column heatmap of the level range through the same LUT, so the bar
and the picture cannot disagree.

## Tests

- `ac2-scene` (`spectrograph_tests.rs`): row resampling (narrow tone, wide low columns,
  invalid bands), ring capacity and scroll, hold between frames sharing one allocation, gaps
  after a break, overflow and clock reset, max within a slot, grid / scale change, the scene
  (rect on the axis, same columns after zoom, colour range with offset, stale opacity, axis
  labels, colour-bar labels, caption, cursor readout, empty states).
- `ac2-plot` (`tests/it/render.rs`): scroll is a rotation, columns follow identity (replace,
  empty, fresh renderer equal), NaN transparent and release, time-up is the transpose, a
  pixel shows its highest cell; the golden `heatmap_scroll` is unchanged.
- `ac2-ui`: `tests/embedded.rs` `spectrograph_from_an_empty_daemon` (session, spectrum
  from the palette, G, the rig's noise fills it, cursor readout, C, the history length, stopped caption,
  G alone, W full size, level keys, G clears); GPU snapshots `tests/it/ui.rs` `spectrograph`
  and `spectrograph_alone` from a pinned 25 s sweep with a gap.

## Open

- Only the pane's measurement is drawn; a second spectrograph (or one per measurement in a
  stack) is not planned (PLAN: "one").
- The history is not kept across app restarts or while hidden; the length and on/off are
  not saved in `ui.toml`.
- Only viridis; a second map (magma for the light theme) would need its own fitted table and
  the theme switch.
- Time is anchored to the newest frame, not to the wall clock: a stalled stream does not
  scroll (no repaints while idle), and the STALE banner says how old the newest frame is.
