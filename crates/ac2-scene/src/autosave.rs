//! The autosave indicator: `autosaved 5 min ago`, `saving…`, `autosave failed: <reason>`.
//! Shared by the app's top bar and `ac2 session status`. Also the SPL logs the daemon
//! cannot write (`LOG NOT SAVED · disk full`), and the state files the daemon found damaged
//! at start (`FILE DAMAGED · rig settings`), for the banners and `ac2 status`.

use ac2_proto::model::{
    Autosave, AutosaveState, DamagedFile, LogWriteFailure, MeterLog, StateFile,
};
use ac2_proto::units::{MeasId, WallNs};

use crate::format;
use crate::time::{self, ClockOffset};

/// Longest failure reason in the label; the whole reason is in [`AutosaveLabel::detail`].
pub const MAX_REASON_CHARS: usize = 60;

/// How the label is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutosaveTone {
    /// Saved, or on with nothing to save yet: dim.
    Quiet,
    /// A change is being written.
    Busy,
    /// The last write failed: warning colour.
    Warning,
}

/// The indicator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutosaveLabel {
    /// Shown text.
    pub text: String,
    /// Colour.
    pub tone: AutosaveTone,
    /// Longer text on hover.
    pub detail: String,
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// The indicator for `a`, or `None` when the daemon does not autosave. Ages are on the
/// daemon clock (`offset`).
pub fn autosave_label(
    a: &Autosave,
    client_now: WallNs,
    offset: ClockOffset,
) -> Option<AutosaveLabel> {
    let when = |at: WallNs| format::ago(time::age_s(at, client_now, offset));
    let last = a.saved_at.map_or_else(
        || "nothing written yet".to_owned(),
        |t| format!("last written {}", when(t)),
    );
    Some(match &a.state {
        AutosaveState::Off => return None,
        AutosaveState::Saved => match a.saved_at {
            Some(t) => AutosaveLabel {
                text: format!("autosaved {}", when(t)),
                tone: AutosaveTone::Quiet,
                detail: "Measurements and traces are autosaved by the daemon and restored \
                         when it restarts."
                    .to_owned(),
            },
            None => AutosaveLabel {
                text: "autosave on".to_owned(),
                tone: AutosaveTone::Quiet,
                detail: "Measurements and traces will be autosaved by the daemon as soon as \
                         there are any."
                    .to_owned(),
            },
        },
        AutosaveState::Pending => AutosaveLabel {
            text: "saving…".to_owned(),
            tone: AutosaveTone::Busy,
            detail: format!("Autosave: a change is being written ({last})."),
        },
        AutosaveState::Failed { reason } => AutosaveLabel {
            text: format!("autosave failed: {}", shorten(reason, MAX_REASON_CHARS)),
            tone: AutosaveTone::Warning,
            detail: format!(
                "Autosave failed: {reason}. It is retried; {last}. `ac2 session save <name>` \
                 saves elsewhere."
            ),
        },
    })
}

/// What the LOG NOT SAVED banner says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFailureText {
    /// `LOG NOT SAVED · <reason>` (the longest-failing log's reason).
    pub banner: String,
    /// Which logs, for how long, and that it is retried.
    pub detail: String,
}

/// How often the daemon tries a failing log again (its append interval).
pub const LOG_RETRY_S: u64 = 30;

fn log_name(log: MeterLog) -> &'static str {
    match log {
        MeterLog::Spl => "SPL log",
        MeterLog::Bands => "band log",
    }
}

/// One line per failing log: `SPL log of Main not saved for 12 min: disk full`. `name`
/// gives a meter's name (`None`: the id is shown). Ages are on the daemon clock
/// (`offset`).
pub fn log_failure_lines(
    failures: &[LogWriteFailure],
    name: impl Fn(MeasId) -> Option<String>,
    client_now: WallNs,
    offset: ClockOffset,
) -> Vec<String> {
    failures
        .iter()
        .map(|f| {
            let who = name(f.meas).unwrap_or_else(|| format!("measurement {}", f.meas.0));
            format!(
                "{} of {who} not saved for {}: {}",
                log_name(f.log),
                format::age(time::age_s(f.since, client_now, offset)),
                f.reason
            )
        })
        .collect()
}

/// The LOG NOT SAVED banner, or `None` while every log is on disk. The Leq history on
/// screen is complete either way; what is missing is its copy on disk, which a restart, a
/// session save or an export would need.
pub fn log_failure_text(
    failures: &[LogWriteFailure],
    name: impl Fn(MeasId) -> Option<String>,
    client_now: WallNs,
    offset: ClockOffset,
) -> Option<LogFailureText> {
    let oldest = failures.iter().min_by_key(|f| f.since)?;
    let lines = log_failure_lines(failures, name, client_now, offset);
    Some(LogFailureText {
        banner: format!(
            "LOG NOT SAVED · {}",
            shorten(&oldest.reason, MAX_REASON_CHARS)
        ),
        detail: format!("{}; retried every {LOG_RETRY_S} s", lines.join("; ")),
    })
}

/// What the FILE DAMAGED banner says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DamageText {
    /// `FILE DAMAGED · <what>` (one file), or `FILE DAMAGED · <n> files`.
    pub banner: String,
    /// A [`damaged_file_lines`] line per file.
    pub detail: String,
}

/// What a state file holds, as the operator calls it.
pub fn state_file_name(k: StateFile) -> &'static str {
    match k {
        StateFile::Autosave => "autosave",
        StateFile::Trace => "trace file",
        StateFile::SplLog => "SPL log",
        StateFile::BandLog => "band log",
        StateFile::Working => "working state",
        StateFile::RigSettings => "rig settings",
        StateFile::RigLogs => "rig meters' logs",
        StateFile::Calibrations => "calibrations",
        StateFile::ServerKey => "server key",
        StateFile::AuthorizedClients => "authorized clients",
    }
}

fn file_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// One line per damaged file: `rig settings rig.json damaged: <reason>; set aside as
/// rig.json.damaged-1792191433`.
pub fn damaged_file_lines(files: &[DamagedFile]) -> Vec<String> {
    files
        .iter()
        .map(|d| {
            let aside = d
                .set_aside_as
                .as_deref()
                .map_or_else(String::new, |a| format!("; set aside as {}", file_name(a)));
            format!(
                "{} {} damaged: {}{aside}",
                state_file_name(d.kind),
                file_name(&d.path),
                d.reason
            )
        })
        .collect()
}

/// The FILE DAMAGED banner, or `None` when the daemon started on sound files. It stays
/// until the daemon restarts: what was lost (a trace, a log, a setting) is for the operator
/// to check, whenever they next look.
pub fn damage_text(files: &[DamagedFile]) -> Option<DamageText> {
    let what = match files {
        [] => return None,
        [one] => state_file_name(one.kind).to_owned(),
        many => format!("{} files", many.len()),
    };
    Some(DamageText {
        banner: format!("FILE DAMAGED · {what}"),
        detail: damaged_file_lines(files).join("; "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damaged_files() {
        assert_eq!(damage_text(&[]), None);
        let rig = DamagedFile {
            kind: StateFile::RigSettings,
            path: "/home/m/.config/ac2/rig.json".into(),
            set_aside_as: Some("/home/m/.config/ac2/rig.json.damaged-17".into()),
            reason: "EOF while parsing; read the previous copy".into(),
            at: WallNs(1),
        };
        let t = damage_text(std::slice::from_ref(&rig)).expect("text");
        assert_eq!(t.banner, "FILE DAMAGED · rig settings");
        assert_eq!(
            t.detail,
            "rig settings rig.json damaged: EOF while parsing; read the previous copy; \
             set aside as rig.json.damaged-17"
        );
        let log = DamagedFile {
            kind: StateFile::SplLog,
            path: "spl/1.csv".into(),
            set_aside_as: None,
            reason: "missing".into(),
            at: WallNs(1),
        };
        let t = damage_text(&[rig, log]).expect("text");
        assert_eq!(t.banner, "FILE DAMAGED · 2 files");
        assert!(
            t.detail.ends_with("; SPL log 1.csv damaged: missing"),
            "{}",
            t.detail
        );
    }

    const S: u64 = 1_000_000_000;

    fn at(state: AutosaveState, saved_at: Option<u64>) -> Autosave {
        Autosave {
            state,
            saved_at: saved_at.map(WallNs),
            log_failures: Vec::new(),
        }
    }

    #[test]
    fn labels() {
        let now = WallNs(10_000 * S);
        let off = ClockOffset::default();
        assert_eq!(
            autosave_label(&at(AutosaveState::Off, None), now, off),
            None
        );
        let l = |a| autosave_label(&a, now, off).expect("label");
        assert_eq!(l(at(AutosaveState::Saved, None)).text, "autosave on");
        let saved = l(at(AutosaveState::Saved, Some(10_000 * S - 5 * S)));
        assert_eq!(saved.text, "autosaved just now");
        assert_eq!(saved.tone, AutosaveTone::Quiet);
        assert_eq!(
            l(at(AutosaveState::Saved, Some(10_000 * S - 300 * S))).text,
            "autosaved 5 min ago"
        );
        let busy = l(at(AutosaveState::Pending, Some(10_000 * S - 300 * S)));
        assert_eq!(
            (busy.text.as_str(), busy.tone),
            ("saving…", AutosaveTone::Busy)
        );
        assert!(
            busy.detail.contains("last written 5 min ago"),
            "{}",
            busy.detail
        );
    }

    #[test]
    fn a_failure_is_a_warning_with_the_reason() {
        let now = WallNs(10_000 * S);
        let reason = "/home/op/.local/share/ac2/autosave: No space left on device (os error 28)";
        let f = autosave_label(
            &at(
                AutosaveState::Failed {
                    reason: reason.into(),
                },
                None,
            ),
            now,
            ClockOffset::default(),
        )
        .expect("label");
        assert_eq!(f.tone, AutosaveTone::Warning);
        assert!(
            f.text.starts_with("autosave failed: /home/op/"),
            "{}",
            f.text
        );
        assert!(f.text.ends_with('…'));
        assert_eq!(
            f.text.chars().count(),
            "autosave failed: ".len() + MAX_REASON_CHARS
        );
        assert!(f.detail.contains(reason));
        assert!(f.detail.contains("nothing written yet"));
        let short = autosave_label(
            &at(
                AutosaveState::Failed {
                    reason: "disk full".into(),
                },
                None,
            ),
            now,
            ClockOffset::default(),
        )
        .expect("label");
        assert_eq!(short.text, "autosave failed: disk full");
    }

    #[test]
    fn failing_logs_name_the_meter_the_reason_and_how_long() {
        let now = WallNs(10_000 * S);
        let off = ClockOffset::default();
        let name = |m: MeasId| (m == MeasId(1)).then(|| "Main".to_owned());
        assert_eq!(log_failure_text(&[], name, now, off), None);
        let fails = [
            LogWriteFailure {
                meas: MeasId(1),
                log: MeterLog::Spl,
                reason: "disk full".into(),
                since: WallNs(10_000 * S - 720 * S),
            },
            LogWriteFailure {
                meas: MeasId(7),
                log: MeterLog::Bands,
                reason: "permission denied".into(),
                since: WallNs(10_000 * S - 5 * S),
            },
        ];
        let t = log_failure_text(&fails, name, now, off).expect("banner");
        assert_eq!(t.banner, "LOG NOT SAVED · disk full");
        assert_eq!(
            t.detail,
            "SPL log of Main not saved for 12 min: disk full; band log of measurement 7 \
             not saved for 5.0 s: permission denied; retried every 30 s"
        );
        assert_eq!(
            log_failure_lines(&fails[..1], name, now, off),
            ["SPL log of Main not saved for 12 min: disk full"]
        );
    }

    /// Ages are on the daemon clock: a daemon 2 min ahead wrote "just now" in its time.
    #[test]
    fn ages_use_the_clock_offset() {
        let now = WallNs(10_000 * S);
        let a = at(AutosaveState::Saved, Some(10_000 * S + 120 * S));
        let l = autosave_label(&a, now, ClockOffset(120 * S as i64)).expect("label");
        assert_eq!(l.text, "autosaved just now");
    }
}
