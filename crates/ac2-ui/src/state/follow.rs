//! What a rig does next, and for which measurement: the follow-ups waiting for the daemon's
//! answer or its next state ([`Due`]), and the measurement the operator chose with what
//! stands in for it when it goes ([`MeasChoice`]).

use super::*;

/// Follow-ups of something this app asked: each is taken once, when the daemon's answer or
/// the state it mirrors next shows what it waits for.
#[derive(Debug, Default)]
pub(crate) struct Due {
    /// The name of a transfer measurement submitted from its form: its creation lays it
    /// out in the transfer stack.
    pub(super) stack_on_create: Option<String>,
    /// The name of a measurement submitted from its form: its creation opens a screen of
    /// its own, one pane of its kind.
    pub(super) pane_on_create: Option<String>,
    /// The name of an alignment (or a spatial average) submitted from the math dialog, and
    /// the view its screen of its own opens in.
    pub(super) split_on_create: Option<(String, TransferView)>,
    /// Open the Sessions dialog once the daemon's state shows no audio I/O (one just started
    /// has none): its New leads on to the audio setup.
    pub(crate) open_session_when_empty: bool,
    /// The session dialog's session opened: ask what to measure ([`Self::choose_first`])
    /// once the daemon's state shows it open.
    pub(super) choose_when_open: bool,
    /// A measurement's row was chosen: its own panes go on screen after the message.
    pub(super) own_panes_due: bool,
    /// Settings sent with the arm in flight; a change made before the arm is confirmed is
    /// sent once it is.
    pub(super) armed_with: Option<GeneratorSettings>,
}

/// The measurement whose row the operator chose, whose own panes are on screen, and what
/// the selection falls back to and comes back from when the daemon takes it away.
#[derive(Debug, Default)]
pub(super) struct MeasChoice {
    /// The measurement whose row was chosen last (or that this app selected by itself, a
    /// measurement made, a sweep just run): its own panes are the ones on screen, and pane
    /// edits are its layout's. A child row, a row dropped on a pane or a click in a pane
    /// never moves it.
    pub(super) chosen: Option<MeasId>,
    /// The measurement whose own panes the operator put on screen here by choosing its row.
    /// An edit of the panes is stored as its layout (while a pane still shows it), even
    /// after a pane or child row selected another measurement: that one is a guest in its
    /// panes. While it is selected, another client's edit of its layout shows here too; no
    /// other measurement's does: a viewer nobody touches keeps its panes.
    pub(super) own_panes: Option<MeasId>,
    /// The lost selection's own panes were on screen: they follow it when it comes back.
    pub(super) lost_own_panes: bool,
    /// The measurement this app selected by itself, the one it had gone (or none chosen
    /// yet): a stand-in while it stays selected, whose own panes never come on screen.
    pub(super) standin: Option<MeasId>,
    /// The selected measurement that went, by name, while a stand-in (or nothing) is
    /// selected: one of that name coming back is selected again.
    pub(super) lost_selection: Option<String>,
    /// Names of the measurements the daemon has listed since this app started: a pane
    /// remembered on one of them waits for it to come back.
    pub(super) meas_seen: BTreeSet<String>,
}
