# Pane curves: a pane draws the curves picked for it, from any rig

Status: design, not built. Operator decisions 2026-11-01 (D1 … D9), **revised 2026-11-02
(D10 … D14)**: panes have **no chip** and no pane list — "remove it as whole from all panes
… lets not do billions of different ways changing the pane view"; the chip was on some
panes only, which made navigation "incredibly confusing". What a pane shows changes one way:
**a row of the measurement list dragged onto it**. Clicking a measurement row still brings
its own panes (P1, unchanged). C's tick list stays the one way to draw several curves in a
pane. The 2026-11-01 decisions stand where D10 … D14 do not replace them; all are settled
and listed at the end.

## Why the current list misleads

The chip list (`pane_menu_rows`, `state/panes/ops.rs`; rigs from `cross_rig.rs`) offers one
row per measurement; picking it sets `PaneContent::Meas { meas }` and the pane draws that
measurement's whole group (live curve, math filed under it, its shown traces) plus its
compare list. A row reads as one curve and brings several; a stored trace cannot be picked at
all (only dragged), and another rig's trace not even that. Besides that, the list as built:

- lists every measurement whatever the pane draws (an SPL pane offers transfer and spectrum
  rows; a pick turns the pane's kind in one step), and the kinds of pane (`pane  Sweep /
  distortion`) and *New measurement…* are appended after the last rig's rows, so they read
  as that rig's;
- names inputs by the backend's port (`capture_3 · line (in 3)`) although the Inputs list
  names the same channel `in 3` with its mic or kind;
- lists a pair's sides a second time on their own (`programme_menu_rows`), apart from the
  other single inputs;
- prefixes rows with the tree's kind tags `TF`, `FFT`, `RTA`, `SPL`, `MATH`, `SWEEP`
  (`meas_list::kind_tag`) and with `meter` / `pane`, words the pane titles do not use.

And (2026-11-02) a pane's view changed five ways: the chip, a dragged row, palette *Choose
pane measurement…*, a measurement row click without an own layout (the focused pane) and
Shift+drop (compare). Not every pane carries a chip, so the way to change a pane depended
on the pane; the operator had never seen the palette entry.

## 1. What a pane holds

Every content reference names the rig whose id it is, because ids are per daemon
(multi-rig.md §1); `LayoutPane.rig` goes and the rig moves into the content. `content`
becomes required.

```rust
pub enum PaneContent {
    /// Any pane kind: nothing picked (D6); the pane keeps its kind.
    Nothing,
    /// Graph panes (Transfer, Spectrum / RTA, Sweep / distortion): what is drawn, the lead
    /// first; at least one entry.
    Curves { entries: Vec<PaneEntry> },
    /// SPL panes: one meter (a meter is a number, read side by side, never overlaid).
    Meter { rig: RigRef, meas: MeasId },
    /// Programme panes, as today.
    Inputs { rig: RigRef, inputs: Vec<u16> },
    Pair { rig: RigRef, left: u16, right: u16 },
    /// The Walk pane.
    Walk { rig: RigRef, walk: WalkId },
}
pub struct PaneEntry { pub rig: RigRef, pub what: EntryRef }
pub enum EntryRef {
    /// A measurement's group: its live curve, its math channels, its shown traces —
    /// what is filed under it now and later (D1).
    Group { meas: MeasId },
    /// One live curve: a measurement's or a math channel's.
    Live { meas: MeasId },
    /// One stored trace.
    Trace { trace: TraceId },
}
pub enum RigRef {
    /// Session files: the rig this session belongs to.
    ThisRig,
    /// Another rig, by its RigId; the name is for the operator while it is absent.
    Rig { id: RigId, name: String },
}
```

- **`RigRef::Active` leaves the wire.** It is only meaningful in the app's layout prefs,
  which have their own type (`PaneEntryFile`, `rig: Option<{ id, name }>`, absent = the
  active rig); on the wire it existed only to be refused. With two variants there is
  nothing to refuse in `check()`, and the rule "another rig's pane names what it shows"
  goes too: `content` is always present.
- **`content: None` is gone.** Today it meant "the measurement whose layout this is", and
  `resolve_meas` (`state/panes/ops.rs`) filled an unpicked pane from the chosen row or the
  first measurement of the pane's home kind. That stand-in is retired (D6): a pane draws
  what it holds, and `Nothing` is the one form of "nothing picked", on any pane kind
  (`Curves` with no entries is refused, so there are not two).
- **The lead** (first entry) names the pane (its title), owns its readouts (delay,
  coherence, the sweep's distortion and IR views) and is what pane keys act on (S, B, P, the
  delay finder). **Make lead** is the one control for it: **Shift+Enter** on a ticked row
  of C's list (the palette offers the same) moves that entry first. Reordering the rest is
  not offered — behind the lead, order only sets layering, which follows tick order — so the one
  question that matters ("which curve do the keys act on?") has one control.
- **SPL, Programme and Walk panes keep one source.** Operator 2026-10-17 on SPL: "add new
  pane … drag another spl measurement there, there you have two meters"; a goniometer is one
  pair. A drop replaces their source; C does not open on them.
- **Limit: 33 entries** (`Compared::MAX + 1`): the `Group` plus up to 32, the most a
  compare list held, so salvage never truncates a list that was legal; no entry twice; a
  `Live`/`Trace` already drawn by a `Group` in the same pane is refused (the group draws it).
- **Kind and content agree**, kept in `check()`: `Nothing` on any pane, `Curves` (at least
  one entry) only on Transfer, Spectrum / RTA and Sweep / distortion panes, `Meter` only on
  SPL, `Inputs` / `Pair` only on Programme, `Walk` only on Walk.

### What a pane starts with (D3, D6, D9)

A pane draws only its content; nothing joins it except through a `Group` entry (D1), a row
dragged onto it, or a tick in C's list. That keeps the 2026-10-09 complaint (every curve in
every pane) out by construction.

| situation | the pane shows |
|---|---|
| a new measurement (Ctrl+M, a sweep, a Leq or SPL dialog, a transfer stack's new measurement, the walk window's walk) | its own pane: it takes the focused pane when that pane is `Nothing` and of the shown rig (an empty pane is room, no split; it turns into the measurement's kind), else opens beside it — never over a pane that shows something, whatever kind. No room to split (a half below the smallest readable size): the measurement is made and selected, no pane changes, and a warning says *no room for a pane: drag TF 3 onto one* |
| a command that needs a pane of a kind none on screen has (SPL view keys, sweep edit) | the command acts on the selected measurement; no pane turns kind for it — the hint bar says *no SPL pane: drag FOH SPL onto one* |
| first launch, empty daemon | one pane, *nothing picked — drag a row of the list here; Ctrl+M makes a measurement*; the first measurement then takes that pane |
| connecting to a rig that has measurements, no remembered panes (`restore_pane_meas`, a kiosk without prefs) | the one pane is given the rig's first measurement once, as an explicit pick written to the pane (stored like a drop, not a stand-in) |
| a split of a graph pane with picks | the focused pane's lead only (D3) |
| a split of an SPL, Programme or Walk pane | a copy of it, as today |
| a split of an empty pane | nothing picked |
| a measurement row clicked, with an own layout | its panes come on screen (P1, D12) |
| a measurement row clicked, no own layout | selection only; every pane keeps what it shows (D9) |
| any list selection while a pane is empty | nothing changes: an empty pane does not follow the selection (D6) |
| the top-bar rig drop-down | that rig is shown; no layout comes (as today: `switch_rig` brings none); panes that follow the shown rig and show something are pinned to the old one, as today |
| a row of another rig reached (click, Enter, Tab) | a rig switch first, as the drop-down (refused with a window open), then the row on that rig, as a click there: an own layout comes, else selection only (D15) |

Today `select_meas_row` (`state/queries/select.rs`) shows a measurement without an own layout
in the focused pane and turns that pane's kind, or splits a pane off for it (`own_pane_for`,
`state/panes/ops.rs`); W's stage view shows another row's measurement in the pane alone.
All of these go in stage 1 (D9). An unpicked pane follows `rig.chosen` today (`resolve_meas`,
`state/panes/ops.rs`): stage 1 ends that in the app — an unpicked pane outside its owner's
layout is *nothing picked*, and in a measurement's own layout `content: None` still means
the owner (that is a stored form, unchanged until stage 2). `focus_kind`, which turns the
focused pane into SPL or Sweep for a key or dialog, and `stack_created` / `open_created`,
which put a new transfer into the focused transfer pane or open none when one of its kind
exists, follow the table instead.

### Invariant: a pick waits, it is never replaced

A content reference naming something the app does not hold yet (its rig not connected or
not synced, the id not yet in that rig's mirror) **waits**: it stays in the pane and in every
stored layout, and the pane says so. It is never swapped for a stand-in (the layout's owner,
the selection, the first fitting measurement), and a stand-in shown meanwhile is never
written back. This holds for every content kind:

| content | while waiting |
|---|---|
| `Curves` entry | its legend line: *TF 1 · waiting for pupu* (a name the prefs or layout kept), *a trace of pupu · waiting* (none) |
| `Meter` | on the pane: *FOH SPL · waiting for pupu* |
| `Inputs` / `Pair` | on each bar: *in 3 · waiting for pupu*; a channel the rig's open session lacks: *in 3 · not in this session* |
| `Walk` | on the pane: *walk 2 · waiting for pupu* |
| a deleted `Meter`, `Walk` or last entry (now `Nothing`) | on the pane: *FOH SPL deleted — drag a row here*; the app remembers the name until the pane changes |

A reference leaves a pane only by an untick, a drop over it, or when its rig, connected and synced,
reports the id deleted. The daemon prunes its own ids from stored layouts against the
measurement set after a load or delete — rig meters included — never against a client's
mirror (as it prunes compare lists today). The **app** prunes another rig's entry from a
layout it shows when that rig reports the id deleted, then stores the layout — an explicit
prune, never a fallback. Pruning the last entry of `Curves`, or a deleted `Meter` or
`Walk`, leaves `Nothing`; the pane keeps its kind, and the *deleted* text is the app's (the
daemon stores no names for it). `Inputs` and `Pair` are channel numbers, not ids: nobody
prunes them; a channel the session lacks reads *not in this session* on its bar.

Today's code is held to this already for a measurement pick (988257b1, on this note's
branch): it keeps a pick the mirror does not list in `Pending::pane_ids`
(`state/panes/meas_layout.rs`),
`layout_of` stores the waiting id, `restore_pane_ids` puts it back when the measurement
arrives, and a split keeps the wait. Stage 2 moves the waiting id from that side table into
the entry itself: an `EntryRef` the mirror does not resolve *is* the waiting state, so
there is no second place to forget. The stale doc on `LayoutPane.content` ("a pick of a
measurement gone since shows the layout's own measurement") and P1's "a pick of a deleted
measurement shows the owner" go with it.

## 2. How the rest fits

- **Compare (C)** becomes per pane (D2): **C** opens the tick list (§3) on the focused
  graph pane; it is the one way to draw more than one entry (Shift+drop goes, D13). The
  daemon's per-measurement compare list (`Compared`,
  `meas.compare`) goes. Draw order stays: lead on top, later entries a layer further back
  and fainter (never below about half).
- **Hide (A / Shift+A) and slots 1–9** stay global and win everywhere (D4): a hidden entry
  stays listed in the pane's legend as *Slot 3 hidden — 3 shows it*, as a hidden lead does
  today.
- **Math channels** are measurements: `Live { meas }` of a math channel picks it alone, its
  owner's `Group` brings it. Cross-rig math stays as multi-rig.md §6 says.
- **Own layouts (P1).** Choosing a measurement's row still brings its panes, and editing
  panes while it is chosen stores them on it. A stored pane keeps every reference, other
  rigs' included, by `Rig { id, name }`, opaque to this daemon (D5); a pane whose references
  are all another rig's stays in the app's prefs only, as N2 says, so bringing a layout
  never duplicates a pinned pane on screen.
- **Layout prefs** (`ui.toml`, `PaneEntryFile`): references by rig id and name (measurement
  name; trace name and id), resolved when the rig is back, as `park_gone_meas` does for
  measurements today.
- **Undo**: each tick, untick, *Make lead* or drop is one pane edit in the app's history.
- **Shared scale**: unchanged — same-kind panes on screen share one range (union).
- **Colours across rigs**: two rigs can hand out the same family. In a pane mixing rigs the
  app gives a later entry the next free family for that pane only, and each other rig's
  legend line carries its rig name (`pupu · TF 1`), as pinned panes' titles do.

## 3. One way into a pane: the drop (D10, D11, D13, D14, D15, D16)

Panes have no chip and no pane list (D10). The pane title is plain text naming the lead
(`1 SPL · FOH SPL`, `pupu · TF 1` for another rig's), never a control. Palette *Choose pane
measurement…* goes (D11). What a pane shows changes only by **dragging a row onto it** — a
row of the measurement list or of the Inputs list — as the guide's *Dragging a row onto a
pane* describes today:

- dropped in the middle: the pane shows that row alone (a measurement row: `Group`, or
  `Meter` on SPL; a live curve or math channel: `Live`; a stored trace: `Trace`; an Inputs
  row: `Inputs`, a pair row: `Pair`), turning into the kind that draws it; the drag label
  says so before release (`FOH SPL → pane 2, becomes SPL`). The drop is the operator's own
  step, so its kind change is not a side effect (D7);
- dropped in the outer fifth: the pane splits on that side and the new half shows the row,
  as today;
- **Shift on release does nothing extra** (D13): Shift+drop compare goes; C is the one
  compare;
- the selection in the list stays; focus moves to the pane dropped on.

A heading row (Rig, Session, a rig's heading) is not draggable; a click on another rig's
heading switches to that rig, then acts as on the shown rig's heading. A Walk pane comes
with its walk (the walk window opens it, as a new measurement's own pane); a drop replaces
it like any pane.

**Every connected rig in the list (D15).** The measurement list shows each connected rig's
measurements under its heading (D8 names: `pupu`, `ketunkolo · ac2d`, `ketunkolo · in this
app`); with one rig the headings go. Any rig's row drags, and a drop pins the pane to that
row's rig (the chip's `pane_show_from` / `pane_meter_from`, `state/cross_rig.rs`, stay and
take a trace variant; `pane_show` sets `here()` and `pane_show_trace` sets `Active`, so a
drop of another
rig's row must not use them). Selection is the shown rig's (`RigState`), so a row of another
rig — clicked, or reached by Enter or Tab (Tab cycles the whole list; with a window open the
switch is refused, so Tab skips other rigs' rows) — is a rig switch
first, then the row on that rig. The top-bar drop-down stays and names the shown rig: where
Ctrl+M and new measurements go and whose rows the keys act on. Shape: `TreeRow` and the drag
item carry a `RigId`; the other rigs' rows are a snapshot taken by `visit` after each
message (as `refresh_menu_rigs`, `state/cross_rig.rs`, does for the chip today); the row
messages (`SelectMeas`, `SelectTrace`, `ToggleShown`, rename, …) carry the `RigId` and
switch first when it is not the shown rig; folding (`collapsed`) is per rig; a row dropped
onto a row of another rig (the math dialog) is refused with a toast (math across rigs is
multi-rig.md §6).

**Inputs rows drag (D16).** Rows of the Inputs list (an input, a pair) drag onto panes like
measurement rows: the drag item is an enum (tree row | input | pair), and a drop makes
`Metered::Inputs` / `Pair` (`state/programme.rs`) in one Programme pane, Meters view for an
input, Goniometer view for a pair (**G** steps the other views; today a pair row opens two
panes). **P** on an Inputs row goes (`CommandId::ProgrammePane`, its key-table row and the
Inputs tip). **E** on a Programme pane, which opens the meter form choosing what the pane
meters (`open_meter_form(Some(focus))`), does nothing there; E on an Inputs row edits the
input as today.

**Without a pointer.** Tab and Enter on a row act as a click (select; an own layout comes),
so keyboard-only use and the Pi kiosks in panes-only mode (lists hidden) change panes only
through own layouts and `ui.toml`. The operator accepted one way (D14); the kiosks are pinned
by `ui.toml` already. Touch works (a touch is a pointer: hold and move a few pixels).

Readying a pane of a kind no measurement exists for (the chip's `pane  Sweep /
distortion` rows) goes with the chip: a new measurement brings its own pane.

### C's tick list (D2, D7)

**C** on a graph pane (Transfer, Spectrum / RTA, Sweep / distortion) opens a list over the
focused pane: what can be drawn *with* the pane's lead, from every connected rig. It holds
only what the pane's kind draws; it never changes the kind or replaces the lead (only *Make
lead* does). On an SPL, Programme or Walk pane C does nothing but say *one source per pane:
drag a row onto it* in the hint bar.

```
pupu                                    ← rig heading (§4)
  ✓ TF 1 · live + 3 traces              ← Group row (D1), the lead
      live
    ✓ Capture 2 · slot 2
      Sweep run 1
    TF 2 · live + 1 trace
ketunkolo · ac2d
    TF 3 · live
```

- **Enter** ticks or unticks and the list stays open; **Shift+Enter** on a ticked row makes
  it lead, on an unticked row ticks it and makes it lead; hint bar *↑/↓ · wheel · Enter
  ticks · Shift+Enter makes lead · Esc closes*. Unticking the lead makes the next entry
  lead; unticking the last leaves `Nothing`.
- Rows use the tree's words: the measurement's name, then what it brings (`live + 3
  traces`); children by their tree labels. No kind tags, no `meter` / `pane` prefixes, no
  *New measurement…* (Ctrl+M) and no kind rows: the list adds curves, nothing else.
- With one rig the headings go.

## 4. Same-host rigs named by host and kind (D8)

An app on ketunkolo with its own embedded daemon and ketunkolo's `ac2d` listed them as *this
computer* and *ac2 on ketunkolo*: two names for one machine. The app names a rig by how it
reaches it, always: a locally reached rig — over ipc, or tcp to a loopback address — is
**`<this host> · ac2d`**, the embedded one **`<this host> · in this app`**; a network rig
keeps the name its hello sends (`pupu`). Headings, the rig drop-down, pinned pane titles and the
Connection page all use it. App only: the daemon's hello name, `ac2d --name` and the mDNS
adverts are unchanged, and there is no `PROTO_VERSION` bump. Stage 0 (ec71a7cc) covers reach
local (ipc) and embedded; a rig reached over tcp on a loopback
address keeps its hello name until a stage-0 follow-up names it too.

## 5. Wire and files

`PaneContent` changes shape, `LayoutPane.rig` moves into the content, `RigRef::Active`
leaves the wire, and `Compared` / `meas.compare` go: **`PROTO_VERSION` 79 → 80, session
format 40 → 41, rig file 8 → 9** (meter layouts; `rig/v8.rs` the upgrade step),
`fixtures/protocol/WIRE_LOCK` regenerated. No serde defaults for old shapes; `session
salvage` and the rig file step map:

- `Meas { meas }` → `Curves [Group(meas)]` on a graph pane, `Meter { meas }` on an SPL pane;
- `content: None` in a measurement's own layout → its owner's `Group` (`Meter` on an SPL
  pane), as before; elsewhere (a rig meter's layout of another kind) → `Nothing`; `Inputs`,
  `Pair`, `Walk` take the pane's rig;
- each pane's `rig` onto each of its references;
- a measurement's compare list onto the panes of its own layout that hold its `Group`, as
  `Live` / `Trace` entries after the `Group`, in the list's order (Group first, then
  compares; 32 fits, nothing is cut);
- a measurement with a compare list but **no layout**: nothing is synthesised; the salvage
  report lists the compares dropped (pre-1.0, and restoring old sessions exactly is low
  priority for the operator).

Renumbering on load: since bbc9c2fe ("a loaded session's renumbered measurements keep their
pane picks and compare lists"), `remap_ids` (`ac2d/src/control/rig_meas.rs`) remaps
`PaneContent::Meas` in every measurement's layout and `Compared::Meas` in its compare list
when a loaded session's measurements are moved past the ids the rig meters hold.

## Stages

Each stage lands alone, with its own user-guide rows, and is tested through `ac2-scene` text
(the list rows, legend lines, pane titles, hint bars) and state tests in `ac2-ui`.

0. **Rig names** (§4). App only. Guide: the rig drop-down and Connection page names.
1. **One way into a pane** (D9–D16; app only, no wire change):
   - the chip, its list and *Readying a pane of a kind* go from every pane (title plain
     text); palette *Choose pane measurement…* (`CommandId::PaneMeasurement`) goes;
     Shift+drop compare goes; `PaneMenu` / `Msg::PaneMenu` / `Msg::PanePick` /
     `Overlay::PaneMenu`, undo's `Watch::Pick` and the chip's menu rows in `cross_rig.rs` go
     (`pane_show_from` / `pane_meter_from` stay for the drop); the state tests'
     `T::show(kind)` gets a `set_pane_kind` helper; `keys.toml` bindings naming a removed
     command: `from_toml` (`keys/keymap.rs`) refuses the whole file on an unknown command
     today, so it learns to skip that binding with a warning;
   - the measurement list shows every connected rig under its heading (D15), rows of any rig
   drag, a drop pins the pane to that rig, a row of another rig is a rig
     switch first (§3: `TreeRow` / drag item with `RigId`, a per-message snapshot of the
     other rigs, row messages with `RigId`, folding per rig, cross-rig row-onto-row
     refused);
   - Inputs list rows (input, pair) drag onto panes (D16, a drag item enum); P
     (`CommandId::ProgrammePane`) goes, E on a Programme pane does nothing;
   - a measurement row without an own layout only selects: `select_meas_row`'s focused-pane
     path, `own_pane_for` and W stage view's "another row shows in the pane alone" go;
   - an unpicked pane outside its owner's layout is *nothing picked* (the app's half of
     D6: `resolve_meas` / `kind_meas` no longer stand in from `rig.chosen` or the first of
     a kind; `content: None` in a measurement's own layout still reads as its owner: `show_layout`
     / `install_node` fill `v.meas = Some(owner)`, and `layout_of` / `wire_pane` write the
     owner back as `content: None`, so a brought layout compares equal to the stored one
     and is not re-stored on each sync). `show_layout`'s fill of the focused pane with
     `rig.chosen` when no pane shows it (`meas_layout.rs`) goes too; its focus fallbacks
     keep only `v.meas == Some(owner)`. `TreeKey::Rig` becomes `TreeKey::Rig(RigId)`. Every
     caller that took the first of a kind
     (`kind_meas` / `resolve_meas(kind, None)`: SPL keys, the delay banner, the math and
     alignment forms' first operand, smoothing, targets, the sweep stimulus, spectrum
     display, settings, Leq export, commands) follows one rule: the selected measurement
     when it is of that kind, else none, and the key or form says *select a transfer
     measurement* (or the kind) instead of guessing;
   - new measurements and commands follow §1's table: `focus_kind` no longer turns the
     focused pane, `stack_created` / `open_created` open the measurement's own pane beside
     the focused one unless that pane is empty; no room: a warning, no pane changes;
   - the drag label names a kind change (`becomes SPL`, scene `pane_edit.rs`).
   C stays as today (the selected measurement's compare list, palette *Clear compare*)
   until stage 2. Guide: every `chip` mention (`grep -n chip docs/user-guide.md`: many
   rigs, choosing what a pane shows, list clicks, W stage view, layouts follow the
   measurement, undo, resolution, sweep pane, PNG export, key hints), the palette and
   Shift+drop rows, the Inputs list (drag, P), the key table and hint bars; *Dragging a row
   onto a pane* becomes the section for changing a pane. Decision table: P2 rewritten. The
   wait invariant is already in for measurement picks (988257b1).
2. **References on the wire** (§1, §5): `PaneContent` with per-reference rig, required
   `content`, `PaneContent::Nothing` (D6, `resolve_meas` retired), salvage, `remap_ids`
   extended to every `ThisRig` reference (entries, `Meter`, `Walk`) and skipping
   `Rig { .. }` ones, another daemon's ids — tested by loading a session numbered from 1
   onto a daemon with a rig meter at id 1, whose layouts hold an entry, a `Meter` and a
   `Walk` naming the moved id and an entry pinned to another rig with the same id;
   pruning of deleted ids by daemon and app, `check()` limits
   and kind/content matrix; panes draw entries; C opens tick mode; compare lists go; the
   waiting id lives in the entry; an empty focused pane is room for a new measurement's own
   pane. What used `kind_meas` / `resolve_meas(kind, None)` (the SPL commands, the delay
   banner) says *nothing picked* or *no SPL pane shows a meter* instead of picking one.
   PROTO / session / rig file bump.
   Guide: Compare section rewritten around C's per-pane list, the empty pane. Decision table:
   P1's pane-content clauses and N2's `Active` / content-less-pane clauses are replaced
   here (P2 already records the decisions).
3. **C's list in full** (§3): `Live` / `Trace` rows, cross-rig entries in one pane,
   rig-tagged legend lines, colour families per pane, *Make lead*. Guide: the tick list,
   *Make lead*, mixed-rig legends.

## Decisions (operator)

2026-11-01:

- **D1** A measurement row, dropped or ticked, is the `Group` entry, which follows its
  group: a new capture or math channel appears in the panes holding the group. Single rows
  are fixed.
- **D2** Compare is per pane; the per-measurement compare list goes, salvaged into entries.
- **D3** A split starts with the focused pane's lead only.
- **D4** Hide (A, slot keys) wins in every pane; the legend says how to show it.
- **D5** A pane mixing rigs is stored whole in a measurement's own layout, other rigs'
  references opaque by rig id; an all-other-rig pane stays prefs-only.
- **D6** A pane with no pick holds `PaneContent::Nothing`, shows *nothing picked* (or *<name>
  deleted — drag a row here* after a prune) and does not follow the list selection.
- **D7** C's list shows only what the pane's kind draws. No click changes a pane's kind; a
  drop does, as the operator's own step, and its label says so; a new measurement's own pane
  taking an empty (`Nothing`) focused pane of the shown rig turns it into its kind.
  (*Turn into…* went with the chip, D10.)
- **D8** Rig names by reach: `<this host> · ac2d`, `<this host> · in this app`, network rigs
  by their hello name; no override, no wire change.
- **D9** Clicking a list row selects; a measurement row with an own layout also brings its
  panes (P1); one without changes no pane.

2026-11-02 ("remove it as whole from all panes to avoid confusion, lets not do billions of
different ways changing the pane view"):

- **D10** No chip and no pane list on any pane; the title is plain text.
- **D11** Palette *Choose pane measurement…* goes ("I have not seen once I think").
- **D12** Clicking a measurement row switches the screen to its own panes, as P1 has it
  ("clicking measurement switches the screen as it should do now").
- **D13** C stays as the one compare; Shift+drop compare goes ("Keep C, drop Shift+drop").
- **D14** Dragging a list row onto a pane is the one way to change what a pane shows.
- **D15** The measurement list shows every connected rig's measurements under its heading,
  so any rig's row drags; the top-bar drop-down shows which rig is shown (new measurements,
  own layouts) (operator: "List shows all").
- **D16** Inputs list rows drag onto panes and make Programme panes; P and E stop changing
  panes (operator: "Drag inputs only").
