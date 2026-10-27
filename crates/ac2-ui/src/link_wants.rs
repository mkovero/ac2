//! Which measurement streams the link receives and how often it hands new frames over:
//! what the visible panes draw and what the reducer folds, at the rate they can show.
//!
//! Every stream costs the daemon a send, the link a decode and the UI a redraw; some cost
//! the daemon work of their own (an IR is derived only while someone subscribes to it). So
//! the app subscribes per topic, following the layout, the panes' measurements and the
//! daemon's measurement list.

use std::collections::HashSet;
use std::time::Duration;

use ac2_proto::model::{MathDomain, MeasKind, PeakMode};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::{MeasId, RigId};

use crate::conn::{DISPLAY_PERIOD, Request};
use crate::state::{AppState, PaneKind, RigState};

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
    /// The streams the rig in `rig` should send now, for its panes on screen:
    /// - each transfer measurement's TF while a transfer pane draws its response, and the
    ///   IR and TF of the one a transfer pane in its IR view shows;
    /// - each spectrum / RTA while the spectrum pane is in view or peak hold or the
    ///   spectrograph is on (both fold every frame, so a hidden pane keeps its history);
    /// - each SPL meter's readout and Leq windows always: the held reading, the Leq history
    ///   and its alarms follow every frame, shown or not.
    ///
    /// Per-measurement input levels are never drawn, so never received.
    pub fn wanted_topics(&self) -> HashSet<Topic> {
        let visible = self.own_visible_panes();
        let shows = |p: PaneKind| {
            visible
                .iter()
                .filter_map(|id| self.layout.view(*id))
                .any(|v| v.kind == p && !v.shows_ir())
        };
        let ir_of: Vec<MeasId> = visible
            .iter()
            .filter(|id| self.layout.view(**id).is_some_and(|v| v.shows_ir()))
            .filter_map(|id| self.pane_meas(*id).map(|m| m.id))
            .collect();
        let spectrum = shows(PaneKind::Spectrum) || self.spectrograph_shown();
        topics_of(&self.rig, shows, &ir_of, spectrum)
    }

    /// How often new frames may reach the UI: [`DISPLAY_PERIOD`], or [`SPL_ONLY_PERIOD`]
    /// when the SPL pane is all there is to see.
    pub fn display_period(&self) -> Duration {
        let kinds: Vec<PaneKind> = self
            .own_visible_panes()
            .into_iter()
            .map(|id| self.layout.kind(id))
            .collect();
        if kinds.is_empty() {
            // No pane shows this rig: what it folds sets the pace. Peak holds fold every
            // frame; SPL readings and Leq windows move slower.
            let spectra = self.wanted_topics().iter().any(|t| {
                matches!(
                    t,
                    Topic::Data {
                        stream: Stream::Spec | Stream::Rta,
                        ..
                    }
                )
            });
            if spectra {
                DISPLAY_PERIOD
            } else {
                SPL_ONLY_PERIOD
            }
        } else if kinds == [PaneKind::Spl] {
            SPL_ONLY_PERIOD
        } else {
            DISPLAY_PERIOD
        }
    }

    /// The requests that bring each rig's link in line with what it should receive; nothing
    /// for a link that already is. Each follows its own panes on screen
    /// ([`Self::wanted_topics`], [`Self::display_period`]) and what it folds whether shown
    /// or not. The app calls it after every message it feeds the reducer.
    pub fn sync_link(&mut self) -> Vec<(RigId, Request)> {
        let mut out = Vec::new();
        let ids: Vec<RigId> = self.rigs().map(|r| r.id).collect();
        for id in ids {
            self.visit(id, |st| {
                let (topics, period) = (st.wanted_topics(), st.display_period());
                sync(&mut st.rig, topics, period, &mut out);
            });
        }
        out
    }
}

/// The requests that bring `rig`'s link to `topics` at `period`.
fn sync(
    rig: &mut RigState,
    topics: HashSet<Topic>,
    period: Duration,
    out: &mut Vec<(RigId, Request)>,
) {
    if rig.link_wants.topics.as_ref() != Some(&topics) {
        rig.link_wants.topics = Some(topics.clone());
        out.push((rig.id, Request::Topics(topics)));
    }
    if rig.link_wants.period != Some(period) {
        rig.link_wants.period = Some(period);
        out.push((rig.id, Request::DisplayPeriod(period)));
    }
}

/// The streams `rig` should send: those of the measurements a pane draws (`shows` a pane
/// kind outside an IR view, `ir_of` the measurements an IR view shows, `spectrum_shown` a
/// spectrum pane or spectrograph), and those folded whether shown or not (a spectrum's
/// peak hold, every SPL meter).
fn topics_of(
    rig: &RigState,
    shows: impl Fn(PaneKind) -> bool,
    ir_of: &[MeasId],
    spectrum_shown: bool,
) -> HashSet<Topic> {
    let spectrum = spectrum_shown
        || rig
            .measurements()
            .iter()
            .any(|m| rig.display_of(m).peak != PeakMode::Off);
    let mut out = HashSet::new();
    for m in rig.measurements() {
        let streams: &[Stream] = match &m.config.kind {
            MeasKind::Transfer { .. } => {
                match (shows(PaneKind::Transfer), ir_of.contains(&m.id)) {
                    (true, true) => &[Stream::Tf, Stream::Ir],
                    (true, false) => &[Stream::Tf],
                    // The IR view carries its transfer stream's banners (no reference, no
                    // signal), which only the transfer frames say.
                    (false, true) => &[Stream::Tf, Stream::Ir],
                    (false, false) => &[],
                }
            }
            MeasKind::Spectrum { .. } if spectrum => &[Stream::Spec],
            MeasKind::Rta { .. } if spectrum => &[Stream::Rta],
            MeasKind::Spectrum { .. } | MeasKind::Rta { .. } => &[],
            // The band frames only while the meter has a band meter: none come otherwise.
            MeasKind::Spl { config } if config.bands.is_some() => {
                &[Stream::Spl, Stream::Leq, Stream::BandLeq]
            }
            MeasKind::Spl { .. } => &[Stream::Spl, Stream::Leq],
            MeasKind::Math { config } => match config.domain {
                MathDomain::Transfer if shows(PaneKind::Transfer) => &[Stream::Tf],
                MathDomain::Spectrum if spectrum => &[Stream::Spec],
                MathDomain::Rta if spectrum => &[Stream::Rta],
                _ => &[],
            },
            // Its results are stored traces, fetched as traces.
            MeasKind::Sweep { .. } => &[],
        };
        out.extend(
            streams
                .iter()
                .map(|&stream| Topic::Data { meas: m.id, stream }),
        );
    }
    out
}
