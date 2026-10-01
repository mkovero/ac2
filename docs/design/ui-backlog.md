# UI polish backlog

Small UX items found while building, to settle during phase 4 (`ac2-ui`). Not decisions —
pick sensible fixes and record them in the scene tests.

- Coherence overlay: γ² = 1 sits on the pane's top border; inset the band ~4 px.
- Coherence overlay: curve runs through magnitude title/legend/delay line; move the legend
  block below the overlay band in overlay mode.
- Overlay mode changes tick density (10 dB / 45° steps) vs pane mode; consider keeping steps
  stable across modes.
- Narrow windows: legend (left) and cursor readout (right) share rows and can collide; wrap or
  move the readout below.
