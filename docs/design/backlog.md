# Backlog

Work found in use, not yet scheduled into a phase. Newest first. Move an item to "Done" with
the commit when it lands.

## From the first rig session on pupu (2026-10-03)

Being fixed on `fix/rig-findings`:
- **Remote generator plays silence.** A remote client (network mode, CURVE) gets ARMED/FIRING
  and audit Fire, but the JACK output ports carry digital silence; the same command from a local
  client works. Suspect: the lease gate latches the source muted before it is opened.
- **JACK outputs never connected by the daemon**, and a generator start that changes routing
  reopens the stream and drops manual `jack_connect`s, so the stimulus never reaches hardware.
- **`ac2 spl watch` reads the wrong level/input** (−68.5 dBFS where an independent recording
  showed −57.7 dBFS at 1 kHz on that input).
- **Numeric measurement names can't be addressed** (`delay find 1083` → "no measurement 1083").

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
- **mDNS discovery** (being fixed on `fix/rig-findings`): the daemon logs "advertising" but holds
  no UDP socket and answers nothing; on a client with two interfaces in one subnet the query
  leaves via the wrong one.
- **Daemon network ports vs host firewall.** On a host with ufw active, network mode is silently
  unreachable. `ac2d --listen` should warn when an active firewall (ufw/firewalld/nftables drop
  policy) is detected and say which ports to open; `ac2 --remote` "not responding" should hint at
  firewalls.
