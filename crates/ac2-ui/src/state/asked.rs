//! What a rig's daemon was last asked for over its link: each rig is asked by what it was
//! asked last, never by what another rig wanted a moment ago.

/// The standing requests last sent to a rig's daemon, and when.
#[derive(Debug)]
pub(crate) struct LastAsked {
    /// What the link was last asked to receive, and how often ([`crate::link_wants`]).
    pub(crate) link_wants: crate::link_wants::Sent,
    /// Whether the link was last asked for the input meters (a new link has none): each
    /// rig's link follows its own session, so it is asked by what it was asked last, never
    /// by what the shown rig wanted a moment ago.
    pub(super) meters_asked: bool,
    /// When the device preview was last asked for.
    pub(super) preview_sent_s: f64,
    /// The session the device list was last asked for on its own (for the input names of
    /// the always-on meters).
    pub(super) devices_for: Option<ac2_proto::units::SessionEpoch>,
}

impl Default for LastAsked {
    fn default() -> Self {
        Self {
            link_wants: crate::link_wants::Sent::default(),
            meters_asked: false,
            // No preview asked yet: the first one is due at once, whatever the clock reads.
            preview_sent_s: f64::NEG_INFINITY,
            devices_for: None,
        }
    }
}
