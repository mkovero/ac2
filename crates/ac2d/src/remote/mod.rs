//! Remote inputs (`docs/design/remote-inputs.md`): one rig measures with another rig's
//! input. The source side serves its captured channels as `audio/<channel>` ([`tap`]); the
//! session side keeps a link to each paired rig ([`link`]), joins the remote samples into
//! its own blocks ([`join`]) and judges the clocks between the rigs ([`peer_clock`]).

use std::ops::Range;
use std::time::Duration;

use ac2_proto::model::{Anchor, RemoteAudio, RemoteLink, SourceState};
use ac2_proto::units::{RigId, Seconds, SessionEpoch};

pub(crate) mod join;
pub(crate) mod link;
pub(crate) mod peer_clock;
pub(crate) mod tap;

/// The longest link offset (a WAN's worth).
pub(crate) const MAX_LINK_OFFSET_S: f64 = 0.5;

/// The default link offset (§4): a remote block exists only at the end of its period, waits
/// up to one period for its rig's hand-off, crosses the network (≈1.3 ms at p99.9 measured
/// between the rigs, so 2 ms), and waits up to one of this rig's periods for its hand-off.
pub(crate) fn default_link_offset(ours_s: f64, theirs_s: f64) -> f64 {
    2.0 * theirs_s + ours_s + 0.002
}

/// The link offset in force: the one set, never below the default (every remote sample
/// would be late by construction) nor above [`MAX_LINK_OFFSET_S`].
pub(crate) fn link_offset(set: Option<f64>, ours_s: f64, theirs_s: f64) -> f64 {
    let floor = default_link_offset(ours_s, theirs_s);
    set.map_or(floor, |s| s.max(floor)).min(MAX_LINK_OFFSET_S)
}

/// What a stream runs for remote inputs, once set up on it.
pub(crate) struct RemoteRun {
    /// Session channel per column of the joined feed: this rig's inputs, then the remote
    /// inputs in `remote_inputs` order; empty without remote inputs.
    pub(crate) joined_map: Vec<u16>,
}

/// What the joined feed marks on one remote column of a block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ColumnMark {
    pub(crate) column: u16,
    /// Frames of the block (from its start) whose remote sample never arrived: late, lost
    /// or slipped. They hold zeros so the block stays rectangular; they are no signal.
    pub(crate) gaps: Vec<Range<u32>>,
    /// The input's map moved by this many samples at this block (a slip); 0: it did not.
    pub(crate) slip: i64,
    /// A frame of the input arrived after its samples were needed since the last block:
    /// its gaps are the link offset's, not lost packets'.
    pub(crate) late: bool,
}

/// The marks of a block's remote columns; empty on device blocks and wherever every remote
/// sample arrived. Per column, so a job reading a whole input never pays for another
/// input's gap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ColumnMarks(Vec<ColumnMark>);

impl ColumnMarks {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn get(&self, column: usize) -> Option<&ColumnMark> {
        self.0.iter().find(|m| usize::from(m.column) == column)
    }

    fn get_mut(&mut self, column: u16) -> &mut ColumnMark {
        match self.0.iter().position(|m| m.column == column) {
            Some(i) => &mut self.0[i],
            None => {
                self.0.push(ColumnMark {
                    column,
                    ..ColumnMark::default()
                });
                let last = self.0.len() - 1;
                &mut self.0[last]
            }
        }
    }

    /// Marks frame `frame` of `column` missing; frames come in order.
    pub(crate) fn gap(&mut self, column: u16, frame: u32) {
        let gaps = &mut self.get_mut(column).gaps;
        match gaps.last_mut() {
            Some(r) if r.end == frame => r.end += 1,
            _ => gaps.push(frame..frame + 1),
        }
    }

    pub(crate) fn slip(&mut self, column: u16, by: i64) {
        self.get_mut(column).slip += by;
    }

    pub(crate) fn late(&mut self, column: u16) {
        self.get_mut(column).late = true;
    }

    /// The missing frames of `column`.
    pub(crate) fn gaps(&self, column: usize) -> &[Range<u32>] {
        self.get(column).map_or(&[], |m| &m.gaps)
    }

    /// Whether averages over `column` restart at this block: a sample of it is missing or
    /// its map moved.
    pub(crate) fn breaks(&self, column: usize) -> bool {
        self.get(column)
            .is_some_and(|m| !m.gaps.is_empty() || m.slip != 0)
    }
}

/// A remote input's figures, as the joiner reports them.
#[derive(Debug, Clone)]
pub(crate) struct InputReport {
    pub(crate) slot: u16,
    pub(crate) audio: RemoteAudio,
    pub(crate) anchor: Option<Anchor>,
    pub(crate) late_frames: u64,
    pub(crate) late_frames_hour: u32,
    pub(crate) slips: u32,
    pub(crate) last_slip: Option<i64>,
    pub(crate) arrival_margin: Option<Seconds>,
}

/// A remote rig's figures, as the joiner reports them.
#[derive(Debug, Clone)]
pub(crate) struct RigReport {
    pub(crate) rig: RigId,
    /// The cross-rig rate over the long and the short window: (ppm, span s).
    pub(crate) long: Option<(f64, f64)>,
    pub(crate) short: Option<(f64, f64)>,
    pub(crate) streaming_for: Option<Duration>,
    pub(crate) disagree: Option<f64>,
}

/// What the links and the joiner tell the control thread.
#[derive(Debug)]
pub(crate) enum RemoteMsg {
    /// A link's state, once a second.
    Link {
        rig: RigId,
        link: RemoteLink,
        bytes_per_s: f64,
        window: u32,
    },
    /// A paired rig's filtered state, as read.
    Source { rig: RigId, state: Box<SourceState> },
    /// The joined feed of the session `epoch`, once a second.
    Joined {
        epoch: SessionEpoch,
        inputs: Vec<InputReport>,
        rigs: Vec<RigReport>,
        link_offset: Seconds,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_link_offset_covers_both_periods_and_transit() {
        let ms = |s: f64| (s * 1e4).round() / 10.0;
        // 256 frames at 96 kHz on both rigs: 10 ms; 1024 at 48 kHz: 66 ms.
        assert_eq!(ms(default_link_offset(256.0 / 96e3, 256.0 / 96e3)), 10.0);
        assert_eq!(ms(default_link_offset(1024.0 / 48e3, 1024.0 / 48e3)), 66.0);
        assert_eq!(ms(link_offset(Some(0.001), 0.005, 0.005)), 17.0);
        assert_eq!(link_offset(Some(2.0), 0.005, 0.005), MAX_LINK_OFFSET_S);
    }
}
