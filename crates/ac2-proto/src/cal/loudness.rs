//! Which inputs carry a loudness figure (docs/design/programme-meter.md §6.1). The daemon
//! meters exactly these sources and every client words its loudness readouts from the same
//! rule, so a pane never waits for a source the daemon does not compute.

use crate::frame::LoudnessSource;
use crate::model::{InputKind, Pair};

/// The loudness sources of `captured` channels: every pair in `pairs` with both inputs
/// captured and neither a mic, in `pairs` order, then every captured input that is in no
/// pair and is not a mic, ascending by channel. An input of a pair whose partner is not
/// captured gets none: metering half a stereo programme as mono would read 3 LU low. A
/// mic gets none: its LUFS figure would follow the preamp gain and be misread beside its
/// dB SPL. `kind` gives each channel's kind (undeclared: [`super::default_kind`]).
pub fn loudness_sources(
    kind: impl Fn(u16) -> InputKind,
    pairs: &[Pair],
    captured: &[u16],
) -> Vec<LoudnessSource> {
    let programme = |ch: u16| captured.contains(&ch) && !matches!(kind(ch), InputKind::Mic { .. });
    let mut out: Vec<LoudnessSource> = pairs
        .iter()
        .filter(|p| programme(p.left) && programme(p.right))
        .map(|p| LoudnessSource::Pair {
            left: p.left,
            right: p.right,
        })
        .collect();
    let mut singles: Vec<u16> = captured
        .iter()
        .copied()
        .filter(|&ch| programme(ch) && !pairs.iter().any(|p| p.left == ch || p.right == ch))
        .collect();
    singles.sort_unstable();
    singles.dedup();
    out.extend(
        singles
            .into_iter()
            .map(|channel| LoudnessSource::Input { channel }),
    );
    out
}

impl LoudnessSource {
    /// Whether `channel` is one of the source's inputs (a reset of that input resets it).
    pub fn has(&self, channel: u16) -> bool {
        match *self {
            Self::Pair { left, right } => channel == left || channel == right,
            Self::Input { channel: c } => channel == c,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cal::default_kind;

    fn pair(left: u16, right: u16) -> Pair {
        Pair {
            left,
            right,
            name: None,
        }
    }

    #[test]
    fn pairs_first_then_unpaired_line_and_digital_inputs() {
        // 0+1 line pair, 2 mic, 3 digital, 4+5 pair with 5 not captured, 6 line.
        let kind = |ch: u16| match ch {
            2 => default_kind(),
            3 => InputKind::Digital,
            _ => InputKind::Line,
        };
        let got = loudness_sources(kind, &[pair(4, 5), pair(0, 1)], &[6, 0, 1, 2, 3, 4]);
        assert_eq!(
            got,
            vec![
                LoudnessSource::Pair { left: 0, right: 1 },
                LoudnessSource::Input { channel: 3 },
                LoudnessSource::Input { channel: 6 },
            ]
        );
    }

    #[test]
    fn a_source_has_its_inputs() {
        let p = LoudnessSource::Pair { left: 0, right: 1 };
        assert!(p.has(0) && p.has(1) && !p.has(2));
        assert!(LoudnessSource::Input { channel: 2 }.has(2));
    }
}
