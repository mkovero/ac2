//! Which analysis stages (`bins/<meas>/<stage>`) the link receives: those a visible transfer
//! pane draws, or is about to draw on a little more zoom or pan.

use std::collections::HashSet;

use ac2_proto::FrameData;
use ac2_proto::model::{MathDomain, MeasKind, Measurement, TransferResolution};
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::tf::bins;

use crate::state::{AppState, PaneId, PaneKind, TransferView};

impl AppState {
    /// The stages each visible transfer pane wants of the curves it draws as their own
    /// (live, running or stopped; a transfer at `bins` or a transfer-domain math with a
    /// layout), from the shown range and the pane's plot width ([`bins::wanted`]). None
    /// for a pane in its IR or alignment view, showing only an unwrapped phase or group
    /// delay, with a peak hold or peak tracking on (both work on the grid), for a smoothed
    /// curve, or before the pane was drawn. A function of what is shown alone: the same
    /// view wants the same stages.
    pub(super) fn bin_topics(&self, visible: &[PaneId]) -> HashSet<Topic> {
        let mut out = HashSet::new();
        for &pane in visible {
            let Some(v) = self.layout.view(pane) else {
                continue;
            };
            if v.kind != PaneKind::Transfer
                || v.shows_ir()
                || v.modes.transfer == TransferView::Alignment
            {
                continue;
            }
            let Some(&px) = self.plot_px.get(&pane) else {
                continue;
            };
            let view = self.view_for(pane);
            let tf = view.tf;
            if !(tf.show_magnitude
                || tf.show_coherence
                || (tf.show_phase && bins::reads_bins(tf.phase)))
            {
                continue;
            }
            let shows = self.pane_meas(pane);
            let curves: Vec<&Measurement> = self
                .measurements()
                .into_iter()
                .filter(|m| {
                    self.live_on_pane(PaneKind::Transfer, m, shows)
                        || self.live_compared_on(PaneKind::Transfer, m, pane)
                })
                .collect();
            let tracks = shows
                .filter(|m| m.config.kind.publishes_tf())
                .is_some_and(|m| self.rig.peak_of(m).tracks());
            if tracks || curves.iter().any(|m| self.rig.peak_of(m).holds()) {
                continue;
            }
            for m in curves.into_iter().filter(|m| has_bins(m)) {
                let Some(f) = crate::scenes::frame(self, m.id, Stream::Tf) else {
                    continue;
                };
                let FrameData::Tf(f) = &f.frame.data else {
                    continue;
                };
                let Some(layout) = f.meta.bins.as_deref() else {
                    continue;
                };
                if f.meta.smoothing.is_some() {
                    continue;
                }
                let unresolved = f.meta.unresolved.as_deref();
                out.extend(
                    bins::wanted(layout, unresolved, view.freq.range(), f64::from(px))
                        .into_iter()
                        .map(|stage| Topic::Bins { meas: m.id, stage }),
                );
            }
        }
        out
    }
}

/// A measurement whose `tf` may state analysis stages: a transfer at `bins`, or a
/// transfer-domain math (whose operands decide).
fn has_bins(m: &Measurement) -> bool {
    match &m.config.kind {
        MeasKind::Transfer { config } => config.resolution == TransferResolution::Bins,
        MeasKind::Math { config } => config.domain == MathDomain::Transfer,
        _ => false,
    }
}
