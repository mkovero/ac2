# Pane curves: a pane draws the curves picked for it, from any rig

Status: design, not built. Operator decisions 2026-11-01: the pane's list (its title chip)
picks individual curves — any connected rig's live curves and stored traces — and the
"pane = its measurement's group" rule of P1 no longer holds for a pane, because a list that
reaches into every rig's groups cannot also mean "this one group". Decisions D1 … D9 are at
the end; all are settled.

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
- names inputs by the backend's port (`capture_3 · line (in 3)`) although the session has a
  channel number, a kind and often a mic name for it;
- lists a pair's sides a second time on their own (`programme_menu_rows`: "a pair's sides
  alone as well"), apart from the other single inputs, so `MM1 34804` stands beside `MM1
  34804 + Vilppu behringer #3` and, on a rig with no names, `capture_1` beside `capture_1 +
  capture_2`;
- prefixes rows with the tree's kind tags `TF`, `FFT`, `RTA`, `SPL`, `MATH`, `SWEEP`
  (`meas_list::kind_tag`) and with `meter` / `pane`, words the pane titles do not use.

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
- **The lead** (first entry) names the pane (chip, title), owns its readouts (delay,
  coherence, the sweep's distortion and IR views) and is what pane keys act on (S, B, P, the
  delay finder). **Make lead** is the one control for it: **Shift+Enter** on a ticked row of
  the list (the palette offers the same) moves that entry first. Reordering the rest is not
  offered — behind the lead, order only sets layering, which follows tick order — so the one
  question that matters ("which curve do the keys act on?") has one control.
- **SPL, Programme and Walk panes keep one source.** Operator 2026-10-17 on SPL: "add new
  pane … drag another spl measurement there, there you have two meters"; a goniometer is one
  pair. Their pick is a radio choice, not a tick list.
- **Limit: 33 entries** (`Compared::MAX + 1`): the `Group` plus up to 32, the most a
  compare list held, so salvage never truncates a list that was legal; no entry twice; a
  `Live`/`Trace` already drawn by a `Group` in the same pane is refused (the group draws it).
- **Kind and content agree**, kept in `check()`: `Nothing` on any pane, `Curves` (at least
  one entry) only on Transfer, Spectrum / RTA and Sweep / distortion panes, `Meter` only on
  SPL, `Inputs` / `Pair` only on Programme, `Walk` only on Walk.

### What a pane starts with (D3, D6, D9)

A pane draws only its content; nothing joins it except through a `Group` entry (D1), the
chip, or a row dragged onto it. That keeps the 2026-10-09 complaint (every curve in every
pane) out by construction.

| situation | the pane shows |
|---|---|
| a new measurement's own pane | `[Group(it)]`; it takes the focused pane when that pane is `Nothing` and of the shown rig (an empty pane is room, no split; it turns into the measurement's kind if needed), else opens beside it — never over a pane that shows something |
| first launch, empty daemon | one pane, *nothing picked — the chip picks, Ctrl+M makes a measurement*; the first measurement then takes that pane |
| a split of a graph pane with picks | the focused pane's lead only (D3) |
| a split of an SPL, Programme or Walk pane | a copy of it, as today |
| a split of an empty pane | nothing picked |
| a measurement row chosen, with an own layout | its panes come on screen (P1, unchanged) |
| a measurement row chosen, no own layout | selection only; every pane keeps what it shows (D9) |
| any list selection while a pane is empty | nothing changes: an empty pane does not follow the selection (D6) |

Today `select_meas_row` (`state/queries/select.rs`) shows a measurement without an own layout
in the focused pane and turns that pane's kind. P1 was read two ways: its text says a row
without an own layout keeps the panes on screen, while the user guide documented the
focused-pane path; D9 fixes the reading, and stage 1 drops the path. The list click model
already says child rows only select; it now holds for every row: a click selects, the chip and a drag put things in panes, and only a
measurement's own layout brings panes.

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
| a deleted `Meter`, `Walk` or last entry (now `Nothing`) | on the pane: *FOH SPL deleted — the chip picks*; the app remembers the name until the pane changes |

A reference leaves a pane only by an untick or pick, or when its rig, connected and synced,
reports the id deleted. The daemon prunes its own ids from stored layouts against the
measurement set after a load or delete — rig meters included — never against a client's
mirror (as it prunes compare lists today). The **app** prunes another rig's entry from a
layout it shows when that rig reports the id deleted, then stores the layout — an explicit
prune, never a fallback. Pruning the last entry of `Curves`, or a deleted `Meter` or
`Walk`, leaves `Nothing`; the pane keeps its kind, and the *deleted* text is the app's (the
daemon stores no names for it). `Inputs` and `Pair` are channel numbers, not ids: nobody
prunes them; a channel the session lacks reads *not in this session* on its bar.

Today's code is held to this already for a measurement pick: branch `spl-pick` (3b6a01e9)
keeps a pick the mirror does not list in `Pending::pane_ids` (`state/panes/meas_layout.rs`),
`layout_of` stores the waiting id, `restore_pane_ids` puts it back when the measurement
arrives, and a split keeps the wait. Stage 2 moves the waiting id from that side table into
the entry itself: an `EntryRef` the mirror does not resolve *is* the waiting state, so
there is no second place to forget. The stale doc on `LayoutPane.content` ("a pick of a
measurement gone since shows the layout's own measurement") and P1's "a pick of a deleted
measurement shows the owner" go with it.

## 2. How the rest fits

- **Compare (C)** becomes per pane (D2): *Add to this pane…* opens the chip list in tick
  mode on the focused pane. The daemon's per-measurement compare list (`Compared`,
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
- **Undo**: each tick, untick, *Make lead* or pick is one pane edit in the app's history.
- **Shared scale**: unchanged — same-kind panes on screen share one range (union).
- **Colours across rigs**: two rigs can hand out the same family. In a pane mixing rigs the
  app gives a later entry the next free family for that pane only, and each other rig's
  legend line carries its rig name (`pupu · TF 1`), as pinned panes' chips do.

## 3. The list (D7)

The list holds only what the focused pane's kind draws; changing the kind is its own step,
**Turn into…**, a row after every rig that opens the kinds. This reverses today's one-step
change (a transfer row picked on a spectrum pane turned it into a transfer pane): neither a
pick on the chip nor a click in the tree (D9) changes what kind of pane something lands in.

```
pupu                                    ← rig heading (§4)
  ✓ TF 1 · live + 3 traces              ← Group row (D1)
      live
    ✓ Capture 2 · slot 2
      Sweep run 1
    TF 2 · live + 1 trace
ketunkolo · ac2d
    Spectrum 1 · live
───────────────
Turn into…
New measurement on pupu…   Ctrl+M
```

On a Programme pane each rig's rows are its inputs instead:

```
pupu
    in 1 + 2 · MM1 34804 + Vilppu behringer #3 · pair
    in 1 · MM1 34804 · mic   in 2 · Vilppu behringer #3 · mic   in 3 · line   …
    in 1–8 side by side
```

- **Graph panes tick, single-source panes choose.** Tick mode (Transfer, Spectrum / RTA,
  Sweep / distortion): **Enter** ticks or unticks and the list stays open; hint bar
  *↑/↓ · wheel · Enter ticks · Shift+Enter makes lead · Esc closes*. Radio mode (SPL,
  Programme, Walk): **Enter** shows the row and closes; hint bar as today, *↑/↓ · wheel ·
  Enter shows · Esc closes*. In the key handler (`state/commands/keys.rs`, the
  `PaneMenu` arm) Shift is checked before the plain tick: Shift+Enter on a ticked row makes
  it lead, on an unticked row ticks it and makes it lead, on a radio list does nothing.
- **SPL panes list SPL measurements only**; inputs are not offered there (an input is
  metered by a Programme pane; an SPL pane needs an SPL measurement's calibration and
  weighting). **Programme panes are reached through inputs rows** (on a Programme pane, or by
  dragging an input or pair row onto a pane), never through *Turn into…*, which lists
  Transfer, Spectrum / RTA, Sweep / distortion and SPL.
- Rows use the tree's words: the measurement's name, then what it brings (`live + 3
  traces`); children by their tree labels. No kind tags and no `meter` / `pane` prefixes: the
  list holds only what the pane draws, so the kind is the pane's title.
- Inputs read as everywhere else: `in N`, then the mic name or the input's kind; a backend
  port name never stands for a channel the session numbers. A pair's sides are listed once,
  in channel order among the single inputs, not again beside the pair.
- *Turn into…* and *New measurement…* sit under a rule after every rig, never under a rig
  heading; *New measurement on <rig>…* names the rig it creates on (the shown one).
- With one rig the headings go, as today.

## 4. Same-host rigs named by host and kind (D8)

An app on ketunkolo with its own embedded daemon and ketunkolo's `ac2d` listed them as *this
computer* and *ac2 on ketunkolo*: two names for one machine. The app names a rig by how it
reaches it, always: a locally reached rig — over ipc, or tcp to a loopback address — is
**`<this host> · ac2d`**, the embedded one **`<this host> · in this app`**; a network rig
keeps the name its hello sends (`pupu`). Headings, the rig drop-down, pinned chips and the
Connection page all use it. App only: the daemon's hello name, `ac2d --name` and the mDNS
adverts are unchanged, and there is no `PROTO_VERSION` bump. Stage 0, built on branch
`rig-names`, covers reach local (ipc) and embedded; a rig reached over tcp on a loopback
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
1. **List wording and clicks** (§3 without ticks or children, D9): filter by pane kind,
   *Turn into…*, *New measurement on <rig>…* out of rig headings, input labels, pair sides
   listed once, no kind tags or prefixes, SPL panes without inputs; a measurement row
   without an own layout only selects (`select_meas_row`'s focused-pane path goes). App
   only. Guide: "Choosing what a pane shows" (the list, *Turn into…*, the end of the
   one-step kind change) and the list click rows (a click selects; the chip and a drag put
   things in panes; an own layout still comes with its row). Decision table: P2 notes the
   app now keeps P1's "one without keeps the panes on screen". The wait invariant is already in for measurement
   picks (`spl-pick`).
2. **References on the wire** (§1, §5): `PaneContent` with per-reference rig, required
   `content`, `PaneContent::Nothing` (D6, `resolve_meas` retired), salvage, `remap_ids`
   extended to every `ThisRig` reference (entries, `Meter`, `Walk`) and skipping
   `Rig { .. }` ones, another daemon's ids — tested by loading a session numbered from 1
   onto a daemon with a rig meter at id 1, whose layouts hold an entry, a `Meter` and a
   `Walk` naming the moved id and an entry pinned to another rig with the same id; pruning of deleted ids by daemon and app, `check()` limits
   and kind/content matrix; panes draw entries; C opens tick mode; compare lists go; the
   waiting id lives in the entry; an empty focused pane is room for a new measurement's own
   pane. What used `kind_meas` / `resolve_meas(kind, None)` (the SPL commands, the delay
   banner) says *nothing picked* or *no SPL pane shows a meter* instead of picking one.
   PROTO / session / rig file bump.
   Guide: Compare section rewritten as *Add to this pane…*, the empty pane. Decision table:
   P1's pane-content clauses and N2's `Active` / content-less-pane clauses are replaced
   here (P2 already records the decisions).
3. **Tick list with children** (§3 in full): `Live`/`Trace` rows, cross-rig entries in one
   pane, rig-tagged legend lines, colour families per pane, *Make lead*. Guide: the tick
   list, *Make lead*, mixed-rig legends.

## Decisions (operator, 2026-11-01)

- **D1** A measurement row stays, as the `Group` entry, which follows its group: a new
  capture or math channel appears in the panes holding the group. Ticked single rows are
  fixed.
- **D2** Compare is per pane; the per-measurement compare list goes, salvaged into entries.
- **D3** A split starts with the focused pane's lead only.
- **D4** Hide (A, slot keys) wins in every pane; the legend says how to show it.
- **D5** A pane mixing rigs is stored whole in a measurement's own layout, other rigs'
  references opaque by rig id; an all-other-rig pane stays prefs-only.
- **D6** A pane with no pick holds `PaneContent::Nothing`, shows *nothing picked* (or *<name>
  deleted — the chip picks* after a prune) and does not follow the list selection.
- **D7** The list shows only what the pane's kind draws; *Turn into…* changes the kind; no
  pick or click changes a pane's kind as a side effect. The one kind change that is not the
  operator's own step: a new measurement's own pane taking an empty (`Nothing`) focused pane
  of the shown rig turns it into the measurement's kind.
- **D9** Clicking a list row only selects. A measurement reaches a pane through the chip or
  a drag; a row with an own layout still brings its panes (P1).
- **D8** Rig names by reach: `<this host> · ac2d`, `<this host> · in this app`, network rigs
  by their hello name; no override, no wire change.
