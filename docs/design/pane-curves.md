# Pane curves: a pane draws the curves picked for it, from any rig

Status: design, not built. Operator decisions 2026-11-01: the pane's list (its title chip)
picks individual curves — any connected rig's live curves and stored traces — and the
"pane = its measurement's group" rule of P1 no longer holds for a pane, because a list that
reaches into every rig's groups cannot also mean "this one group". Rigs on the same host are
named by host and kind. Open operator decisions are marked **D1 … D5** with a
recommendation.

## Why the current list misleads

The chip list (`pane_menu_rows`, `state/panes/ops.rs`; rigs from `cross_rig.rs`) offers one
row per measurement; picking it sets `PaneContent::Meas { meas }` and the pane draws that
measurement's whole group (live curve, math filed under it, its shown traces) plus its
compare list. A row reads as one curve and brings several; a stored trace cannot be picked at
all (only dragged), and another rig's trace not even that. Besides that, the list as built:

- lists every measurement whatever the pane draws (an SPL pane offers TF and FFT rows; a pick
  turns the pane's kind), and the kinds of pane (`pane  Sweep / distortion`) and *New
  measurement…* are appended after the last rig's rows, so they read as that rig's;
- names inputs by the backend's port (`capture_3 · line (in 3)`) although the session has a
  channel number, a kind and often a mic name for it;
- lists a pair's sides again on their own (`programme_menu_rows`: "a pair's sides alone as
  well"), so `MM1 34804` stands beside `MM1 34804 + Vilppu behringer #3` and, on a rig with
  no names, `capture_1` beside `capture_1 + capture_2`;
- tags rows `SPL`, `TF`, `FFT`, `meter`, `pane`: abbreviations the pane titles and the tree
  do not all use for the same thing.

## 1. What a pane holds

A graph pane (Transfer, Spectrum / RTA, Sweep / distortion) holds an ordered list of
**entries**; the first is the pane's **lead**:

```rust
pub enum PaneContent {
    /// Graph panes: what is drawn, the lead first.
    Curves { entries: Vec<PaneEntry> },
    /// SPL panes: one meter (a meter is a number, read side by side, never overlaid).
    Meter { rig: RigRef, meas: MeasId },
    Inputs { rig: RigRef, inputs: Vec<u16> },   // Programme, as today
    Pair { rig: RigRef, left: u16, right: u16 }, // Programme, as today
    Walk { walk: WalkId },                      // Walk pane, as today
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
```

- **The rig moves from the pane to the entry** (`LayoutPane.rig` goes): ids are per daemon
  (multi-rig.md §1), so every reference carries the rig whose id it is. `RigRef` keeps its
  meanings (`Active` in app prefs only, `ThisRig` in session files only, `Rig { id, name }`
  pinned); `check()` refuses `Active` in `meas.layout` as today.
- **The lead** names the pane (chip, title), owns its readouts (delay, coherence, the
  sweep's distortion and IR views) and is what pane keys act on (S, B, P, the delay
  finder). Pane keys never act on a non-lead entry; selecting it in the tree does.
- **SPL, Programme and Walk panes keep one source.** The operator ruled SPL out of compare
  (2026-10-17: "add new pane … drag another spl measurement there"); a goniometer is one
  pair. Their pick is a radio choice, not a tick list.
- Limits in `check()`: at most 16 entries (a legend past that no longer reads), no entry
  twice, a `Group` and a `Live`/`Trace` of the same group together refused (the group
  already draws it).

**Starting content — never "everything".** A pane draws only its entries; nothing joins a
pane of its own accord except through a `Group` entry (D1). A new measurement's own pane
starts as `[Group(it)]`; a split starts with the focused pane's lead entry only (D3); a
pane of a kind with nothing picked says *nothing picked — the chip picks*. That keeps the
2026-10-09 complaint (every curve in every pane) out by construction.

### Invariant: a pick waits, it is never replaced

An entry naming something the app does not hold yet (its rig not connected or not synced,
the id not yet in that rig's mirror) **waits**: it stays in the pane and in every stored
layout, and its legend line says so (*FOH SPL · waiting for pupu*). It is never swapped for a
fallback (the layout's owner, the selection), and a fallback shown meanwhile is never
written back. An entry leaves a pane only by an untick, or when its rig, connected and
synced, reports the id deleted (the daemon prunes deleted ids from stored entries, as it
prunes compare lists today). Today's code breaks this: `LayoutPane.content`'s doc ("a pick
of a measurement gone since shows the layout's own measurement"), P1's "a pick of a deleted
measurement shows the owner", and `show_layout` (`meas_layout.rs`) filtering picks against
the mirror — which is how SPL 2's FOH SPL pane was stored as SPL 2.

## 2. How the rest fits

- **Compare (C)** becomes per pane (D2): *Add to this pane…* opens the chip list in tick
  mode on the focused pane. The daemon's per-measurement compare list (`Compared`,
  `meas.compare`) goes; salvage turns each measurement's list into `Live`/`Trace` entries of
  the panes of its own layout that hold its `Group`. Draw order stays: lead on top, later
  entries a layer further back and fainter (never below about half).
- **Hide (A / Shift+A) and slots 1–9** stay global and win everywhere (D4): a hidden entry
  stays listed in the pane's legend as *Slot 3 hidden — 3 shows it*, as a hidden lead does
  today.
- **Math channels** are measurements: `Live { meas }` of a math channel picks it alone, its
  owner's `Group` brings it. Cross-rig math stays as multi-rig.md §6 says.
- **Own layouts (P1).** Choosing a measurement's row still brings its panes, and editing
  panes while it is chosen stores them on it. A stored pane keeps every entry, other rigs'
  included, by `Rig { id, name }`, opaque to this daemon (D5); a pane whose entries are all
  another rig's stays in the app's prefs only, as N2 says, so bringing a layout never
  duplicates a pinned pane on screen.
- **Layout prefs** (`ui.toml`): entries by rig id and name (measurement name; trace name and
  id), resolved when the rig is back, as `park_gone_meas` does for measurements today.
- **Undo**: each tick, untick, reorder or row pick is one pane edit in the app's history.
- **Shared scale**: unchanged — same-kind panes on screen share one range (union).
- **Colours across rigs**: two rigs can hand out the same family. In a pane mixing rigs the
  app gives a later entry the next free family for that pane only, and each other rig's
  legend line carries its rig name (`pupu · TF 1`), as pinned panes' chips do.

## 3. The list

One list for every pane kind, filtered to what the pane can draw; **Enter** ticks or unticks
and the list stays open (a radio list closes on Enter), **Esc** closes. Ticks mark what the
pane holds; the lead carries *lead* (palette *Make lead*).

```
pupu                                    ← rig heading (§4)
  ✓ TF 1 · live + 3 traces              ← Group row (D1)
      live
    ✓ Capture 2 · slot 2
      Sweep run 1
    TF 2 · live + 1 trace
ketunkolo · ac2d
    Spectrum 1 · live
Inputs (pupu)                           ← Programme/SPL panes only
    in 1 + 2 · MM1 34804 + Vilppu behringer #3 · pair
    in 3 · line   in 4 · line   …
    in 1–8 side by side
───────────────
Turn into: Spectrum · Sweep / distortion · Programme
New measurement on pupu…   Ctrl+M
```

- Rows use the tree's words: the measurement's name, then what it brings (`live + 3
  traces`); children by their tree labels. No kind tags: the list holds only what the pane
  draws, so the kind is the pane's title. A row the pane cannot draw is absent, not greyed.
- Inputs read as everywhere else: `in N`, then the mic name or the input's kind; a backend
  port name never stands for a channel the session numbers. A pair's sides are not repeated
  on their own: they are the plain `in 1`, `in 2` rows.
- *Turn into* (pane kinds) and *New measurement…* sit under a rule after every rig, never
  under a rig heading; *New measurement on <rig>…* names the rig it creates on (the shown
  one).
- With one rig the headings go, as today.

## 4. Same-host rigs named by host and kind

An app on ketunkolo with its own embedded daemon and ketunkolo's `ac2d` listed them as *this
computer* and *ac2 on ketunkolo*: two names for one machine. Headings, the rig drop-down,
pinned chips and the Connection page name a rig on the app's host as **`<host> · ac2d`**
(reach local) and **`<host> · in this app`** (embedded); a network rig keeps the name it
sends (`pupu`). `ac2d --name` set by the operator wins over the composed name. App only:
the embedded daemon's `config.name` (`embedded.rs`) stops being the display name.

## 5. Wire and files

`PaneContent` changes shape and `LayoutPane.rig` moves into entries; `Compared` and
`meas.compare` go: **`PROTO_VERSION` 79 → 80, session format 40 → 41, rig file bump** (meter
layouts), `fixtures/protocol/WIRE_LOCK` regenerated. No serde defaults for old shapes;
`session salvage` and the rig file upgrade map: `Meas { meas }` → `Curves [Group(meas)]`
(`Meter` on an SPL pane), `content: None` → the owner's `Group`, the pane's `rig` onto each
entry, each compare list onto entries as in §2.

## Stages

Each stage lands alone and is tested through `ac2-scene` text (the list rows, legend lines,
pane titles) and state tests in `ac2-ui`.

0. **Rig names** (§4). App only.
1. **List wording** (§3 without ticks): filter by pane kind, *Turn into* / *New measurement*
   out of rig headings, input labels, no repeated pair sides, no kind tags. App only. The
   §1 invariant lands here or before (the SPL 2 fix), on today's `Meas` content.
2. **Entries on the wire** (§1, §5): `PaneContent::Curves`, per-entry rig, salvage, daemon
   pruning of deleted ids, `check()` limits; panes draw entries; C opens tick mode; compare
   lists go. PROTO / session / rig file bump.
3. **Tick list with children** (§3 in full): `Live`/`Trace` rows, cross-rig entries in one
   pane, rig-tagged legend lines, colour families per pane. Then the user guide's "Choosing
   what a pane shows" and Compare sections, and the P1 / N2 rows of `open-questions.md`.

## Operator decisions

- **D1 — Does a measurement row stay, and do new captures join?** Recommended: yes, as the
  `Group` entry, which follows its group (a new capture or math channel appears in the
  panes holding the group: capture-then-look stays one key). Individually ticked rows are
  fixed.
- **D2 — Compare becomes per pane.** Recommended: yes; the per-measurement compare list
  goes, salvaged into entries. Two lists for one idea would break "one control per concept".
- **D3 — What a split starts with.** Recommended: the focused pane's lead only (not a copy
  of all its entries, not empty).
- **D4 — Hidden vs ticked.** Recommended: hide (A, slot keys) wins in every pane; a tick
  never overrides it, the legend says how to show it.
- **D5 — Mixed-rig panes in a measurement's own layout.** Recommended: stored whole, other
  rigs' entries opaque by rig id (revises N2's "own layouts store only `ThisRig` panes" for
  mixed panes; an all-other-rig pane stays prefs-only).
