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

`EqSet { id, name, rev, realisation, peak_form, enabled, in_console, sections }`,
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
| `peaking` | `f`, `gain`, `bw: Bandwidth`, `form: PeakForm` |
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

**PeakForm** — the two analog peaking families, which is what "Q convention" means in practice.
With ω normalised to f₀, `A = 10^{g/40}`, `K = 10^{g/20}`:
- `symmetric` — the RBJ cookbook peaking EQ:
  `H(s) = (s² + s·A/Q + 1) / (s² + s/(A·Q) + 1)`. Setting `|H|² = A²` (half the dB gain)
  gives `(1−ω²)² = ω²/Q²`, independent of A: **Q is the bandwidth at the half-gain points,
  the same at every gain**, and a cut is the exact inverse of the equal boost. Measured instead
  at 3 dB below the peak, the poles' damping `1/(AQ)` makes the hump narrow as the boost grows
  (effective Q ≈ A·Q) — which is why the same filter is sold as "constant-Q" by one
  manufacturer and behaves "proportional-Q" by another's yardstick.
- `constant_peak` — the band-pass-sum form `H = 1 + (K−1)·BP(s)`, `BP = (s/Q)/(s² + s/Q + 1)`
  for a boost (poles fixed at damping 1/Q), its inverse for a cut (zeros fixed). The bandwidth
  3 dB from the peak tends to f₀/Q at large gains (Rane's "constant-Q" sense); the half-gain
  bandwidth widens as √K (`(1−ω²)² = K·ω²/Q²`), so small boosts look broad.

The user selects the form per section (editor field `Q is: half-gain (cookbook) | from peak`);
the set's default form is a field of the set, so entering a console's 20 bands does not mean
20 changes. No third, manufacturer-specific "proportional-Q" law is invented: a processor that
scales Q with gain by an undisclosed rule is matched by the `response` escape hatch or by the
user entering the Q they measured. Each form is labelled by what its Q *measures*, never by a
brand.

**Shelves.** `ShelfSlope` = `q { value }` | `s { value }` (the cookbook slope, S = 1 the
steepest without overshoot; `1/Q = √((A + 1/A)(1/S − 1) + 2)`) | `octaves`. The 2nd-order
shelf is the cookbook one; the 1st-order shelf has its pole and zero a factor K apart, their geometric
mean at f. `ShelfFreq` = `midpoint` (f is where the gain is half the dB gain — cookbook)
or `corner` (f is the 3 dB-from-flat point of the flat side, which several consoles label;
solved numerically in core and converted to the midpoint). A shelf with |gain| < 3 dB has no
corner: the section is refused with that reason.

Validation (core, reported per section by index): 0 < f < fs/2 for digital, Q in (0, 100],
|gain| ≤ 30 dB, order range per alignment, biquad stability, `response` trace exists and is a
transfer trace. A refused section makes the set refuse; nothing is clamped silently.

## 2. Where it is computed, stored, attached

- **Math**: `ac2_core::eq` (pure: section → complex response at a list of frequencies, plus
  the parameter conversions above). The daemon evaluates a set on the curve's grid when a math
  channel's `Operand::Eq` asks; the UI never evaluates a filter (`tests/it/no_dsp.rs` stays as
  is; `ac2-scene` links only `ac2-proto`). A drag (below) round-trips through the daemon:
  `meas.eq` → new columns → redraw; ~1 k columns × 20 sections is microseconds.
- **Stored**: `Measurement.eq: Vec<EqSet>` (several per measurement — `EQ 1`, `EQ 2` — the
  what-if is A/B'ing two plans). `EqSet` adds `id`, `rev` (bumped on every edit) and
  `in_console: Option<EqSnapshot>` (§ below). The filter curve alone is published as a
  daemon-kept, locked trace `EQ 1` owned by the measurement (`TraceSource::Eq {meas, set}`,
  columns recomputed on edit and on load — the set is the truth, the columns a cache), so it is
  drawn, exported and compared like any trace. Session `VERSION` 28 → 29; `PROTO_VERSION` bump
  with `fixtures/protocol/WIRE_LOCK` and the protocol fixtures.
- **Which curves get a prediction**: only the ones the user toggles on (**Alt+E** on a selected
  curve of the measurement, `--on` in the CLI); a new set starts with the live curve on. Each
  toggled curve is one math channel; toggling off deletes it. Curves of other measurements never
  get this set (copy it: `meas eq copy`).
- **Bypass**: `enabled` per section and per set. A disabled section is skipped; a disabled set
  evaluates as no filter, so the A/B is one key and nothing is rebuilt. Legend `EQ 1 · bypassed`.
- **Invariants**: deleting a set deletes its predictions (the confirmation names them); a
  `response` section's trace cannot be deleted while named.

**The double-apply rule.** Once the EQ is really dialled into the console, new captures and the
live curve already contain it; multiplying them by it again predicts nonsense. ac2 cannot see
the console, so the user says when: **Shift+Enter** in the editor (`meas eq mark`) — *"in the
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

**Editor** (**Alt+Q**, palette `EQ…`) opens on the selected measurement's set, or creates
`EQ n` when it has none. Alt+E (toggle the selected curve's prediction) and Alt+Q are free in
the key table; check the alignment aid did not claim them. Keyboard first:
- Left: the section list (↑/↓ select, **N** new section after the selected, **X** delete,
  **Space** bypass section, **Shift+Space** bypass set, **Shift+↑/↓** reorder).
- Right: the selected section's fields (Tab / Shift+Tab move, ←/→ step a value: f in 1/24
  octave, gain 0.5 dB, Q in 1/12 oct of bandwidth; **Shift** ×4 coarser, **Alt** finer; typing
  a number replaces it). The shape field cycles with ←/→ and keeps f/gain/Q where the new shape
  has them. The Q field shows `Q 2.0 · 0.71 oct` and its definition field beside it.
- Top: set name and switcher (`EQ 1 | EQ 2`, ←/→ on the field), realisation (`digital 48 kHz` /
  `analog`), default peak form, `in console (rev 4)` / `not marked` (**Shift+Enter** marks).
- Bottom: the daemon's refusal for the selected section, verbatim.
Every change sends `meas.eq` (debounced to the frame rate); Ctrl+Z / Ctrl+Shift+Z undo as the
app's other edits do. No new Ctrl chord. **[shared]** The alignment aid also picks a transfer
curve of a measurement and draws a predicted overlay; the curve picker and the "predicted"
styling should be one scene rule used by both.

**Plot handles (option, not in the first cut).** A dot per enabled peaking/shelf/pass section at
(f, gain) on the magnitude pane while the editor is open; drag moves f and gain, Shift-drag
(or wheel) changes Q. The app sends the new parameters, the daemon draws the curve. Built only
if the operator wants it after using the keyboard editor (open question 2).

## 4. CLI and text format

```
ac2 meas eq new  "Main L" [--name "EQ 1"] [--rate 96k | --analog] [--form half-gain|from-peak]
ac2 meas eq add  "Main L" peaking 1.25k -4 --q 2 | --oct 0.7 [--form …] [--at 3] [--set "EQ 2"]
ac2 meas eq add  "Main L" high-pass 80 --lr 4 | --bw 2 | --bessel 2 --norm phase
ac2 meas eq add  "Main L" low-shelf 120 +3 --s 1 | --q 0.7 [--corner]
ac2 meas eq add  "Main L" biquad --rate 48k b0 b1 b2 a1 a2 | response --trace "FIR v3"
ac2 meas eq set  "Main L" 3 --gain -3.5 | --off | --on      # one section by its index
ac2 meas eq rm   "Main L" 3 ;  ac2 meas eq bypass "Main L" [--off]
ac2 meas eq on   "Main L" "capture 2" [--off]               # predict for this curve
ac2 meas eq mark "Main L" [--clear]                          # "in the console"
ac2 meas eq copy "Main L" --to "Main R"
ac2 meas eq show "Main L"                                    # sections, basis per curve
ac2 meas eq export "Main L" eq1.txt ;  ac2 meas eq import "Main L" eq1.txt [--format rew]
```
`--set` picks among several sets (default: the only one, else refused naming them).

**ac2 EQ text** (`ac2_traces::text::eq`, neutral, hand-editable, one section per line):
```
# ac2 eq 1
name = EQ 1
realisation = digital 96000
peaking   f=1250 gain=-4.0 q=2.0 form=symmetric
high_pass f=80 alignment=linkwitz_riley order=4
low_shelf f=120 gain=3.0 s=1.0 f_at=midpoint   off
```
Keys are the serde field names; unknown keys and shapes are refused with the line number.
**REW import** (cheap): its filter-settings text (`Filter  1: ON  PK  Fc 1250 Hz  Gain -4.0 dB
Q 2.00` under an `Equaliser:` header) maps `PK LS HS LP HP LPQ HPQ NO AP` for the `Generic`
equaliser; another equaliser or an unmapped type refuses with the line (its per-equaliser Q
conventions are not guessed). Export writes the ac2 text only.

## 5. Tests

- `ac2-core` `eq` (analytic, quick tier): every shape against its closed form at f₀ and
  decades either side — peaking: |H(f₀)| = gain, half-gain points at the Q-bandwidth for
  `symmetric` at ±3, ±12 dB, √K widening for `constant_peak`; boost × equal cut = 0 dB (symmetric);
  Butterworth −3 dB at f and n·6 dB/oct asymptote; LR −6 dB at f, LR4 HP + LP sum flat with
  0 dB and all-pass phase; Bessel norms (phase-matched asymptote, unit delay, −3 dB); all-pass
  |H| = 1 and −180°/−360° at f; shelf midpoint and S = 1 monotonic; Q ↔ octaves ↔ S
  round-trips; digital vs analog agree within 0.05 dB below fs/10 and the digital peaking
  matches the cookbook coefficient formulas; columns ≥ fs/2 are NaN; unstable biquad refused.
- `tools/refgen/sets/eq.py`: golden vectors from scipy (`signal.bilinear_zpk`, `freqz`,
  `freqs`, `butter`/`bessel(norm=…)`) and the cookbook formulas written out independently, so
  the Rust and the reference do not share code; compared with `ac2-testkit` tolerances.
- `ac2-traces`: text format round-trip, every refusal by line; a REW sample file (written by
  hand from REW's documented format, no rig data).
- `ac2-proto`: wire fixtures for `EqShape`, `Operand::Eq`, `TraceSource::Eq`, WIRE_LOCK bumped.
- `ac2d` `tests/it/eq.rs` from an empty daemon on the fake backend: a transfer measurement on a
  flat fake path; `eq new` + peaking −6 dB @ 1 kHz → the prediction reads −6 dB at 1 kHz and
  0 dB at 100 Hz, its phase the closed form; section and set bypass; a capture toggled on gets its
  own prediction; **double-apply**: mark, insert the same peaking in the fake path, capture → the
  capture records `eq_in`, its and the live prediction equal the curve (`no change`); edit to
  −8 dB → both predict −2 dB more; an unmarked import predicts the whole set; a capture of a
  prediction keeps its values; session save/load recomputes the columns; refusals (f ≥ fs/2, LR order 3).
- `ac2-cli` `tests/it/eq_rig.rs`: command shapes and refusals by message; import/export.
- `ac2-ui` `tests/it/embedded/eq.rs`: Alt+Q from an empty daemon, add a peaking section by keys,
  the scene's legend reads `Main L after EQ 1 · assumes none of it in` and `EQ 1`, Space
  bypasses, Shift+Enter turns it into `no change`; one GPU snapshot
  `transfer_eq` for the final look.

## 6. Open questions for the operator

1. Peaking forms: `half-gain (cookbook)` and `from peak` (band-pass sum) are the two offered.
   Do you need a third Q measure — Q read at 3 dB below the peak of the *symmetric* form, which
   some consoles print — or is "enter the Q you measured" enough?
2. Plot handles (drag f / gain, Shift-drag Q) in the first cut, or keyboard editor only first?
3. Keys: Alt+Q opens the editor, Alt+E toggles "after EQ" on the selected curve, Shift+Enter
   in the editor marks "in the console" — acceptable, or other plain/Shift keys?
4. A linked pair (one set feeding L and R of a stereo processor): `copy` makes independent
   sets; is a shared set across measurements wanted, or is copy enough?
5. Should predictions also be offered on spectrum / RTA panes (`spectrum × EQ`, which math
   channels already support for a stored transfer B), or transfer only for now?
