//! An SPL meter's Leq log exported as CSV on this computer: the prompt with a dated name in
//! `~/ac2-exports/`, and the path typed turned into the export request (the rows are read
//! off the UI thread, `crate::conn`).

use crate::state::*;
use ac2_proto::model::SplLogWhich;
use ac2_proto::units::WallNs;
use std::path::{Path, PathBuf};

/// The folder the export prompt suggests, under the home directory.
pub const EXPORT_DIR: &str = "~/ac2-exports";

/// The suggested file name of meter `meter`'s log exported at `at` with the local clock
/// `offset_s` ahead of UTC: `FOH-SPL-leq-2026-10-17-0012.csv`. What a file name cannot hold
/// (spaces, separators) becomes `-`.
pub fn leq_export_name(meter: &str, at: WallNs, offset_s: i32) -> String {
    let name = file_name_part(meter);
    let secs = (at.0 / 1_000_000_000) as i64 + i64::from(offset_s);
    let when = chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%Y-%m-%d-%H%M").to_string())
        .unwrap_or_default();
    format!("{name}-leq-{when}.csv")
}

/// `s` as part of a file name: what a file name cannot hold (spaces, separators) becomes `-`.
pub(in crate::state) fn file_name_part(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `text` typed as a path: `~/` is the home directory `home`, and so is the base of a
/// relative path.
pub fn export_path(text: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("type a file path".into());
    }
    let rest = t.strip_prefix("~/").or_else(|| (t == "~").then_some(""));
    let p = match (rest, home) {
        (Some(r), Some(h)) => h.join(r),
        (Some(_), None) => return Err("no home directory to put ~ in".into()),
        (None, _) => PathBuf::from(t),
    };
    match home {
        Some(h) if p.is_relative() => Ok(h.join(p)),
        _ => Ok(p),
    }
}

impl AppState {
    /// The export prompt for the focused (or only) SPL meter's current or previous log.
    pub(in crate::state) fn ask_leq_export(&mut self, previous: bool) {
        let Some(m) = self.kind_meas(PaneKind::Spl).cloned() else {
            self.warn(self.no_spl_meter());
            return;
        };
        let now = crate::state::sessions_impl::wall_now();
        let name = leq_export_name(&m.config.name, now, self.local_zone.offset_s(now));
        self.prompt(
            PromptKind::LeqExport {
                meas: m.id,
                previous,
            },
            format!("{EXPORT_DIR}/{name}"),
        );
    }

    /// The export prompt's path entered: the request, or why not.
    pub(in crate::state) fn leq_export_entered(
        &self,
        meas: MeasId,
        previous: bool,
        text: &str,
        out: &mut Vec<Request>,
    ) -> Result<(), String> {
        let path = export_path(text, std::env::home_dir().as_deref())?;
        let st = self.daemon().ok_or("not connected")?;
        let info = st
            .measurements
            .iter()
            .find(|m| m.id == meas)
            .and_then(|m| ac2_client::spl_log::log_info(st, m))
            .ok_or("that SPL meter is gone")?;
        out.push(Request::ExportLeqLog {
            info,
            log: if previous {
                SplLogWhich::Previous
            } else {
                SplLogWhich::Current
            },
            path,
        });
        Ok(())
    }
}
