# Settings view, system max level at run time, output names

Status: implemented (PROTO 23). Operator request 2026-10-06: *"how do you choose which
output devices to use for stimulus? how do you set system max output limit from UI? should
we have some 'settings' page instead of gazillion of different configurables that one might
find or not under ctrl+k?"*

## 1. One full-window view

Settings were scattered: the session dialog (Shift+O), the input setup / calibrations view
(palette only), the Leq dialog (Shift+L), palette prompts (*Stimulus: type output
channels…*), `ui.toml`, daemon flags. They are now pages of one view:

| Page | Holds | Whose |
|---|---|---|
| Inputs & outputs | input rows (in session, R reference / M mic, mic name, curve, meter), output rows by name (the rig's label, else the device's channel name) with **S** ticks for the stimulus, the reference pair, loopback detection, the **system max level** | the rig (stimulus ticks: this app) |
| Audio | backend, device, rate, buffer | the rig |
| Calibration | the former calibrations view: inputs, mic library, sensitivities, the acoustic / electrical dialogs | the rig |
| SPL / Leq | the former Leq dialog, for the SPL pane's meter | the rig |
| Recording | the record toggle's limit; the daemon's recording folder | this app / the rig |
| Display | theme, key hints, the SPL number's hold, the spectrograph's span, level axes reset | this app |
| Connection | the link, this client's id and key, reconnect, the connect dialog (pairing); the daemon's mode, mDNS name, authorized clients (add, revoke) and refused keys | this app / the rig |

Every page says whose its settings are: **this app** (kept in `ui.toml` on this computer)
or **the rig — all clients** (kept by the daemon; every client sees the change).

Views are per pane (the pane tree, PLAN §8.1): the Display page's spectrum-view and
sweep-view rows set the view of the pane of that kind focused last, and with no such pane on
screen say so instead of making one. The layout itself — the tree, each pane's kind, views
and measurement by name — is this app's, kept in `ui.toml` under `[layout]` (`[layout.tree]`
and one `[[layout.panes]]` per pane); a layout this version cannot read is dropped and the
app starts with one pane.

**Keys** (decision K9 holds: an open window owns the keyboard): `Ctrl+P` (palette
*Settings…*, the ⚙ in the top bar) opens it at the page last shown; `Ctrl+PgUp / PgDn`
(or `Ctrl+Tab`) step pages, `Alt+1…7` jump; ↑/↓ ←/→ Enter as in every window; Esc closes
the topmost window (a calibration dialog or the raise confirmation over a page, else the
view); Shift+Esc stops the stimulus from anywhere. `Ctrl+,` — the desktop convention — is the
transfer pane's whole-sample delay step, so the letter P (preferences) it is; layout-safe on
every keyboard. The keys that opened the old dialogs open their pages: Shift+O → Audio,
Shift+L → SPL / Leq, palette *Input setup…* → Inputs & outputs, *Calibrations…* →
Calibration, *Stimulus outputs…* → Inputs & outputs on the outputs. No dialog is left
behind beside its page.

The view covers the window below the top bar: what drives the speakers (badge, level,
outputs, ■ Stop) stays in sight while settings change.

The session model (`SessionDialog`) is shared by the Inputs & outputs and Audio pages:
each shows its part of the rows; Enter on either opens (or reopens) the session with what
both say. A mouse click on a row shows its page.

## 2. Output names and stimulus outputs

The rig's outputs get operator labels (`Main L`, `Sub`): `state.outputs`, `session.outputs`
upserting `{channel, label}` (`nil` clears). They belong to the rig, not to a session — a
loaded session must not rename the wiring — so the daemon keeps them in its **rig settings
file** with the system max level (§3). Keyed by channel number like the input setup's mic
names (one rig, one interface; a different device shows the same labels on the same
numbers). N on an output row names it; Enter sends it to the daemon for every client; the
top bar names the stimulus outputs by label (`→ Main L, out 3`).

Which outputs carry the stimulus stays *this app*'s choice, per output device (decision K4,
`ui.toml`): **S** on an output ticks it. When the open session already has that output the
stimulus moves there at once (the stream carries every session output; an armed or firing
stimulus is re-routed with its fade); otherwise the tick applies when the session opens. The
typed-channels prompt is gone.

The **default reference** is the reference pair the session opens with: the R input and the
first ticked output (`Reference (loopback): input 1 · Loop ← output 1 · Main L`), shown under
the channels, with *applies when the session opens (Enter)* until it is the open session's.

## 3. System max level at run time

The generator ceiling was only `ac2d --max-level`; changing it meant restarting the daemon.

- `generator.ceiling` is the level in force, `generator.ceiling_bound` the `--max-level`
  flag: a **hard upper bound** no client can exceed (the flag's default, −10 dBFS RMS, is
  unchanged). `gen.ceiling {ceiling, confirm_raise}`, any client, no lease (lowering is a
  safety action like stop; raising is guarded below).
- **Lowering applies at once.** The output path's peak limit follows on the running stream
  (`GeneratorHandle::set_max_level`, an atomic the callback reads per block; the stream opens
  with the bound's limit, so a later raise needs no reopen). A stimulus armed or playing
  above the new maximum, and a sweep running above it, is **stopped and disarmed** (20 ms
  fade, audited as a stop by the client that lowered it). Stopping, not turning down: a
  quietly lowered level would leave the owner's typed level, the top bar and the measurement
  disagreeing with what plays, and a sweep's analysis assumes the level it was armed with; a
  stop is unambiguous and the owner re-arms at a level within the maximum. A stimulus at or
  below the new maximum carries on untouched.
- **Raising** needs `confirm_raise` (the app asks the operator to type `raise`; the CLI
  `--yes`), is refused while anything is armed or playing (or a sweep or loopback detection
  runs), never exceeds the bound, and is logged at warn level in the daemon's audit log with
  the client's name (`ac2d::audit`: *system max level raised from −50.0 to −40.0 dBFS by
  laptop*). The event's `last_action` is `ceiling_raised` / `ceiling_lowered` with the
  client.
- **Persisted** by the daemon in `rig.json` (config directory; `ac2d --settings` names
  another file), atomic write, before the change applies (a level that would come back
  different after a restart is worse than a refused change). At start the level in force is
  min(kept, bound): a later, lower `--max-level` wins. A file that cannot be read is never
  written (the daemon starts at the bound and refuses changes with the reason); one of
  another format version is set aside.
- Every emitting path keeps enforcing it: `gen.set`, `sweep.run`, `session.detect_loopback`
  refuse levels above it; the peak limit of the running stream and of the detection burst's
  stream follow it.
- CLI: `ac2 gen ceiling` shows it (with the bound and who changed it last),
  `ac2 gen ceiling -40dbfs` lowers, `--yes` confirms a raise.

## 4. Server features on the Connection page

`server.info` reports the transport (`embedded`, `local {ctrl}`, `network {ctrl, data,
server_key, fingerprint, advertised_as, authorized, refused}`) and the recording folder.
In network mode the page lists the authorized clients (name, fingerprint) and the keys
refused since the daemon started (fingerprint, address, count, age; at most 32, newest
first — kept by the ZAP audit hook). **A** on a refused key authorizes it under a typed name;
*Authorize a client by its key…* takes `name key`; **Delete** twice revokes a client.
`server.authorize` / `server.revoke` write the authorized-clients file atomically and
replace the handshake check's keys (`AuthorizedHandle`); a revoked client's requests are
refused at once (its data socket keeps receiving until the connection drops — libzmq has no
per-peer disconnect on ROUTER/XPUB). Nobody revokes their own key (it would lock that
operator out). Any authorized client may do this: in network mode every paired client is
equally trusted already (it can drive the stimulus). Local and embedded daemons have no
keys (`unsupported`).

This client's key (fingerprint and Z85, for a rig's operator to authorize) is read from the
key directory, never created there: pairing in the connect dialog makes it. *Connect to
another daemon, or pair with a rig…* opens the connect dialog over the view.

## 5. What is not in the session file

Neither the output labels nor the system max level are session content (`ac2-traces` session
format unchanged): both are the rig's, kept by the daemon across sessions and restarts.

## 6. Open

- Bulk palette prompts (*Input setup: type mic names (3=M30, 4=ECM)…*, *Mic curve on input
  N…*, *Calibration: delete … (input=mic)…*) stay as typed shortcuts; their effect is visible
  on the pages. Remove them if operators never use them.
- Labels follow channel numbers, not devices; per-device labels if rigs switch interfaces.
- A revoked client keeps its data stream until it reconnects.
