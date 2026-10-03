//! The autosave indicator: `autosaved 5 min ago`, `saving…`, `autosave failed: <reason>`.
//! Shared by the app's top bar and `ac2 session status`.

use ac2_proto::model::{Autosave, AutosaveState};
use ac2_proto::units::WallNs;

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

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    fn at(state: AutosaveState, saved_at: Option<u64>) -> Autosave {
        Autosave {
            state,
            saved_at: saved_at.map(WallNs),
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

    /// Ages are on the daemon clock: a daemon 2 min ahead wrote "just now" in its time.
    #[test]
    fn ages_use_the_clock_offset() {
        let now = WallNs(10_000 * S);
        let a = at(AutosaveState::Saved, Some(10_000 * S + 120 * S));
        let l = autosave_label(&a, now, ClockOffset(120 * S as i64)).expect("label");
        assert_eq!(l.text, "autosaved just now");
    }
}
