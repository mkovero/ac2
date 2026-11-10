# 3D spectrograph

Status: design (draft, review round 1 applied). Builds on [spectrograph.md](spectrograph.md):
same history, same level range, a different picture of it.

## Purpose and what the operator sees

The 2D spectrograph is the reading instrument: colour is level, a click reads a point. The
3D spectrograph is the *seeing* one: level is height, so a feedback ring, a resonance that
keeps returning or a band that swells reads as a shape across the room without decoding a
colour scale. It is meant to be left running (on a kiosk too) and to look good doing so.

**G** in the spectrum pane steps spectrum → spectrum + spectrograph → spectrograph →
**3D spectrograph** → spectrum; **Shift+G** back. The 3D view takes the whole pane, like the
2D one alone (full screen with **W** / F11), and draws the pane's measurement only.

The picture: a dark floor seen from a low camera (8° above it by default). Frequency runs
left → right across the floor, log, over the shared frequency axis's current span (set from a
pane that shows it as an axis, or one G back). Time runs from the front edge (newest) back
into the distance over the measurement's history (**Shift+B**, 10 – 120 s). On every cell
stands one thin vertical spike whose height is the cell's level within the pane's level
range: a loud band is a tall bright skyline, the noise floor a low grass. Rows behind show
through the gaps (no hidden-line removal), dimming and fogging with depth, so the front row
is the crisp "now" and the history a haze behind it.

Name everywhere (view toast, caption, hint tooltip, help, user guide, CLI, `keys.toml`
section, wire): **3D spectrograph**, spelled `spectrograph_3d` in identifiers and on the
wire. Never "waterfall": in acoustics that is a cumulative spectral decay plot.

No cursor and no readout: which spike, at what depth, a pointer is over is ambiguous in a
perspective without hidden lines, and a value that could belong to the row in front or
behind is a misreadable readout. The 2D view, one G back, reads values.

## The look

Settled by a look spike (a CPU emulation of the spike pass, Rgba8-quantised, on a synthetic
pink-noise history and a sparse tonal one, at the default and a tilted camera): **additive
spikes saturate** — with rows ~1 px apart at the default camera, dozens of rows sum into
every pixel, and pink noise (the ordinary PA signal) becomes a white wall; normalising each
row's gain by its projected row spacing only delays it. **Max blending** (each pixel keeps
the brightest spike over it) keeps the skyline's shape for any density; an additive copy at
low gain feeds only the bloom, so where many rows stand together the haze is denser, and
that sum is clamped before it is blurred. Constants in `ac2-scene::spectrograph_3d`:

- **World.** Floor x ∈ [−1, 1] (frequency), z ∈ [0, 1.2] (age, 0 = newest), heights
  y ∈ [0, 0.7]. Wider than deep keeps 10 octaves readable from the default camera; 0.7 lets
  a full-range spike rise above the far rows without leaving the frame.
- **Spike.** A camera-facing vertical quad from the floor to its height. Width 35 % of the
  projected column pitch, between 1 and 3 device px (thinner aliases into shimmer as rows
  scroll; wider costs fill and stops reading as a spike). Brightness along the spike rises
  from 0.25 at the foot to 1.0 at the tip (`0.25 + 0.75·t²`), the top 6 % at 1.5×: the tips
  carry the light and the bloom, as in the reference.
- **Level to light.** Normalised level `n` (0 at the bottom of the level range, 1 at the
  top, clamped; `n = 0` draws nothing) sets height `0.7·n`, colour `LUT(n)` and intensity
  `0.3 + 0.7·n`, so the grass is dim as well as low.
- **Depth.** Brightness × `exp(−2.0·age/1.2)` (the back row at 13 %) × `exp(−0.35·d)` from
  the eye. Under max blending factors are ignored, so depth scales the colour: a level keeps
  its hue at any depth but not its lightness. Hence no claim that a colour reads the same as
  in 2D; the level reference is the colour bar and the ceiling line (*Axes*).
- **Floor band.** One additive floor quad, brightest along the front edge (0.12 of
  `LUT(0.5)`), fading with the depth curve, plus a 1 px "now" line at 0.35.
- **Bloom.** The spikes again, into a quarter-resolution Rgba8Unorm target, additive at gain
  0.12 (coverage-scaled where a spike is under 1 quarter-res px), blurred by a separable
  9-tap Gaussian (σ = 2 quarter-res px, ≈ 8 px full size), added over the pane at 0.6 (ice)
  or 0.35 (colour maps, so hues stay legible). Rgba8 drops far, dim rows below one LSB from
  the bloom; acceptable, they are the haze's edge and still drawn sharp. Float targets would
  need extensions the GLES downlevel limits do not promise.
- **Blend space.** Max is per channel and monotonic, so it is the same in sRGB and linear
  targets. The bloom composite adds encoded values when egui's target is not sRGB, slightly
  brighter than a linear sum; the goldens fix the result.
- **Palette.** By default the measurement's spectrograph colour map, through the 2D view's
  LUT. New map **ice**: #0B1A2B through #2D6FA8 to #CFE8FF, monotonic in lightness, stopping
  short of white as `grey` does. Ice + bloom is the holographic look.
- **Field.** The plot rect is always #05080C, in the light theme too, and the labels, colour
  bar text and caption inside it use the dark theme's text colours: light added to a white
  field is invisible.
- **Motion.** Rows glide back continuously: the phase is the newest bucket's fill fraction
  (slots filled ÷ slots per bucket), from the data, never a local clock, so a stalled stream
  stands still. Heights are the data as received, never interpolated.

Spike PNGs (not committed): `look/sheet_pink.png`, `sheet_tonal.png`,
`sheet_max_vs_sumbloom.png` in the session scratchpad; the script stays out of the tree, its
pink-noise and tonal cases become goldens (*Tests*).

## Axes and level reference

Labels are scene truth: anchors projected through the camera, culled behind the eye or off
the rect, thinned where they would overlap.

- **Frequency** along the front edge, the 2D axis's tick values (decades first, then 2 / 5).
  As yaw grows the edge foreshortens and thinning drops labels; at ±80° only the decades may
  remain. The edge never changes sides: frequency always reads left → right (yaw is clamped).
- **Time** along the side edge nearer the eye (right for yaw > 0, left otherwise; right at
  0): `now` at the front, then the span's ticks (`−10 s`, … as the 2D view's).
- **Level**: the 2D colour bar at the pane's right, with the range's dB labels and unit
  (`dBFS`, `dB SPL`), showing the front row's colours. A **ceiling line** at the range top
  along the back edge, faint grey and dashed (the gentle plot-hint style), labelled with the
  top value; a spike touching it is at the top of the range.
- **T** (plot chrome): *Full* — faint floor grid (decade lines front to back, time ticks
  across), labels, colour bar, ceiling line; *NoGrid* — the same without the floor grid;
  *TracesOnly* — spikes and the floor band only.
- **Caption**: measurement, window/calibration and freshness as in 2D, plus the cell size
  the picture is made of (`1/19 oct · 0.30 s`), derived from the grid in use.

## Data path

No daemon data, no new topic: the source is the `SpectrographHistory` the UI keeps for the
measurement while any spectrograph view is shown (`SpectrumMode::spectrograph()` is true for
the 3D view, so history keeps building across G).

`ac2_scene::spectrograph_3d::Skyline` (pure) is the decimated grid:

- **Columns**: C log-spaced over the pane's frequency span ∩ the history's row range; each
  the highest level among the history rows it covers, like the 2D view's max-per-pixel.
- **Rows**: buckets of an integer k slots, `R = ceil(slots / k)`, anchored on absolute slot
  numbers (`floor(slot / k)`; the history already places frames by capture time ÷ slot
  length), so buckets never shift as history scrolls. Each is the highest level of its
  slots; a bucket with no frame draws nothing.

| span | slots | Full: k → R (row) | Lite/Bare: k → R (row) |
|---|---|---|---|
| 10 s | 300 | 3 → 100 (0.10 s) | 6 → 50 (0.20 s) |
| 30 s | 900 | 9 → 100 (0.30 s) | 19 → 48 (0.63 s) |
| 60 s | 1800 | 19 → 95 (0.63 s) | 38 → 48 (1.27 s) |
| 120 s | 3600 | 38 → 95 (1.27 s) | 75 → 48 (2.50 s) |

- **C = 192** (Full) / 128: over 10 octaves ≈ 1/19 (1/13) oct, finer than the 1/12 oct read
  by eye in 3D; a 1200 px pane gets a 6 px pitch, so 2 px spikes leave the gaps that make it
  a skyline, not a wall. **R ≈ 96**: at 8° pitch the floor's depth projects to ~100 px, about
  a pixel a row, so more rows only add fill; tilted up they are 5 px apart and distinct.
- **Incremental**: a new frame folds into the newest bucket (~1000 max operations) and
  replaces that bucket's `Arc<[f32]>`; a new bucket advances the ring. A change of frequency
  span, tier, span length or history reset rebuilds from the history: slots × rows, at 120 s
  and 20 Hz – 20 kHz 3600 × 960 ≈ 3.5 M max operations, a few ms, once.
- Kept beside the history in UI state, folded only while the 3D view is on screen, rebuilt
  on entry.

The scene carries the ring as `Vec<Option<Arc<[f32]>>>` + scroll + phase, as `Heatmap` does,
so the renderer's upload-by-identity applies: one row (768 B) per new frame.

## Scene / plot split

`ac2-scene` (display truth, headless): the skyline, the camera (angles, distance, clamps,
steps, eye-outside-box), the view-projection matrix, heights from the level range and the
measurement's offset, labels (text, projected place, culling), ceiling line, chrome level,
caption, the tier. New primitive `Skyline3d { ring, scroll, phase, cols, rows, view_proj,
range, colours, bloom_gain, tier }`.

`ac2-plot` (pixels only): an R32Float C×R ring texture written row by row, the colormap LUT,
one instanced draw of C×R spikes (`instance_index` → column, row; the vertex shader reads the
cell with `textureLoad`, no storage buffers — the WebGL2-class limits GL gets have none),
`BlendOperation::Max` with One/One, the floor quad, the bloom chain. No depth buffer and no
sorting: max and add are order-independent. `skyline.rs`, `shaders/skyline.wgsl`.

**Renderer API change for bloom.** Today `PlotCallback::prepare` ignores egui's encoder
(`ac2-ui/src/plot.rs`) and `Renderer::prepare` has none. New signature:
`Renderer::prepare(&mut self, device, queue, encoder: &mut wgpu::CommandEncoder, scene,
target)`; it records the quarter-res pass and the two blur passes into `encoder`.
`PlotCallback::prepare` passes egui's (egui submits it before the render pass);
`OffscreenTarget::render` / `paint_prepared` use one encoder for prepare and paint, so
`render_to_image` — the goldens and `pane_export.rs` — get bloom too; the bench and render
tests pass theirs. `paint` draws the spikes at full resolution straight into the pass and
adds the blurred texture over the pane rect: no full-resolution offscreen copy. Bloom targets
live in the renderer slot, reallocated on resize.

## Camera and controls

Orbit around a fixed target at the floor's centre, 0.15 up; vertical FOV 35°; near plane
0.05, far 20.

| | default | range | key step | mouse |
|---|---|---|---|---|
| yaw | 0° (frequency left → right) | ±80° | 15° | 0.3°/px drag x |
| pitch | 8° | 2° – 75° | 5° | 0.2°/px drag y |
| distance | d₀: the floor's front corners fit with 5 % margin at the pane's aspect | see below | 3 zoom steps | wheel / pinch: 1 step a notch |

- **Zoom is a dolly** in steps, distance `d₀ · 2^(−step/8)` (≈ 9 % a step), step −8 … +4.
  Moving the camera keeps the perspective cue (near spikes larger) that conveys depth when
  nothing hides anything; a narrowed FOV from afar flattens the skyline into a strip.
- **The eye never enters the skyline**: distance is at least `max(0.7·d₀, t_exit + 0.15)`,
  `t_exit` where the ray from the target towards the eye leaves the box x ∈ [−1, 1],
  y ∈ [0, 0.7], z ∈ [0, 1.2]. Without it, the closest dolly put the eye inside the box from
  about 50° yaw. A zoom step beyond the clamp does nothing.
- **Yaw is clamped** at ±80°: past 90° frequency would read right to left. **Pitch** stays
  above the floor (spikes edge-on vanish) and below top-down (that is the 2D view).
- Key and wheel steps ease through `anim::Spring`, as the frequency axis does; repaints run
  only while it settles. A drag follows the pointer and snaps to whole degrees on release.

**Keys.** The 3D view has its own scope, `Scope::Spectrograph3d` (`[spectrograph_3d]` in
`keys.toml`, *Spectrum / RTA: 3D spectrograph* in the keyboard map), chosen by
`View::scope()` as the IR view's is, so its hint line names camera keys. The frequency keys
are taken over by a nav intercept beside `ir_nav::is_nav` (`spectrograph_3d::is_nav` checked
in `commands.rs` before the frequency-axis arms), not by rebinding global keys:

| key | in the 3D view | command |
|---|---|---|
| ← / → | orbit | `pan_left` / `pan_right` (intercepted) |
| PageUp / PageDown | tilt | `tilt_up` / `tilt_down` (new, this scope only) |
| I / O | dolly in / out | `zoom_in` / `zoom_out` (intercepted) |
| Home | reset camera | `reset_view` (intercepted) |

PageUp/PageDown because ↑/↓ are the reserved stimulus level, Ctrl+↑/↓ and Ctrl+I/O the level
range (still meaningful: it sets the heights), Alt+↑/↓ the offset; PageUp/Down are bound only
in the walk scope. No Ctrl chord, nothing on Esc.

The spectrum scope's commands in the 3D scope:

| command (key) | in 3D | why |
|---|---|---|
| start_stop (S), reset_average (R), next/prev_view (G, Shift+G), spectrograph_span (Shift+B) | bound, same keys | act on the measurement or the view |
| offset (J), colour maps (palette) | in the palette and help, J bound | the offset moves the heights; the map colours them |
| smooth_coarser/finer (K, Shift+K), smoothing presets | inert, unbound | the history is built from unsmoothed frames |
| spectrum_style (B), peak_hold (P), next/prev peak and notch (Alt+P/N), cursor_mark / clear_mark (Alt+D) | inert, unbound | they act on the curve or the cursor, not drawn here |
| global Shift+←/→ (cursor), C (compare), Shift+T | inert, intercepted | no cursor and no compared curves in 3D |

Commands bound in both scopes keep the same default keys (a test checks it). `keys.toml`
rebinds are per section, as `[ir]`'s are: a `start_stop` rebound in `[spectrum]` is not
carried into `[spectrograph_3d]`; the user guide says so.

Hint line: orbit, tilt, zoom, reset camera, view, start/stop, span, maximise. Every new or
intercepted key ships with its key-table entry, help row, hint and a user-guide row (a
*3D spectrograph* section after *Spectrograph*, noting that the frequency span changes from
other panes or one G back) in the same change.

**Mouse** (`view/panes/mouse.rs::navigate`): a 3D branch after the legend early-return and
after the Ctrl/Shift+wheel level block (both kept, so Ctrl+wheel and Shift+wheel still zoom
and pan the level range), before the frequency-axis handling, as the IR and strip branches
are. Drag orbits instead of panning frequency, the plain wheel dollies instead of zooming
frequency, a touch pinch (egui's zoom delta without Ctrl) dollies, a click only focuses.
Putting curves into a pane is unaffected: that drag starts on a list row and runs through
`row_drag`, never the pane's drag response; legend plates keep the pointer first
(`plates_mouse`).

## Persistence and wire

- `SpectrumMode::Spectrograph3d` (scene) and `SpectrumView::Spectrograph3d`
  (`"spectrograph_3d"`) join the G chain.
- **Camera saved with the layout**: `LayoutPane.spectrum_camera: SpectrumCamera { yaw_deg:
  i8, pitch_deg: u8, zoom_step: i8 }`, a required field (`LayoutPane` is
  `deny_unknown_fields`; pre-1.0, no serde default). A camera is a view of the pane like its
  level range, which already travels in `LayoutPane.levels`: a kiosk pinned to a pane or a
  reopened session comes back framed the same. Whole degrees and zoom steps round-trip
  exactly and keep equality exact, so an unchanged camera is not re-sent (`send_layout`).
  Sent on drag release or key step through the one-in-flight `meas.layout` queue, never per
  pointer move.
- **Colour option with the measurement**: `Colormap::Ice` in
  `SpectrumDisplay.spectrograph_colours` (`Colormap::ALL` grows to 6), set where the others
  are (palette `colour map ice`, `ac2 meas set <meas> --colours ice`). One control for
  colours; ice works in 2D too. Bloom strength follows the map; no separate glow switch.
- One `PROTO_VERSION` bump, `fixtures/protocol/WIRE_LOCK` and the Python fixtures; the
  session manifest `VERSION` bumps since layouts and displays are in it.
- **Quality** is this machine's, not the wire's: Settings › Display › *3D quality*: Auto
  (default) / Full / Lite / Bare, in `ui.toml`.

## Performance

Fill dominates. Worst case, every spike at full height (a loud pink noise at the range top),
1920×1080 pane, counted with the spike geometry above: Full 23 M fragments at the default
camera, 40 M at the closest dolly; Lite (128×48) 11 M / 16 M before its half-resolution
target. Realistic pink sits nearer 0.6 of the range (× 0.6).

**Tiers**: *Full* 192×~96, spikes at full resolution, 9-tap bloom. *Lite* 128×~48, spikes
into a half-resolution target (Max) composited with bilinear upscale — a quarter of the fill,
4 M worst case — and 5-tap bloom. *Bare* as Lite without bloom.

| per frame, 1080p pane | Full, discrete / Apple GPU | Lite, Pi 4 / CM4 V3D |
|---|---|---|
| CPU (fold, scene, labels) | ≤ 0.3 ms | ≤ 2 ms |
| GPU at default camera | ≤ 3 ms | ≤ 12 ms (under half a 30 fps frame) |
| GPU at worst clamp corner | ≤ 6 ms | ≤ 20 ms (30 fps still holds with egui) |
| upload | 768 B | 512 B |

**Tier choice (v1)**: *Auto* picks by adapter, never by backend — the Pi runs GLES 3.1 or
v3dv Vulkan (`ac2-ui/src/gpu.rs`): `Rasterizer::Software` → Bare; adapter name containing
`V3D` (either backend), or `max_texture_dimension_2d` ≤ 4096, or another integrated GPU that
is not Apple → Lite; otherwise Full. The Settings override (Full / Lite / Bare) wins. No
automatic step-down in v1: with the app pacing its own frames on a never-waiting present
mode, a late frame does not separate a busy GPU from a busy UI thread, and a kiosk that
stepped down once (a blanking stall) would stay degraded for months.

- **Not visible, no cost**: a pane not on screen builds no scene, so nothing folds, uploads
  or draws; the skyline is rebuilt from the history when it reappears.
- Textures are far inside the 2048 limit; no float render targets, no storage buffers, no
  depth attachment, no independent blend (GLES 3.1 lacks it, so max and add are separate
  passes).

## Tests

- `ac2-scene` (`spectrograph_3d_tests.rs`): column decimation (tone in one column, wide low
  bands, invalid bands); bucket max, integer k per span × tier, buckets anchored on absolute
  slots across scrolls; gaps empty; reset on span / grid / scale / frequency-span change;
  frame-by-frame folding equals a rebuild (property test); phase = bucket fill fraction;
  heights from range and offset (bottom → 0, top → full, clamp); camera clamps, steps, reset,
  exact round-trip of `SpectrumCamera`; **eye outside the grown box at every clamp corner**
  (yaw ±80/0 × pitch 2/75 × closest zoom × narrow and wide aspects); floor corners inside the
  rect at the default; label texts, sides by yaw, culling, thinning at ±80°; T levels;
  ceiling line; caption cell size from k; Auto tier for software, V3D on GL, V3D on Vulkan,
  4096-limit integrated, Apple, discrete; override wins.
- `ac2-plot` (`tests/it/render.rs`, `golden.rs`): a single spike lands at the pixel the scene
  projects; max blending (two overlapping spikes give the brighter, not the sum); ring rows
  upload by identity; an empty cell draws nothing; goldens `skyline_pink` (the wall case:
  must show structure, not saturate) and `skyline_tonal`, ice and viridis, Full and Bare,
  320×200, blessed on lavapipe at the line tolerance (24 / 0.2 %: blur float math differs at
  edges like AA does); `render_to_image` output includes bloom.
- **GL in CI**: the skyline render tests also run with `WGPU_BACKEND=gl` (llvmpipe, which
  gets the WebGL2-class limits): vertex-stage `textureLoad` of R32Float and instancing are
  exercised on the backend the Pi uses.
- `ac2-ui`: G / Shift+G through four views; scope, hint line; ←/→, PgUp/PgDn, I/O, Home move
  the camera and leave the frequency axis; inert commands; equal defaults in both scopes;
  drag, wheel, pinch messages; camera sent on release only; Esc does not touch it.
- `ac2-e2e`: `spectrograph_3d_from_an_empty_daemon` (session, spectrum from the palette, G ×3,
  the fake rig's noise fills the skyline, camera keys, the camera survives a second client
  choosing the measurement, T, ice from the palette, G back); GPU snapshot `spectrograph_3d`
  in `tests/it/ui/views.rs` from the pinned 25 s sweep with a gap.

## Stages

0. **Look spike** (done, above): max spikes + additive bloom source chosen.
1. **v1a**: wire (view, camera, ice), skyline, camera, scene, axes, spikes + floor without
   bloom (the Bare tier), keys, mouse, hints, user guide, tests incl. the GL run.
2. **Gate**: a CM4 hardware run of the Bare/Lite goldens' scenes for frame time (needs the
   operator's OK to use the kiosk); its numbers replace the Pi column's targets.
3. **v1b**: renderer encoder API, bloom, half-res Lite, Settings quality, goldens per tier.
   1a and 1b ship together in one deploy: v1 is "pretty and fast" together.
4. Later, not planned: a cursor frequency line on the floor (no value), moving the orbit
   target, the peak-hold line as a ghost row.

## Open questions (with recommendations)

1. **Automatic step-down** by measured frame time: not in v1 (above). If wanted later, it
   needs a GPU-time source (timestamp queries where present) and a way back up.
2. **Ice as a colour map or a 3D-only glow style?** Colour map: one control, useful in 2D.
   It has fewer hues than viridis for fine steps; the operator picks it.
3. **Floor over the shared frequency span or always the full history range?** Shared span,
   as the 2D view: zoom a band elsewhere, G, and the 3D floor shows that band in more detail.
4. **Newest row at the front or the back?** Front: crisp and nearest, history recedes.
5. **Double-click / double-tap to reset the camera?** Not in v1 (Home); revisit on the touch
   kiosk.
6. **Bloom gain 0.12 for the additive source** was set on synthetic data; retune on a real
   pink-noise and programme recording from the rig before v1b's goldens are blessed.
