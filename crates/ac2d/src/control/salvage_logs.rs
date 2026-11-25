//! `file.salvage`'s SPL logs (`docs/design/protocol-next.md` §7): each `spl/*.csv` of the
//! log versions this build reads, with its band log (`*.bands.csv`) and high-water mark
//! beside it, becomes a stopped SPL meter of session scope holding the log as its current
//! log. The session's manifest is of another version and not read, so a log is paired with
//! its band log by file name, and the meter takes its name and input from the log's header.
//! A log of another version is refused and stays on disk as plain CSV.

use std::path::Path;

use ac2_proto::model::{
    LeqConfig, MeasConfig, MeasKind, MeasScope, PeakWeighting, SalvageRefusal, SplConfig,
    TimeWeighting, Weighting,
};
use ac2_proto::spl_log;
use ac2_proto::units::MeasId;
use ac2_traces::band_log;
use ac2_traces::log_mark::{self, MarkRead};
use ac2_traces::session::{SPL_DIR, SavedSplLog};

use super::Control;
use crate::leq_log::LeqLog;

/// An SPL log read for salvage: its file, and the log with its band log and mark, or why
/// it was not read.
pub(crate) struct SalvagedLog {
    pub(crate) file: String,
    pub(crate) log: Result<SavedSplLog, String>,
}

/// The SPL logs of session directory `dir`, by file name. Disk work.
pub(super) fn read_logs(dir: &Path) -> Vec<SalvagedLog> {
    let spl = dir.join(SPL_DIR);
    let mut names: Vec<String> = match std::fs::read_dir(&spl) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".csv") && !n.ends_with(".bands.csv"))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let p = spl.join(&name);
            SalvagedLog {
                file: format!("{SPL_DIR}/{name}"),
                log: read_log(&p),
            }
        })
        .collect()
}

fn read_log(p: &Path) -> Result<SavedSplLog, String> {
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    let read = spl_log::import_csv(&bytes).map_err(|e| e.to_string())?;
    let bands_path = p.with_extension("bands.csv");
    let bands = match std::fs::read(&bands_path) {
        Ok(b) => {
            band_log::import_csv(&b)
                .map_err(|e| format!("its band log {}: {e}", bands_path.display()))?
                .rows
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("its band log {}: {e}", bands_path.display())),
    };
    let mark = match log_mark::read(&log_mark::path_of(p)) {
        MarkRead::Read(m) => Some(m),
        MarkRead::Missing | MarkRead::Unreadable(_) => None,
    };
    Ok(SavedSplLog {
        info: read.info,
        rows: read.rows,
        bands,
        mark,
    })
}

/// A salvaged meter's configuration: the default on the logged input. The header holds no
/// window configuration; the operator applies limits again to judge.
fn salvaged_config(name: &str, input: u16) -> MeasConfig {
    MeasConfig {
        scope: MeasScope::Session,
        name: format!("{name} (salvaged)"),
        kind: MeasKind::Spl {
            config: SplConfig {
                cue: None,
                bands: None,
                input,
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
                peak_weighting: PeakWeighting::C,
                leq: LeqConfig::default_windows(),
                position: None,
            },
        },
    }
}

impl Control {
    /// Each salvaged log as a stopped SPL meter holding it: the meters made, and the logs
    /// refused with why.
    pub(super) fn import_salvaged_logs(
        &mut self,
        logs: Vec<SalvagedLog>,
    ) -> (Vec<MeasId>, Vec<SalvageRefusal>) {
        let mut made = Vec::new();
        let mut refused = Vec::new();
        for l in logs {
            let log = match l.log {
                Ok(log) => log,
                Err(reason) => {
                    refused.push(SalvageRefusal {
                        file: l.file,
                        reason,
                    });
                    continue;
                }
            };
            let config = salvaged_config(&log.info.name, log.info.input);
            match self.meas_create(config) {
                Ok(m) => {
                    let mut log = log;
                    log.info.meas = m.id;
                    self.set_spl_log(m.id, LeqLog::restored(log));
                    // The meter's log entity takes the salvaged log's start.
                    self.ensure_spl_log(&m, None);
                    made.push(m.id);
                }
                Err(e) => refused.push(SalvageRefusal {
                    file: l.file,
                    reason: e.msg,
                }),
            }
        }
        (made, refused)
    }
}
