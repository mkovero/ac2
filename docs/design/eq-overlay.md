# EQ what-if overlay

Status: design, not built. Operator decisions 2026-10-20: the user lists the filters they plan
to set on a console or processor; ac2 draws the measured transfer function multiplied by them
(**after EQ**, the prediction) and the filters' response alone (**EQ**). There is no single Q
convention and no single shape list, so neither is hard-coded: the editor exposes each shape's
parameters and how its Q is meant. To be built after the two-system alignment aid
(`alignment.md`, in flight); shared parts are marked **[shared]**.

## The idea in one paragraph

An EQ set lives **on the measurement** (operator, 2026-10-20): it models the processor feeding
that system, so any of the measurement's transfer curves — live, captures, sweep runs, imports
filed under it — can show "after EQ". Each prediction shown is a math channel
`curve × EQ 1` — the cascade `math-channels.md` already defines (delays add, the filter's is 0;
the lower γ² carried, the filter's is 1) — whose B operand is the new typed operand
`Operand::Eq`, evaluated by the daemon. So the prediction updates live, is captured with
Ctrl+1 … 9, coloured, listed and refused with reasons like any math channel.

## 1. Filter model

`EqSet { id, name, rev, realisation, peak_q, enabled, in_console, sections }`,
`EqSection { enabled, shape: EqShape }`; quantities typed (`Hz`, `Db`, `Seconds`).

**Realisation** (per set — a processor runs every section the same way):
- `digital { rate: Hz }` (default: the session rate; the user sets the *processor's* rate,
  48 / 96 kHz, which is not necessarily ac2's capture rate). Each section is designed in the s
  domain and mapped by the bilinear transform prewarped at its own f₀, then evaluated exactly
  as `H(e^{j2πf/fs})` at every column (a polynomial ratio, no FFT). That is what most consoles
  do, so it shows their cramping near Nyquist (a 16 kHz peak at 48 kHz squeezed towards fs/2)
  before the user sets it. Columns at or above fs/2 are gaps (NaN), not zero.
- `analog`: the s-domain prototype at `s = j2πf` — decramped / FIR-based processors.

**Shapes** (`EqShape`, a serde enum, `#[serde(tag = "type")]`):

| shape | parameters |
|---|---|
| `peaking` | `f`, `gain`, `bw: Bandwidth`, `q_measured: PeakQ` |
| `low_shelf` / `high_shelf` | `f`, `gain`, `order: first \| second {slope: ShelfSlope}`, `f_at: ShelfFreq` |
| `high_pass` / `low_pass` | `f`, `alignment: Alignment`, `order: 1..=8` (6 … 48 dB/oct) |
| `high_pass_q` / `low_pass_q` | `f`, `q` — one free 2nd-order section (resonant HP/LP) |
| `notch` | `f`, `bw: Bandwidth` (−3 dB bandwidth of the notch) |
| `band_pass` | `f`, `bw: Bandwidth` (0 dB peak) |
| `all_pass_1` | `f` (90° point) |
| `all_pass_2` | `f`, `q` |
| `gain` | `gain` |
| `polarity` | — (×−1) |
| `delay` | `seconds` (`e^{−j2πfτ}`; exact, not rounded to samples — **[shared]** with the alignment aid's delay) |
| `biquad` | `b0 b1 b2 a1 a2` (a0 = 1) at `rate: Hz` — escape hatch; always digital at its own rate; refused when a pole is on or outside the unit circle |
| `response` | `trace: TraceId` — a stored transfer trace multiplied in (a FIR's exported response, a processor's measured preset); interpolated on the log grid like a math operand; gaps stay gaps |

`Alignment`: `butterworth`, `linkwitz_riley` (even orders only: 2, 4, 6, 8 = 12 … 48 dB/oct;
the square of the Butterworth of half the order, −6 dB at f), `bessel { norm: BesselNorm }`
with `norm` = `phase` (the asymptotic phase matches the Butterworth's — the common crossover
convention), `delay` (unit group delay ⇒ f is 1/(2πτ)) or `minus_3db` (−3 dB at f). Butterworth
is −3 dB at f. Pole sets are the textbook prototypes, cascaded as 2nd-order sections plus one
1st-order section for odd orders.

**Bandwidth** (how "free Q" is expressed): `q { value }` or `octaves { n }`, converted by the
analog relation `1/Q = 2·sinh(ln2/2 · N)` (exact for the s-domain prototype; the digital
section keeps the prototype's Q, it does not re-warp the bandwidth). The user types either; the
editor shows the other alongside (`Q 1.41 · 1.00 oct`).

**PeakQ** — what the entered Q measures; this is what "Q convention" means in practice. Two
analog families, three readings. With ω normalised to f₀, `A = 10^{|g|/40}`, `K = 10^{|g|/20}`:
- `half_gain` — the RBJ cookbook (symmetric) peaking EQ
  `H(s) = (s² + s·A/Q + 1) / (s² + s/(A·Q) + 1)`. Setting `|H|² = A²` (half the dB gain) gives
  `(1−ω²)² = ω²/Q²`, independent of A: **Q is f₀ over the bandwidth between the half-gain
  points, the same at every gain**; a cut is the exact inverse of the equal boost.
- `minus_3db_from_peak` — the same symmetric filter, Q read between the points 3 dB below the
  peak (above the dip), as some consoles print it. Solving `|H|² = A⁴/2` the same way gives
  `Δω = A / (Q·√(A⁴ − 2))`, so the entered `Q₃` maps to `Q = Q₃·A/√(A⁴ − 2)` (exact, no
  iteration); for large gains `Q₃ ≈ A·Q`, which is why one filter reads "constant-Q" by one
  yardstick and "proportional-Q" by the other. Undefined at |gain| ≤ 3.01 dB: the section is
  refused with that reason, never clamped.
- `band_pass` — the band-pass-sum family `H = 1 + (K−1)·BP(s)`, `BP = (s/Q)/(s² + s/Q + 1)`
  for a boost (poles fixed at damping 1/Q), its inverse for a cut (zeros fixed): Q is the
  added band-pass's own. The bandwidth 3 dB from the peak tends to f₀/Q at large gains; the
  half-gain bandwidth widens as √K (`(1−ω²)² = K·ω²/Q²`), so small boosts look broad.

The user picks the reading per section (editor field `Q measured: at half gain | 3 dB from
peak | of the band-pass`); the set's default (`peak_q`) is a field of the set, so entering a
console's 20 bands does not mean 20 changes. No manufacturer's gain-dependent Q law is
invented: a processor that scales Q by an undisclosed rule is matched by the `response` escape
hatch or by entering the Q measured. Readings are named by what Q measures, never by a brand.

**Shelves.** `ShelfSlope` = `q { value }` | `s { value }` (the cookbook slope, S = 1 the
steepest without overshoot; `1/Q = √((A + 1/A)(1/S − 1) + 2)`) | `octaves`. The 2nd-order
shelf is the cookbook one; the 1st-order shelf has its pole and zero a factor K apart, their geometric
mean at f. `ShelfFreq` = `midpoint` (f is where the gain is half the dB gain — cookbook)
or `corner` (f is the 3 dB-from-flat point of the flat side, which several consoles label;
solved numerically in core and converted to the midpoint). A shelf with |gain| < 3 dB has no
corner: the section is refused with that reason.

Validation (core, per section by index): 0 < f < fs/2 (digital), Q in (0, 100], |gain| ≤ 30 dB,
order per alignment, biquad stability, `response` a transfer trace; a refusal refuses the set.

## 2. Where it is computed, stored, attached

- **Math**: `ac2_core::eq` (pure: section → complex response at a list of frequencies, plus
  the parameter conversions above). The daemon evaluates a set on the curve's grid when a math
  channel's `Operand::Eq` asks; the UI never evaluates a filter (`tests/it/no_dsp.rs` stays as
  is; `ac2-scene` links only `ac2-proto`). Every edit round-trips through the daemon:
  `meas.eq` → new columns → redraw; ~1 k columns × 20 sections is microseconds.
- **Stored**: `Measurement.eq: Vec<EqSet>` (several per measurement — `EQ 1`, `EQ 2` — the
  what-if is A/B'ing two plans). `EqSet` adds `id`, `rev` (bumped on every edit) and
  `in_console: Option<EqSnapshot>` (§ below). The filter curve alone is published as a
  daemon-kept, locked trace `EQ 1` owned by the measurement (`TraceSource::Eq {meas, set}`,
  columns recomputed on edit and on load — the set is the truth, the columns a cache), so it is
  drawn, exported and compared like any trace. Session `VERSION` 28 → 29; `PROTO_VERSION` +
  `WIRE_LOCK` bump.
- **Which curves get a prediction**: only the ones the user ticks in the editor's curve list
  (`meas eq on`); a new set starts with the live curve ticked. Each ticked curve is
  one math channel; unticking deletes it. Other measurements get a copy (`meas eq copy`).
- **Bypass**: `enabled` per section and per set. A disabled section is skipped; a disabled set
  evaluates as no filter, so the A/B is one key and nothing is rebuilt. Legend `EQ 1 · bypassed`.
- **Invariants**: deleting a set deletes its predictions (the confirmation names them); a
  `response` section's trace cannot be deleted while named.

**The double-apply rule.** Once the EQ is really dialled into the console, new captures and the
live curve already contain it; multiplying them by it again predicts nonsense. ac2 cannot see
the console, so the user says when: **M** in the editor, then a confirmation (`meas eq mark`) — *"in the
console"* — snapshots the set's current sections and rev as `in_console`. From then on:
- Every prediction is of the **difference** `H_current / H_in`, never of the whole set, for
  the live curve and for captures taken while that snapshot was marked. A capture records the
  snapshot in force when it was taken (`TraceMeta.eq_in: Vec<EqSnapshot>`, one per set of the
  measurement that was marked; the snapshot's sections, not just its rev, so later edits cannot
  change what the trace is known to contain).
- A trace's basis is therefore always known: `eq_in` of that set present → predict
  `H_current / H_eq_in`; absent (taken before any mark, an import, a sweep run never marked) →
  predict the whole set. When the difference is exactly none (same sections), no curve is drawn
  and the legend says `EQ 1 in · no change`.
- Unmarking (`meas eq mark --clear`: the console was reset or bypassed) clears `in_console`;
  later captures record nothing; earlier ones keep what they recorded.
- The legend always states the basis: `Main L after EQ 1 · assumes none of it in` /
  `Main L after EQ 1 · change since rev 4`. A capture's prediction says the same about the
  capture. That label is the guard for the one case ac2 cannot detect (the console changed
  without a mark). Bypassing a marked set predicts `1 / H_in`: taking the EQ out again.

## 3. App

**Names** (one vocabulary — row, legend, cursor readout, CLI): the set is `EQ 1` (renamable),
its filter curve's row and legend `EQ 1`; a prediction reads `Main L after EQ 1` (a capture's
`Main L · capture 2 after EQ 1`; the expression column shows `× EQ 1`). A section reads
`3 · peaking 1.25 kHz −4.0 dB Q 2.0 (half-gain)`. Marked: `EQ 1 · in console (rev 4)`.

**Drawing.** On the transfer pane the prediction is a math channel result: the measurement's shade,
solid, with phase on the phase pane and the curve's coherence mask. The EQ curve is a trace in
another shade, drawn dashed and on the same magnitude axis about 0 dB, with its phase on the
phase pane (all-pass and delay sections only show there). Both have tree dots and toggle like
any curve. Overlay curves (dashed reference, a second curve in a measurement's shade) are
**[shared]** with the alignment aid's predicted sum; one scene rule for "predicted" styling.

**Editor**: **Alt+E** in Transfer scope (unbound today: plain E is phase reference there,
Shift+E notifications) and the palette entry `EQ…`; it opens on the selected measurement's set,
or creates `EQ n` when it has none. Edits apply live (nothing to submit). Keys follow the Leq dialog (`state/settings_impl.rs` `leq_key`): **↑/↓** move
between rows (as every dialog does), **Tab / Shift+Tab** between fields, **←/→** step or cycle,
**Insert / Delete** add / remove. Space, Enter, Shift+↑/↓ are not used (the stimulus cluster;
Enter stays inert as in the other dialogs). Letters act only when the focus is not a text field.
- Sections (left): **Insert** adds after the selected, **Delete** removes, **B** bypasses the
  section, **Shift+B** the whole set, **[ / ]** move it up / down.
- Fields (right): ←/→ step f in 1/24 octave, gain 0.5 dB, Q in 1/12 oct of bandwidth
  (**Shift** ×4 coarser, **Alt** finer); typing a number replaces it. The shape field cycles
  and keeps f/gain/Q where the new shape has them. The Q field shows `Q 2.0 · 0.71 oct` and its
  `Q measured` field beside it.
- Curves (bottom left): every transfer curve of the measurement (live, captures, sweep runs,
  imports filed under it), each with its basis (`assumes none in` / `change since rev 4`);
  **P** ticks / unticks "after EQ" for the row.
- Top: set name and switcher (`EQ 1 | EQ 2`, ←/→; **Shift+Insert** a new set), realisation
  (`digital 48 kHz` / `analog`), default `Q measured`, `in console (rev 4)` / `not marked`.
  **M** asks *"Mark EQ 1 rev 5 (6 sections) as in the console? Captures of Main L from now on
  record it; predictions show only later changes. M marks, Esc cancels"*; **Shift+M** in that
  confirmation clears the mark instead.
- Bottom: the daemon's refusal for the selected section, verbatim. Every change sends `meas.eq` (debounced to the frame rate); Ctrl+Z / Ctrl+Shift+Z undo as the
app's other edits do. No new Ctrl chord. **[shared]** The alignment aid also picks a transfer
curve of a measurement and draws a predicted overlay; the curve list and the "predicted"
styling should be one scene rule used by both.


## 4. CLI and text format

```
ac2 meas eq new  "Main L" [--name "EQ 1"] [--rate 96k | --analog] [--q-measured half-gain|3db-from-peak|band-pass]
ac2 meas eq add  "Main L" peaking 1.25k -4 --q 2 | --oct 0.7 [--q-measured …] [--at 3] [--set "EQ 2"]
ac2 meas eq add  "Main L" high-pass 80 --lr 4 | --bw 2 | --bessel 2 --norm phase
ac2 meas eq add  "Main L" low-shelf 120 +3 --s 1 | --q 0.7 [--corner]
ac2 meas eq add  "Main L" biquad --rate 48k b0 b1 b2 a1 a2 | response --trace "FIR v3"
ac2 meas eq set  "Main L" 3 --gain -3.5 | --off | --on      # one section by its index
ac2 meas eq rm   "Main L" 3 ;  ac2 meas eq bypass "Main L" [--off]
ac2 meas eq on   "Main L" "capture 2" [--off]               # predict for this curve
ac2 meas eq mark "Main L" [--clear]                          # "in the console"
ac2 meas eq copy "Main L" --to "Main R"
ac2 meas eq show "Main L"                                    # sections, basis per curve
ac2 meas eq export "Main L" eq1.txt [--format ac2|apo [--skip-unsupported]]
ac2 meas eq import "Main L" eq1.txt [--format ac2|apo]   # default: by content
```
`--set` picks among several sets (default: the only one, else refused naming them).

**ac2 EQ text** (`ac2_traces::text::eq`, neutral, hand-editable, one section per line):
```
# ac2 eq 1
name = EQ 1
realisation = digital 96000
peaking   f=1250 gain=-4.0 q=2.0 q_measured=half_gain
high_pass f=80 alignment=linkwitz_riley order=4
low_shelf f=120 gain=3.0 s=1.0 f_at=midpoint   off
```
Keys are the serde field names; unknown keys and shapes are refused with the line number.
The ac2 text carries the full model; the APO format below is for exchange.

**Equalizer APO / REW filter text** (`--format apo`, `ac2_traces::text::apo`): the
`Filter: ON PK Fc 1250 Hz Gain -4.0 dB Q 2.00` line grammar Equalizer APO's `config.txt` reads
and REW's "export filter settings as text" writes; one reader for both. Spellings marked † are
to be verified against Equalizer APO's documentation and a REW export before the parser is
written (the test plan writes sample files from them).

| read | maps to |
|---|---|
| `Filter:` or `Filter N:` (N ignored), then `ON` / `OFF` | a section, `enabled` = ON |
| `PK Fc Gain` + `Q q` or `BW Oct n` | `peaking`, `q_measured: half_gain` (APO's PK is the cookbook peaking EQ †); `BW Oct` by the cookbook's bandwidth relation † (whether APO warps it with `w0/sin w0` at the device rate decides the conversion) |
| `LP` / `HP Fc` | `low_pass_q` / `high_pass_q` with APO's fixed Q † (0.7071 expected: Butterworth 2nd order) |
| `LPQ` / `HPQ Fc Q` | `low_pass_q` / `high_pass_q` |
| `LS` / `HS Fc Gain` (and `LS 6dB`, `LS 12dB`, `HS …` †) | `low_shelf` / `high_shelf`: 6 dB → first order, 12 dB / plain → second order with APO's fixed slope † |
| `LSC` / `HSC [x dB] Fc Gain [Q q]` † | second-order shelf with the given Q; the `x dB` slope form as APO defines it † (refused by line until verified, not guessed) |
| `NO Fc [Q q]`, `AP Fc [Q q]`, `BP Fc [Q q]` | `notch`, `all_pass_2`, `band_pass` (APO's default Q when absent †) |
| `Preamp: g dB` | a `gain` section first in the list |
| `#` comment lines; REW's header lines (`Filter Settings file`, `Room EQ V…`, `Dated:`, `Equaliser:`, `Notes:` †); REW's `Filter N: ON None` | ignored; REW's `Equaliser:` other than `Generic` is refused (its Q conventions differ and are not guessed) |
| `Delay: t ms` † | `delay` (the same operation; it does not depend on channels) |
| any other command (`Include`, `Channel`, `Device`, `Copy`, `Stage`, `If` / `Eval`, `GraphicEQ`, `Convolution`, …) or an unknown filter type | **refused with the line number**: they select channels / devices or reference other files, so the list would not be the filter one output sees |

An import refused anywhere imports nothing. Export writes the set as APO lines, and
**only what the grammar expresses exactly**:
- `peaking`: `half_gain` as is; `minus_3db_from_peak` converted to the cookbook Q by the
  closed form (same filter); `band_pass` reading refused (another filter family).
- `high_pass` / `low_pass` Butterworth or Linkwitz-Riley of even order: cascaded `HPQ` / `LPQ`
  lines at the same Fc with the prototype's section Qs (exact: APO prewarps each biquad at its
  Fc, which is the section's f). Odd orders refused unless a 1st-order line exists †. Bessel
  refused: its sections sit at different frequencies, so per-line prewarping differs from
  ac2's near Nyquist.
- Shelves: second order → `LSC` / `HSC` with Q (S and the corner reading converted exactly);
  first order → `LS 6dB` † only if APO's definition matches; else refused.
- `notch`, `all_pass_2`, `band_pass`, `high_pass_q`/`low_pass_q` as lines; `gain` sections summed
  into one `Preamp`; `delay` → `Delay` †; `polarity` (channel `Copy` in APO), `all_pass_1`,
  `biquad`, `response` refused; an `analog` set refused (APO filters are digital); a digital
  set's rate written as a comment (APO runs at the device's rate).
- Disabled sections are written `OFF`. The in-console mark is not a filter and is not written
  (the reply says so).
A refused section is named with its reason (`3 · bessel high-pass: per-line prewarp differs`)
and nothing is written; `--skip-unsupported` writes the rest and lists every skipped section as
a `#` comment at the top of the file. Never approximated.

## 5. Tests

- `ac2-core` `eq` (analytic, quick tier): every shape against its closed form at f₀ and
  decades either side — peaking: |H(f₀)| = gain, half-gain points at the Q-bandwidth for
  `half_gain` at ±3, ±12 dB, the −3 dB-from-peak points for `minus_3db_from_peak` at ±6, ±15 dB
  (refused at ±3 dB), √K widening for `band_pass`; boost × equal cut = 0 dB (symmetric);
  Butterworth −3 dB at f and n·6 dB/oct asymptote; LR −6 dB at f, LR4 HP + LP sum flat with
  0 dB and all-pass phase; Bessel norms (phase-matched asymptote, unit delay, −3 dB); all-pass
  |H| = 1 and −180°/−360° at f; shelf midpoint and S = 1 monotonic; Q ↔ octaves ↔ S
  round-trips; digital vs analog agree within 0.05 dB below fs/10 and the digital peaking
  matches the cookbook coefficient formulas; columns ≥ fs/2 are NaN; unstable biquad refused.
- `tools/refgen/sets/eq.py`: golden vectors from scipy (`signal.bilinear_zpk`, `freqz`,
  `freqs`, `butter`/`bessel(norm=…)`) and the cookbook formulas written out independently, so
  the Rust and the reference do not share code; compared with `ac2-testkit` tolerances.
- `ac2-traces`: ac2 text round-trip, every refusal by line. APO: sample files written from
  Equalizer APO's documentation and a REW export (no rig data), one per † spelling above;
  import → export → import identity for every expressible shape; every export refusal named;
  `--skip-unsupported` comments; an APO `Channel` / `Include` line refused by number.
- `ac2-proto`: wire fixtures for `EqShape`, `Operand::Eq`, `TraceSource::Eq`, WIRE_LOCK bumped.
- `ac2d` `tests/it/eq.rs` from an empty daemon on the fake backend: a transfer measurement on a
  flat fake path; `eq new` + peaking −6 dB @ 1 kHz → the prediction reads −6 dB at 1 kHz and
  0 dB at 100 Hz, its phase the closed form; section and set bypass; a capture toggled on gets its
  own prediction; **double-apply**: mark, insert the same peaking in the fake path, capture → the
  capture records `eq_in`, its and the live prediction equal the curve (`no change`); edit to
  −8 dB → both predict −2 dB more; an unmarked import predicts the whole set; a capture of a
  prediction keeps its values; session save/load recomputes the columns; refusals (f ≥ fs/2, LR order 3).
- `ac2-cli` `tests/it/eq_rig.rs`: command shapes and refusals by message; import/export.
- `ac2-ui` `tests/it/embedded/eq.rs`: Alt+E from an empty daemon, add a peaking section by keys,
  the scene's legend reads `Main L after EQ 1 · assumes none of it in` and `EQ 1`, B
  bypasses, P ticks a capture, M + M turns it into `no change`; one GPU snapshot
  `transfer_eq` for the final look.

## 6. Later (not in the first cut)

- **Plot handles**: a dot per peaking / shelf / pass section at (f, gain) while the editor is
  open; drag moves f and gain, Shift-drag or the wheel Q; the daemon still draws.
- **Linked pair**: one set shared by L and R of a stereo processor. First cut: `copy` only.
- **Spectrum / RTA × EQ**: math channels already take a stored transfer B for levels, but what
  a level becomes through a filter depends on the programme material (the RTA band gain is
  exact only for pink noise, `math-channels.md` *Levels*), so it waits for a use case.
