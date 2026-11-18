//! The reducer's side of Settings › Display: its keys, what ←/→ change on each line, and
//! the toggles its palette commands share with the page.

use ac2_scene::view::{CoherencePlacement, DistortionUnit, LeqStyle};
use eframe::egui::Key;

use super::{AppState, Overlay, PaneKind};
use crate::keys::Chord;
use crate::settings::{DisplayRow, step_hold, step_theme};

impl AppState {
    pub(super) fn display_key(&mut self, chord: Chord) {
        let Overlay::Settings(s) = &mut self.overlay else {
            return;
        };
        let row = s.display;
        match chord.key {
            Key::ArrowUp => s.display_move(-1),
            Key::Tab if chord.shift => s.display_move(-1),
            Key::ArrowDown | Key::Tab => s.display_move(1),
            Key::ArrowLeft => self.display_change(row, -1),
            Key::ArrowRight | Key::Space => self.display_change(row, 1),
            Key::Enter => self.display_change(row, 0),
            _ => {}
        }
    }

    /// ←/→ (`d`) or Enter (`0`) on a line of the Display page: changes it at once.
    pub(super) fn display_change(&mut self, row: DisplayRow, d: i32) {
        if let Overlay::Settings(s) = &mut self.overlay {
            s.display = row;
        }
        match row {
            DisplayRow::Theme if d != 0 => {
                self.theme = step_theme(self.theme, d);
                self.prefs.theme = Some(self.theme);
                self.prefs_dirty = true;
            }
            DisplayRow::KeyHints if d != 0 => {
                self.prefs.key_hints = !self.prefs.key_hints;
                self.prefs_dirty = true;
            }
            DisplayRow::WarningToasts if d != 0 => self.toggle_warning_toasts(),
            DisplayRow::SplHold if d != 0 => {
                self.prefs.spl_hold_ms = step_hold(self.prefs.spl_hold_ms, d);
                self.prefs_dirty = true;
            }
            DisplayRow::EmberDecay if d != 0 => {
                self.prefs.ember_decay_s =
                    crate::settings::step_ember_decay(self.prefs.ember_decay_s, d);
                self.prefs_dirty = true;
            }
            DisplayRow::EmberGlow if d != 0 => {
                self.prefs.ember_glow = crate::settings::step_ember_glow(self.prefs.ember_glow, d);
                self.prefs_dirty = true;
            }
            // The G keys of the pane of that kind worked in last, so the spectrograph's
            // history comes and goes as it does there; the focus stays where it was.
            DisplayRow::SpectrumView | DisplayRow::SweepView if d != 0 => {
                let kind = if row == DisplayRow::SpectrumView {
                    PaneKind::Spectrum
                } else {
                    PaneKind::Distortion
                };
                let Some(pane) = self.lead(kind) else {
                    self.warn(format!(
                        "no {} pane: a {} measurement selected in the list shows one in the \
                         focused pane",
                        kind.title(),
                        kind.what()
                    ));
                    return;
                };
                let focus = self.layout.focus;
                self.layout.set_focus(pane);
                self.step_view(d > 0);
                self.layout.set_focus(focus);
            }
            // The palette's commands without focusing a pane: the page stays in front.
            DisplayRow::LeqStyle if d != 0 => self.toggle_leq_layout(false),
            DisplayRow::LeqHistory if d != 0 => self.toggle_leq_layout(true),
            DisplayRow::LeqBars if d != 0 => self.toggle_bar_ease(),
            DisplayRow::LeqScale if d != 0 => self.toggle_shared_scale(),
            DisplayRow::LevelFollow if d != 0 => self.toggle_level_follow(),
            DisplayRow::SpectrumGlide if d != 0 => self.toggle_spec_ease(),
            DisplayRow::Quality3d if d != 0 => self.step_quality_3d(d),
            DisplayRow::InvertDrag3d if d != 0 => self.toggle_invert_drag_3d(),
            DisplayRow::DistortionUnit if d != 0 => self.toggle_distortion_unit(),
            DisplayRow::Coherence if d != 0 => self.toggle_coherence_placement(),
            DisplayRow::ResolutionMarker if d != 0 => self.toggle_resolution_marker(),
            DisplayRow::RangeBand if d != 0 => self.toggle_range_band(),
            DisplayRow::SimHints if d != 0 => self.toggle_sim_hints(),
            DisplayRow::PlateBackground if d != 0 => self.toggle_plate_background(),
            DisplayRow::CursorJump if d != 0 => {
                let db = ac2_scene::cursor_jump::step_prominence(self.view.jump_prominence_db, d);
                self.view.jump_prominence_db = db;
                self.prefs.jump_prominence_db = db;
                self.prefs_dirty = true;
            }
            DisplayRow::Advice
            | DisplayRow::AdviceRefHeadroom
            | DisplayRow::AdviceLowCoherence
            | DisplayRow::AdviceLowLevel
                if d != 0 =>
            {
                self.step_advice(row, d);
            }
            DisplayRow::LevelAxes if d == 0 => {
                self.level_axes.pane_levels.clear();
                self.level_axes.level_set.clear();
                self.hold_all_levels();
                crate::prefs::IrLevelPrefs::default().apply(&mut self.view);
                self.toast("every pane's level axis is back at its default");
            }
            _ => {}
        }
    }
}

impl AppState {
    /// The Display setting and its palette command. Its own toast is information, so it
    /// shows either way.
    pub(super) fn toggle_warning_toasts(&mut self) {
        self.prefs.warning_toasts = !self.prefs.warning_toasts;
        self.prefs_dirty = true;
        self.toast(format!(
            "warning toasts {}",
            crate::settings::warning_toasts_text(self.prefs.warning_toasts)
        ));
    }

    /// The Display page's spectrum glide line; remembered in the prefs.
    pub(super) fn toggle_spec_ease(&mut self) {
        self.prefs.spec_ease = !self.prefs.spec_ease;
        self.prefs_dirty = true;
        self.toast(format!(
            "Spectrum {}",
            crate::settings::spec_ease_text(self.prefs.spec_ease)
        ));
    }

    /// The Display page's Leq bars line; remembered in the prefs.
    pub(super) fn toggle_bar_ease(&mut self) {
        self.prefs.bar_ease = !self.prefs.bar_ease;
        self.prefs_dirty = true;
        self.toast(format!(
            "Leq bars {}",
            crate::settings::bar_ease_text(self.prefs.bar_ease)
        ));
    }

    /// The Display page's shared scale line; remembered in the prefs.
    pub(super) fn toggle_shared_scale(&mut self) {
        self.prefs.shared_scale = !self.prefs.shared_scale;
        self.prefs_dirty = true;
        self.toast(format!(
            "Shared scale: {}",
            crate::settings::shared_scale_text(self.prefs.shared_scale)
        ));
    }

    /// The Display page's level follow line; remembered in the prefs.
    pub(super) fn toggle_level_follow(&mut self) {
        self.prefs.level_follow = !self.prefs.level_follow;
        self.prefs_dirty = true;
        self.toast(format!(
            "Level axis follows {}",
            crate::settings::level_follow_text(self.prefs.level_follow)
        ));
    }

    /// The Display page's 3D vertical drag line; remembered in the prefs.
    pub(super) fn toggle_invert_drag_3d(&mut self) {
        self.prefs.invert_drag_3d = !self.prefs.invert_drag_3d;
        self.prefs_dirty = true;
        self.toast(format!(
            "Invert vertical drag {}",
            crate::settings::invert_drag_3d_text(self.prefs.invert_drag_3d)
        ));
    }

    /// The Display page's Resolution marker line; remembered in the prefs.
    pub(super) fn toggle_resolution_marker(&mut self) {
        self.view.unresolved = !self.view.unresolved;
        self.prefs.resolution_marker = self.view.unresolved;
        self.prefs_dirty = true;
        self.toast(format!(
            "resolution marker {}",
            crate::settings::resolution_marker_text(self.view.unresolved)
        ));
    }

    /// The Display page's Range across positions line; remembered in the prefs.
    pub(super) fn toggle_range_band(&mut self) {
        self.view.range_band = !self.view.range_band;
        self.prefs.range_band = self.view.range_band;
        self.prefs_dirty = true;
        self.toast(format!(
            "range across positions {}",
            crate::settings::range_band_text(self.view.range_band)
        ));
    }

    /// The Display page's Plate background line, for every plate of every pane; remembered
    /// in the prefs.
    /// The Display page's 3D quality: auto → full → lite → bare; remembered in the prefs.
    /// On a GPU without the 3D view the setting stays as it is: it says so instead.
    pub(super) fn step_quality_3d(&mut self, d: i32) {
        if !self.three_d() {
            self.warn(format!(
                "3D quality: {}",
                crate::settings::QUALITY_3D_UNAVAILABLE
            ));
            return;
        }
        self.prefs.quality_3d = self.prefs.quality_3d.step(d.signum());
        self.prefs_dirty = true;
        self.toast(format!(
            "3D quality {}",
            crate::settings::quality_3d_text(self.prefs.quality_3d, self.auto_tier)
        ));
    }

    pub(super) fn toggle_plate_background(&mut self) {
        self.prefs.plate_background = self.prefs.plate_background.toggled();
        self.prefs_dirty = true;
        self.toast(format!(
            "plate background {}",
            crate::settings::plate_background_text(self.prefs.plate_background)
        ));
    }

    /// The Display page's headroom advice lines; remembered in the prefs. A new threshold
    /// starts every measurement's advice afresh: what was shown was judged on the old one.
    pub(super) fn step_advice(&mut self, row: DisplayRow, d: i32) {
        use ac2_scene::advice::{
            LOW_COHERENCE, LOW_LEVEL_DBFS, REF_HEADROOM_DB, low_coherence_text, low_level_text,
            on_text, ref_headroom_text, step_choice,
        };
        let a = &mut self.prefs.advice;
        let said = match row {
            DisplayRow::Advice => {
                a.on = !a.on;
                format!("headroom advice {}", on_text(a.on))
            }
            DisplayRow::AdviceRefHeadroom => {
                a.ref_headroom_db = step_choice(&REF_HEADROOM_DB, a.ref_headroom_db, d);
                format!(
                    "advise lowering at {}",
                    ref_headroom_text(a.ref_headroom_db)
                )
            }
            DisplayRow::AdviceLowCoherence => {
                a.low_coherence = step_choice(&LOW_COHERENCE, a.low_coherence, d);
                format!("advise raising at {}", low_coherence_text(a.low_coherence))
            }
            DisplayRow::AdviceLowLevel => {
                a.low_level_dbfs = step_choice(&LOW_LEVEL_DBFS, a.low_level_dbfs, d);
                format!(
                    "advise raising with the {}",
                    low_level_text(a.low_level_dbfs)
                )
            }
            _ => return,
        };
        self.rig.folded.advice.clear();
        self.prefs_dirty = true;
        self.toast(said);
    }

    /// Columns ↔ tiles, or the history strip on / off; remembered in the prefs.
    pub(super) fn toggle_leq_layout(&mut self, history: bool) {
        let l = &mut self.view.spl.layout;
        if history {
            l.history = !l.history;
        } else {
            l.style = match l.style {
                LeqStyle::Columns => LeqStyle::Tiles,
                LeqStyle::Tiles => LeqStyle::Columns,
            };
        }
        self.prefs.leq = self.view.spl.layout;
        self.prefs_dirty = true;
    }

    pub(super) fn toggle_distortion_unit(&mut self) {
        let u = &mut self.view.distortion.unit;
        *u = match *u {
            DistortionUnit::Db => DistortionUnit::Percent,
            DistortionUnit::Percent => DistortionUnit::Db,
        };
    }

    pub(super) fn toggle_coherence_placement(&mut self) {
        let c = &mut self.view.tf.coherence_placement;
        *c = match *c {
            CoherencePlacement::Pane => CoherencePlacement::OverlayOnMagnitude,
            CoherencePlacement::OverlayOnMagnitude => CoherencePlacement::Pane,
        };
    }
}
