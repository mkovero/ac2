//! When one topic of a job goes out ([`Pace`]), and what an emit left for later ([`Flush`]).

use std::time::{Duration, Instant};

use ac2_proto::frame::ProtectionFlags;
use ac2_proto::topic::Topic;

use super::{Emitter, StampArgs};
use crate::cadence::Cadence;

/// Longest a client goes without a frame of a result that has not changed (settled or
/// gated by protection). A client marks a topic STALE after a second without a new frame
/// (`ac2_client::data::STALE_AFTER`): re-sending an unchanged result four times a second
/// keeps a live but steady measurement fresh with room for a late or dropped frame, while
/// only a stopped stream (no audio, so no blocks and no frames at all) goes STALE.
pub(crate) const REFRESH: Duration = Duration::from_millis(250);

/// What a [`Pace`] says about one result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Due {
    /// Build and send it now.
    Send,
    /// Nothing to send: nobody receives it, or it is unchanged and was sent recently.
    Skip,
    /// Changed, or due a refresh, but not yet; ask again at this instant.
    Later(Instant),
}

/// When one topic of a job is sent: on a new result (at most every `min_period` on
/// average, on a [`Cadence`] so jittery wakeups do not cost rate), on a change of its
/// protection flags, and every [`REFRESH`] while unchanged as long as audio keeps coming —
/// and only while someone subscribes to it.
#[derive(Debug)]
pub(crate) struct Pace {
    cadence: Cadence,
    sent: Option<Sent>,
}

#[derive(Clone, Copy, Debug)]
struct Sent {
    generation: u64,
    prot: ProtectionFlags,
    audio_sample: u64,
    at: Instant,
}

impl Pace {
    /// Sends a new result at most every `min_period` (zero: whenever the job emits).
    pub(crate) fn new(min_period: Duration) -> Self {
        Self {
            // A job emits on hand-offs a fraction of `min_period` apart; a quarter period of
            // lead lets the hand-off just before a slot take it instead of the one after.
            cadence: Cadence::new(min_period, min_period / 4),
            sent: None,
        }
    }

    /// Whether `topic`'s result of generation `generation` (a count the job advances whenever
    /// the result may have changed), stamped `stamp`, goes out at `now`. A `Send` is taken
    /// to be sent.
    pub(crate) fn due(
        &mut self,
        e: &Emitter,
        topic: Topic,
        generation: u64,
        stamp: &StampArgs,
        now: Instant,
    ) -> Due {
        if !e.wants(topic) {
            // A new subscriber gets the result on the next emit, whatever it was before.
            self.unsent();
            return Due::Skip;
        }
        let due = match self.sent {
            None => Due::Send,
            Some(s) => {
                if s.generation != generation || s.prot != stamp.protection {
                    match self.cadence.ready_at() {
                        Some(t) if now < t => Due::Later(t),
                        _ => Due::Send,
                    }
                } else if stamp.audio_sample <= s.audio_sample {
                    // Nothing new at all: the stream has stopped, and the client's STALE
                    // must say so.
                    Due::Skip
                } else if now.saturating_duration_since(s.at) >= REFRESH {
                    Due::Send
                } else {
                    // The refresh carries the newest stamp even after the last block.
                    Due::Later(s.at + REFRESH)
                }
            }
        };
        if due == Due::Send {
            self.cadence.take(now);
            self.sent = Some(Sent {
                generation,
                prot: stamp.protection,
                audio_sample: stamp.audio_sample,
                at: now,
            });
        }
        due
    }

    /// The result taken as sent did not go out: send it on the next emit.
    pub(crate) fn unsent(&mut self) {
        self.sent = None;
        self.cadence.reset();
    }
}

/// Whether `emit` sent everything it had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flush {
    Done,
    /// Something was held back (by its rate, or until its refresh); emit again at this
    /// instant even without new audio, so the newest result goes out after the last block.
    Pending(Instant),
}

impl Flush {
    /// [`Flush::Pending`] if `due` said to ask again.
    pub(crate) fn from_due(due: Due) -> Self {
        match due {
            Due::Later(t) => Self::Pending(t),
            Due::Send | Due::Skip => Self::Done,
        }
    }

    /// Pending if either is, at the sooner instant.
    pub(crate) fn and(self, o: Flush) -> Flush {
        match (self, o) {
            (Flush::Pending(a), Flush::Pending(b)) => Flush::Pending(a.min(b)),
            (Flush::Pending(t), Flush::Done) | (Flush::Done, Flush::Pending(t)) => {
                Flush::Pending(t)
            }
            (Flush::Done, Flush::Done) => Flush::Done,
        }
    }
}
