//! What a graph pane draws behind its lead: the entries C ticks and a stored layout holds,
//! in drawing order. Each names a group, one live curve or one stored trace by its rig's
//! id; an id the mirror does not list yet waits in the entry itself, so there is no second
//! place where the wait could be forgotten.

use ac2_proto::model::layout::{EntryRef, PaneContent};
use ac2_proto::units::RigId;

/// Entries behind a lead at most: the wire's limit less the lead.
pub const MAX_WITH: usize = PaneContent::MAX_ENTRIES - 1;

/// One entry behind a pane's lead. `rig: None`: the pane's own rig, whichever that is;
/// `Some`: another rig's ids, kept as stored and drawn by no pane of this stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pick {
    pub rig: Option<RigId>,
    pub what: EntryRef,
}

impl Pick {
    pub fn here(what: EntryRef) -> Self {
        Self { rig: None, what }
    }
}

/// The entries behind a lead, nearest layer first. A fixed array, so a pane's view stays a
/// plain value; no entry twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct With {
    n: u8,
    picks: [Pick; MAX_WITH],
}

impl Default for With {
    fn default() -> Self {
        Self {
            n: 0,
            picks: [Pick::here(EntryRef::Group {
                meas: ac2_proto::units::MeasId(0),
            }); MAX_WITH],
        }
    }
}

impl With {
    pub fn as_slice(&self) -> &[Pick] {
        &self.picks[..usize::from(self.n)]
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn len(&self) -> usize {
        usize::from(self.n)
    }

    pub fn contains(&self, p: &Pick) -> bool {
        self.as_slice().contains(p)
    }

    /// `p` added last; `false` when it is there already or there is no room.
    pub fn push(&mut self, p: Pick) -> bool {
        if self.contains(&p) || self.len() >= MAX_WITH {
            return false;
        }
        self.picks[self.len()] = p;
        self.n += 1;
        true
    }

    /// Keeps the entries `keep` says, in order; how many went.
    pub fn retain(&mut self, mut keep: impl FnMut(&Pick) -> bool) -> usize {
        let kept: Vec<Pick> = self
            .as_slice()
            .iter()
            .copied()
            .filter(|p| keep(p))
            .collect();
        let gone = self.len() - kept.len();
        *self = Self::of(&kept);
        gone
    }

    /// The first entry, taken out.
    pub fn take_first(&mut self) -> Option<Pick> {
        let first = *self.as_slice().first()?;
        *self = Self::of(&self.as_slice()[1..]);
        Some(first)
    }

    /// `list` in order, duplicates and what does not fit dropped.
    pub fn of(list: &[Pick]) -> Self {
        let mut w = Self::default();
        for p in list {
            w.push(*p);
        }
        w
    }

    pub fn clear(&mut self) {
        self.n = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::units::{MeasId, TraceId};

    #[test]
    fn entries_keep_their_order_once_each_up_to_the_limit() {
        let mut w = With::default();
        let a = Pick::here(EntryRef::Group { meas: MeasId(2) });
        let b = Pick::here(EntryRef::Trace { trace: TraceId(7) });
        assert!(w.push(a));
        assert!(w.push(b));
        assert!(!w.push(a), "no entry twice");
        assert_eq!(w.as_slice(), [a, b]);
        assert_eq!(w.take_first(), Some(a));
        assert_eq!(w.as_slice(), [b]);
        for i in 0..40 {
            w.push(Pick::here(EntryRef::Live {
                meas: MeasId(100 + i),
            }));
        }
        assert_eq!(w.len(), MAX_WITH);
        assert_eq!(w.retain(|p| p.what != b.what), 1);
        assert_eq!(w.len(), MAX_WITH - 1);
    }
}
