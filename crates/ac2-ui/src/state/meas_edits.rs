//! An edit made on a base waits for the edits of its measurement in flight. A `meas.update`
//! moves the measurement's `edit_rev` (only a change of its configuration does), and an EQ
//! edit its set's `rev`, so a `meas.update` (or an EQ `put`) made before the reply of the
//! one before it would be refused as stale: it waits for the replies of the edits before it
//! and goes on the token the newer (by the measurement's `rev`, moved by every commit) of
//! the last reply and the mirror carries. The edits that carry no base (colour, offset,
//! layout, the other EQ edits, start, stop) are last write wins and go at once, unless a
//! based edit waits ahead of them: the daemon executes one client's requests in the order
//! sent.
//!
//! The next edit is made on what this app will have made of the measurement: the newer of
//! the mirror and the last reply, with the configuration of the newest `meas.update` sent
//! or waiting ([`AppState::meas`] reads it). A step key pressed twice before the first
//! step's reply steps twice; a toggle pressed twice ends where it began. A waiting edit then
//! only takes the newer token, and only when what it replaces is still what it was made on
//! (nothing but this app's own edits changed it); otherwise it goes as made and the daemon
//! refuses it.
//!
//! A `stale_meas` refusal means another client or the daemon changed the configuration in
//! between: it is said, and the edits waiting behind it are dropped, as they were made on
//! what the operator saw before that change. So are the edits waiting when the link goes
//! down or another daemon answers: the ids they name may be another measurement's now. Each
//! edit dropped is answered to whatever waits for its reply, as a refusal would be.

use std::collections::{HashMap, VecDeque};

use ac2_proto::Command;
use ac2_proto::model::{EqEdit, MeasConfig, Measurement};
use ac2_proto::units::MeasId;

use super::{AppState, Overlay};
use crate::conn::{ConnEvent, Edited, MeasRefusal, Request, meas_edit_target};

#[derive(Debug, Default)]
pub struct MeasEdits {
    chains: HashMap<MeasId, Chain>,
    /// Moved when the link goes down or another daemon answers while edits are in flight
    /// (the connection is told, [`Request::EditLink`]): every edit is answered with the
    /// generation it was sent on, so an answer to an edit sent before is told apart from
    /// one to an edit sent since, in whatever order they arrive.
    link: u64,
}

#[derive(Debug, Default)]
struct Chain {
    /// Edits sent on the current link and not answered, oldest first: the configuration of
    /// each `meas.update`.
    sent: VecDeque<Option<MeasConfig>>,
    /// The measurement as the last answered edit committed it, while the mirror is behind.
    known: Option<Measurement>,
    queued: VecDeque<Queued>,
    /// What the next edit of the measurement is made on, while it differs from the mirror.
    view: Option<Measurement>,
}

#[derive(Debug)]
struct Queued {
    cmd: Command,
    what: String,
    /// The measurement as this app had it when the edit was made.
    built_on: Option<Measurement>,
}

/// Why an edit was not sent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Unsent {
    /// Another client or the daemon changed the measurement first.
    ChangedElsewhere,
    /// The link went down, or another daemon answers on it.
    LinkLost,
    /// This app may only view the rig.
    ViewOnly,
}

impl Unsent {
    fn words(self) -> String {
        match self {
            Self::ChangedElsewhere => "not sent: it changed elsewhere".into(),
            Self::LinkLost => "not sent: the link to the rig went down".into(),
            Self::ViewOnly => format!("not sent: {}", ac2_scene::top_bar::VIEW_ONLY_DETAIL),
        }
    }
}

impl MeasEdits {
    /// The link generation edits are sent on now.
    #[cfg(test)]
    pub(super) fn link(&self) -> u64 {
        self.link
    }

    /// Measurement `id` as the next edit of it is made on, when that is not the mirror's.
    pub(super) fn view(&self, id: MeasId) -> Option<&Measurement> {
        self.chains.get(&id)?.view.as_ref()
    }
}

impl Chain {
    /// The configuration the newest `meas.update` waiting or sent sets.
    fn intent(&self) -> Option<&MeasConfig> {
        let waiting = self.queued.iter().rev().find_map(|q| match &q.cmd {
            Command::MeasUpdate { config, .. } => Some(config),
            _ => None,
        });
        waiting.or_else(|| self.sent.iter().rev().find_map(Option::as_ref))
    }

    fn refresh(&mut self, mirror: Option<&Measurement>) {
        self.view = mirror.and_then(|m| {
            let known = self.known.as_ref().filter(|k| k.rev > m.rev);
            let intent = self.intent();
            if known.is_none() && intent.is_none_or(|c| *c == m.config) {
                return None;
            }
            let mut v = known.unwrap_or(m).clone();
            if let Some(c) = intent {
                v.config = c.clone();
            }
            Some(v)
        });
    }
}

impl AppState {
    /// Holds back each measurement edit in `out` while an earlier one of the same
    /// measurement is unanswered, and sends the next waiting edit of an answered one. `out`
    /// is what the rig in `rig` is asked.
    pub(super) fn chain_meas_edits(&mut self, out: &mut Vec<Request>) {
        let waiting: Vec<MeasId> = (self.rig.meas_edits.chains.iter())
            .filter(|(_, c)| !c.queued.is_empty())
            .map(|(id, _)| *id)
            .collect();
        if waiting.is_empty()
            && !out
                .iter()
                .any(|r| matches!(r, Request::Call { cmd, .. } if meas_edit_target(cmd).is_some()))
        {
            self.prune_meas_edits();
            return;
        }
        let mut sent = Vec::with_capacity(out.len());
        for id in waiting {
            self.send_next_edit(id, &mut sent);
        }
        for r in std::mem::take(out) {
            match r {
                Request::Call { cmd, what } if let Some(id) = meas_edit_target(&cmd) => {
                    let built_on = self.meas(id).cloned();
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
        let rig = &mut self.rig;
        let mirror = (rig.mirror.as_ref().and_then(|m| m.state.as_deref()))
            .and_then(|s| s.measurements.iter().find(|m| m.id == id));
        let Some(chain) = rig.meas_edits.chains.get_mut(&id) else {
            return;
        };
        while chain
            .queued
            .front()
            .is_some_and(|q| chain.sent.is_empty() || !based(&q.cmd))
        {
            let Some(q) = chain.queued.pop_front() else {
                return;
            };
            // The reply and the event of one commit come apart: the mirror may already hold
            // what the last reply said, and the chain then forgot the reply.
            let newest = [chain.known.as_ref(), mirror].into_iter().flatten();
            let cmd = made_on(newest, q.built_on.as_ref(), q.cmd);
            chain.sent.push_back(match &cmd {
                Command::MeasUpdate { config, .. } => Some(config.clone()),
                _ => None,
            });
            out.push(Request::Call { cmd, what: q.what });
        }
    }

    /// Forgets what the mirror now holds, and the measurements with nothing waiting; the
    /// rest take the mirror's news.
    fn prune_meas_edits(&mut self) {
        let mut chains = std::mem::take(&mut self.rig.meas_edits.chains);
        chains.retain(|id, c| {
            let mirror = self.rig.mirrored_meas(*id);
            if c.known
                .as_ref()
                .is_some_and(|k| mirror.is_none_or(|m| m.rev >= k.rev))
            {
                c.known = None;
            }
            c.refresh(mirror);
            !c.sent.is_empty() || c.known.is_some() || !c.queued.is_empty()
        });
        self.rig.meas_edits.chains = chains;
    }

    /// The answer to an edit of `meas` sent on link generation `link`: the chain moves on,
    /// then the answer is handled as the reply (or the `conflict`) of any other call. One
    /// to an edit sent before the link went down says nothing about the measurement as the
    /// daemon now holds it, and moves no chain.
    pub(super) fn meas_edited(
        &mut self,
        meas: MeasId,
        what: String,
        link: u64,
        result: Edited,
        out: &mut Vec<Request>,
    ) {
        let current = link == self.rig.meas_edits.link;
        let chain = self.rig.meas_edits.chains.entry(meas).or_default();
        let mut dropped = Vec::new();
        if current {
            chain.sent.pop_front();
            match &result {
                Ok(Some(m)) => {
                    if chain.known.as_ref().is_none_or(|k| k.rev < m.rev) {
                        chain.known = Some((**m).clone());
                    }
                }
                Ok(None) => {}
                Err(MeasRefusal::Stale(_)) => {
                    chain.known = None;
                    dropped.extend(chain.queued.drain(..));
                }
                Err(MeasRefusal::Conflict(_) | MeasRefusal::Failed(_)) => {}
            }
        }
        self.prune_meas_edits();
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
        if !dropped.is_empty() {
            let name = self
                .meas(meas)
                .map_or_else(String::new, |m| m.config.name.clone());
            self.notify(
                super::Severity::Warning,
                ac2_scene::osc::edits_dropped(&name, dropped.len()),
                super::Muted::Never,
            );
            for q in dropped {
                self.answer_unsent(&q.what, Unsent::ChangedElsewhere, out);
            }
        }
    }

    /// A new mirror: another daemon (`reborn`) drops the chains, as a lost link does;
    /// otherwise what it now holds is forgotten from them.
    pub(super) fn meas_mirrored(&mut self, reborn: bool, out: &mut Vec<Request>) {
        if reborn {
            self.drop_meas_edits(out);
        }
        self.prune_meas_edits();
    }

    /// The link went down or another daemon answers it: the edits waiting are not sent (the
    /// ids they name may be another measurement's now), and the answers to those in flight
    /// move nothing. Says how many were not sent.
    pub(super) fn drop_meas_edits(&mut self, out: &mut Vec<Request>) {
        let mut dropped = Vec::new();
        // Only an edit in flight can be answered late.
        let edits = &mut self.rig.meas_edits;
        if edits.chains.values().any(|c| !c.sent.is_empty()) {
            edits.link += 1;
            out.push(Request::EditLink(edits.link));
        }
        for c in self.rig.meas_edits.chains.values_mut() {
            c.sent.clear();
            c.known = None;
            dropped.extend(c.queued.drain(..));
        }
        self.prune_meas_edits();
        if dropped.is_empty() {
            return;
        }
        self.notify(
            super::Severity::Warning,
            ac2_scene::osc::edits_unsent_link(dropped.len()),
            super::Muted::Never,
        );
        for q in dropped {
            self.answer_unsent(&q.what, Unsent::LinkLost, out);
        }
    }

    /// Answers `what`, an edit not sent, to whatever waits for its reply, as a refusal
    /// would be: the layout queue sends the layout waiting behind it, the EQ editor takes
    /// the daemon's set again, and an SPL / Leq page whose Apply it was opens again with its
    /// edits. Nothing more is said here: what dropped the edit says so once for all.
    pub(super) fn answer_unsent(&mut self, what: &str, why: Unsent, out: &mut Vec<Request>) {
        if self.layout_unsent(what, out) {
            return;
        }
        let refused = Err(why.words());
        if self.eq_reply(what, &refused, out) {
            return;
        }
        if let Overlay::Settings(s) = &mut self.overlay {
            s.cal.reply(what, &refused);
        }
        if why == Unsent::ChangedElsewhere {
            self.leq_conflict(what, out);
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

/// `cmd`, made on `built_on`, on the newest of `newer` (the last reply of this app's edits,
/// the mirror) when that is newer than `built_on`: only when its base is `built_on`'s (a base
/// deliberately older stays, for the daemon to refuse) and what it replaces is as it was
/// made on (nothing but this app's own edits changed it). Otherwise as made.
fn made_on<'a>(
    newer: impl Iterator<Item = &'a Measurement>,
    built_on: Option<&Measurement>,
    cmd: Command,
) -> Command {
    let Some(b) = built_on else {
        return cmd;
    };
    let newest = newer.max_by_key(|k| k.rev).filter(|k| k.rev > b.rev);
    match cmd {
        Command::MeasUpdate { meas, base, config } => {
            let base = match newest {
                Some(k) if base == b.edit_rev && k.config == b.config => k.edit_rev,
                _ => base,
            };
            Command::MeasUpdate { meas, base, config }
        }
        Command::MeasEq {
            meas,
            edit: EqEdit::Put { set, base, body },
        } => {
            let was = b.eq.iter().find(|s| s.id == set);
            let now = newest.and_then(|k| k.eq.iter().find(|s| s.id == set));
            let base = match (was, now) {
                (Some(w), Some(n)) if w.rev == base && w.body == n.body => n.rev,
                _ => base,
            };
            Command::MeasEq {
                meas,
                edit: EqEdit::Put { set, base, body },
            }
        }
        other => other,
    }
}
