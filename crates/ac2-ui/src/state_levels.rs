//! The level (vertical) axis of each pane: its own range, zoomed, panned, reset and fitted
//! by the keys and the wheel on that pane alone, so two panes of one kind (two transfer
//! measurements side by side) keep scales of their own. The frequency axis stays shared:
//! panes read against each other at the same frequencies.

use ac2_proto::model::{LevelScale, Measurement, TraceKind};
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::axis::Range;
use ac2_scene::format;
use ac2_scene::grid::column_frequencies;
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

impl AppState {
    /// Pane `pane`'s level ranges (a pane never zoomed: the defaults).
    pub fn pane_levels(&self, pane: PaneId) -> PaneLevels {
        self.pane_levels.get(&pane).copied().unwrap_or_default()
    }

    /// The level range pane `pane` draws on now, if its kind has a level axis.
    pub fn level_range(&self, pane: PaneId) -> Option<Range> {
        let scale = crate::scenes::spectrum_scale(self);
        self.pane_levels(pane).range(self.layout.kind(pane), scale)
    }

    /// Sets the range pane `pane` draws on now to what `f` makes of it.
    fn change_level(&mut self, pane: PaneId, f: impl FnOnce(Range) -> Range) {
        let kind = self.layout.kind(pane);
        let scale = crate::scenes::spectrum_scale(self);
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
    }

    /// Ranges of panes no longer in the layout go.
    pub(super) fn prune_levels(&mut self) {
        let views = &self.layout.views;
        self.pane_levels.retain(|id, _| views.contains_key(id));
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
        self.change_level(pane, |r| {
            level::zoom(r, about.unwrap_or((r.lo + r.hi) / 2.0), factor)
        });
    }

    /// Pan pane `pane`'s level axis by `db`.
    pub(super) fn level_pan(&mut self, pane: PaneId, db: f64) {
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
                let scale = crate::scenes::spectrum_scale(self);
                if let Some(d) = PaneLevels::default().range(kind, scale) {
                    self.change_level(p, |_| d);
                }
            }
            C::LevelFit => match level::fit(self.level_values(p)) {
                Some(fit) => {
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
                self.change_level(pane, |_| fit);
            }
        }
    }

    /// A started spectrum / RTA measurement's first frame is in: the spectrum panes' level
    /// axes frame what they show, as Shift+Home does (frequency left as it is). A level
    /// range that suits one signal rarely suits the next, so each start begins framed.
    pub(super) fn fit_started_spectra(&mut self) {
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
        if arrived.is_empty() {
            return;
        }
        for id in &arrived {
            self.spectrum_fit.remove(id);
        }
        self.fit_panes(PaneKind::Spectrum);
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
        if self.shown_sweep().map(|(t, _)| t.meta.id) != Some(id) {
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
            Some((f.frame.clone(), column_frequencies(g)))
        };
        let shown = self.pane_meas(pane);
        match self.layout.kind(pane) {
            PaneKind::Transfer => {
                for m in self.measurements().into_iter().filter(|m| {
                    self.live_on_transfer_pane(m, shown) || self.live_compared_on_transfer(m, shown)
                }) {
                    if let Some((f, freqs)) = live(Stream::Tf, m)
                        && let ac2_proto::FrameData::Tf(tf) = &f.data
                    {
                        add(&freqs, &tf.mag, self.edit(m.id).offset_db);
                    }
                }
                for (t, g) in self.traces.values() {
                    if self.on_transfer_pane(&t.meta, shown)
                        || self.trace_compared_on_transfer(&t.meta, shown)
                    {
                        add(&column_frequencies(g), &t.mag_db, t.meta.edit.offset.0);
                    }
                }
            }
            PaneKind::Spectrum => {
                for m in self
                    .measurements()
                    .into_iter()
                    .filter(|m| !self.meas_hidden(m))
                {
                    if !m.config.kind.publishes_levels() {
                        continue;
                    }
                    let Some(stream) = m.config.kind.stream() else {
                        continue;
                    };
                    if let Some((f, freqs)) = live(stream, m) {
                        let level = match &f.data {
                            ac2_proto::FrameData::Spec(s) => &s.level,
                            ac2_proto::FrameData::Rta(r) => &r.level,
                            _ => continue,
                        };
                        add(&freqs, level, self.edit(m.id).offset_db);
                    }
                }
                for (t, g) in self.traces.values() {
                    if t.meta.edit.visible
                        && matches!(
                            t.meta.kind,
                            TraceKind::Spectrum { .. } | TraceKind::Rta { .. }
                        )
                    {
                        add(&column_frequencies(g), &t.mag_db, t.meta.edit.offset.0);
                    }
                }
            }
            PaneKind::Distortion => {
                if let Some((t, g)) = self.shown_sweep()
                    && let Some(s) = &t.sweep
                {
                    let freqs = column_frequencies(g);
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
