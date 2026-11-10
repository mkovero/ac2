//! Spectrum panes' level axes following what they show ([`ac2_scene::view::follow`]).
//!
//! Only an automatic range follows (its slot not in `level_set`): a range the operator set
//! stays until a fit (Shift+Home, a double-tap) makes it automatic again. A followed range is
//! as automatic as a framed one: never sent, never saved. Same-unit automatic panes on screen
//! follow as one group with one range (under the shared-scale setting), so side-by-side panes
//! never end up a step apart. Each frame's arrival feeds the pane ([`AppState::feed_follow`],
//! with the frame's rig in `rig`); each tick decides and eases ([`AppState::step_follow`]).

use std::collections::{BTreeMap, BTreeSet};

use ac2_proto::FrameData;
use ac2_proto::frame::ValidityMask;
use ac2_proto::model::LevelScale;
use ac2_proto::units::{DaemonIncarnation, MeasId, SessionEpoch};
use ac2_scene::axis::Range;
use ac2_scene::view::follow::{EASE_S, Ease, Follow, HANDS_OFF_S, SHADE_S, Sample};
use ac2_scene::view::level;

use super::levels::LevelSlot;
use crate::state::{AppState, Cam3dMsg, Msg, PaneId, PaneKind};

/// Two taps this close in time on one pane are a double-tap, s: a fingertip on a touch
/// panel is slower than a mouse's double click.
pub const DOUBLE_TAP_S: f64 = 0.4;
/// … and this close in place, logical px: a fingertip drifts further than a mouse.
pub const DOUBLE_TAP_PX: f32 = 24.0;

/// What a pane's live curves last gave the follower.
#[derive(Clone, Debug, PartialEq)]
struct Feed {
    scale: LevelScale,
    /// Per live curve, the signal history its frames belong to and the newest frame taken.
    curves: BTreeMap<MeasId, (SessionEpoch, DaemonIncarnation, u64)>,
    sample: Option<Sample>,
    /// When the newest frame arrived (the app's clock).
    at: f64,
    /// A frame not yet given to the follower.
    fresh: bool,
    /// A curve's rig restarted its session or daemon: the history so far is another signal's.
    reset: bool,
}

/// Which panes share a follower: one per unit when same-unit panes share a scale, else one
/// per pane and unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    pane: Option<PaneId>,
    spl: bool,
}

#[derive(Clone, Debug, Default)]
struct Follower {
    follow: Follow,
    members: BTreeSet<PaneId>,
    /// The last move, and whether its range is written in full.
    ease: Option<(Ease, bool)>,
}

/// A tap on a pane, kept to tell the next one a double-tap; with the cursor as it was before
/// it, which the tap moved and a double-tap puts back.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tap {
    pane: PaneId,
    x: f32,
    y: f32,
    at: f64,
    cursor_hz: Option<f64>,
    cursor_s: Option<f64>,
}

/// The followers and what feeds them.
#[derive(Clone, Debug, Default)]
pub(in crate::state) struct LevelFollow {
    feeds: BTreeMap<PaneId, Feed>,
    followers: BTreeMap<Key, Follower>,
    /// The last key or pointer input on each pane (the app's clock).
    input: BTreeMap<PaneId, f64>,
    /// Panes a pointer button is down on.
    held: BTreeSet<PaneId>,
    /// Panes whose group frames again on the next tick (`true`: its history clears too).
    refit: BTreeMap<PaneId, bool>,
    tap: Option<Tap>,
    /// The last tap was the second of a double-tap.
    double: bool,
}

/// The levels of `frame` as drawn, its columns at `freqs`: offset by `offset`, NaN where a
/// column holds no valid value (flagged invalid, or finer than a multi-resolution
/// spectrum's window resolves, where neighbouring columns share one bin).
fn drawn_levels(data: &FrameData, freqs: &[f64], offset: f64) -> Option<Vec<f64>> {
    let (level, validity, unresolved) = match data {
        FrameData::Spec(f) => (
            &f.level,
            None,
            f.meta.bands.as_deref().map(|b| &b.unresolved),
        ),
        FrameData::Rta(f) => (&f.level, Some(f.validity.as_slice()), None),
        _ => return None,
    };
    Some(
        level
            .iter()
            .zip(freqs)
            .enumerate()
            .map(|(i, (l, f))| {
                let ok =
                    validity.is_none_or(|v| v.get(i).is_some_and(|m| *m == ValidityMask::NONE));
                let resolved = unresolved
                    .is_none_or(|u| !u.ranges.iter().any(|r| *f >= r.lo.0 && *f <= r.hi.0));
                if ok && resolved {
                    f64::from(*l) + offset
                } else {
                    f64::NAN
                }
            })
            .collect(),
    )
}

impl AppState {
    /// A frame is in (the rig it came from in `rig`): each spectrum pane of that rig drawn
    /// now takes its live curves' newest frames, its own and compared ones, as drawn. Stored
    /// traces never drift and the peak hold only rises: neither counts.
    pub(in crate::state) fn feed_follow(&mut self) {
        for pane in self.own_visible_panes() {
            if self.layout.kind(pane) != PaneKind::Spectrum {
                self.level_follow.feeds.remove(&pane);
                continue;
            }
            let scale = crate::scenes::spectrum_scale(self, pane);
            let shown = self.pane_meas(pane);
            let mut curves = BTreeMap::new();
            let mut drawn: Vec<(Vec<f64>, Vec<f64>)> = Vec::new();
            for m in self.measurements() {
                if !(self.live_on_pane(PaneKind::Spectrum, m, shown)
                    || self.live_compared_on(PaneKind::Spectrum, m, pane))
                {
                    continue;
                }
                let Some(stream) = crate::state::spectrum_stream(m) else {
                    continue;
                };
                let Some(tf) = crate::scenes::frame(self, m.id, stream) else {
                    continue;
                };
                let Some(grid) = (tf.frame.stamp.grid_id)
                    .and_then(|g| self.rig.data.as_ref()?.grids.get(&g).cloned())
                else {
                    continue;
                };
                let freqs = grid.centres();
                let Some(levels) = drawn_levels(&tf.frame.data, &freqs, self.meas_offset(m.id))
                else {
                    continue;
                };
                let s = &tf.frame.stamp;
                curves.insert(m.id, (s.session_epoch, s.daemon_incarnation, s.seq));
                drawn.push((freqs, levels));
            }
            let sample = Sample::of(drawn.iter().map(|(f, l)| (&f[..], &l[..])));
            let now = self.now_s;
            let feed = self.level_follow.feeds.entry(pane).or_insert(Feed {
                scale,
                curves: BTreeMap::new(),
                sample: None,
                at: now,
                fresh: false,
                reset: false,
            });
            let mut fresh = false;
            for (id, (epoch, inc, seq)) in &curves {
                match feed.curves.get(id) {
                    Some((e, i, _)) if (e, i) != (epoch, inc) => {
                        feed.reset = true;
                        fresh = true;
                    }
                    Some((_, _, s)) if s == seq => {}
                    _ => fresh = true,
                }
            }
            feed.curves = curves;
            if feed.scale != scale {
                // Another unit is another slot: its history is its own.
                feed.scale = scale;
                feed.reset = true;
            }
            if fresh {
                feed.sample = sample;
                feed.at = now;
                feed.fresh = sample.is_some();
            }
        }
    }

    /// Pane `pane` was framed for what it shows (`clear`: a new signal: another measurement,
    /// a new run): its group frames again over all its members on the next tick.
    pub(in crate::state) fn follow_refit(&mut self, pane: PaneId, clear: bool) {
        let r = self.level_follow.refit.entry(pane).or_default();
        *r |= clear;
    }

    /// The pane message `msg` is the operator's input on: `Some(None)` the focused one (a
    /// key acts on it; a frequency gesture comes after the click that focused it).
    pub(in crate::state) fn worked_on(&self, msg: &Msg) -> Option<Option<PaneId>> {
        match msg {
            Msg::Key(_)
            | Msg::Command(_)
            | Msg::Zoom { .. }
            | Msg::Pan { .. }
            | Msg::CursorAt(_)
            | Msg::SpectrographCursor { .. }
            | Msg::IrNav(..) => Some(None),
            Msg::LevelZoom { pane, .. }
            | Msg::LevelPan { pane, .. }
            | Msg::FocusPane(pane)
            | Msg::LegendPick(pane, _)
            | Msg::LeqStrip(pane, _) => Some(Some(*pane)),
            Msg::Cam3d(pane, m) if !matches!(m, Cam3dMsg::Hover(_)) => Some(Some(*pane)),
            _ => None,
        }
    }

    /// A key or pointer input on pane `pane`: its group holds still for a while.
    pub(in crate::state) fn pane_input(&mut self, pane: PaneId) {
        self.level_follow.input.insert(pane, self.now_s);
    }

    /// A pointer button went down on pane `pane` (`held`) or came up.
    pub(in crate::state) fn pane_pointer(&mut self, pane: PaneId, held: bool) {
        self.pane_input(pane);
        if held {
            self.level_follow.held.insert(pane);
        } else {
            self.level_follow.held.remove(&pane);
        }
    }

    /// A tap (a click) on pane `pane`'s plot at `x`, `y` (logical px), outside its legend
    /// and plates. The second of two within [`DOUBLE_TAP_S`] and [`DOUBLE_TAP_PX`] fits the
    /// pane's level axis (frequency untouched; a following spectrum stays automatic), no
    /// toast, and puts back the cursor the first tap moved: a kiosk is left with no stray
    /// cursor.
    pub(in crate::state) fn pane_tap(&mut self, pane: PaneId, x: f32, y: f32) {
        self.pane_input(pane);
        let now = self.now_s;
        let first = self.level_follow.tap.take().filter(|t| {
            t.pane == pane
                && now - t.at <= DOUBLE_TAP_S
                && (t.x - x).hypot(t.y - y) <= DOUBLE_TAP_PX
        });
        self.level_follow.double = first.is_some();
        let Some(first) = first else {
            self.level_follow.tap = Some(Tap {
                pane,
                x,
                y,
                at: now,
                cursor_hz: self.view.cursor_hz,
                cursor_s: self.view.spectrum.spectrograph.cursor_s,
            });
            return;
        };
        self.view.cursor_hz = first.cursor_hz;
        self.view.spectrum.spectrograph.cursor_s = first.cursor_s;
        if !self.draws_level_axis(pane) {
            return;
        }
        if let Some(fit) = level::fit(self.level_values(pane)) {
            self.fit_level(pane, fit);
        }
    }

    /// The last tap was a double-tap: the view leaves that click at that (no focus, no
    /// cursor).
    pub fn tap_was_double(&self) -> bool {
        self.level_follow.double
    }

    /// How strongly pane `pane`'s level gutter is shaded, 0 to 1: during a follow move and
    /// fading after it.
    pub fn follow_shade(&self, pane: PaneId) -> f32 {
        self.level_follow
            .followers
            .values()
            .filter(|f| f.members.contains(&pane))
            .filter_map(|f| f.ease.map(|(e, _)| e.shade(self.now_s)))
            .fold(0.0, f32::max)
    }

    /// A follow move is under way or its shade still fading: the spectrum panes redraw.
    pub fn levels_following(&self) -> bool {
        self.level_follow
            .followers
            .values()
            .any(|f| f.ease.is_some())
    }

    /// The range pane `pane` draws in `scale`.
    fn slot_range(&self, pane: PaneId, scale: LevelScale) -> Option<Range> {
        LevelSlot::of(PaneKind::Spectrum, scale).map(|s| s.get(&self.pane_levels(pane)))
    }

    fn set_slot_range(&mut self, pane: PaneId, scale: LevelScale, r: Range) {
        if let Some(s) = LevelSlot::of(PaneKind::Spectrum, scale) {
            s.set(self.pane_levels.entry(pane).or_default(), r);
        }
    }

    /// Every level pane `pane` draws, read with its rig in `rig`.
    fn level_values_of(&mut self, pane: PaneId) -> Vec<f64> {
        let Some(rig) = self.layout.view(pane).map(|v| self.pane_rig(v)) else {
            return Vec::new();
        };
        self.visit(rig, |st| st.level_values(pane))
            .unwrap_or_default()
    }

    /// Once a tick (the clock at `now_s`): the groups form, a group a member joined frames
    /// over all of them, new frames go to each group's follower, and each group that may
    /// move decides and eases its one range onto every member.
    pub(in crate::state) fn step_follow(&mut self) {
        let now = self.now_s;
        let visible = self.visible_panes();
        let kinds: BTreeMap<PaneId, PaneKind> =
            visible.iter().map(|p| (*p, self.layout.kind(*p))).collect();
        let lf = &mut self.level_follow;
        lf.feeds
            .retain(|p, _| kinds.get(p).is_some_and(|k| *k == PaneKind::Spectrum));
        lf.input.retain(|_, t| now - *t < HANDS_OFF_S);
        lf.held.retain(|p| kinds.contains_key(p));
        let mut groups: BTreeMap<Key, Vec<PaneId>> = BTreeMap::new();
        if self.prefs.level_follow {
            for (p, feed) in &self.level_follow.feeds {
                let auto = LevelSlot::of(PaneKind::Spectrum, feed.scale)
                    .is_some_and(|s| !self.level_set.contains(&(*p, s)));
                if !auto || feed.sample.is_none() {
                    continue;
                }
                let spl = feed.scale == LevelScale::DbSpl;
                let key = Key {
                    pane: (!self.prefs.shared_scale).then_some(*p),
                    spl,
                };
                groups.entry(key).or_default().push(*p);
            }
        }
        self.level_follow
            .followers
            .retain(|k, _| groups.contains_key(k));
        let refit = std::mem::take(&mut self.level_follow.refit);
        for (key, members) in groups {
            self.step_group(key, &members, &refit, now);
        }
        for feed in self.level_follow.feeds.values_mut() {
            feed.fresh = false;
            feed.reset = false;
        }
    }

    fn step_group(
        &mut self,
        key: Key,
        members: &[PaneId],
        refit: &BTreeMap<PaneId, bool>,
        now: f64,
    ) {
        let scale = if key.spl {
            LevelScale::DbSpl
        } else {
            LevelScale::Dbfs
        };
        let lf = &mut self.level_follow;
        let f = lf.followers.entry(key).or_default();
        let joined = members.iter().any(|p| !f.members.contains(p));
        let asked = members
            .iter()
            .filter_map(|p| refit.get(p))
            .copied()
            .reduce(|a, b| a || b);
        if members
            .iter()
            .any(|p| lf.feeds.get(p).is_some_and(|x| x.reset))
            || asked == Some(true)
        {
            f.follow.clear();
        }
        f.members = members.iter().copied().collect();
        // One range for the group: framed over all its members when one joins (a second
        // pane on screen, a pane turned automatic) or a member was framed; a pane alone keeps
        // the range its own first show framed.
        if members.len() > 1 && (joined || asked.is_some()) {
            let values: Vec<f64> = members
                .iter()
                .flat_map(|p| self.level_values_of(*p))
                .collect();
            if let Some(fit) = level::fit(values) {
                for p in members {
                    self.set_slot_range(*p, scale, fit);
                }
                let f = self.follower(key);
                f.ease = None;
                f.follow.moved(now);
            }
        } else if asked.is_some() {
            let f = self.follower(key);
            f.ease = None;
            f.follow.moved(now);
        }
        let lf = &mut self.level_follow;
        let fresh: Vec<&Feed> = members.iter().filter_map(|p| lf.feeds.get(p)).collect();
        let sample = fresh.iter().filter_map(|x| x.sample).reduce(Sample::with);
        let at = fresh
            .iter()
            .filter(|x| x.fresh)
            .map(|x| x.at)
            .reduce(f64::max);
        let hands_off = members
            .iter()
            .any(|p| lf.held.contains(p) || lf.input.get(p).is_some_and(|t| now - t < HANDS_OFF_S));
        let f = lf.followers.entry(key).or_default();
        if let (Some(s), Some(at)) = (sample, at) {
            f.follow.push(at, s);
        }
        match f.ease {
            Some((e, false)) => {
                let r = e.at(now);
                f.ease = Some((e, e.done(now)));
                for p in members {
                    self.set_slot_range(*p, scale, r);
                }
            }
            _ if hands_off || at.is_none() => {}
            _ => {
                let Some(current) = self.slot_range(members[0], scale) else {
                    return;
                };
                let f = self.follower(key);
                if let Some(m) = f.follow.decide(now, current) {
                    f.ease = Some((
                        Ease {
                            from: current,
                            to: m.to,
                            start_s: now,
                        },
                        false,
                    ));
                }
            }
        }
        // A shade long faded is no move in progress.
        if let Some((e, true)) = self.follower(key).ease
            && now - e.start_s > EASE_S + SHADE_S
        {
            self.follower(key).ease = None;
        }
    }

    fn follower(&mut self, key: Key) -> &mut Follower {
        self.level_follow.followers.entry(key).or_default()
    }
}
