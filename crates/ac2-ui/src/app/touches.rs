//! Which panes a message makes stale, so a frame rebuilds only those.

use ac2_proto::topic::{Stream, Topic};

use crate::conn::DataSnapshot;
use crate::state::{Msg, PaneKind};

/// The panes a frame tick rebuilds: every motion in progress moves its own panes on the same
/// tick. A tick given to one motion alone stops the others: the spectrum curve would halt
/// for the 0.3 s of each once-a-second Leq bar ease. The last step of each lands it.
pub(super) fn tick_touches(
    navigating: bool,
    bars_easing: bool,
    spectrum_gliding: bool,
) -> Vec<PaneKind> {
    let mut panes = Vec::new();
    // The frequency axis moves while navigation settles.
    if navigating {
        panes.extend([PaneKind::Transfer, PaneKind::Spectrum, PaneKind::Distortion]);
    }
    // The Leq bars glide to their newest frame.
    if bars_easing {
        panes.push(PaneKind::Spl);
    }
    // The live spectrum curves glide to their newest frame.
    if spectrum_gliding && !panes.contains(&PaneKind::Spectrum) {
        panes.push(PaneKind::Spectrum);
    }
    panes
}

/// Which panes a message can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Touches {
    /// The frame clock: whatever is in motion ([`tick_touches`]).
    Tick,
    /// New frames: the panes drawing the topics whose picture changed.
    Data,
    /// A mirror view: every pane, unless only keepalive time moved.
    Mirror,
    /// The SPL meter's Leq history.
    Spl,
    /// The toasts' hover and dismissal: no pane.
    Toasts,
    /// The mouse on a plate: the panes that draw plates, rebuilt on every step of a drag.
    Plates,
    /// Keys, commands, replies: anything may change.
    All,
}

impl Touches {
    pub(super) fn of(msg: &Msg) -> Self {
        use crate::conn::ConnEvent;
        match msg {
            Msg::Tick { .. } => Self::Tick,
            // A row dragged over the panes changes the drag's highlight and label, drawn
            // over the panes; the panes' own pictures stay until it is dropped.
            Msg::ToastsHeld(_)
            | Msg::DismissToast(_)
            | Msg::Rasterizer(_)
            | Msg::RowDrag(
                crate::state::DragMsg::Start(_)
                | crate::state::DragMsg::Over { .. }
                | crate::state::DragMsg::OverRow(_),
            ) => Self::Toasts,
            Msg::Plate(_) | Msg::LegendScroll { .. } => Self::Plates,
            Msg::LeqStrip(..) => Self::Spl,
            Msg::Conn(_, e) => match **e {
                ConnEvent::Data(_) => Self::Data,
                ConnEvent::Mirror(_) => Self::Mirror,
                ConnEvent::LeqBackfill { .. } | ConnEvent::LeqSpan { .. } => Self::Spl,
                _ => Self::All,
            },
            _ => Self::All,
        }
    }
}

/// The panes that draw `topic`. Input meters are drawn by the chrome, outside any pane, and
/// by the Programme panes.
fn topic_panes(topic: &Topic) -> &'static [PaneKind] {
    match topic {
        Topic::SessionLevels | Topic::SessionGonio => &[PaneKind::Programme],
        Topic::Data { stream, .. } => match stream {
            Stream::Tf => &[PaneKind::Transfer, PaneKind::Walk],
            Stream::Ir => &[PaneKind::Transfer],
            Stream::Spec | Stream::Rta => &[PaneKind::Spectrum],
            Stream::Spl | Stream::Leq | Stream::BandLeq => &[PaneKind::Spl],
            Stream::Levels => &[],
        },
        _ => &[],
    }
}

/// The panes whose picture differs between snapshots `old` and `new`: a topic coming or
/// going, new content, a STALE flip; every pane when the daemon's liveness flips.
pub(super) fn data_touches(
    old: Option<&DataSnapshot>,
    new: Option<&DataSnapshot>,
) -> Vec<PaneKind> {
    let (Some(old), Some(new)) = (old, new) else {
        return PaneKind::ALL.to_vec();
    };
    if old.latest.responding != new.latest.responding {
        return PaneKind::ALL.to_vec();
    }
    let mut out: Vec<PaneKind> = Vec::new();
    let mut add = |t: &Topic| {
        for p in topic_panes(t) {
            if !out.contains(p) {
                out.push(*p);
            }
        }
    };
    for (k, n) in &new.latest.frames {
        let changed = old.latest.frames.get(k).is_none_or(|o| {
            o.stale != n.stale
                || (o.frame.stamp.seq != n.frame.stamp.seq
                    && !crate::conn::same_picture(&o.frame, &n.frame))
        });
        if changed {
            add(&n.topic);
        }
    }
    for (k, o) in &old.latest.frames {
        if !new.latest.frames.contains_key(k) {
            add(&o.topic);
        }
    }
    out
}
