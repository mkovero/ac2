//! The level (vertical) axis of each pane: its own range, zoomed, panned, reset and fitted
//! by the keys and the wheel on that pane alone, so two panes of one kind (two transfer
//! measurements side by side) keep scales of their own. The frequency axis stays shared:
//! panes read against each other at the same frequencies.
//!
//! A range the operator set (zoom, pan, reset, fit) is kept with the panes: in the layout of
//! the measurement whose row brought them (`meas.layout`) and in `ui.toml`. A range framed
//! automatically is not: wherever the panes come back, a pane frames its first curve.

use ac2_proto::model::layout::{LayoutLevels, LevelRange};
use ac2_proto::model::{LevelScale, Measurement};
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::axis::Range;
use ac2_scene::format;
use ac2_scene::view::{ViewState, level};

use super::{AppState, PaneId, PaneKind};

/// Zoom factor of one level-zoom key press.
pub const LEVEL_ZOOM_FACTOR: f64 = 1.5;

/// The level axis ranges of one pane, dB: one per kind it may show (a pane keeps the
/// others while it shows another kind), the spectrum's one per scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneLevels {
    pub transfer: Range,
    pub spectrum_dbfs: Range,
    pub spectrum_spl: Range,
    pub distortion: Range,
}

impl PaneLevels {
    /// The ranges `view` holds.
    pub fn of(view: &ViewState) -> Self {
        Self {
            transfer: view.tf.magnitude_db,
            spectrum_dbfs: view.spectrum.level,
            spectrum_spl: view.spectrum.level_spl,
            distortion: view.distortion.range_db,
        }
    }

    /// Puts the ranges on `view`, the one a pane is drawn with.
    pub fn apply(&self, view: &mut ViewState) {
        view.tf.magnitude_db = self.transfer;
        view.spectrum.level = self.spectrum_dbfs;
        view.spectrum.level_spl = self.spectrum_spl;
        view.distortion.range_db = self.distortion;
    }

    /// The range a pane of `kind` draws on; the spectrum's is the one for `scale`, the scale
    /// its curves are shown in ([`crate::scenes::spectrum_scale`]). The SPL pane has none.
    pub fn range(&self, kind: PaneKind, scale: LevelScale) -> Option<Range> {
        match kind {
            PaneKind::Transfer => Some(self.transfer),
            PaneKind::Spectrum => Some(match scale {
                LevelScale::Dbfs => self.spectrum_dbfs,
                LevelScale::DbSpl => self.spectrum_spl,
            }),
            PaneKind::Distortion => Some(self.distortion),
            PaneKind::Spl => None,
        }
    }

    fn range_mut(&mut self, kind: PaneKind, scale: LevelScale) -> Option<&mut Range> {
        match kind {
            PaneKind::Transfer => Some(&mut self.transfer),
            PaneKind::Spectrum => Some(match scale {
                LevelScale::Dbfs => &mut self.spectrum_dbfs,
                LevelScale::DbSpl => &mut self.spectrum_spl,
            }),
            PaneKind::Distortion => Some(&mut self.distortion),
            PaneKind::Spl => None,
        }
    }
}

impl Default for PaneLevels {
    fn default() -> Self {
        Self::of(&ViewState::default())
    }
}

/// One of a pane's level ranges ([`PaneLevels`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LevelSlot {
    Transfer,
    SpectrumDbfs,
    SpectrumSpl,
    Distortion,
}

impl LevelSlot {
    const ALL: [LevelSlot; 4] = [
        LevelSlot::Transfer,
        LevelSlot::SpectrumDbfs,
        LevelSlot::SpectrumSpl,
        LevelSlot::Distortion,
    ];

    /// The range a pane of `kind` draws on in `scale`; the SPL pane has none.
    fn of(kind: PaneKind, scale: LevelScale) -> Option<Self> {
        match kind {
            PaneKind::Transfer => Some(Self::Transfer),
            PaneKind::Spectrum => Some(match scale {
                LevelScale::Dbfs => Self::SpectrumDbfs,
                LevelScale::DbSpl => Self::SpectrumSpl,
            }),
            PaneKind::Distortion => Some(Self::Distortion),
            PaneKind::Spl => None,
        }
    }

    fn get(self, l: &PaneLevels) -> Range {
        match self {
            Self::Transfer => l.transfer,
            Self::SpectrumDbfs => l.spectrum_dbfs,
            Self::SpectrumSpl => l.spectrum_spl,
            Self::Distortion => l.distortion,
        }
    }

    fn set(self, l: &mut PaneLevels, r: Range) {
        match self {
            Self::Transfer => l.transfer = r,
            Self::SpectrumDbfs => l.spectrum_dbfs = r,
            Self::SpectrumSpl => l.spectrum_spl = r,
            Self::Distortion => l.distortion = r,
        }
    }

    fn wire(self, l: &mut LayoutLevels) -> &mut Option<LevelRange> {
        match self {
            Self::Transfer => &mut l.transfer,
            Self::SpectrumDbfs => &mut l.spectrum_dbfs,
            Self::SpectrumSpl => &mut l.spectrum_spl,
            Self::Distortion => &mut l.distortion,
        }
    }
}

/// A spectrum pane's level axis was framed automatically, or set by the operator
/// (`operator`), while the pane showed `shown`: it is left as it is until the pane shows
/// something else.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SpectrumHold {
    shown: SpectrumShown,
    operator: bool,
}

/// What a spectrum pane's level axis is framed for: the measurement it shows, by name (the
/// identity a pane keeps across daemons, so a reconnect or a restarted daemon is no new
/// measurement), and the scale its curves are in (each scale has a range of its own).
pub(super) type SpectrumShown = (Option<String>, LevelScale);

impl AppState {
    /// Pane `pane`'s level ranges (a pane never zoomed: the defaults).
    pub fn pane_levels(&self, pane: PaneId) -> PaneLevels {
        self.pane_levels.get(&pane).copied().unwrap_or_default()
    }

    /// The level range pane `pane` draws on now, if its kind has a level axis.
    pub fn level_range(&self, pane: PaneId) -> Option<Range> {
        let scale = crate::scenes::spectrum_scale(self, pane);
        self.pane_levels(pane).range(self.layout.kind(pane), scale)
    }

    /// Sets the range pane `pane` draws on now to what `f` makes of it.
    pub(super) fn change_level(&mut self, pane: PaneId, f: impl FnOnce(Range) -> Range) {
        let kind = self.layout.kind(pane);
        let scale = crate::scenes::spectrum_scale(self, pane);
        let levels = self.pane_levels.entry(pane).or_default();
        if let Some(r) = levels.range_mut(kind, scale) {
            *r = f(*r);
        }
    }

    /// A pane split off `from` starts on its ranges: both halves show what it did.
    pub(super) fn copy_levels(&mut self, from: PaneId, to: PaneId) {
        if let Some(l) = self.pane_levels.get(&from).copied() {
            self.pane_levels.insert(to, l);
        }
        if let Some(h) = self.spectrum_hold.get(&from).cloned() {
            self.spectrum_hold.insert(to, h);
        }
        for s in LevelSlot::ALL {
            if self.level_set.contains(&(from, s)) {
                self.level_set.insert((to, s));
            }
        }
    }

    /// Ranges of panes no longer in the layout go.
    pub(super) fn prune_levels(&mut self) {
        let views = &self.layout.views;
        self.pane_levels.retain(|id, _| views.contains_key(id));
        self.spectrum_hold.retain(|id, _| views.contains_key(id));
        self.level_set.retain(|(id, _)| views.contains_key(id));
    }

    /// The ranges the operator set on pane `pane`, as the panes are kept.
    pub fn operator_levels(&self, pane: PaneId) -> LayoutLevels {
        let l = self.pane_levels(pane);
        let mut w = LayoutLevels::default();
        for s in LevelSlot::ALL {
            if self.level_set.contains(&(pane, s)) {
                let r = s.get(&l);
                *s.wire(&mut w) = Some(LevelRange { lo: r.lo, hi: r.hi });
            }
        }
        w
    }

    /// Pane `pane` (just on screen) takes the ranges the operator set on it when its panes
    /// were kept; the others are framed when it first shows a curve.
    pub(super) fn set_operator_levels(&mut self, pane: PaneId, w: &LayoutLevels) {
        let mut w = *w;
        let mut l = self.pane_levels(pane);
        for s in LevelSlot::ALL {
            if let Some(r) = *s.wire(&mut w) {
                s.set(&mut l, Range::new(r.lo, r.hi));
                self.level_set.insert((pane, s));
            } else {
                self.level_set.remove(&(pane, s));
            }
        }
        if l == PaneLevels::default() {
            self.pane_levels.remove(&pane);
        } else {
            self.pane_levels.insert(pane, l);
        }
    }

    /// What spectrum pane `pane`'s level axis frames now.
    fn spectrum_shown(&self, pane: PaneId) -> SpectrumShown {
        (
            self.pane_meas(pane).map(|m| m.config.name.clone()),
            crate::scenes::spectrum_scale(self, pane),
        )
    }

    /// The operator set pane `pane`'s level axis: it is kept with the panes, and a spectrum
    /// pane keeps it until it shows another measurement.
    fn hold_level(&mut self, pane: PaneId) {
        let scale = crate::scenes::spectrum_scale(self, pane);
        if let Some(s) = LevelSlot::of(self.layout.kind(pane), scale) {
            self.level_set.insert((pane, s));
        }
        if self.layout.kind(pane) != PaneKind::Spectrum {
            return;
        }
        let shown = self.spectrum_shown(pane);
        self.spectrum_hold.insert(
            pane,
            SpectrumHold {
                shown,
                operator: true,
            },
        );
    }

    /// Every pane's level axis is back at its default by the operator's hand: kept so.
    pub(super) fn hold_all_levels(&mut self) {
        for pane in self.layout.root.leaves() {
            self.hold_level(pane);
        }
    }

    // ----- reducer -----------------------------------------------------------------------

    /// The pane the level keys act on (the focused one), or why not.
    fn level_target(&mut self) -> Option<PaneId> {
        let f = self.layout.focused();
        // The level axis is the magnitude's: the transfer pane's phase, coherence and IR
        // views do not draw it.
        let has = match f.kind {
            PaneKind::Distortion => f.modes.sweep == ac2_scene::view::SweepMode::Response,
            PaneKind::Transfer => f.modes.transfer == super::TransferView::Magnitude,
            PaneKind::Spectrum => true,
            PaneKind::Spl => false,
        };
        if !has {
            let what = match f.kind {
                PaneKind::Transfer => format!("{} ({})", f.title(), f.modes.transfer.label()),
                _ => f.title().to_owned(),
            };
            self.warn(format!("{what} has no level axis to zoom"));
            return None;
        }
        Some(self.layout.focus)
    }

    /// Zoom pane `pane`'s level axis by `factor` (> 1 in) about `about` (dB; `None`: the
    /// middle).
    pub(super) fn level_zoom(&mut self, pane: PaneId, about: Option<f64>, factor: f64) {
        self.hold_level(pane);
        self.change_level(pane, |r| {
            level::zoom(r, about.unwrap_or((r.lo + r.hi) / 2.0), factor)
        });
    }

    /// Pan pane `pane`'s level axis by `db`.
    pub(super) fn level_pan(&mut self, pane: PaneId, db: f64) {
        self.hold_level(pane);
        self.change_level(pane, |r| level::pan(r, db));
    }

    pub(super) fn level_key(&mut self, c: crate::keys::CommandId) {
        use crate::keys::CommandId as C;
        let Some(p) = self.level_target() else {
            return;
        };
        let Some(r) = self.level_range(p) else {
            return;
        };
        let kind = self.layout.kind(p);
        match c {
            C::LevelZoomIn => self.level_zoom(p, None, LEVEL_ZOOM_FACTOR),
            C::LevelZoomOut => self.level_zoom(p, None, 1.0 / LEVEL_ZOOM_FACTOR),
            C::LevelPanUp => self.level_pan(p, level::pan_step(r)),
            C::LevelPanDown => self.level_pan(p, -level::pan_step(r)),
            C::LevelReset => {
                let scale = crate::scenes::spectrum_scale(self, p);
                if let Some(d) = PaneLevels::default().range(kind, scale) {
                    self.hold_level(p);
                    self.change_level(p, |_| d);
                }
            }
            C::LevelFit => match level::fit(self.level_values(p)) {
                Some(fit) => {
                    self.hold_level(p);
                    self.change_level(p, |_| fit);
                    self.toast(format!(
                        "{}: level {} … {} dB",
                        kind.title(),
                        format::fixed(fit.lo, 0),
                        format::fixed(fit.hi, 0)
                    ));
                }
                None => self.warn(format!("{}: no curve shown to fit", kind.title())),
            },
            _ => {}
        }
    }

    /// Every pane of `kind` frames what it shows on its level axis, as Shift+Home does.
    fn fit_panes(&mut self, kind: PaneKind) {
        for pane in self.layout.root.leaves() {
            if self.layout.kind(pane) != kind {
                continue;
            }
            if let Some(fit) = level::fit(self.level_values(pane)) {
                // Framed for the new curves: no longer a range the operator set.
                let scale = crate::scenes::spectrum_scale(self, pane);
                if let Some(s) = LevelSlot::of(kind, scale) {
                    self.level_set.remove(&(pane, s));
                }
                self.change_level(pane, |_| fit);
            }
        }
    }

    /// A frame is in: each spectrum pane showing something it has not framed yet frames it
    /// on its level axis, as Shift+Home does (frequency left as it is: the axis is shared,
    /// and another pane's measurement would lose its zoom). A level range kept from another
    /// measurement or the defaults rarely suits this one, so each first show begins framed;
    /// a range the operator set stays until the pane shows another measurement.
    pub(super) fn fit_first_shows(&mut self) {
        self.rearm_started_spectra();
        for pane in self.layout.root.leaves() {
            // A pane waiting for its remembered measurement shows a stand-in.
            if self.layout.kind(pane) != PaneKind::Spectrum
                || self.pending.pane_meas.contains_key(&pane)
            {
                continue;
            }
            let shown = self.spectrum_shown(pane);
            let hold = self.spectrum_hold.get(&pane);
            if hold.is_some_and(|h| h.shown == shown) {
                continue;
            }
            let slot = LevelSlot::of(PaneKind::Spectrum, shown.1);
            let set = slot.is_some_and(|s| self.level_set.contains(&(pane, s)));
            // A pane just put on screen with a range the operator set (the panes kept with a
            // measurement, or in ui.toml) shows its first curve on it: that range is the
            // operator's, framed for this curve already.
            if hold.is_none() && set {
                self.spectrum_hold.insert(
                    pane,
                    SpectrumHold {
                        shown,
                        operator: true,
                    },
                );
                continue;
            }
            let Some(fit) = level::fit(self.level_values(pane)) else {
                continue;
            };
            if let Some(s) = slot {
                self.level_set.remove(&(pane, s));
            }
            self.change_level(pane, |_| fit);
            self.spectrum_hold.insert(
                pane,
                SpectrumHold {
                    shown,
                    operator: false,
                },
            );
        }
    }

    /// A started spectrum / RTA measurement's first new frame is in: panes framed
    /// automatically on its last run frame it again. A new run is often a new signal
    /// (another source, another gain); a range the operator set stays.
    fn rearm_started_spectra(&mut self) {
        let arrived: Vec<_> = self
            .spectrum_fit
            .iter()
            .filter(|(id, shown)| {
                self.spectrum_frame(**id).is_some_and(|f| {
                    shown
                        .as_ref()
                        .is_none_or(|old| !std::sync::Arc::ptr_eq(old, &f.frame))
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in arrived {
            self.spectrum_fit.remove(&id);
            let Some(name) = self.meas(id).map(|m| m.config.name.clone()) else {
                continue;
            };
            self.spectrum_hold
                .retain(|_, h| h.operator || h.shown.0.as_ref() != Some(&name));
        }
    }

    /// A finished sweep's data is in: the sweep panes' level axes frame its harmonics and
    /// THD, as Shift+Home does (frequency left as it is). Distortion levels differ by tens
    /// of dB from one device or drive level to the next, so a range kept from the last
    /// result often shows the new one off the plot. Only once per result: a zoom after it
    /// stays until the next sweep.
    pub(super) fn fit_new_sweep(&mut self) {
        let Some(id) = self.sweep.fit else {
            return;
        };
        if !self.traces.contains_key(&id) {
            return;
        }
        self.sweep.fit = None;
        let shown = self.layout.root.leaves().into_iter().any(|p| {
            self.layout.kind(p) == PaneKind::Distortion
                && self.pane_sweep(p).map(|(t, _)| t.meta.id) == Some(id)
        });
        if !shown {
            return;
        }
        self.fit_panes(PaneKind::Distortion);
    }

    /// Every level pane `pane` draws in the shown frequency range, as drawn (offsets
    /// included).
    fn level_values(&self, pane: PaneId) -> Vec<f64> {
        let (lo, hi) = (self.view.freq.lo, self.view.freq.hi);
        let mut v = Vec::new();
        let mut add = |freqs: &[f64], vals: &[f32], offset: f64| {
            v.extend(
                freqs
                    .iter()
                    .zip(vals)
                    .filter(|(f, _)| **f >= lo && **f <= hi)
                    .map(|(_, x)| f64::from(*x) + offset),
            );
        };
        let live = |stream: Stream, m: &Measurement| {
            let d = self.data.as_ref()?;
            let f = d.latest.get(&Topic::Data { meas: m.id, stream })?;
            let g = d.grids.get(&f.frame.stamp.grid_id?)?;
            Some((f.frame.clone(), g.centres()))
        };
        let shown = self.pane_meas(pane);
        match self.layout.kind(pane) {
            kind @ (PaneKind::Transfer | PaneKind::Spectrum) => {
                for m in self.measurements().into_iter().filter(|m| {
                    self.live_on_pane(kind, m, shown) || self.live_compared_on(kind, m, shown)
                }) {
                    let Some(stream) = m.config.kind.stream() else {
                        continue;
                    };
                    if let Some((f, freqs)) = live(stream, m) {
                        let level = match &f.data {
                            ac2_proto::FrameData::Tf(tf) => &tf.mag,
                            ac2_proto::FrameData::Spec(s) => &s.level,
                            ac2_proto::FrameData::Rta(r) => &r.level,
                            _ => continue,
                        };
                        add(&freqs, level, self.meas_offset(m.id));
                    }
                }
                for (t, g) in self.traces.values() {
                    if self.on_pane(kind, &t.meta, shown)
                        || self.trace_compared_on(kind, &t.meta, shown)
                    {
                        add(&g.centres(), &t.mag_db, t.meta.edit.offset.0);
                    }
                }
            }
            PaneKind::Distortion => {
                let runs = self
                    .pane_sweep(pane)
                    .into_iter()
                    .chain(self.compared_sweeps(pane));
                for (t, g) in runs {
                    let Some(s) = &t.sweep else {
                        continue;
                    };
                    let freqs = g.centres();
                    for c in s.harmonics.iter().map(|h| &h.curve).chain([&s.thd]) {
                        add(&freqs, &c.level_db, 0.0);
                        add(&freqs, &c.floor_db, 0.0);
                    }
                }
            }
            PaneKind::Spl => {}
        }
        v
    }
}
