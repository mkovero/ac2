//! What a rig keeps of its frames only to draw them: eases, holds, histories and display
//! caches, folded over time as the data comes in ([`RigState::fold_data`]).

use super::*;

/// A rig's drawing state. None of it is the daemon's: it is what the frames so far leave
/// behind for the next picture, and a rig in the background keeps folding its own.
#[derive(Debug, Default)]
pub struct Folded {
    /// Per Leq and band Leq topic, the frame the newest one replaced: the bars ease from it
    /// ([`ac2_scene::bar_ease`]).
    pub(crate) bar_from: HashMap<Topic, Arc<ac2_proto::Frame>>,
    /// Per live spectrum or RTA topic, the ease of its curve to a slow newest frame
    /// ([`ac2_scene::curve_ease`]).
    pub(crate) spec_glide: HashMap<Topic, SpecGlide>,
    /// Per measurement, the display-clock time axis its spectrograph scrolls on
    /// ([`ac2_scene::scroll_clock`]).
    pub(crate) scroll: BTreeMap<MeasId, ac2_scene::scroll_clock::ScrollClock>,
    /// Per transfer topic, its walk readiness bar's glide to the newest frame's `filled`
    /// ([`FillGlide`]).
    pub(super) fill_glide: HashMap<Topic, FillGlide>,
    /// Per running transfer measurement, its headroom advice over time
    /// ([`ac2_scene::advice::Hold`]).
    pub(super) advice: HashMap<MeasId, ac2_scene::advice::Hold>,
    /// The transfer view's stored-trace display math, kept between frames.
    pub(crate) tf_display: ac2_scene::trace::DisplayCache,
    /// Peak hold per spectrum / RTA measurement, with the last frame folded in and its
    /// capture time.
    pub peaks: BTreeMap<MeasId, (Option<FoldedAt>, u64, PeakHold)>,
    /// Tracked peaks per spectrum / RTA measurement, with the last frame folded in, kept
    /// while peak tracking is on or a pane picked its curve.
    pub peak_tracks: BTreeMap<MeasId, (Option<FoldedAt>, ac2_scene::spectrum::peaks::PeakTracker)>,
    /// The peaks of stored spectra a pane picked, found once per data: a stored trace is
    /// fixed.
    pub(crate) trace_peaks: BTreeMap<TraceId, Vec<ac2_scene::spectrum::peaks::Peak>>,
    /// Spectrograph history per spectrum / RTA measurement, kept while the spectrograph is
    /// shown.
    pub spectrographs: BTreeMap<MeasId, ac2_scene::spectrograph::SpectrographHistory>,
    /// Each SPL meter's displayed reading, held for its display period.
    pub spl_hold: BTreeMap<MeasId, ac2_scene::spl::SplHold>,
    /// Started spectrum / RTA measurements whose first frame fits the spectrum pane's level
    /// axis (as Shift+Home), with the frame shown when they started (a stopped run's last).
    pub(super) spectrum_fit: BTreeMap<MeasId, Option<Arc<ac2_proto::Frame>>>,
    /// Spectrum / RTA measurements seen running, so a start is told from a run going on.
    pub(super) spectrum_running: BTreeSet<MeasId>,
}
