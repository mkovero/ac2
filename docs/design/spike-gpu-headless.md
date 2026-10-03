# Spike: headless GPU rendering and golden tests

> **History.** A record of the phase 0 spike, not a description of the product. The
> renderer is `crates/ac2-plot`, the app `crates/ac2-ui` (egui on wgpu, as chosen here); its
> snapshot tests are in `crates/ac2-ui/tests/snapshots`.

Crate: `spikes/gpu-headless` (`spike-gpu-headless`). Throwaway; this file is the output.
Question: can `ac2-plot` render plot scenes with wgpu on a software adapter, read the pixels
back and compare them to a reference image, so renderer tests run in CI without a GPU (§4.4)?

**Answer: yes.** wgpu 30 creates a windowless device on lavapipe in ~30 ms, renders a
1920x1080 frame with 16 × 480-point curves + grid + a text label and reads it back in
~8–10 ms (12 ms with 2 rasterizer threads). The golden test runs in 0.3 s and is
bit-identical run-to-run on one adapter.

## What was built

| file | content |
|---|---|
| `src/lib.rs` | `Gpu::new()` (headless device), `demo_scene()`, `Renderer` (instanced segments, readback), `TextLayer` (glyphon), `Image` PNG I/O, `compare()` with `Tolerance` |
| `src/plot.wgsl` | segment-quad vertex shader, capsule SDF fragment shader with join ownership |
| `src/main.rs` | adapter log, benchmark, `--bless`, `--out frame.png` |
| `tests/golden.rs` | golden compare, repeatability, join double-blend check, text ink check |
| `tests/reference/plot_800x450.png` | reference (29 KB), blessed on lavapipe |

```
cargo test -p spike-gpu-headless                               # skip if no adapter
AC2_REQUIRE_GPU=1 cargo test -p spike-gpu-headless             # CI: missing adapter = fail
cargo run -p spike-gpu-headless --release -- --bless           # regenerate reference
AC2_BLESS=1 cargo test -p spike-gpu-headless                   # same, from the test
cargo run -p spike-gpu-headless --release -- --out x.png       # bench + save 1080p frame
```

## Adapter selection

- `wgpu::Instance::new(InstanceDescriptor::new_without_display_handle_from_env())` — no
  window, no surface, no display handle. Backends come from `WGPU_BACKEND`
  (`vulkan`, `gl`, `dx12`, `metal`).
- `request_adapter` with `compatible_surface: None`, `force_fallback_adapter` set by
  `AC2_GPU_FALLBACK=1`. wgpu-core implements "fallback" as `DeviceType::Cpu`; it filters
  per backend, so lavapipe (Vulkan), llvmpipe (GL, detected by renderer string) and WARP
  (DX12, software flag) all qualify. macOS has no CPU Metal adapter: never force fallback
  there.
- Device limits: `Limits::downlevel_defaults()` so the same code passes on GL/llvmpipe.
- Logged at start: every adapter seen (`enumerate_adapters`) and the one picked, with
  backend, device type and driver string. Golden failures need that string to triage.

This machine (Arch, Mesa 26.2.2, LLVM 22, Threadripper 3945WX 12c) has **no hardware GPU
adapter**; it exposes only:

```
llvmpipe (LLVM 22.1.8, 256 bits) [Vulkan, Cpu, driver: llvmpipe Mesa 26.2.2]   <- picked
llvmpipe (LLVM 22.1.8, 256 bits) [Gl, Cpu, driver: 4.6 (Core Profile) Mesa 26.2.2]
```

Lavapipe reports itself as "llvmpipe" in `AdapterInfo::name`; the Vulkan backend
distinguishes it. Installed packages: `vulkan-swrast` (lavapipe ICD
`/usr/share/vulkan/icd.d/lvp_icd.json`), `vulkan-icd-loader`, `mesa`.

No-adapter path, verified with `VK_ICD_FILENAMES=/nonexistent.json WGPU_BACKEND=vulkan`:
tests print `SKIP <test>: no wgpu adapter ... Linux needs a Vulkan ICD
(mesa-vulkan-drivers -> lavapipe) or EGL/GL (libegl-mesa0 -> llvmpipe, select with
WGPU_BACKEND=gl)` and pass; with `AC2_REQUIRE_GPU=1` the same message is a panic.

## Line rendering and anti-aliasing

- One instance per polyline segment (64 B: prev, p0, p1, next, RGBA, alpha at both ends,
  half width, flags); 4-vertex triangle strip per instance; a single draw call for all
  lines. Grid lines are 2-point polylines through the same pipeline, snapped to pixel
  centres so a 1 px line is exactly one crisp column.
- The quad is the segment extended by `half_width + 1 px` on every side. The fragment
  computes the exact distance from the **pixel centre** (`@builtin(position)`) to the
  segment; coverage `clamp(hw + 0.5 - d, 0, 1)` gives a 1 px analytic AA ramp. Capsules
  have round caps, so round joins come for free.
- **Join ownership:** adjacent capsules overlap at a join. With translucent strokes the
  overlap blends twice and leaves dark/bright beads at every vertex (480 per curve,
  very visible on coherence-blanked parts). Each fragment also measures distance to the
  previous and next segment and discards unless it is the nearest one (ties to the later
  segment). Both neighbours evaluate the same `seg_dist` on the same pixel centre and
  same endpoints, so they agree and every pixel is drawn exactly once. Cost: 3 distance
  evaluations per fragment. Not covered: a curve folding back over non-adjacent segments
  (still double blends; acceptable for plot traces).
- Per-vertex alpha is interpolated along the owning segment by the projection parameter,
  continuous across joins. The reference image shows the coherence fade below ~40 Hz and at
  250 Hz without beads.
- Blending: premultiplied alpha into `Rgba8Unorm` (non-sRGB), i.e. blending on encoded
  values — what egui does too (egui-wgpu prefers `Rgba8Unorm`/`Bgra8Unorm` targets), so the
  same shader output looks the same inside egui.
- MSAA: not needed; analytic coverage is smoother than 4x MSAA for thin lines and keeps
  the readback path a plain copy. It would only be needed for non-SDF geometry.
- Quality: zoomed crops show even 1 px ramps on 2.5/3.5 px strokes at all angles, no
  joints visible, no gaps. Missing for the real renderer: scissor/clip to the plot rect
  (curves currently run off the plot area), and a minimum-width rule (strokes < 1 px should
  scale alpha rather than thin the ramp).

Proof the tests guard this: disabling the ownership discard makes both
`translucent_joins_do_not_double_blend` (join 192 vs segment 128) and the golden test
(2869 px over tolerance, 0.80 % > 0.2 %) fail.

## Text

glyphon 0.12 (cosmic-text 0.19, wgpu 30) works headless with no changes: `TextAtlas`
created for the offscreen format, `prepare` before the pass, `render` into the same pass
after the lines. Effort: ~70 lines; first-frame cost includes font DB scan (system fonts)
and atlas creation (~40 ms extra once). Steady state adds nothing measurable.

Text is **not** in the golden: `FontSystem::new()` loads system fonts, and which face
"sans-serif" resolves to differs per machine (here it picked a monospace face). The test
checks ink inside the label box and none outside. For `ac2-plot`: vendor one OFL font
(e.g. Inter or DejaVu Sans, ~300 KB), build `FontSystem` with an empty `fontdb::Database`
plus that font only, and then text can go into goldens like lines.

## Test tolerance strategy

- Compare per pixel: max channel |diff| ≤ `channel`; image passes if failing pixels ≤
  `max_bad_fraction`. Spike values: `channel = 24`, `max_bad_fraction = 0.2 %`.
- Rationale: rasterizers disagree only on AA edge pixels by a few LSB (f32 shader math,
  coverage rounding, blend rounding). A real regression — a stroke one pixel off, a missing
  segment, double-blended joins — moves thousands of pixels by far more than 24.
- Measured: lavapipe vs lavapipe re-run: bit-identical (asserted in a test). lavapipe
  (Vulkan) vs llvmpipe (GL) against the same reference: 1 px differs, by 1 LSB. Same LLVM
  rasterizer core, so this is a lower bound; WARP and Metal will differ more on edges. The
  bound to set in CI must be measured once on each runner (see below); expect edge-only
  diffs well under 0.2 %.
- On failure the test writes `target/gpu-headless/plot_actual.png` and `plot_diff.png`
  (red = over tolerance, green tint = within). CI should upload that directory as an
  artifact.
- Bless: `--bless` on the binary or `AC2_BLESS=1` on the test. Recommend blessing on
  lavapipe only (the CI Linux adapter) and holding other OSes to the tolerance; if one OS
  needs a looser bound, keep per-OS tolerances in the test, not per-OS reference images.
- Keep goldens small (800x450 here, 29 KB PNG) and render scene features that matter (AA
  edges, joins, alpha ramps, grid snapping); keep 1080p for benchmarks.

## CI requirements per OS

| OS runner | adapter | setup | env |
|---|---|---|---|
| ubuntu-latest | lavapipe (Vulkan, CPU) | `sudo apt-get install -y mesa-vulkan-drivers` (pulls `libvulkan1` loader) | `AC2_REQUIRE_GPU=1 AC2_GPU_FALLBACK=1 WGPU_BACKEND=vulkan` |
| ubuntu alt | llvmpipe (GL via EGL, CPU) | `libegl1 libegl-mesa0 libgl1-mesa-dri` | `WGPU_BACKEND=gl` — fallback if lavapipe breaks; no X/Wayland needed (EGL surfaceless) |
| windows-latest | WARP (DX12, CPU) | none, WARP ships with Windows (`d3d10warp.dll`); wgpu also enumerates it via DXGI | `AC2_REQUIRE_GPU=1 AC2_GPU_FALLBACK=1 WGPU_BACKEND=dx12` |
| macos-14+ (arm64) | Apple paravirtual Metal device | none | `AC2_REQUIRE_GPU=1 WGPU_BACKEND=metal`, **no** fallback flag (no CPU Metal adapter) |

Windows and macOS rows are from wgpu's own CI practice and docs; this spike ran on Linux
only. Before relying on them, run the spike test once on each runner and record the diff
numbers the test prints (`N px differ at all, M px > 24, max channel diff K`).
Debian/Ubuntu package names: lavapipe is in `mesa-vulkan-drivers`; Arch: `vulkan-swrast`;
Fedora: `mesa-vulkan-drivers`.

## Timings

Release build (workspace profile; deps also `opt-level=2` in dev, so `cargo test` is close).
Scene: 1920x1080, 16 curves × 480 points + 37 grid lines = 7701 segments, readback of
8.3 MB RGBA. Median of 20 frames after 3 warm-up.

| adapter | device create | first frame | render+readback median | split (encode / GPU+map / unpack) |
|---|---|---|---|---|
| lavapipe Vulkan, 12 threads | 28.6 ms (146 ms very first run, cold caches) | 26 ms | 8.1–9.9 ms (min 7.6) | 0.2 / 6.9–8.7 / 1.0 ms |
| lavapipe Vulkan, `LP_NUM_THREADS=2` (≈ CI runner) | 28.4 ms | 33 ms | 12.0 ms | 0.2 / 10.7 / 1.0 ms |
| llvmpipe GL, 12 threads | 56 ms | 48 ms | 9.4 ms | 8.5 (GL is synchronous on submit) / 0.02 / 0.9 ms |
| + text label (glyphon), lavapipe | — | 69 ms (font scan + atlas) | 8.6 ms | — |

Golden test suite (4 GPU tests + 3 unit tests): 0.3 s wall on lavapipe. The renderer's
CPU side (building 7.7k instances, encode) is 0.2 ms; nearly all time is rasterization
and the 8 MB copy. On a hardware GPU the §8.3 frame budget is not at risk from this
design; on lavapipe it is a usable software fallback (~100 fps at 1080p).

## Plugging into a UI toolkit (docs only)

**egui (eframe 0.36 / egui-wgpu 0.36, on wgpu 30 — same version as this spike).**
`egui_wgpu::Callback::new_paint_callback(rect, PlotCallback)` with `CallbackTrait`:
`prepare(device, queue, screen_descriptor, egui_encoder, resources)` uploads instances
(`queue.write_buffer`) and may return extra command buffers; `paint(info, render_pass,
resources)` draws into egui's own render pass with viewport/scissor set to the widget rect
(`PaintCallbackInfo` gives viewport and clip in pixels). Long-lived pipeline/buffers live
in `CallbackResources` (type map on the egui-wgpu renderer). Consequences for `ac2-plot`:
- Split the renderer as `prepare(&Scene) -> uploads` and `paint(&mut RenderPass)`; keep
  offscreen render+readback as a thin wrapper for tests (what this spike does in one call).
- Pipeline must be built for egui's target format and sample count (egui is non-sRGB,
  1x by default) — our premultiplied `Rgba8Unorm` blending already matches.
- Text in the plot: either glyphon inside the callback (same pass, as here) or egui's own
  painter for labels over the callback rect; ac2-scene owns the strings either way.
- `egui_kittest` (features `wgpu`, `snapshot`) renders whole egui UIs headless through
  egui-wgpu and compares PNG snapshots, so UI-level goldens on lavapipe use the same CI
  setup as `ac2-plot`'s.

**iced (0.14).** `iced::widget::shader` with a `shader::Program` producing a `Primitive`;
`Primitive::prepare(pipeline, device, queue, bounds, viewport)` and `draw(pipeline,
render_pass)` (or `render(encoder, target, clip)` for a separate pass); the `Pipeline`
type is created once per format (`Pipeline::new(device, queue, format)`). The model maps
just as cleanly. But iced_wgpu 0.14 depends on **wgpu 27**, three majors behind; `ac2-plot`
would have to pin wgpu 27 (and glyphon-equivalent `cryoglyph`) or wait. iced's Elm-style
architecture also suits the scene layer, but its custom-widget and text story is less
direct for a dense instrument UI.

Verdict for §8: egui (eframe) — same wgpu as today's crates, paint callback reuses the
spike's pipeline unchanged, and headless UI snapshot testing exists. Keep `ac2-plot`
toolkit-agnostic (takes `&wgpu::Device`, `&Queue`, target format, `&mut RenderPass`) so an
iced front end stays possible.

## Recommendations for `ac2-plot`

1. Keep this design: instanced segment quads, capsule SDF, join ownership, per-vertex
   alpha, premultiplied blending, no MSAA. Add plot-rect scissor and sub-pixel width rule.
2. Device creation as here (`*_from_env`, `AC2_GPU_FALLBACK`, downlevel limits, adapter
   log). Tests skip without adapter locally, fail in CI via `AC2_REQUIRE_GPU=1`.
3. Goldens: small, feature-focused scenes; reference blessed on lavapipe; per-channel +
   fraction tolerance (start 24 / 0.2 %, tighten after measuring WARP/Metal); artifacts
   (actual + diff PNG) on failure; `--bless`/`AC2_BLESS=1`. Put `Image`/`compare`/PNG I/O in
   `ac2-testkit` so UI snapshot tests share it.
4. Plus structural tests that don't need goldens (join double-blend check, repeatability,
   ink-in-box), which fail with a clear message instead of a diff image.
5. Vendor one font for text so labels can be in goldens.
6. Split renderer API into prepare/paint for egui callbacks; offscreen wrapper only for
   tests and screenshots.
7. Spectrograph (§8.1, scrolling texture + LUT) was not spiked; the same readback/golden
   path applies.
