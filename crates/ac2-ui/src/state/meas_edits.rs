//! An edit made on a base waits for the edits of its measurement in flight. Every commit of
//! a measurement moves its `edit_rev`, so a `meas.update` (or an EQ `put`) built from the
//! mirror before the last edit's event arrives would be refused as stale: it waits for the
//! replies of the edits before it and is made on the measurement the last reply carries.
//! The edits that carry no base (colour, offset, layout, the other EQ edits, start, stop)
//! are last write wins and go at once, unless a based edit waits ahead of them: the daemon
//! executes one client's requests in the order sent. A `stale_meas` refusal then means
//! another client or the daemon committed the measurement in between: it is said, and the
//! edits queued behind it are dropped, as they were made on what the operator saw before
//! that change.

use std::collections::{HashMap, VecDeque};

use ac2_proto::Command;
use ac2_proto::model::{EqEdit, MeasConfig, Measurement};
use ac2_proto::units::{MeasId, Rev};
use serde_json::Value;

use super::AppState;
use crate::conn::{ConnEvent, Edited, MeasRefusal, Request, meas_edit_target};

#[derive(Debug, Default)]
pub struct MeasEdits {
    chains: HashMap<MeasId, Chain>,
}

#[derive(Debug, Default)]
struct Chain {
    /// Edits sent and not answered.
    in_flight: usize,
    /// The measurement as the last answered edit committed it, while the mirror is behind.
    known: Option<Measurement>,
    queued: VecDeque<Queued>,
}

#[derive(Debug)]
struct Queued {
    cmd: Command,
    what: String,
    /// The measurement's `edit_rev` and configuration in the mirror when the edit was made.
    built_on: Option<(Rev, MeasConfig)>,
}

impl AppState {
    /// Holds back each measurement edit in `out` while an earlier one of the same
    /// measurement is unanswered, and sends the next waiting edit of an answered one.
    pub(super) fn chain_meas_edits(&mut self, out: &mut Vec<Request>) {
        let waiting = self
            .rig
            .meas_edits
            .chains
            .values()
            .any(|c| !c.queued.is_empty());
        if !waiting
            && !out
                .iter()
                .any(|r| matches!(r, Request::Call { cmd, .. } if meas_edit_target(cmd).is_some()))
        {
            self.prune_meas_edits();
            return;
        }
        let mut sent = Vec::with_capacity(out.len());
        let ids: Vec<MeasId> = self.rig.meas_edits.chains.keys().copied().collect();
        for id in ids {
            self.send_next_edit(id, &mut sent);
        }
        for r in std::mem::take(out) {
            match r {
                Request::Call { cmd, what } if let Some(id) = meas_edit_target(&cmd) => {
                    let built_on = self.meas(id).map(|m| (m.edit_rev, m.config.clone()));
                    let chain = self.rig.meas_edits.chains.entry(id).or_default();
                    chain.queued.push_back(Queued {
                        cmd,
                        what,
                        built_on,
                    });
                    self.send_next_edit(id, &mut sent);
                }
                r => sent.push(r),
            }
        }
        *out = sent;
        self.prune_meas_edits();
    }

    fn send_next_edit(&mut self, id: MeasId, out: &mut Vec<Request>) {
        let Some(chain) = self.rig.meas_edits.chains.get_mut(&id) else {
            return;
        };
        while chain
            .queued
            .front()
            .is_some_and(|q| chain.in_flight == 0 || !based(&q.cmd))
        {
            let Some(q) = chain.queued.pop_front() else {
                return;
            };
            chain.in_flight += 1;
            out.push(Self::made_on_known(chain.known.as_ref(), q));
        }
    }

    fn made_on_known(known: Option<&Measurement>, q: Queued) -> Request {
        // Made on the mirror, unless an answered edit of this app's has moved the
        // measurement past what the mirror held then.
        let known = known.filter(|k| q.built_on.as_ref().is_none_or(|(rev, _)| k.edit_rev > *rev));
        let cmd = match known {
            Some(k) => rebase(q.cmd, q.built_on.as_ref().map(|(_, c)| c), k),
            None => q.cmd,
        };
        Request::Call { cmd, what: q.what }
    }

    /// Forgets what the mirror now holds, and the measurements with nothing waiting.
    fn prune_meas_edits(&mut self) {
        let mut chains = std::mem::take(&mut self.rig.meas_edits.chains);
        chains.retain(|id, c| {
            if c.known
                .as_ref()
                .is_some_and(|k| self.meas(*id).is_none_or(|m| m.edit_rev >= k.edit_rev))
            {
                c.known = None;
            }
            c.in_flight > 0 || c.known.is_some() || !c.queued.is_empty()
        });
        self.rig.meas_edits.chains = chains;
    }

    /// The answer to an edit of `meas`: the chain moves on, then the answer is handled as
    /// the reply (or the `conflict`) of any other call.
    pub(super) fn meas_edited(
        &mut self,
        meas: MeasId,
        what: String,
        result: Edited,
        out: &mut Vec<Request>,
    ) {
        let chain = self.rig.meas_edits.chains.entry(meas).or_default();
        chain.in_flight = chain.in_flight.saturating_sub(1);
        let mut dropped = 0;
        match &result {
            Ok(Some(m)) => {
                if chain.known.as_ref().is_none_or(|k| k.edit_rev < m.edit_rev) {
                    chain.known = Some((**m).clone());
                }
            }
            Ok(None) => {}
            Err(MeasRefusal::Stale(_)) => {
                chain.known = None;
                dropped = chain.queued.len();
                chain.queued.clear();
            }
            Err(MeasRefusal::Conflict(_) | MeasRefusal::Failed(_)) => {}
        }
        let event = match result {
            Ok(_) => ConnEvent::Reply {
                what,
                result: Ok(()),
            },
            Err(MeasRefusal::Stale(error) | MeasRefusal::Conflict(error)) => {
                ConnEvent::Conflict { what, error }
            }
            Err(MeasRefusal::Failed(e)) => ConnEvent::Reply {
                what,
                result: Err(e),
            },
        };
        self.conn_event(event, out);
        if dropped > 0 {
            let name = self
                .meas(meas)
                .map_or_else(String::new, |m| m.config.name.clone());
            self.notify(
                super::Severity::Warning,
                ac2_scene::osc::edits_dropped(&name, dropped),
                super::Muted::Never,
            );
        }
    }
}

/// Whether `cmd` carries a base taken from the measurement.
fn based(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::MeasUpdate { .. }
            | Command::MeasEq {
                edit: EqEdit::Put { .. },
                ..
            }
    )
}

/// `cmd`, built on `built_on`, made on `now` instead: the base is `now`'s, and a whole
/// configuration keeps what the edit changed and takes the rest from `now`, so an earlier
/// edit's change still in flight when this one was made is not undone.
fn rebase(cmd: Command, built_on: Option<&MeasConfig>, now: &Measurement) -> Command {
    match cmd {
        Command::MeasUpdate { meas, config, .. } => Command::MeasUpdate {
            meas,
            base: now.edit_rev,
            config: built_on.map_or(config.clone(), |b| merge(b, &config, &now.config)),
        },
        Command::MeasEq {
            meas,
            edit: EqEdit::Put { set, base, body },
        } => Command::MeasEq {
            meas,
            edit: EqEdit::Put {
                set,
                base: now.eq.iter().find(|s| s.id == set).map_or(base, |s| s.rev),
                body,
            },
        },
        other => other,
    }
}

/// `now` with what changed from `old` to `new`.
fn merge(old: &MeasConfig, new: &MeasConfig, now: &MeasConfig) -> MeasConfig {
    let (Ok(o), Ok(n), Ok(c)) = (
        serde_json::to_value(old),
        serde_json::to_value(new),
        serde_json::to_value(now),
    ) else {
        return new.clone();
    };
    serde_json::from_value(merge_value(&o, &n, &c)).unwrap_or_else(|_| new.clone())
}

/// Field by field where all three are objects of one variant (a tagged enum's `type` the
/// same); anywhere else the edited value whole, when the edit changed it.
fn merge_value(old: &Value, new: &Value, now: &Value) -> Value {
    if old == new {
        return now.clone();
    }
    match (old, new, now) {
        (Value::Object(o), Value::Object(n), Value::Object(c))
            if o.get("type") == n.get("type") && n.get("type") == c.get("type") =>
        {
            let mut out = c.clone();
            for (k, nv) in n {
                let merged = match (o.get(k), c.get(k)) {
                    (Some(ov), Some(cv)) => merge_value(ov, nv, cv),
                    _ => nv.clone(),
                };
                out.insert(k.clone(), merged);
            }
            out.retain(|k, _| n.contains_key(k) || !o.contains_key(k));
            Value::Object(out)
        }
        _ => new.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_merge_keeps_the_other_edits_change() {
        let old = json!({"name": "a", "kind": {"type": "spl", "x": 1, "y": 1}});
        let new = json!({"name": "a", "kind": {"type": "spl", "x": 2, "y": 1}});
        let now = json!({"name": "b", "kind": {"type": "spl", "x": 1, "y": 3}});
        assert_eq!(
            merge_value(&old, &new, &now),
            json!({"name": "b", "kind": {"type": "spl", "x": 2, "y": 3}})
        );
        // A variant changed is taken whole.
        let other = json!({"name": "a", "kind": {"type": "rta", "z": 0}});
        assert_eq!(
            merge_value(&old, &other, &now),
            json!({"name": "b", "kind": {"type": "rta", "z": 0}})
        );
    }
}
