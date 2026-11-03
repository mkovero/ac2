//! When a live math channel forms a new result: at its operands' rate, not on every
//! operand's new result.
//!
//! Each live operand forms results at its own rate, out of step with the others. A result
//! formed on every operand's new result would come N times as often as any operand's (three
//! transfer functions at 28 frames/s give an average at 55–60), each differing from the
//! last by one operand's step only: no more information than one result per operand period,
//! sent several times over. So a new result forms once every running live operand has a
//! newer result than the last one went in with. Then each operand's newest state is in it,
//! and the result follows the operands at the rate they themselves change.
//!
//! An operand that stalls (its job busy, its input gone quiet without stopping) or runs
//! slower than the rest must not hold the result: once the first newer result has waited as
//! long as the slowest running operand's own period, the result forms with what has
//! arrived, and an operand two of its periods past its last result is no longer waited for
//! until it forms one again. So the newest operand result goes into the channel's result within about one
//! operand period, which is the delay a reader of that operand alone would see between
//! two of its frames. Times are sample indices of the hand-offs, so the pace follows the
//! audio clock, as the operands do.

/// Per live operand: its result count and how often it forms results.
#[derive(Clone, Copy, Debug, Default)]
struct Operand {
    /// Result count as last seen; `None` while it is not running.
    results: Option<u64>,
    /// Sample index at which the last new result was seen.
    changed_at: Option<u64>,
    /// Samples between its results, as last measured.
    period: Option<u64>,
    /// A newer result than the last formed result went in with.
    fresh: bool,
}

/// The pace of a live math channel's results ([module docs](self)).
#[derive(Clone, Debug, Default)]
pub(crate) struct ResultPace {
    /// One per operand in expression order; `None` for a stored one (it never changes).
    operands: Vec<Option<Operand>>,
    /// Sample index of the first newer operand result since the last formed result.
    pending_since: Option<u64>,
    /// A result has formed.
    started: bool,
}

impl ResultPace {
    /// Whether a new result forms at the hand-off ending at sample `now`, given each
    /// operand's state: `None` for a stored operand, `Some(None)` for a live one that is not
    /// running, `Some(Some(n))` for a running one that has formed `n` results.
    pub(crate) fn observe(&mut self, now: u64, states: &[Option<Option<u64>>]) -> bool {
        if self.operands.len() != states.len() {
            self.operands = states
                .iter()
                .map(|s| s.map(|_| Operand::default()))
                .collect();
            for (o, s) in self.operands.iter_mut().zip(states) {
                if let (Some(o), Some(r)) = (o, s) {
                    o.results = *r;
                }
            }
            self.started = false;
        }
        // An operand that starts or stops changes what goes in: a new result at once.
        let mut at_once = !self.started;
        for (o, s) in self.operands.iter_mut().zip(states) {
            let (Some(o), Some(r)) = (o, s) else { continue };
            match (o.results, *r) {
                (Some(a), Some(b)) if a != b => {
                    if let Some(t) = o.changed_at {
                        let step = now.saturating_sub(t);
                        // A stall ends with one long step; a period no more than doubles at
                        // a time, so one late result does not make the next wait long.
                        o.period = Some(o.period.map_or(step, |p| step.min(2 * p.max(1))));
                    }
                    o.changed_at = Some(now);
                    o.fresh = true;
                    self.pending_since.get_or_insert(now);
                }
                (Some(_), Some(_)) | (None, None) => {}
                (None, Some(_)) | (Some(_), None) => {
                    o.changed_at = None;
                    o.period = None;
                    o.fresh = false;
                    at_once = true;
                }
            }
            o.results = *r;
        }
        let running = || {
            self.operands
                .iter()
                .flatten()
                .filter(|o| o.results.is_some())
        };
        // An operand two of its periods past its last result has stalled: the rest no
        // longer wait for it, so the result keeps their rate, not a slower one.
        let keeping_up = |o: &&Operand| match (o.changed_at, o.period) {
            (Some(t), Some(p)) => now.saturating_sub(t) <= 2 * p,
            _ => true,
        };
        let all_fresh = running().filter(keeping_up).all(|o| o.fresh);
        let slowest = running()
            .filter(keeping_up)
            .filter_map(|o| o.period)
            .max()
            .unwrap_or(0);
        let waited = self
            .pending_since
            .is_some_and(|t| now.saturating_sub(t) >= slowest);
        let form = at_once || (self.pending_since.is_some() && (all_fresh || waited));
        if form {
            self.started = true;
            self.pending_since = None;
            for o in self.operands.iter_mut().flatten() {
                o.fresh = false;
            }
        }
        form
    }
}
