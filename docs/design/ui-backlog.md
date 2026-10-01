# UI polish backlog

Small UX items found while building, to settle during phase 4 (`ac2-ui`). Not decisions —
pick sensible fixes and record them in the scene tests.

## Done (in `ac2-scene`, tested in `tf.rs` / `axis.rs`)

- Coherence overlay: γ² = 1 sat on the pane's top border → the band is inset 4 px
  (`OVERLAY_INSET`); `coherence_overlay_layout`.
- Coherence overlay: curve ran through the magnitude title / legend / delay line → in overlay
  mode the title, legend, delay line and cursor values start below the band;
  `overlay_text_sits_below_the_band`.
- Overlay mode changed tick density (10 dB / 45° steps) vs pane mode → y-axis steps are chosen
  from the pane-mode heights in both placements (`axis::axis_with_density`), so toggling the
  overlay relabels nothing; `coherence_overlay_layout`, `density_sets_the_step_not_the_mapping`.
- Narrow windows: legend and cursor readout shared rows and collided → when any readout row
  would run into its legend text (0.62 em/char estimate), all readout rows move below the
  legend and delay line; `narrow_panes_move_cursor_values_below_the_legend`.

- Narrow panes (found in the first `ac2-ui` snapshots): a banner row too narrow for text and
  detail drops the detail (`narrow_rows_drop_the_detail`); the IR origin line moves under the
  title (`narrow_plot_puts_the_origin_under_the_title`); SPL statistics wrap to two rows
  (`narrow_meter_wraps_the_statistics`).

## Open

- Overlay mode in a short magnitude pane: with the text block below the band, the legend now
  sits over the 0 dB region where traces usually are. A translucent legend backing (plot
  background at ~70 %) would keep both readable; needs a design call on legend chrome.
