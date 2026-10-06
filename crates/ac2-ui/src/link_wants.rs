//! Which measurement streams the link receives and how often it hands new frames over:
//! what the visible panes draw and what the reducer folds, at the rate they can show.
//!
//! Every stream costs the daemon a send, the link a decode and the UI a redraw; some cost
//! the daemon work of their own (an IR is derived only while someone subscribes to it). So
//! the app subscribes per topic, following the layout, the panes' measurements and the
//! daemon's measurement list.

use std::collections::HashSet;
use std::time::Duration;

use ac2_proto::model::{MathDomain, MeasKind};
use ac2_proto::topic::{Stream, Topic};

use crate::conn::{DISPLAY_PERIOD, Request};
use crate::state::{AppState, PaneKind};

/// Display period when only the SPL pane is in view: its readout is held for at least a
/// tenth of a second and its Leq windows move once a second, so faster shows nothing new.
pub const SPL_ONLY_PERIOD: Duration = Duration::from_millis(100);

/// What the link was last asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sent {
    topics: Option<HashSet<Topic>>,
    period: Option<Duration>,
}

impl AppState {
    /// The streams to receive now:
    /// - each transfer measurement's TF while the transfer pane is in view, and the IR and
    ///   TF of the one the IR pane follows while that pane is;
    /// - each spectrum / RTA while the spectrum pane is in view or peak hold or the
    ///   spectrograph is on (both fold every frame, so a hidden pane keeps its history);
    /// - each SPL meter's readout and Leq windows always: the held reading, the Leq history
    ///   and its alarms follow every frame, shown or not.
    ///
    /// Per-measurement input levels are never drawn, so never received.
    pub fn wanted_topics(&self) -> HashSet<Topic> {
        let visible = self.layout.visible();
        let shows = |p: PaneKind| visible.contains(&p);
        let ir_of = shows(PaneKind::Ir)
            .then(|| crate::scenes::focus_tf(self).map(|m| m.id))
            .flatten();
        let spectrum = shows(PaneKind::Spectrum)
            || self.view.spectrum.peak_hold
            || self.view.spectrum.mode.spectrograph();
        let mut out = HashSet::new();
        for m in self.measurements() {
            let streams: &[Stream] = match &m.config.kind {
                MeasKind::Transfer { .. } => match (shows(PaneKind::Transfer), ir_of == Some(m.id))
                {
                    (true, true) => &[Stream::Tf, Stream::Ir],
                    (true, false) => &[Stream::Tf],
                    // The IR pane carries its transfer stream's banners (no reference, no
                    // signal), which only the transfer frames say.
                    (false, true) => &[Stream::Tf, Stream::Ir],
                    (false, false) => &[],
                },
                MeasKind::Spectrum { .. } if spectrum => &[Stream::Spec],
                MeasKind::Rta { .. } if spectrum => &[Stream::Rta],
                MeasKind::Spectrum { .. } | MeasKind::Rta { .. } => &[],
                MeasKind::Spl { .. } => &[Stream::Spl, Stream::Leq],
                MeasKind::Math { config } => match config.domain {
                    MathDomain::Transfer if shows(PaneKind::Transfer) => &[Stream::Tf],
                    MathDomain::Spectrum if spectrum => &[Stream::Spec],
                    MathDomain::Rta if spectrum => &[Stream::Rta],
                    _ => &[],
                },
            };
            out.extend(
                streams
                    .iter()
                    .map(|&stream| Topic::Data { meas: m.id, stream }),
            );
        }
        out
    }

    /// How often new frames may reach the UI: [`DISPLAY_PERIOD`], or [`SPL_ONLY_PERIOD`]
    /// when the SPL pane is all there is to see.
    pub fn display_period(&self) -> Duration {
        if self.layout.visible() == [PaneKind::Spl] {
            SPL_ONLY_PERIOD
        } else {
            DISPLAY_PERIOD
        }
    }

    /// The requests that bring the link in line with [`Self::wanted_topics`] and
    /// [`Self::display_period`]; nothing when it already is. The app calls it after every
    /// message it feeds the reducer.
    pub fn sync_link(&mut self) -> Vec<Request> {
        let mut out = Vec::new();
        let topics = self.wanted_topics();
        if self.link_wants.topics.as_ref() != Some(&topics) {
            self.link_wants.topics = Some(topics.clone());
            out.push(Request::Topics(topics));
        }
        let period = self.display_period();
        if self.link_wants.period != Some(period) {
            self.link_wants.period = Some(period);
            out.push(Request::DisplayPeriod(period));
        }
        out
    }
}
