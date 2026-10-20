//! The focused pane exported as PNG on this computer: the prompt with a dated name in
//! `~/ac2-exports/`, and the path typed turned into the export the app renders (the GPU is
//! the window's, `crate::pane_export`).

use super::leq_export::{EXPORT_DIR, export_path, file_name_part};
use super::*;
use ac2_proto::units::WallNs;
use std::path::PathBuf;

/// A pane to write as PNG: the app takes it after the reducer, renders the pane's scene as
/// last drawn and answers with a toast.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneExport {
    pub pane: PaneId,
    pub path: PathBuf,
}

/// The suggested file name of a pane showing `meas` in view `view`, exported at `at` with
/// the local clock `offset_s` ahead of UTC: `FOH-Transfer-20261020-142301.png`. A pane
/// without a measurement is named by its view alone.
pub fn pane_export_name(meas: Option<&str>, view: &str, at: WallNs, offset_s: i32) -> String {
    let secs = (at.0 / 1_000_000_000) as i64 + i64::from(offset_s);
    let when = chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_default();
    let view = file_name_part(view);
    match meas.map(file_name_part).filter(|m| !m.is_empty()) {
        Some(m) => format!("{m}-{view}-{when}.png"),
        None => format!("{view}-{when}.png"),
    }
}

impl AppState {
    /// The export prompt for the focused pane.
    pub(super) fn ask_pane_export(&mut self) {
        let id = self.layout.focus;
        let Some(view) = self.layout.view(id) else {
            return;
        };
        let title = view.title();
        let now = super::sessions_impl::wall_now();
        let meas = self.pane_meas(id).map(|m| m.config.name.clone());
        let name = pane_export_name(meas.as_deref(), title, now, self.local_zone.offset_s(now));
        self.prompt(PromptKind::PaneExport(id), format!("{EXPORT_DIR}/{name}"));
    }

    /// The export prompt's path entered: the export for the app to render, or why not. A
    /// name typed without an extension gets `.png`.
    pub(super) fn pane_export_entered(&mut self, pane: PaneId, text: &str) -> Result<(), String> {
        let mut path = export_path(text, std::env::home_dir().as_deref())?;
        if self.layout.view(pane).is_none() {
            return Err("that pane is closed".into());
        }
        if path.extension().is_none() {
            path.set_extension("png");
        }
        self.pane_export = Some(PaneExport { pane, path });
        Ok(())
    }
}
