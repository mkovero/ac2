# Rig: pupu

Dedicated audio test rig, shared with the archived `ac` project (its profile:
`~/src/ac/docs/rigs/pupu.md`, the authority for wiring and baseline restore). This page holds
what ac2 learned on it.

- **Access:** private (`$AC_HOME/rig-hosts/pupu.access.env`). Never build on the rig; ship
  binaries. Take the rig lock first: `~/src/ac/bin/rig.sh --lock "<who, what>"`.
- **Audio:** RME Fireface 400, JACK `jack-ac.service`, 96 kHz / 256 frames / 3 periods, `-S`.
  JACK port order can move (ADAT block before/after analog): check silently before emitting —
  captures 9–14 read exact digital zero when analog is first.
- **Wiring:** out 1 (AN1) → Genelec 1083 (three-way); out 2 (AN2) → cable → in 2 (reference
  loopback); in 1 = measurement mic, beyerdynamic MM1 (449350, s/n 34804), **mounted at 90°
  (pointing up)** → use `~/src/ac/beyer/449350_34804_90Grad.txt`.
- **Emission ceilings:** −40 dBFS standing, **−50 dBFS on anything that drives the speaker**.
- **Baseline check (2026-10-03):** 1 kHz at −60 dBFS on out 2 only → in 2 −57.7 dBFS (profile
  −57.5), no leakage into in 1 (−126 dBFS at 1 kHz); mic room noise ≈ −72 dBFS, with occasional
  60–200 Hz rumble bursts up to ≈ −53 dBFS.
- **Network:** ufw active. Since 2026-10-03 (operator approved): `47820/tcp`, `47821/tcp` and
  `5353/udp` allowed from 192.168.9.0/24 only, one rule per port — the RT kernel lacks the
  iptables `multiport` module, so a ufw port *range* is listed but silently not enforced.
  Clients: ketunkolo (192.168.9.25) authorized as `ketunkolo`.

## First measurement (2026-10-03, ac2 850a3a4)

Pink noise −50 dBFS on out 1 + out 2, transfer in 1 vs in 2, delay finder: 3.32 ms (319
samples), mic ≈ 1 m. ac2's transfer function agreed with an independent numpy H1 within 0.5 dB
from 100 Hz to 16 kHz. Response re 1 kHz (1/6 oct): within ±3 dB 100 Hz–1.6 kHz; **broad dip
−5…−9.5 dB from 2 to 6.3 kHz** (MM1 90° correction changes it by < 1 dB) — not normal for a
1083 on axis; check mic height/aim against the speaker's reference axis, or the mid/tweeter
section (the `ac` profile notes intermittent distortion). Floor bounce ≈ +3.5 ms at −23 dB;
strong late reflection at +17.9 ms (−18 dB); broadband T20 ≈ 0.1 s. Data:
`/home/mui/ac2-1083-transfer*.png`.

## Network test (2026-10-03)

ketunkolo → pupu over the LAN, CURVE: remote CLI (status, session, measurements) ✓; app on
ketunkolo's display with live JACK data ✓; CLI + app on one daemon stay in sync ✓; unpaired and
pinned-but-unauthorized clients refused ✓ (messages unclear — backlog); daemon killed → app shows
DAEMON NOT RESPONDING + STALE with traces kept, restarted → app resyncs by itself ✓. mDNS
discovery ✗ (backlog). Remote generator ✗ (silent output, being fixed).
