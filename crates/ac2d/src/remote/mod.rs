//! Remote inputs (`docs/design/remote-inputs.md`): one rig measures with another rig's
//! input. The source side serves its captured channels as `audio/<channel>` ([`tap`]); the
//! session side keeps a link to each paired rig ([`link`]), joins the remote samples into
//! its own blocks ([`join`]) and judges the clocks between the rigs ([`peer_clock`]).

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
