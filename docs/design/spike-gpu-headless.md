# Spike: headless GPU rendering and golden tests

> **Concluded; spike code removed.** The renderer is `crates/ac2-plot`, the app
> `crates/ac2-ui` (egui on wgpu, as chosen here). Every check the spike made is carried by
> `ac2-plot`'s tests (`tests/it/golden.rs`: per-feature golden images, `AC2_REQUIRE_GPU` /
> `AC2_BLESS`; `tests/it/render.rs`: repeatability, translucent joins without double
> blending, label placement and ink) and the tolerance compare in `ac2-testkit` (feature
> `image`). UI snapshot tests are in `crates/ac2-ui/tests/snapshots`.

**Question:** can `ac2-plot` render plot scenes with wgpu on a software adapter, read the
pixels back and compare them to a reference image, so renderer tests run in CI without a
GPU (§4.4)?

**Answer: yes.** wgpu 30 creates a windowless device on lavapipe in ~30 ms and renders and
reads back a 1920x1080 frame (16 × 480-point curves + grid + label) in ~8–10 ms (12 ms with
2 rasterizer threads). Bit-identical run-to-run on one adapter.

## Adapter selection (`crates/ac2-plot/src/offscreen.rs`)

- `InstanceDescriptor::new_without_display_handle_from_env()`: no window, surface or
  display handle; backends from `WGPU_BACKEND` (`vulkan`, `gl`, `dx12`, `metal`).
- `request_adapter` with `compatible_surface: None`; `force_fallback_adapter` set by
  `AC2_GPU_FALLBACK=1`. wgpu-core treats "fallback" as `DeviceType::Cpu` per backend, so
  lavapipe (Vulkan), llvmpipe (GL) and WARP (DX12) all qualify. macOS has no CPU Metal
  adapter: never force fallback there.
- `Limits::downlevel_defaults()` so the same code passes on GL/llvmpipe.
- Log every adapter seen and the one picked (backend, device type, driver string): golden
  failures need that string to triage. Lavapipe reports itself as "llvmpipe" in
  `AdapterInfo::name`; only the backend distinguishes it.
- No adapter: tests print a SKIP naming the packages to install and pass; with
  `AC2_REQUIRE_GPU=1` the same message is a panic.

## Line rendering and anti-aliasing

- One instance per polyline segment (prev, p0, p1, next, colour, alpha at both ends, half
  width, flags), a 4-vertex strip each, one draw call for all lines. Grid lines go through
  the same pipeline, snapped to pixel centres so a 1 px line is one crisp column.
- The quad is the segment grown by `half_width + 1 px`. The fragment computes the exact
  distance `d` from the **pixel centre** to the segment; coverage `clamp(hw + 0.5 − d, 0, 1)`
  is a 1 px analytic AA ramp. Capsules give round caps and joins for free.
- **Join ownership:** adjacent capsules overlap at a join, so translucent strokes would blend
  twice and leave beads at every vertex. Each fragment also measures distance to the
  previous and next segment and discards unless its own segment is nearest (ties to the
  later one). Neighbours evaluate the same function on the same pixel centre and endpoints,
  so they agree and each pixel is drawn exactly once, at 3 distance evaluations per
  fragment. A curve folding back over non-adjacent segments still double-blends
  (acceptable for plot traces). Disabling the discard fails both the join test (join 192 vs
  segment 128) and the golden (0.80 % > 0.2 % bad pixels).
- Per-vertex alpha is interpolated along the owning segment by the projection parameter,
  continuous across joins (coherence fades without beads).
- Premultiplied alpha into `Rgba8Unorm` (non-sRGB), i.e. blending on encoded values, as
  egui does, so the output looks the same inside egui.
- No MSAA: analytic coverage is smoother than 4x MSAA for thin lines and keeps readback a
  plain copy.
- Built after the spike: plot-rect scissor (`geometry.rs` `scissor_of`) and a sub-pixel
  width rule (strokes < 1 px scale alpha instead of thinning the ramp).

## Text

glyphon works headless unchanged (`TextAtlas` for the offscreen format, `prepare` before the
pass, `render` after the lines). With system fonts, "sans-serif" resolves differently per
machine, so `ac2-plot` vendors one font (`fonts/Inter-Regular.ttf`) into an otherwise empty
`fontdb::Database`, which lets text go into goldens.

## Test tolerance

- Per pixel: max channel |diff| ≤ `channel`; the image passes if failing pixels ≤
  `max_bad_fraction`. Lines: 24 / 0.2 % (text 32 / 0.5 %, heatmap 4 / 0.1 %;
  `tests/it/golden.rs`).
- Why: rasterizers disagree only on AA edge pixels, by a few LSB (f32 shader math, coverage
  and blend rounding). A real regression (a stroke a pixel off, a missing segment,
  double-blended joins) moves many pixels by far more than 24.
- Measured: lavapipe re-run bit-identical; lavapipe vs llvmpipe 1 px off by 1 LSB (same LLVM
  core, a lower bound; WARP and Metal differ more on edges).
- Failures write actual/expected/diff PNGs under `<target>/golden-images/`; bless on lavapipe
  only (`AC2_BLESS=1`) and hold other OSes to the tolerance; per-OS tolerances, never per-OS
  reference images.
- Goldens are small feature-focused scenes; 1080p only for benchmarks.

## CI per OS (as in `.github/workflows/ci.yml`)

| OS runner | adapter | setup | env |
|---|---|---|---|
| ubuntu-latest | lavapipe (Vulkan, CPU) | `sudo apt-get install -y mesa-vulkan-drivers` (pulls `libvulkan1` loader) | `AC2_REQUIRE_GPU=1 AC2_GPU_FALLBACK=1 WGPU_BACKEND=vulkan` |
| ubuntu alt | llvmpipe (GL via EGL, CPU) | `libegl1 libegl-mesa0 libgl1-mesa-dri` | `WGPU_BACKEND=gl` — fallback if lavapipe breaks; no X/Wayland needed (EGL surfaceless) |
| windows-latest | WARP (DX12, CPU) | none, WARP ships with Windows (`d3d10warp.dll`); wgpu also enumerates it via DXGI | `AC2_REQUIRE_GPU=1 AC2_GPU_FALLBACK=1 WGPU_BACKEND=dx12` |
| macos-14+ (arm64) | Apple paravirtual Metal device | none | `AC2_REQUIRE_GPU=1 WGPU_BACKEND=metal`, **no** fallback flag (no CPU Metal adapter) |

Windows and macOS rows are from wgpu's own CI practice and docs; this spike ran on Linux
only. Before relying on them, run the spike test once on each runner and record the diff
numbers the test prints (`N px differ at all, M px > 24, max channel diff K`).

## Timings (release, 1920x1080, 7701 segments, 8.3 MB readback, median of 20)

| adapter | device create | render+readback |
|---|---|---|
| lavapipe Vulkan, 12 threads | 28.6 ms | 8.1–9.9 ms |
| lavapipe Vulkan, `LP_NUM_THREADS=2` (≈ CI runner) | 28.4 ms | 12.0 ms |
| llvmpipe GL, 12 threads | 56 ms | 9.4 ms (GL synchronous on submit) |

CPU side (building instances, encode) is 0.2 ms; the rest is rasterization and the copy. On a
hardware GPU the §8.3 frame budget is not at risk; lavapipe is a usable software fallback
(~100 fps at 1080p). First text frame adds ~40 ms once (font scan, atlas).

## UI toolkit decision

**egui (eframe / egui-wgpu 0.36, wgpu 30).** `egui_wgpu::Callback::new_paint_callback` with
`CallbackTrait::prepare` (uploads) and `paint` (draws into egui's render pass with the
widget's viewport/scissor); long-lived pipelines live in `CallbackResources`. Hence
`ac2-plot`'s renderer is split into `prepare(&Scene)` and `paint(&mut RenderPass)`, with an
offscreen wrapper for tests and screenshots; its pipeline matches egui's non-sRGB 1x target.
`egui_kittest` (features `wgpu`, `snapshot`) gives UI-level goldens on the same CI setup.

**iced 0.14 rejected:** its `shader` widget maps just as cleanly, but iced_wgpu 0.14 depends
on wgpu 27, three majors behind, and its custom-widget and text story is less direct for a
dense instrument UI. `ac2-plot` stays toolkit-agnostic (device, queue, target format, render
pass) so an iced front end remains possible.
