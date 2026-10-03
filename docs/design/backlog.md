# Backlog

Work found in use, not yet scheduled into a phase. Newest first. Move an item to "Done" with
the commit when it lands.

## From the first rig session on pupu (2026-10-03)

Open:
- **Mic curve on stored traces.** Apply (and remove) a mic correction curve to an already
  captured trace, so captures taken before calibration can be corrected afterwards; recorded in
  the trace metadata like smoothing. Today ac2 corrects only live measurement inputs.
- **Set trace smoothing from the CLI** (`ac2 trace smooth <t> 1/12`, or `trace update
  --smoothing`), so smoothed pictures and exports don't need the keyboard.
- **"No audio session" hint covers stored traces** in the transfer pane; it should yield (or move
  to the banner strip) when the pane has data to show.
- **Finder reports "AMBIGUOUS · merged arrivals" while listing a single candidate.** Either the
  ambiguity is real and the second candidate must be listed, or the outcome should be accepted.
- **Clearer refusal messages.** An unpaired CLI fails locally with a bare
  "client.key: No such file" (should say "not paired: run `ac2 auth pair <host>`"). A paired but
  unauthorized client only sees "daemon … is not responding" (CURVE refusal looks like silence):
  add "or this client is not authorized on the daemon (fingerprint …)" to that message, and have
  the daemon log every refused key's fingerprint and address (rate-limited) — the rig's log had
  no line for a refused client.

## Done

Rig findings fixed on `fix/rig-findings`:
- **Remote generator played silence on JACK.** Arming reopened the stream to change the
  generator routing, which closed and re-created the JACK client: every connection to
  `ac2:out_N` (the recorder, the hand patch to the interface) was gone while the state said
  FIRING. Routing now changes inside the running stream. The lease gate also never mutes a
  source that has not played, and a mute on an expired deadline disarms with an `expiry`
  event.
- **JACK outputs never connected by the daemon.** The outputs chosen for the stimulus (and the
  loopback output) are connected to the playback ports of the same number, and again after a
  reopen; nothing else is connected or disconnected.
- **`ac2 spl watch` read the wrong level.** `--input` reused any running SPL meter with the same
  settings, typically one a killed watch had left behind, whose Leq integrated since long
  before the tone. The watch now always runs its own meter, cleans it up on SIGTERM/SIGHUP too,
  and prints the input and meter id; `--for` ends it after a set time.
- **Numeric measurement names** resolve: id first, then name; a value that is one measurement's
  id and another's name is refused naming both.
- **Daemon network ports vs host firewall.** `ac2d --listen` names its ports and warns when ufw
  or firewalld is active (one rule per port); "not responding" from a remote client hints at
  the firewall.
- **The daemon never answered mDNS.** `Handle::wait` dropped the advert before blocking, so the
  responder said goodbye and shut down right after "advertising". The advert now lives until the
  daemon stops; responder errors are logged.
- **`ac2 discover` asked on some interfaces only.** The browser now also asks on every interface
  address itself (0, 1, 3 s) and lists where it asked when nothing answered.
