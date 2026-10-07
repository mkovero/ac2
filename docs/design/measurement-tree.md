# The measurement tree: a measurement owns its traces

Status: implemented (protocol 24, session format 11). The UI's sidebar is
`ac2_scene::meas_list::tree_rows`; ownership is kept by the daemon (`control/owners.rs`).

## Why

The operator compared captures of a transfer function with math on them, and saw two
unrelated lists: *Measurements* and *Traces*. A capture is part of the measurement it came
from; math made while looking at one measurement belongs to it; a sweep is a measurement
whose results are its runs. So the sidebar is one tree, and every stored result is filed
under a measurement:

```
TF  Main L            running
  ├ Main L (live)
  ├ pre-EQ            capture
  ├ post-EQ           capture
  └ pre ÷ post        math (live)
SWEEP  Genelec 1 m    2 runs
  ├ Run 1             sweep run · 3 s −50.0 dBFS
  └ Run 2             sweep run · 3 s −50.0 dBFS
SPL  FOH SPL          running
Imported              1 trace
  └ 1083 94cm         imported
```

## Model

- `TraceOwner` = `meas {meas}` | `imported`. Every stored trace has one
  (`TraceEdit.owner`), and so has every math channel (`MathConfig.owner`). An owner is an
  existing measurement other than a math channel — the daemon refuses anything else, so a
  tree is never deeper than two levels and never cyclic.
- Who files what:

  | result | owner |
  |---|---|
  | capture (`trace.capture`, Ctrl+1 … 9) | the measurement it came from (`TraceSource::Captured.meas` says so too; the owner is where it is *listed*, the source where it *came from*, and the two part when it is moved) |
  | math channel | the measurement selected when Shift+M was pressed (a selected trace's or math channel's owner); CLI: `--under`, else its first operand / that trace's owner |
  | math capture | the math channel's owner |
  | sweep run (`sweep.run`) | its sweep measurement |
  | `trace.average` | the owner its inputs share, else `imported` |
  | import, target | `imported` |

- Moving is an edit, nothing else: `trace.update` with another owner (a locked trace moves
  too: the lock protects the curve, not where it is listed), `meas.update` of a math channel
  with another owner — the daemon applies that in place, without restarting the job or
  bumping `config_rev`, since the result does not change.
- A measurement with traces or math channels cannot become a math channel (math owns
  nothing).

## Deleting a measurement

`meas.delete {meas, traces: keep | delete}` — the client says every time what becomes of what
the measurement owns. `keep` moves its traces and math channels to `imported`; `delete`
deletes them with it (locked traces included: the operator chose it). Refused, with nothing
changed, while a math channel that **stays** would lose an operand (the measurement itself,
or a trace deleted with it), and while a run of the measurement plays. The app asks *Keep
them (move to Imported) / Delete them too / Cancel* with Keep the default; an answer the
daemon would refuse is shown with its reason and cannot be taken (Cancel becomes the
default when Keep is refused). The CLI needs `--keep-traces` or `--delete-traces` when the
measurement owns anything.

## Sweep measurements

`MeasKind::Sweep {config: SweepConfig}` holds a sweep's settings: reference and measurement
inputs, outputs, typed level, `EssSpec`, repeats, gate, silence after. It has no job and
publishes no stream (`MeasKind::stream()` is `None`): `meas.start/stop/freeze/reset` and
`trace.capture` of it are `invalid`. Creating it plays nothing.

`sweep.run {lease_token, meas, name}` replaces `ir.capture`: the same checks and the same
arm → fire safety (lease, armed by the caller, typed level against the ceiling at every
run, session inputs and outputs), the settings taken from the measurement. Each run is a
`sweep` trace under it, named `Run <n>` (n = one more than the highest run number of its
stored runs, `TraceSource::Sweep.number`) unless named. Editing the measurement
(`meas.update`) changes the next run.

In the app, Shift+S makes a sweep measurement (the dialog's Enter arms nothing); Space on
the sweep pane arms a run of the selected sweep measurement (the selected one, else the
selected run's owner, else the pane's); Enter plays it. A level or outputs changed while
armed are stored into the measurement before the run (`meas.update`, then `sweep.run`), so
the run plays what the top bar named. The sweep pane shows the selected run, else the
newest run of the selected sweep measurement.

`ac2 ir capture` keeps its flags: it runs the sweep measurement whose settings equal the
flags exactly, making one (`Sweep <n>`) when there is none. Repeating a command is a re-run
of the same measurement — what Space is in the app — while different settings are a
different measurement; a new measurement per invocation would litter the tree with
identical measurements of one run each. `ac2 meas new sweep …` + `ac2 sweep run <meas>` is
the explicit form.

## Sessions and autosave

Owners and sweep measurements are part of the saved metadata, so sessions and the autosave
keep them: format 11. A session whose owners name no measurement of it (or a math channel)
is refused as invalid. Older sessions are refused with their version, as always; an export
re-imported is an import (`imported`), a sweep export a run under Imported until moved.

## Display

- One tree, `ac2_scene::meas_list::tree_rows`: measurements (not math channels) by id,
  then Imported when it holds anything. Under a measurement: its live curve (transfer,
  spectrum, RTA), its stored traces in list order (slot, then oldest first), its math
  channels. A sweep measurement's header says `no runs yet`, `2 runs`, `playing 1/2`,
  `analysing`. Folding is this app's display (`AppState::collapsed`).
- The selection rule is unchanged (PLAN §8.2): the item selected last has the keys — a
  measurement (its header, its live curve or a math channel's row) or a stored trace.
  Shift+A toggles the selected item's whole group (live curves are this app's display, the
  traces' visibility the daemon's). V steps through the traces in the tree's order.
- Every row that stands for a curve — a live curve, a stored trace or sweep run, a math
  channel's result — has a dot in exactly the colour the panes draw that curve in, filled
  when shown, a ring when hidden; a click on it shows / hides the curve. A curve no visible
  pane draws keeps its colour. Headers (a measurement, Imported) stand for a group and have
  no dot. The list's header says what filled and ring mean.
- The transfer pane's legend groups by the same order: a measurement's live curve, its
  traces and its math channels together.

## Colour families

One rule, `ac2_scene::families::curve_colours`, colours every curve for the panes, legends,
cursor readouts and tree dots:
- Each measurement (not a math channel) owns a **hue** from the theme's eight families
  (`Theme::families`, per theme: Okabe & Ito's six hues at lightnesses that stay apart in
  simulated protan / deutan vision, then violet and cyan). Assignment follows the
  measurement id, so deleting one repaints no other: id `n` prefers family `(n − 1) mod 8`;
  walking the measurements by id, one whose family is taken takes the next free one. Past
  eight measurements the families repeat.
- Its **live curve** is the family's base colour. Its stored traces (captures, sweep runs,
  in tree order: slot, then oldest first) and then its math channels' results take the
  family's **shades**: the base's OKLCH hue and chroma at 0.1 steps of OKLab lightness,
  nearest first, lighter before darker, kept inside the theme's lightness range (3:1
  contrast against the plot, and still showing the hue). Three shades, then they repeat.
  A sweep measurement has no live curve: its first run takes the base.
- **Imported** traces (and those of a deleted measurement kept) are the neutral grey family.
  A trace or math channel **moved** to another measurement takes that measurement's family
  and its place in it.
- The daemon's per-trace colour (`TraceEdit.color`, on the wire and in session files) is
  not used for drawing; it stays on the wire unchanged for scripts.

