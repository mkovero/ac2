# 3D spectrograph

Status: design (draft). Builds on [spectrograph.md](spectrograph.md): same history, same
level range, a different picture of it.

## Purpose and what the operator sees

The 2D spectrograph is the reading instrument: colour is level, a click reads a point. The
3D spectrograph is the *seeing* one: level is height, so a feedback ring, a resonance that
keeps returning or a band that swells reads as a shape across the room without decoding a
colour scale. It is meant to be left running (on a kiosk too) and to look good doing so.

**G** in the spectrum pane steps spectrum → spectrum + spectrograph → spectrograph →
**3D spectrograph** → spectrum; **Shift+G** back. The 3D view takes the whole pane, like the
2D one alone (full screen with **W** / F11). It shows the pane's measurement only.

The picture: a dark floor seen from a low camera (8° above it by default). Frequency runs
left → right across the floor on a log scale over the shared frequency axis's current span
(the span the other panes show; set there). Time runs from the front edge (newest) back into
the distance, over the measurement's spectrograph history (**Shift+B**, 10 – 120 s). On every
cell of that grid stands one thin vertical spike whose height is the cell's level within the
pane's level range: a loud band is a tall bright skyline, the noise floor a low grass. Rows
glow through one another (additive light, no hidden lines), fading and fogging with depth,
so the front row is the crisp "now" and the history a haze behind it.

Name everywhere (view toast, caption, hint tooltip, help, user guide, CLI output): **3D
spectrograph**. Never "waterfall": in acoustics that is a cumulative spectral decay plot.

No cursor and no readout in this view: which spike, at what depth, a pointer is over is
ambiguous in a perspective with no hidden lines, and a value that could belong to the row in
front or behind is a misreadable readout. The 2D view, one G back, reads values.

## The look

Concrete defaults (constants in `ac2-scene::spectrograph_3d`, tuned against the golden):

- **World.** Floor x ∈ [−1, 1] (frequency), z ∈ [0, 1.2] (age, 0 = newest), heights
  y ∈ [0, 0.7]. A wider-than-deep floor keeps a 10-octave span readable from the default
  camera; 0.7 lets a full-range spike rise above the far rows without leaving the frame.
- **Spike.** A camera-facing vertical quad, from the floor to its height. Width 35 % of the
  column pitch in world units, never under 1 device pixel (thinner aliases into shimmer as
  rows scroll; below 1 px the quad stays 1 px and its alpha scales by the lost coverage).
  Along the spike, alpha rises from 0.25 at the foot to 1.0 at the tip (`0.25 + 0.75·t²`), and
  the top 6 % is 1.5× brighter: the tips carry the light and feed the bloom, as in the
  reference.
- **Level to light.** Normalised level `n` (0 at the bottom of the level range, 1 at the
  top, clamped; `n = 0` draws nothing) sets height `n·0.7` and intensity `0.3 + 0.7·n`, so
  the grass is dim as well as low.
- **Depth.** Each row is multiplied by `exp(−2.0·age/1.2)` (the back row at 13 %), then by
  distance fog `exp(−0.35·d)` from the camera: depth reads by falloff even while orbiting.
- **Floor band.** The floor is one additive quad: a soft band brightest along the front
  edge (0.12 of the palette's mid colour) fading with the same depth curve, plus a 1 px "now"
  edge line at 0.35. It gives the skyline a ground to stand on without a grid.
- **Bloom.** Spikes are drawn a second time into a quarter-resolution offscreen target,
  blurred by a separable 9-tap Gaussian (σ = 2 quarter-res px, ≈ 8 px at full size) and
  added over the pane at 0.6 (ice) or 0.35 (colour maps, so hues stay legible).
- **Palette.** By default the measurement's spectrograph colour map, through the same LUT as
  the 2D view (so a level is the same colour in both). New map **ice**: near-black blue
  (#0B1A2B) through #2D6FA8 to pale ice blue (#CFE8FF), rising monotonically in lightness
  and stopping short of white, as `grey` does. Ice + bloom is the holographic look.
- **Field.** The plot area is always near-black (#05080C), in the light theme too: additive
  light only reads on a dark field (`Blend::Additive`'s own note).
- **Motion.** Rows glide back continuously: the scene gives a sub-row phase from the newest
  frame's capture time, so a new row slides in instead of the whole floor jumping a row.
  Spike heights are the data as received, never interpolated between frames.

## Data path

No daemon data, no new topic: the source is the `SpectrographHistory` the UI already keeps
for the measurement while any spectrograph view is shown (`SpectrumMode::spectrograph()` is
true for the 3D view, so history keeps building across G).

`ac2_scene::spectrograph_3d::Skyline` (pure, unit-tested) is the decimated grid:

- **Columns**: C log-spaced columns over the intersection of the pane's frequency span and
  the history's row range. A column is the highest level among the history rows it covers,
  as the 2D view's max-per-pixel: a narrow tone keeps its level.
- **Rows**: R time buckets over the history span, each the highest level of its slots, so a
  short event never drops out between buckets. Empty slots (gaps) stay empty; a bucket with
  no frame draws no spikes.
- **C × R = 192 × 96** (Full tier). 192 columns over 10 octaves is ≈ 1/19 oct: finer than
  the 1/12 oct a tuning engineer reads by eye, and at a typical 1200 px pane a 6 px pitch
  leaves visible gaps between 2 px spikes, which is what makes it read as a skyline rather
  than a wall. 96 rows: at 8° pitch the floor's depth projects to ~100 px, ~1 px a row, so
  more rows merge into the same haze and only cost fill; tilted up, 96 rows are 5 px apart
  and still distinct. At 30 s a row is 0.31 s (9 slots).
- **Incremental**: each new history frame folds into the newest bucket (≈ C × rows-per-column
  max operations, ~1000) and replaces that bucket's `Arc<[f32]>`; a full bucket advances the
  ring. A change of frequency span, tier, history reset or span length rebuilds from the
  history (≤ slots × rows, ~1 M max ops, < 2 ms, once).
- Lives beside the history in the UI state, updated only while the 3D view is on screen;
  on entry it is rebuilt from the history.

The scene carries the ring as `Vec<Option<Arc<[f32]>>>` plus scroll and phase, exactly as
`Heatmap` does, so the renderer's upload-by-identity applies unchanged: steady state uploads
one 192-float row (768 B) per frame.

## Scene / plot split

`ac2-scene` (display truth, headless): the skyline, the camera (yaw, pitch, distance, its
clamps and steps), the 4×4 view-projection matrix, normalised heights from the level range
and the measurement's offset, the projected positions and texts of the axis labels and which
of them survive culling, the caption, the chrome level, the tier. New primitive
`Skyline3d { ring, scroll, phase, cols, rows, view_proj, range, colours, look, tier }` in
`primitives.rs`.

`ac2-plot` (pixels only): an R32Float C×R ring texture written row by row, the existing
colormap LUT, one instanced draw of C×R spikes whose vertex shader reads its cell with
`textureLoad` (`instance_index` → column, row; no storage buffers: the WebGL2-class limits
the Pi gets have none), the floor quad, the bloom chain. No depth buffer and no sorting:
additive blending is order-independent. New shader `skyline.wgsl`, `skyline.rs`; the bloom
targets are allocated per pane size and live in the renderer slot.

Bloom runs in the egui callback's `prepare` (which has the command encoder): spikes into the
quarter-res target, blur H, blur V. `paint` then draws the spikes at full resolution straight
into egui's pass and adds the blurred texture over the pane rect. No full-resolution
offscreen copy: on the Pi the composite costs one bilinear sample per pane pixel.

## Camera and controls

Orbit camera around a fixed target at the floor's centre, 0.15 up:

| | default | range | key step | mouse |
|---|---|---|---|---|
| yaw | 0° (looking along time, frequency left → right) | ±80° | 15° | 0.3°/px drag x |
| pitch | 8° | 2° – 75° | 5° | 0.2°/px drag y |
| distance | framing the floor at 35° vertical FOV | 0.35× – 2.5× default | ×1.25 | wheel |

- **Zoom is a dolly**, not FOV: moving the camera keeps the perspective cue (near spikes
  larger) that conveys depth when nothing hides anything; a narrowed FOV from far away
  flattens the skyline into a telephoto strip.
- **Yaw is clamped** at ±80°: past 90° frequency would read right to left. **Pitch** stops
  above the floor (spikes edge-on vanish) and below top-down (that is the 2D view).
- Key and wheel steps ease through `anim::Spring`, as the frequency axis does; repaints run
  only while the spring settles. Drag follows the pointer directly.

**Keys** — the 3D view has its own key scope, `Scope::Spectrograph3d`, as the IR view has
`Scope::Ir`, so its hint line names camera keys. It takes over the frequency-navigation
keys the way the IR picture does (`ir_nav::is_nav`): the shared frequency axis is changed
from a pane that shows it as an axis, not from a perspective floor.

| key | in the 3D view | command (`keys.toml`) |
|---|---|---|
| ← / → | orbit left / right | `pan_left` / `pan_right` (taken over) |
| PageUp / PageDown | tilt up / down | `tilt_up` / `tilt_down` (new) |
| I / O | dolly in / out | `zoom_in` / `zoom_out` (taken over) |
| Home | reset camera | `reset_view` (taken over) |
| S, R, G, Shift+G, Shift+B | as in the spectrum scope | bound in the new scope too |

PageUp/PageDown: ↑/↓ are the reserved stimulus level, Ctrl+↑/↓ and Ctrl+I/O stay the level
axis (they set the height range, still meaningful), Alt+↑/↓ the offset; PageUp/PageDown are
bound only in the walk scope. No Ctrl chord, nothing on Esc. Shift+←/→ (cursor) still move
the shared cursor other panes show; this view draws none. B (style) and P (peaks) are not
bound in the scope: they change the curve, which this view does not draw.

Hint line (`hints(Scope::Spectrograph3d)`): orbit, tilt, zoom, reset camera, view, start/stop,
maximise. Each new or taken-over key also gets its key-table entry, a help row and a
user-guide row (a *Spectrum / RTA: 3D spectrograph* key table and a *3D spectrograph*
section after *Spectrograph*) in the same change.

**Mouse** (`view/panes/mouse.rs::navigate`, a branch before the frequency-axis handling, as
the IR and strip branches are): drag orbits (yaw, pitch) instead of panning frequency; wheel
dollies instead of zooming frequency; Ctrl+wheel and Shift+wheel keep zooming and panning the
level range; a click only focuses. This does not collide with putting curves into a pane:
that drag starts on a list row and is handled by `row_drag`, never by the pane's own drag
response, and the legend plate keeps the pointer first through `plates_mouse`.

## Persistence and wire

- `SpectrumMode::Spectrograph3d` (scene) and `SpectrumView::Spectrograph3d`
  (`"spectrograph_3d"`, wire) join the G chain.
- **Camera saved with the layout** (recommended): `LayoutPane.spectrum_camera:
  SpectrumCamera { yaw_deg: i16, pitch_deg: i16, zoom_pct: u16 }`. A camera is a view of
  the pane like its level range, which already travels in `LayoutPane.levels`; saving it
  means a kiosk pinned to a pane, or a session reopened, comes back framed the same, and
  another client choosing the measurement sees the same angle. Integers keep it lean and make
  equality exact, so a layout re-sent with an unchanged camera is dropped by `send_layout`. A
  drag sends on release, a key step on the step, through the existing one-in-flight
  `meas.layout` queue — never per pointer move.
- **Colour option in the measurement's display**: `Colormap::Ice` added to the colour maps
  in `SpectrumDisplay.spectrograph_colours`, set where the others are (palette, `ac2 meas set
  <meas> --colours ice`). One control per concept: the 3D view uses the measurement's map
  like the 2D one, and ice also works in 2D (a monotone dark-room map). The bloom strength
  follows the map (ice 0.6, others 0.35); no separate "glow" switch.
- One `PROTO_VERSION` bump, `fixtures/protocol/WIRE_LOCK` and the Python fixtures updated;
  the session manifest version bumps since layouts and displays are in it.

## Performance

Budget (pane of 1920×1080, 3D view shown, live 30 frames/s):

| | desktop GPU (Full) | Pi 4 / CM4 V3D (Lite) |
|---|---|---|
| CPU per frame (fold, scene, labels) | ≤ 0.3 ms | ≤ 2 ms |
| GPU per frame (spikes, bloom, composite) | ≤ 1.5 ms | ≤ 12 ms (under half a 30 fps frame, leaving egui its share) |
| upload per frame | 1 row, 768 B | 1 row, 512 B |

Fill dominates: worst case C×R spikes × 2 px × mean 40 px is ~1.5 M fragments at Full,
~0.25 M at Lite, plus one composite pass over the pane.

**Tiers**: Full 192×96 + 9-tap bloom; Lite 128×48 + 5-tap bloom; Bare 128×48, no bloom.

- **Start tier by adapter**: `Rasterizer::Software` → Bare (it already gets the reduced frame
  rate and no drop shadows); GL backend (the Pi's V3D, running on WebGL2-class limits) →
  Lite; otherwise Full. Known at start, no measurement needed, deterministic in tests.
- **Step down by measured rate**: while the 3D view is shown and frames are offered (live
  data or a settling camera), the paint rate achieved is compared with the rate offered (the
  stream's frame rate, capped at the frame interval). Achieved < 75 % of offered over a 3 s
  window steps down one tier. No GPU timers (GLES has none); a GL present blocking on a
  behind GPU shows up as late frames. Never steps up within a run: no oscillation; a restart
  starts from the adapter tier again.
- The caption states the resolution the picture is made of (`1/19 oct · 0.31 s`), so a
  coarser tier is never a silent change in what a spike stands for.
- **Not visible, no cost**: a pane not on screen builds no scene, so nothing folds, uploads
  or draws; the skyline is rebuilt from the history when it reappears. History itself costs
  what the 2D view's does today.
- Texture sizes are far inside the 2048 limit; no float render targets (bloom is
  Rgba8Unorm, filterable everywhere), no storage buffers, no depth attachment.

## Tests

- `ac2-scene` (`spectrograph_3d_tests.rs`): column decimation (tone in one column, wide low
  bands, invalid bands), bucket max, gaps empty, holds, reset on span / grid / scale /
  frequency-span change; folding frame by frame equals a rebuild from the history (property
  test); phase from capture time; heights from range and offset (bottom → 0, top → full,
  clamp); camera clamps, steps, reset, dolly bounds; projection of the floor corners into
  the rect at the default camera; label texts, culling behind the camera and off the rect,
  overlap thinning; T: Full (floor grid at decades and 10 s ticks, labels), NoGrid (labels),
  TracesOnly (spikes and floor band only); caption; tier from adapter; step-down from a
  synthetic interval sequence (and none while idle).
- `ac2-plot` (`tests/it/render.rs`, `golden.rs`): a single spike lands at the pixel the scene
  projects; ring rows upload by identity (replace, scroll, fresh renderer equal); an empty
  cell draws nothing; goldens `skyline_colormap`, `skyline_ice` (Full) and `skyline_bare`,
  320×200, blessed on lavapipe at the line tolerance (24 / 0.2 %: the blur's float math
  differs at edges across rasterisers like AA does).
- `ac2-ui`: G / Shift+G through four views; scope and hint line in the 3D view; ←/→, PgUp/PgDn,
  I/O, Home move the camera and leave the frequency axis; drag and wheel messages; camera sent
  on release only; Esc does not touch it.
- `ac2-e2e`: `spectrograph_3d_from_an_empty_daemon` (session, spectrum from the palette, G ×3,
  the fake rig's noise fills the skyline, camera keys, the camera survives a second client
  choosing the measurement, T, ice from the palette, G back to spectrum); GPU snapshot
  `spectrograph_3d` in `tests/it/ui/views.rs` from the pinned 25 s sweep with a gap.

## Stages

1. **v1a**: wire (view, camera, ice), skyline, camera, scene, spikes + floor without bloom
   (the Bare tier), keys, mouse, hints, user guide, tests.
2. **v1b**: bloom, Lite/Full tiers, step-down, goldens per tier. Ships with 1a in one deploy:
   v1 is "pretty and fast" together.
3. Later, not planned yet: a cursor frequency line on the floor (no value), moving the orbit
   target, the peak-hold line as a ghost row.

## Open questions (with recommendations)

1. **Camera in the layout or this app's only?** Layout (above): kiosks and sessions restore
   the framing; cost is a field and a layout send on release.
2. **Ice as a colour map or a 3D-only "glow" style?** Colour map: one control for colours,
   useful in 2D, no new setting. Risk: ice has fewer hues than viridis for fine level steps in
   2D; acceptable, the user picks it.
3. **Floor over the shared frequency span or always the full history range?** Shared span,
   as the 2D view: zooming a band elsewhere and pressing G shows that band in 3D, with more
   columns per octave.
4. **Newest row at the front or the back?** Front: the live row is crisp and closest, the
   history recedes into fog, as in the reference.
5. **Auto step-up?** No: a tier that stepped down because the Pi was busy would flap.
6. **Double-click / double-tap to reset the camera?** Not in v1 (Home does it); revisit for
   the touch kiosk.
7. **Dark field in the light theme** breaks the theme for one view. Recommend accepting it:
   additive light on white is invisible (the goniometer's embers have the same need; check how
   that pane handles the light theme and match it).
