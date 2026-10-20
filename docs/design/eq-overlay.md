# EQ what-if overlay

Status: design, not built. Operator decisions 2026-10-20: the user lists the filters they plan
to set on a console or processor; ac2 draws the measured transfer function multiplied by them
(**after EQ**, the prediction) and the filters' response alone (**EQ**). There is no single Q
convention and no single shape list, so neither is hard-coded: the editor exposes each shape's
parameters and how its Q is meant. To be built after the two-system alignment aid
(`alignment.md`, in flight); shared parts are marked **[shared]**.

## The idea in one paragraph

An EQ set is a stored transfer trace whose columns the daemon computes from a list of filter
sections (`TraceSource::Eq`), and "after EQ" is a math channel `subject × EQ` — the cascade
`math-channels.md` already defines (delays add, the filter's is 0; the lower γ² carried, the
filter's is 1). So the filter curve is drawn, listed, coloured, exported and compared like any
trace, and the prediction updates live, is captured with Ctrl+1 … 9 and refuses with reasons
like any math channel. What is new: the section model, its evaluation in `ac2-core`, the
`eq.*` commands, the editor and a text format.

## 1. Filter model

`EqSet { name, rate: Realisation, enabled: bool, sections: Vec<EqSection> }`,
`EqSection { enabled: bool, shape: EqShape }`. Frequencies in Hz, gains in dB, typed
(`Hz`, `Db`).

**Realisation** (per set — a processor runs every section the same way):
- `digital { rate: Hz }` (default: the session rate; the user sets the *processor's* rate,
  48 / 96 kHz, which is not necessarily ac2's capture rate). Each section is designed in the s
  domain and mapped by the bilinear transform prewarped at its own f₀, then evaluated exactly
  as `H(e^{j2πf/fs})` at every column — a polynomial ratio, no FFT, no impulse response. This
  is what most consoles do, so it reproduces their cramping near Nyquist: a 16 kHz peak with
  Q 2 at 48 kHz is visibly narrower and asymmetric, and that difference is exactly what the
  user needs to see before setting it. Columns at or above fs/2 are gaps (NaN), not zero.
- `analog`: the s-domain prototype evaluated at `s = j2πf` — for decramped / FIR-based
  processors that match the analog response, and as the reference the digital form departs
  from.

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
  the parameter conversions above). The daemon evaluates the set on the subject's grid and
  stores the columns as the EQ trace (`mag_db`, `phase_deg`, coherence `None`, `delay` 0); the
  math channel does the multiply. The UI never evaluates a filter (`tests/it/no_dsp.rs` stays as
  is; `ac2-scene` links only `ac2-proto`). A drag (below) round-trips through the daemon:
  `eq.set` → new columns → redraw; at ~1 k columns × 20 sections that is microseconds.
- **Stored**: `TraceSource::Eq { set: EqSet }` on the EQ trace; the session saves the set and
  the columns are recomputed on load (the set is the truth, the columns a cache). An `EqSet`
  edit re-evaluates and restarts dependent math channels with the new columns, the path a mic
  curve applied to a stored operand already takes. Session `VERSION` 28 → 29;
  `PROTO_VERSION` bump with `fixtures/protocol/WIRE_LOCK` and the protocol fixtures.
- **Attachment** (operator note 2026-10-20: a measurement holds a live curve, captures, sweep
  runs and imports). The EQ trace is **owned by the measurement** (`TraceOwner::meas`, listed
  under it); it is applied to an explicit list of **subjects**, each one math channel
  `subject × EQ`. Creating an EQ set from a measurement's header or live curve makes the live
  curve its first subject; from a stored trace (capture, sweep run, import), that trace. **Alt+E**
  on another selected transfer curve adds it as a subject ("after EQ" for a capture beside the
  live one). Reason for explicit subjects over "whatever is shown": the prediction of a stored
  capture is a separate curve the user compares with the live one — two predictions at once
  must exist, and an implicit rule would make curves appear and vanish with visibility. See
  open question 1.
- **Several sets per measurement**: yes (`EQ 1`, `EQ 2` …) — the what-if is A/B'ing two plans.
- **Bypass**: `enabled` per section and per set. A disabled section is skipped; a disabled set
  evaluates to 0 dB / 0° (the EQ curve flat, "after EQ" equal to the subject) so the A/B is one
  key and nothing is rebuilt. The legend says `EQ 1 · bypassed`.
- **Copy / link**: `eq copy <set> --to <meas>` makes an independent copy under another
  measurement; `eq apply <set> --to <curve>` adds a subject of any owner, so one set can be
  the same filter on L and R (a linked processor) — editing it moves both predictions.
- **Invariants** (as for math operands): an EQ trace with subjects cannot be deleted without
  deleting its predictions (the delete confirmation names them); a `response` section's trace
  cannot be deleted while named.

## 3. App

**Names** (one vocabulary — row, legend, cursor readout, CLI): the set is `EQ 1` (renamable);
its trace row and legend read `EQ 1`; a prediction reads `Main L after EQ 1` (math channel name
following the expression; the expression column still shows `Main L × EQ 1`). A section reads
`3 · peaking 1.25 kHz −4.0 dB Q 2.0 (half-gain)`.

**Drawing.** On the transfer pane the prediction is a math channel result: its owner's shade,
solid, with phase on the phase pane and the subject's coherence mask. The EQ curve is a trace in
another shade, drawn dashed and on the same magnitude axis about 0 dB, with its phase on the
phase pane (all-pass and delay sections only show there). Both have tree dots and toggle like
any curve. Overlay curves (dashed reference, a second curve in a measurement's shade) are
**[shared]** with the alignment aid's predicted sum; one scene rule for "predicted" styling.

**Editor** (Alt+E with no other transfer curve selected opens it for the selected set, or
creates `EQ n` when there is none; palette `EQ…`). Alt+E is free in the key table; check the
alignment aid did not claim it. A dialog in the existing dialog style, keyboard first:
- Left: the section list (↑/↓ select, **N** new section after the selected, **X** delete,
  **Space** bypass section, **Shift+Space** bypass set, **Shift+↑/↓** reorder).
- Right: the selected section's fields (Tab / Shift+Tab move, ←/→ step a value: f in 1/24
  octave, gain 0.5 dB, Q in 1/12 oct of bandwidth; **Shift** ×4 coarser, **Alt** finer; typing
  a number replaces it). The shape field cycles with ←/→ and keeps f/gain/Q where the new shape
  has them. The Q field shows `Q 2.0 · 0.71 oct` and its definition field beside it.
- Top: set name, realisation (`digital 48 kHz` / `analog`), default peak form, subjects.
- Bottom: the daemon's refusal for the selected section, verbatim.
Every change sends `eq.set` (debounced to the frame rate); Ctrl+Z / Ctrl+Shift+Z undo as the
app's other edits do. No new Ctrl chord.

**Plot handles (option, not in the first cut).** A dot per enabled peaking/shelf/pass section at
(f, gain) on the magnitude pane while the editor is open; drag moves f and gain, Shift-drag
(or wheel) changes Q. The app sends the new parameters, the daemon draws the curve. Built only
if the operator wants it after using the keyboard editor (open question 3).

## 4. CLI and text format

```
ac2 eq new  --on "Main L" [--name "EQ 1"] [--rate 96k | --analog] [--form half-gain|from-peak]
ac2 eq add  "EQ 1" peaking 1.25k -4 --q 2 | --oct 0.7 [--form …] [--at 3]
ac2 eq add  "EQ 1" high-pass 80 --lr 4 | --bw 2 | --bessel 2 --norm phase
ac2 eq add  "EQ 1" low-shelf 120 +3 --s 1 | --q 0.7 [--corner]
ac2 eq add  "EQ 1" biquad --rate 48k b0 b1 b2 a1 a2 | response --trace "FIR v3"
ac2 eq set  "EQ 1" 3 --gain -3.5 | --off | --on        # one section by its index
ac2 eq rm   "EQ 1" 3 ;  ac2 eq bypass "EQ 1" [--off]
ac2 eq apply "EQ 1" --to "Main L · capture 2" ;  ac2 eq copy "EQ 1" --to "Main R"
ac2 eq show "EQ 1"                                     # sections, as the editor reads them
ac2 eq export "EQ 1" eq1.txt ;  ac2 eq import eq1.txt --on "Main L" [--format rew]
```
Under `eq` rather than `meas eq` because a set is a trace with subjects of any owner, not a
setting of one measurement; `ac2 meas list` lists the predictions, `ac2 trace list` the sets.

**ac2 EQ text** (`ac2_traces::text::eq`, neutral, hand-editable, one section per line):
```
# ac2 eq 1
name = EQ 1
realisation = digital 96000
peaking   f=1250 gain=-4.0 q=2.0 form=half_gain
high_pass f=80 alignment=linkwitz_riley order=4
low_shelf f=120 gain=3.0 s=1.0 f_at=midpoint   off
```
Keys are the serde field names; unknown keys and shapes are refused with the line number.
**REW import** (cheap, worth it): REW's "filter settings" text lists
`Filter  1: ON  PK  Fc 1250 Hz  Gain -4.0 dB  Q 2.00` lines under an `Equaliser:` header;
read `PK`, `LS`/`HS` (with and without `Q`/dB slope), `LP`/`HP` (`LPQ`/`HPQ`), `NO`, `AP`,
mapping to `peaking { form: symmetric }` etc. for the `Generic` equaliser; another
equaliser type, or a filter type not mapped, refuses with the line (REW's per-equaliser Q
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
- `ac2-proto`: wire fixtures for `EqShape` / `TraceSource::Eq`, WIRE_LOCK bumped.
- `ac2d` `tests/it/eq.rs` from an empty daemon on the fake backend: a transfer measurement on a
  flat fake path; `eq new` + peaking −6 dB @ 1 kHz → the prediction reads −6 dB at 1 kHz and
  0 dB at 100 Hz, its phase the closed form; section bypass → 0 dB; set bypass; a second subject
  (a capture) gets its own prediction; edit moves both; a capture of the prediction keeps the
  values; session save/load recomputes the columns; refusals (f ≥ fs/2, LR order 3).
- `ac2-cli` `tests/it/eq_rig.rs`: command shapes and refusals by message; import/export.
- `ac2-ui` `tests/it/embedded/eq.rs`: Alt+E from an empty daemon, add a peaking section by keys,
  the scene's legend reads `Main L after EQ 1` and `EQ 1`, Space bypasses; one GPU snapshot
  `transfer_eq` for the final look.

## 6. Open questions for the operator

1. Subjects: an EQ set applies to an explicit list of curves (live curve by default, Alt+E adds a
   capture or import). Alternative: it applies automatically to every transfer curve of its
   measurement that is shown. Explicit is proposed — keep it?
2. Peaking forms: `half-gain (cookbook)` and `from peak` (band-pass sum) are the two offered.
   Do you need a third Q measure — Q read at 3 dB below the peak of the *symmetric* form, which
   some consoles print — or is "enter the Q you measured" enough?
3. Plot handles (drag f / gain, Shift-drag Q) in the first cut, or keyboard editor only first?
4. Key: Alt+E for the editor and for "apply to the selected curve" — acceptable, or another
   plain/Shift key?
5. Should the predicted curves also be offered on spectrum / RTA panes (`spectrum × EQ`, which
   math channels already support for a stored transfer B), or transfer only for now?
