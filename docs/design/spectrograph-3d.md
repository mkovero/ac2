# 3D spectrograph

Status: built, v1a and v1b (*Stages* 0–3; `SpectrumView::Spectrograph3d`, `ac2_scene::spectrograph_3d`, `ac2-plot` skyline); design draft with review rounds 1–2 applied. Builds on [spectrograph.md](spectrograph.md):
same history, same level range, a different picture of it.

## Purpose and what the operator sees

The 2D spectrograph is the reading instrument: colour is level, a click reads a point. The
3D spectrograph is the *seeing* one: level is height, so a feedback ring, a resonance that
keeps returning or a band that swells reads as a shape across the room without decoding a
colour scale. It is meant to be left running and to look good doing so. It is an x86 /
Apple desktop view: a Raspberry Pi's V3D has no 3D view (*Performance*).

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

Cursor by picking: which spike, at what depth, a pointer is over is ambiguous in a
perspective without hidden lines. The pick is the spike the pixel under the pointer shows
(the renderer composites by level: of spikes whose drawn width covers the pointer, the
highest; off every body, the nearest within 6 px). While the pointer is over the view
that spike is marked (a light line and a ring at its tip), and a click commits that one:
the cursor is that cell, its column's centre frequency and its row's frame, held as audio
time, the same cursor the 2D views read. Live it moves back with its row, its age counting
up, until it leaves the history; stopped (S) the picture stands still to pick in. The
cell is marked on its spike (thick, in the cursor's colour on a dark edge, a box at its
tip) and read (the cell's level) on the cursor values' plate. The measurement's tracked
peaks (**P**) stand on the front row, where no depth is in doubt.

## The look

Settled by two rounds of a look spike (a CPU emulation of the spike pass with 8-bit targets;
round 2 on room-like synthetic data: per-bin pink with fixed LF modes, a comb from one
reflection and a crossover dip, averaged so it moves slowly; an RTA pink of 31 third-octave
plateaus; a sparse tonal case). **Additive spikes saturate**: with rows ~1 px apart at the
default camera, dozens of rows sum into every pixel and pink noise, the ordinary PA signal,
becomes a white wall; normalising each row's gain by its projected spacing only delays it.
**Max blending** keeps the skyline's shape for any density. An additive copy at low gain
feeds only the bloom, so where many rows stand together the haze is denser.

**Level and brightness are kept apart until the end.** Spikes are drawn into an offscreen
**Rg8Unorm** target with `BlendOperation::Max`: R = normalised level `n`, G = brightness.
The composite pass applies the LUT, `LUT(R) · G`. A per-channel max of RGB colours would mix
hues no level has (viridis: the red of one spike with the green of another); here the hue is
always the LUT at a level some spike under the pixel has — the loudest one — and the
brightness that of the brightest. Both may come from different spikes (a tall far spike
behind a near short one gives the far one's hue at the near one's brightness): no invented
hue, and nothing reads values off a pixel. Format: R16Float / Rg16Float would be finer, but
on GL wgpu makes them renderable only with `EXT_color_buffer_half_float` or
`EXT_color_buffer_float` (`wgpu-hal` gles adapter: `half_float_renderable` is a private
capability); Rg8Unorm is renderable, blendable and filterable on every backend. 256 level
steps match the 256-entry LUT. Full and Lite both draw offscreen, at full and half
resolution, so the tiers differ only in target size and bloom.

Constants in `ac2-scene::spectrograph_3d`:

- **World.** Floor x ∈ [−1, 1] (frequency), z ∈ [0, 1.2] (age, 0 = newest), heights
  y ∈ [0, 0.7]. Wider than deep keeps 10 octaves readable from the default camera; 0.7 lets
  a full-range spike rise above the far rows without leaving the frame.
- **Spike.** A camera-facing vertical quad from the floor to its height. Width 35 % of the
  projected column pitch, between 1 and 3 device px (thinner aliases into shimmer as rows
  scroll; wider costs fill and stops reading as a spike); Lite: 1–2 px of its half-res
  target. Its coverage is a box filter across a width a quarter pixel wider each side than
  the spike (no snapping to whole pixels): about width + ½ px of light a row, as a
  rasterizer lighting every pixel a spike touches gives. Brightness along the spike rises
  steeply from 0.05 at the foot to 1.0 at the tip (`0.05 + 0.95·t^3.5`), and a fixed
  2.5 px cap at the tip is 1.5× of the spike's light, clipped at 1 after that light (a dim
  far or low spike still gets its bright tip): thin bright tips over faint bodies, as in
  the reference. Round 2 compared this with the round-1
  constants (foot 0.25, `t²`, top 6 % bright): the new ones keep a slab of pink noise from
  reading as a solid wall and make the tips of a tonal case the brightest thing on screen.
  Round 3 (programme material) kept them: a tip gain of 1.25 or 1.0 dulled the tips into the
  bodies, and a higher foot (0.1, `t³`) only greys them. At the default camera the body of a
  tall spike reads lit even so, because the tips of the rows behind it land on it (rows ~1 px
  apart); that is the history standing behind the newest row, not the body's own light.
- **Level to light.** Normalised level `n` (0 at the bottom of the level range, 1 at the
  top, clamped; `n = 0` draws nothing) sets height `0.7·n`, colour `LUT(n)` and intensity
  `0.3 + 0.7·n`, so the grass is dim as well as low. Height is the level, nothing else: no
  automatic range, no exaggeration. Pink noise over a 100 dB range is a slab of near-equal
  spikes; the level range keys (Ctrl+I/O, Ctrl+↑/↓) narrowed to ~40 dB around it turn the
  slab into terrain (modes, comb, the crossover dip) — the user guide says so. Programme
  material is the same story: a mastered track's bins sit around −70…−5 dBFS (tone level,
  65 536-point default), so on −100…0 they form a plateau at mid height; about **−60…0**
  drops the bed to near-floor grass with the bass line and tonal partials standing out.
  Narrowing must keep the top near 0: hot masters reach −5…0 dBFS per bin, and a top at
  −20 clamps them into a flat white ceiling.
- **Depth.** Brightness × `exp(−2.0·age/1.2)` (the back row at 13 %) × `exp(−0.35·d)`,
  `d` the depth along the view beyond the floor's nearest point (its nearest corner's clip
  `w`), not the distance from the eye: the front row keeps full light at any dolly. Round 3 tried 3.0 and 3.5: at the default camera they barely change the picture,
  tilted they hide the older half of a 30 s span, so 2.0 stays. Depth changes the G channel
  only: a level keeps its hue at any depth, not its lightness. No claim that a colour reads
  the same as in 2D; the level reference is the height scale and the ceiling line (*Axes*).
- **Floor band.** One additive floor quad, brightest along the front edge (0.12 of
  `LUT(0.5)`), fading with the depth curve, plus a 1 px "now" line at 0.35.
- **Bloom.** The spikes again, coloured `LUT(n) · max(0, brightness − 0.25) / 0.75`, drawn
  straight into a quarter-resolution (of the pane, every tier) Rgba8Unorm target cleared
  to 0, additive (One/One) at gain 0.12. Each quarter-res pixel covers four pane pixels
  along the spike, so the fragment averages the light at those four (the 2.5 px tip cap
  falls within one) and scales by its coverage across, the spike's own width plus the same
  quarter pane pixel each side: a spike under one quarter-res pixel wide adds its share.
  Then a separable Gaussian (σ = 2 quarter-res px, ≈ 8 px full size, cut at ±4 and
  normalised): Full 9 point taps; Lite 5 bilinear fetches, each pair of taps read where the
  filter weighs them as the kernel does, so Lite's glow is Full's; Bare none. Added over
  the pane (bilinear upscale) at 0.6 (ice) or 0.35 (colour maps, so hues stay legible). Rgba8 drops far, dim rows below one LSB from
  the bloom; acceptable, they are the haze's edge and still drawn sharp. Float targets would
  need extensions the GLES downlevel limits do not promise. The threshold (round 3): on
  programme material at the default camera, the rows stacked ~1 px apart summed the dim
  bodies of a loud low end past 1.0 in the 8-bit bloom target (0.3 % of its pixels at a
  65 536-point FFT, 1 % at 8192), a flat milky blob over the bass with no shape. Dropping
  the light under 0.25 leaves under 0.005 % of source pixels at 1.0 on all six clips while
  the tips, the bright part, still glow; tilted, where the rows stand apart, it changes
  nothing visible.
- **Blend space.** The max is taken on level and brightness, before any encoding. The
  spike composite is blended with Max into egui's target over the dark field (max of the
  field and the spike colour), the bloom composite with Add; both with
  `write_mask: COLOR`, so egui's alpha is left as it is (the embers' additive pipeline also
  adds alpha; that is harmless there but not needed here). The bloom adds encoded values
  when egui's target is not sRGB, slightly brighter than a linear sum; goldens fix it.
- **Palette.** By default the measurement's spectrograph colour map, through the 2D view's
  LUT. New map **ice**: #0B1A2B through #2D6FA8 to #CFE8FF, monotonic in lightness, stopping
  short of white as `grey` does. Ice + bloom is the holographic look.
- **Field.** The plot rect is always #05080C, in the light theme too, and the labels and
  caption inside it use the dark theme's text colours: light added to a white
  field is invisible.
- **Motion.** Rows scroll back on the display's clock: at rest the phase is the newest
  bucket's fill fraction (slots filled ÷ slots per bucket), from the data. Frames carry
  whole analysis blocks, so the audio they add steps unevenly (10.7–42.7 ms at 96 kHz), and
  drawn as received the floor judders. The UI keeps a `ScrollClock` per measurement
  (`ac2_scene::scroll_clock`): the time axis is `now − offset` in session audio time, the
  offset the earliest recent arrival, so the newest frame shows the pass it arrives and the
  axis moves the same distance every display frame. It passes how many slots the axis
  stands past the newest slot (`lead_slots`, −1…8); `Skyline::drawn` adds it to the phase,
  the newest row stretching to cover the lead; the flat heatmap shifts its time axis by the
  same lead in the shader. Past the frames' spread and one hop the axis waits, so a stalled
  stream stands still, and the pacer runs only while one scrolls. Settings › Display
  "jumps to each new frame" turns it off with the 2D glide. Heights are the data as
  received, never interpolated.

Spike PNGs (not committed, session scratchpad `look/`): round 1 `sheet_pink.png`,
`sheet_tonal.png`, `sheet_max_vs_sumbloom.png`; round 2 `sheet_r2_default.png`,
`sheet_r2_tilted.png` (old vs new constants, ice and viridis, full and narrow level range ×
pink-room, RTA-pink, tonal). No local raw capture existed to run through it; the constants
were checked on programme material in round 3 (`look-tune-own/sheets/`): six 50 s excerpts
of electrical programme (ambient/neoclassical, acid techno, dense guitars, acoustic jazz,
house, drum and bass) through an ac2-like history (65 536-point Hann, tone dBFS, display hop,
96 rows/oct, 30 slots/s) and the Full-tier skyline at 30 s, default and tilted cameras,
−100…0 and −60…0, ice and viridis (`final_default.png`, `final_tilted.png`,
`final_default_fft8k.png`, `strip_acid_*.png`). Only the bloom threshold changed. A room-mic
recording of pink noise through a PA is still welcome before goldens are blessed. The
scripts stay out of the tree; round 2's three cases become goldens (*Tests*).

## Axes and level reference

Labels are scene truth: anchors projected through the camera, culled behind the eye or off
the rect, thinned where they would overlap.

- **Frequency** along the front edge, the 2D axis's tick values (decades first, then 2 / 5).
  As yaw grows the edge foreshortens and thinning drops labels; at ±80° only the decades may
  remain. The edge never changes sides: frequency always reads left → right (yaw is clamped).
- **Time** along the side edge nearer the eye (right for yaw > 0, left otherwise; right at
  0): `now` at the front, then the span's ticks (`−10 s`, … as the 2D view's).
- **Level**: a **height scale** — a vertical post at the front corner nearer the eye, from
  the floor to the range top, with dB ticks (the level axis's step, thinned by projected
  length) and the unit (`dBFS`, `dB SPL`) at its top — and a **ceiling line** at the range top
  along the back edge, faint grey and dashed (the gentle plot-hint style). Heights are the
  level; no colour bar in 3D: colour there is lightness × depth, not a scale to read.
- **No label is cut** by the field's edge: one that would not fit whole is dropped (half a
  number reads as another). The height scale's numbers stand outside the post; when the
  field would cut one there (a small pane), all of them stand inside it, so the scale reads
  on one side. The 7 % fit margin leaves a 1080p pane room for them outside.
- **On demand.** The labels, the height scale, the floor grid and the ceiling line cover
  the skyline, so at rest they are hidden: the view is the spikes, the floor band and the
  caption. They show while the pointer is over the pane, and for 3 s after a touch or click
  on it (a kiosk's touch screen has no hover; a press or release there counts), a camera
  change (key, drag, wheel, pinch, Home) or a level-range change; they fade in over 0.15 s
  and out over 0.25 s. The scene decides: `AxesReveal` keeps the hover and the last
  interaction with the alpha and time of the last event, so the alpha is a ramp in time
  between events (a second touch while fading carries on from where it is); the UI
  reports the events and draws with `alpha(now)`. It repaints while a fade runs, wakes
  once when a hold ends, and is idle otherwise. No key: **T** still picks what shows, and
  *TracesOnly* keeps it hidden on hover. An exported PNG shows the axes whole: an export
  is a documented picture, read rather than revealed.
- **Plates.** Each label stands on a faint dark rounded plate (the field's colour at 55 %)
  drawn over the spikes and their bloom, so a number over a bright spike still reads; the
  text pipeline has no halo.
- **T** (plot chrome): *Full* — faint floor grid (decade lines front to back, time ticks
  across), labels, height scale, ceiling line; *NoGrid* — the same without the floor grid;
  *TracesOnly* — spikes and the floor band only.
- **Caption**: measurement, window/calibration and freshness as in 2D, plus the cell size
  the picture is made of (`1/19 oct · 0.30 s`), derived from the grid in use, and the tier
  when Settings forces one (`· lite`); Auto names none.

## Data path

No daemon data, no new topic: the source is the `SpectrographHistory` the UI keeps for the
measurement while any spectrograph view is shown (`SpectrumMode::spectrograph()` is true for
the 3D view, so history keeps building across G).

`ac2_scene::spectrograph_3d::Skyline` (pure) is the decimated grid:

- **Columns**: C log-spaced over the pane's frequency span ∩ the history's row range; each
  the highest level among the history rows it covers, like the 2D view's max-per-pixel.
- **Rows**: buckets of an integer k slots, anchored on absolute slot numbers
  (`floor(slot / k)`; the history already places frames by capture time ÷ slot length), so
  buckets never shift as history scrolls. Each is the highest level of its slots in the
  history; a bucket with no frame draws nothing. Since buckets are anchored, the history
  covers `R = ceil(slots / k)` of them plus a partial one at each end: the ring holds
  **R + 1** buckets (texture C × (R + 1)). The oldest is recomputed from its slots still in
  the history whenever one drops out (k × rows max operations, ≤ 36 k at 120 s), so the
  skyline is always exactly a function of the history and folding equals rebuilding.

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
  replaces that bucket's `Arc<[f32]>`, the oldest is recomputed as above; a new bucket
  advances the ring. A change of frequency
  span, tier, span length or history reset rebuilds from the history: slots × rows, at 120 s
  and 20 Hz – 20 kHz 3600 × 960 ≈ 3.5 M max operations, a few ms, once.
- Kept beside the history in UI state, folded only while the 3D view is on screen, rebuilt
  on entry.

The scene carries the ring as `Vec<Option<Arc<[f32]>>>` + scroll + phase, as `Heatmap` does,
so the renderer's upload-by-identity applies: two rows (newest, oldest; 1.5 kB) per new frame.

## Scene / plot split

`ac2-scene` (display truth, headless): the skyline, the camera (angles, distance, clamps,
steps, eye-outside-box), the view-projection matrix, heights from the level range and the
measurement's offset, labels (text, projected place, culling), ceiling line, chrome level,
caption, the tier. New primitives `Skyline3d { id, rect, clip, cols, rows, newest, phase,
view_proj, eye, world, range, colormap, tier, look }` (`look`: the spike and bloom-source
constants) and `SkylineGlow { id, clip, gain }` (the bloom composite of the skyline with
that id; the scene adds one only for a tier with bloom).

**Draw order** is fixed and follows `Layer`'s (`primitives.rs`: rects, heatmaps, polygons,
bands, grids, polylines, points, labels). `Layer` gains `skylines` after `heatmaps` and
`glows` after `points`. The scene puts the field (a rect), the skyline, the floor band
(polygon), the floor grid and the ceiling line / height scale (grids, polylines) and the
glow in one layer, the labels in it too: field → spikes (Max) → floor band, grid, ceiling →
bloom (Add) → labels. The floor band is under the bloom and never bloomed.

`ac2-plot` (pixels only): an R32Float C×(R+1) ring texture written row by row, the colormap
LUT, one instanced draw of the spikes (`instance_index` → column, row; the vertex shader reads
the cell with `textureLoad`, no storage buffers — the WebGL2-class limits GL gets have none)
into the Rg8Unorm spike target cleared to 0 with `BlendOperation::Max` (One/One), the bloom
chain, and two composite draws in the pass: spike target → `LUT(R)·G` with Max, bloom with
Add. No depth buffer and no sorting: max and add are order-independent. `skyline.rs`,
`shaders/skyline.wgsl`.

**Renderer API change for bloom.** Today `PlotCallback::prepare` ignores egui's encoder
(`ac2-ui/src/plot.rs`) and `Renderer::prepare` has none. New signature:
`Renderer::prepare(&mut self, device, queue, encoder: &mut wgpu::CommandEncoder, scene,
target)`; it records the spike pass (full or half resolution), the quarter-res bloom pass
and the two blur passes into `encoder`.
`PlotCallback::prepare` passes egui's (egui submits it before the render pass);
`OffscreenTarget::render` / `paint_prepared` use one encoder for prepare and paint, so
`render_to_image` — the goldens and `pane_export.rs` — get bloom too; the bench and render
tests pass theirs. `paint` composites the two targets over the pane rect. Targets live in
the renderer slot, reallocated on resize. **Export** (`pane_export.rs`) always renders the
Full tier whatever this machine's tier: an exported picture is the same on every machine.

## Camera and controls

Orbit around a fixed target at the floor's centre, 0.15 up; vertical FOV 35°; near plane
0.05, far 20.

| | default | range | key step | mouse |
|---|---|---|---|---|
| yaw | 0° (frequency left → right) | ±80° | 15° | 0.3°/px drag x |
| pitch | 8° | 2° – 75° | 5° | 0.2°/px drag y |
| distance | d₀: the box's corners fit with 7 % margin at the pane's aspect and the pose's angles, centred up and down | see below | 2 zoom steps | wheel / pinch: 1 step a notch |

- **Centred up and down** by an off-axis lens shift: with the target on the floor's middle a
  low camera puts the box high in the plot, a dead band under the floor; the shift (from
  the box's corners at the pose's zoom-0 distance, at most half the plot) slides the frame
  so the box's top and bottom sit equally far from the plot's edges. The pose and its
  perspective are unchanged, and a dolly scales about the same point.
- **Fitted per angle**: d₀ is found for the pose's own yaw and pitch, not once at the
  default, so a steep or turned camera (yaw −35°, pitch 30°) keeps the front floor corner
  in the picture at zoom 0. Each corner's fit distance is continuous in the angles, so a
  camera easing through an orbit dollies smoothly with the fit; no pop.
- **Zoom is a dolly** in steps, distance `d₀ · 2^(−step/8)` (≈ 9 % a step), step −8 … +4.
  Moving the camera keeps the perspective cue (near spikes larger) that conveys depth when
  nothing hides anything; a narrowed FOV from afar flattens the skyline into a strip.
- **The eye never enters the skyline**: the effective distance is at least `t_exit + 0.15`,
  `t_exit` where the ray from the target towards the eye leaves the box x ∈ [−1, 1],
  y ∈ [0, 0.7], z ∈ [0, 1.2]. With d₀ fitted to the pane it binds only on very wide panes
  (small d₀) at the closest steps; there the stored zoom step stays as set and only the
  distance drawn is clamped.
- **Yaw is clamped** at ±80°: past 90° frequency would read right to left. **Pitch** stays
  above the floor (spikes edge-on vanish) and below top-down (that is the 2D view).
- Key and wheel steps ease through `anim::Spring`, as the frequency axis does; repaints run
  only while it settles. A drag follows the pointer and snaps to whole degrees on release.
- **Validation**: `PaneLayout::check` (where level ranges are checked) refuses a camera
  outside yaw ±80, pitch 2 … 75, zoom step −8 … +4, as it refuses a bad level range: a
  layout from another client or a session is rejected, never silently clamped.

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
| spectrum_style (B), peak_hold (P), next/prev peak and notch (Alt+P/N), cursor_mark / clear_mark (Alt+D) | inert, unbound | they act on the curve or the cursor, not drawn here; the front-row peaks follow the measurement's Track, set with P on the 2D views |
| global Shift+←/→ (cursor steps), C (compare) | inert, intercepted | the cursor is picked with the mouse; no compared curves in 3D. Shift+T toggles the cursor as everywhere |

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
  (default) / Full / Lite / Bare, in `ui.toml`. A forced tier is named in the caption.

## Performance

Fill dominates. Worst case, every spike at full height (a loud pink noise at the range top),
1920×1080 pane, counted with the spike geometry above: Full 23 M fragments at the default
camera, 40 M at the closest dolly; Lite (128×48) 11 M / 16 M before its half-resolution
target. Realistic pink sits nearer 0.6 of the range (× 0.6).

**Tiers**: *Full* 192×~96, spike target at full resolution, 9-tap bloom. *Lite* 128×~48,
spike target at half resolution composited with bilinear upscale — a quarter of the fill,
4 M worst case — and 5-tap bloom. *Bare* as Lite without bloom. Every tier adds one composite
pass over the pane (two with bloom); Rg8 writes half the bytes of Rgba8.

| per frame, 1080p pane | Full, discrete / Apple GPU |
|---|---|
| CPU (fold, scene, labels) | ≤ 0.3 ms |
| GPU at default camera | ≤ 3 ms |
| GPU at worst clamp corner | ≤ 6 ms |
| upload | 1.5 kB |

**Measured** (v1b, `bench_frame_cost`: the whole view at a 1920×1080 pane, every spike near
full height, 30 s history, median of 30 frames, release build). The development desktop has
no hardware GPU, so its adapter is Mesa's software rasterizer: GPU times are a CPU
rasterizer's and say nothing against the GPU budgets above; they rank the tiers (the bloom
costs about two thirds of Bare's GPU time there, the full-resolution target three times
Lite's). CPU is the frame's work up to the submit (fold, scene, labels, prepare, encode).
Worst corner: the worst of yaw ±80/0 × pitch 2/75 at the closest zoom.

| tier, lavapipe (Vulkan), timestamp queries | CPU default | GPU default | CPU worst | GPU worst |
|---|---|---|---|---|
| Full | 0.37 ms | 200 ms | 0.42 ms | 235 ms (−80/2/4) |
| Lite | 0.37 ms | 61 ms | 0.44 ms | 77 ms (−80/2/4) |
| Bare | 0.41 ms | 37 ms | 0.44 ms | 45 ms (−80/2/4) |

On llvmpipe GL the driver rasterizes inside the submit, so CPU ≈ GPU there (Full 156 / 225
ms, Lite 46 / 53, Bare 25 / 40, default / worst). The CPU column is over the 0.3 ms budget
by a tenth on this machine: the scene with labels at 1080p.

**Desktop GPU** (NVIDIA RTX 2070, Vulkan, 1080p, every spike at full height, the host under
load), default camera:

| tier | GPU | CPU |
|---|---|---|
| Full | 1.57 ms | 0.21 ms |
| Lite | 0.38 ms | 0.20 ms |
| Bare | 0.24 ms | 0.18 ms |

Full is at half its 3 ms budget; Lite and Bare take a quarter and a sixth of Full's GPU time,
the margin a weaker x86 GPU (an Intel iGPU, which Auto puts on Lite) has to work with.

**CM4 (V3D)**: functional pass on GLES 3.1 and on v3dv Vulkan (every skyline test and golden
passes on both), but Bare alone takes 96–142 ms of GPU a frame, 8–10× over a 30 fps frame:
the fill rate, not a feature, is missing, and no tier the look allows closes that.
**Decision (operator)**: the Pi is off; the view targets x86 (and Apple) desktops. On a
V3D-class adapter the 3D view is unavailable: **G** / **Shift+G** skip it, a layout that
names it draws the 2D spectrograph with *3D needs a faster GPU* ending its caption, and
Settings › 3D quality reads *unavailable on this GPU* and does not step.

**Tier choice (v1)**: *Auto* picks by adapter, never by backend — a Pi runs GLES 3.1 or
v3dv Vulkan (`ac2-ui/src/gpu.rs`): adapter name containing `V3D` (either backend) → no tier,
the view unavailable; `Rasterizer::Software` → Bare; `max_texture_dimension_2d` ≤ 4096, or
an integrated GPU that is not Apple → Lite; otherwise Full. The Settings override (Full / Lite / Bare) wins. No
automatic step-down in v1: with the app pacing its own frames on a never-waiting present
mode, a late frame does not separate a busy GPU from a busy UI thread, and a kiosk that
stepped down once (a blanking stall) would stay degraded for months.

- **Not visible, no cost**: a pane not on screen builds no scene, so nothing folds, uploads
  or draws; the skyline is rebuilt from the history when it reappears.
- Textures are far inside the 2048 limit; no float render targets, no storage buffers, no
  depth attachment, no half-float render target, no independent blend (GLES 3.1 lacks it,
  so max and add are separate passes).

## Tests

- `ac2-scene` (`spectrograph_3d/tests/`): column decimation (tone in one column, wide low
  bands, invalid bands); bucket max, integer k per span × tier, buckets anchored on absolute
  slots across scrolls, the oldest bucket after slots drop out, R + 1 ring; gaps empty; reset on span / grid / scale / frequency-span change;
  frame-by-frame folding equals a rebuild (property test); phase = bucket fill fraction;
  an update without a new frame keeps every row (nothing re-uploaded); the drawn rows glide
  back steadily across a new bucket, never drawn in front of the floor;
  heights from range and offset (bottom → 0, top → full, clamp); camera clamps, steps, reset,
  exact round-trip of `SpectrumCamera`, `PaneLayout::check` refusing each out-of-range
  field; **eye outside the grown box at every clamp corner** (yaw ±80/0 × pitch 2/75 ×
  closest zoom × narrow and very wide aspects), stored step unchanged by the clamp; floor corners inside the
  rect at the default; label texts, sides by yaw, culling, thinning at ±80°; height scale
  ticks and unit; T levels; ceiling line; draw order within the layer; caption cell size
  from k and tier name when forced; phase per data frame; box centred up and down at the default; labels whole inside the field at every pane size
  and camera, the level numbers on one side; Auto tier for software, none for V3D on GL and on Vulkan,
  4096-limit integrated, Apple, discrete; override wins.
- `ac2-plot` (`tests/it/render.rs`, `golden.rs`): a single spike lands at the pixel the scene
  projects; max blending (two overlapping spikes give the brighter, not the sum, and the
  hue of a level one of them has); egui target alpha unchanged; ring rows upload by
  identity; an empty cell draws nothing; goldens `skyline_pink_room` (must show structure,
  not saturate), `skyline_rta_pink` and `skyline_tonal`, ice and viridis, Full, Lite and Bare,
  320×200, blessed on lavapipe at the line tolerance (24 / 0.2 %: blur float math differs at
  edges like AA does); `render_to_image` output includes bloom and is Full whatever tier.
  The glow only adds and lights pixels beside the tips; a glow on Bare, or without its
  skyline, is refused. `bench_frame_cost` (`AC2_SKYLINE_BENCH=1`) prints CPU and GPU time
  per tier (*Performance*).
- **GL in CI**: the skyline render tests also run with `WGPU_BACKEND=gl` (llvmpipe, which
  gets the WebGL2-class limits): vertex-stage `textureLoad` of R32Float, instancing, Max
  into Rg8Unorm (the two-spike test) are exercised on GL as well as Vulkan.
- `ac2-scene` (`spectrograph_tests.rs`): the 3D view asked of the 2D scene (a GPU without
  it) is the spectrograph alone with *3D needs a faster GPU* in its caption.
- `ac2-ui`: G / Shift+G through four views, three on a GPU without the 3D view (no camera
  keys, 3D quality does not step); scope, hint line; ←/→, PgUp/PgDn, I/O, Home move
  the camera and leave the frequency axis; inert commands; equal defaults in both scopes;
  drag, wheel, pinch messages; camera sent on release only; Esc does not touch it; the rows
  scroll evenly per display frame under uneven audio steps and wait when frames stop, the
  pacer only while one scrolls, not with "jumps to each new frame"
  (`state/tests/display/scroll.rs`).
- `ac2-e2e`: `spectrograph_3d_from_an_empty_daemon` (session, spectrum from the palette, G ×3,
  the fake rig's noise fills the skyline, camera keys, the camera survives a second client
  choosing the measurement, T, ice from the palette, G back); GPU snapshot `spectrograph_3d`
  in `tests/it/ui/views.rs` from the pinned 25 s sweep with a gap.

## Stages

0. **Look spike** (done): max on level and brightness into Rg8, LUT in the composite,
   additive bloom source; steep body with a fixed tip cap.
1. **CM4 run** (done): the v1b build on the CM4, GLES 3.1 and v3dv Vulkan: every test
   passes, Bare 96–142 ms of GPU a frame (*Performance*). The Pi is off; V3D has no 3D view.
2. **v1a** (done, branch `spectrograph-3d`): wire (view, camera, ice, validation), skyline,
   camera, scene, axes; renderer encoder API, the offscreen spike target and composite at
   full and half resolution, floor; Settings quality; keys, mouse, hints, user guide; goldens
   for Full, Lite and Bare (Full and Lite without bloom until v1b), the GL CI run. Where it
   differs from the text above:
   - **Ring, not scroll**: the level texture is a ring indexed by `newest` and `phase` (row
     `j` at `z = (j + phase) · depth / rows`), so a new bucket uploads one row, not the
     texture. `Skyline3d` carries `newest` instead of `scroll`.
   - **Spike target encoding**: R the level, G `sqrt(brightness · coverage)` (more of the 8
     bits for the dim far rows); the composite reads R nearest (a filtered level is a level
     no spike has) and G bilinear, squared back. Coverage is a box filter across a fractional
     width a quarter pixel wider each side: no snapping to whole pixels, and about
     width + ½ pixels of light a row, as the emulator's lit-every-touched-pixel spikes the
     look was tuned on.
   - **Read as the emulator does** (parity pass at 960×540 against `look.py` without bloom,
     within ~10 % mean brightness): `d` in the depth fade is the depth along the view beyond
     the floor's nearest corner, not the distance from the eye (that dimmed the front row to
     ~40 %); the tip cap is 1.5× of the light and clips at 1 after it, not before.
   - **Floor band** under the spikes drawn in the base layer, before the composite.
   - **Fitted distance** fits every corner of the box (floor and top) in x and y, not only
     the floor's front corners: at high pitch the far top edge bound otherwise. (Since the
     app review below: 7 % margin and the box centred by a lens shift.)
   - **Colour maps** moved from `ac2-ui` to `ac2-scene` (`colormap`), so the skyline's LUT
     and the 2D spectrograph's come from one table.
   - The skyline goldens are their own test binary (`ac2-plot --test skyline`), run on GL in
   CI and on the CM4 run with `WGPU_BACKEND=gl` or `vulkan`.
3. **v1b** (done, branch `spectrograph-3d`): bloom as *The look* has it, `SkylineGlow` in
   the scene's data layer after the ceiling line and before the axis labels (the floor band
   and floor grid are in the base layer under the spikes, as in v1a); goldens re-blessed
   with it (Bare unchanged), on lavapipe and llvmpipe GL. 1a and 1b ship together in one
   deploy: v1 is "pretty and fast" together. Where it differs from the text above:
   - **Source drawn at quarter resolution**, not drawn full size and box-reduced: the
     fragment averages the light at the four pane pixels it spans along the spike and takes
     its coverage across, which is that mean for a vertical spike at a quarter of the fill.
   - **Lite's 5 taps are Full's kernel** in bilinear fetches (*The look*), not a narrower
     blur: the glow looks the same on every tier that has one.
   - **Bloom targets are a quarter of the pane on Lite too**, not of its half-res spike
     target.
   - **Parity with bloom** (960×540, `look.py` with the 0.25 threshold, sheets
     `sheet_default_bloom.png`, `sheet_tilted_bloom.png` beside v1a's): the glow sits on the
     tips and on the dense bass at the default camera, nothing milky. The bloom adds about
     0.8 of the emulator's (mean 0.2–2.9 of 255 per pixel against 0.2–3.4) because the
     spikes it is made of are the ~10 % dimmer ones of v1a's parity; gain left at 0.12.
     Pixels at 255 in the real picture are under 0.6 % (the emulator: 1.1 %).

4. **App review** (1920×1080 app screenshots against the fake rig, a pinned synthetic 30 s
   history shaped as a raw 8192-point FFT of pink noise through a room, per-bin chi-square
   spread, with tones): the box sat high with a dead band under the floor at the default
   camera, and the height scale's numbers were cut by the field's right edge (`−1C`), worst
   in the light theme and in a small pane. Now the frame is centred (*Camera and
   controls*), the fit margin is 7 %, and labels are never cut (*Axes and level
   reference*); the unit stands clear of the top tick. Goldens re-blessed on lavapipe;
   llvmpipe GL within tolerance. A second look: the axes were not pretty over the
   skyline, so they come on demand (hover, or 3 s after a touch or a change) and the
   labels stand on faint plates; the zoom-0 distance is fitted per angle (the orbit shot
   cut the front floor corner). The orbit golden re-blessed for the new fit.

5. Later, not planned: a cursor frequency line on the floor (no value), moving the orbit
   target, the peak-hold line as a ghost row.

## Open questions (with recommendations)

1. **Automatic step-down** by measured frame time: not in v1 (above). If wanted later, it
   needs a GPU-time source (timestamp queries where present) and a way back up.
2. **Ice as a colour map or a 3D-only glow style?** Colour map: one control, useful in 2D.
   It has fewer hues than viridis for fine steps; the operator picks it.
3. **Floor over the shared frequency span or always the full history range?** Shared span,
   as the 2D view: zoom a band elsewhere, G, and the 3D floor shows that band in more detail.
4. **Newest row at the front or the back?** Front: crisp and nearest, history recedes.
5. **Double-click / double-tap to reset the camera?** Not in v1 (Home); revisit with a
   touch screen on a desktop.
6. **Look constants** (foot 0.05, `t^3.5`, 2.5 px tip cap at 1.5×, depth `exp(−2.0·age/1.2)`,
   bloom source gain 0.12 above a 0.25 brightness threshold): set on synthetic data and
   tuned on programme material (six electrical music excerpts, *The look*); only the bloom
   threshold changed. Still open: a room-mic recording of pink noise through a PA before the
   goldens are blessed. Programme also showed that the 65 536-point default (1.37 s window,
   a frame every 0.17 s) smears kicks and hats into the bass and tonal layer; the 3D view
   reads transients only on a short FFT (8192 points: ~47 frames/s, every slot filled),
   where the max per 0.3 s bucket keeps them from flickering.
7. **Settings › 3D quality** (Auto / Full / Lite / Bare): decided, the operator keeps the
   row; Auto stays the default and names the tier it picked.
