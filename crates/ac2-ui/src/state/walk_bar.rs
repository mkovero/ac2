//! The walk pane's readiness bar glides from one transfer frame's `filled` to the next, as
//! the Leq bars ease between their frames ([`ac2_scene::bar_ease`]): `filled` counts the
//! averages' blocks, so drawn as received the bar jumps in the stages' coarse steps.

use super::*;

use std::time::Instant;

use ac2_proto::FrameData;
use ac2_proto::topic::{Stream, Topic};

/// A transfer topic's readiness bar gliding to its newest frame's `filled`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillGlide {
    /// `seq` of the frame it glides to; a glide to any other frame is over.
    pub to_seq: u64,
    /// The fill drawn when that frame was taken.
    pub from: f32,
    /// When the frame was taken.
    pub start: Instant,
}

fn filled(f: &ac2_proto::Frame) -> Option<f32> {
    match &f.data {
        FrameData::Tf(t) => Some(t.meta.filled),
        _ => None,
    }
}

impl FillGlide {
    /// The fill drawn at `now` for frame `to`; `None` when it is drawn as received (another
    /// frame, or the glide is over).
    pub fn drawn(&self, to: &ac2_proto::Frame, now: Instant) -> Option<f32> {
        let since = now.saturating_duration_since(self.start).as_secs_f64();
        if to.stamp.seq != self.to_seq || !ac2_scene::bar_ease::easing(since) {
            return None;
        }
        let e =
            ac2_scene::bar_ease::eased(&[f64::from(self.from)], &[f64::from(filled(to)?)], since);
        Some(e[0] as f32)
    }
}

impl RigState {
    /// Starts each transfer topic's bar glide to a new frame from where the bar is drawn now.
    /// A frame from another session, daemon or configuration is not the same averages
    /// filling: it is drawn as received.
    pub(super) fn keep_fill_glide(&mut self, d: &DataSnapshot) {
        let old = self.data.as_deref();
        self.fill_glide.retain(|t, _| d.latest.get(t).is_some());
        for tf in d.latest.frames.values() {
            let Topic::Data {
                stream: Stream::Tf, ..
            } = tf.topic
            else {
                continue;
            };
            let Some(prev) = old.and_then(|o| o.latest.get(&tf.topic)) else {
                self.fill_glide.remove(&tf.topic);
                continue;
            };
            let (a, b) = (&prev.frame.stamp, &tf.frame.stamp);
            if a.seq == b.seq {
                continue;
            }
            let same = a.session_epoch == b.session_epoch
                && a.daemon_incarnation == b.daemon_incarnation
                && a.config_rev == b.config_rev;
            let (Some(p), Some(to)) = (filled(&prev.frame), filled(&tf.frame)) else {
                continue;
            };
            if !same {
                self.fill_glide.remove(&tf.topic);
                continue;
            }
            // Mid-glide, the bar moves on from where it is drawn, so it never jumps.
            let from = self
                .fill_glide
                .get(&tf.topic)
                .and_then(|g| g.drawn(&prev.frame, d.drained))
                .unwrap_or(p);
            if from == to {
                self.fill_glide.remove(&tf.topic);
                continue;
            }
            self.fill_glide.insert(
                tf.topic,
                FillGlide {
                    to_seq: b.seq,
                    from,
                    start: d.drained,
                },
            );
        }
    }
}

impl AppState {
    /// The readiness bar of walk measurement `meas` as drawn at `now`; `None`: drawn at its
    /// frame's value (no glide, the glide is over, or the operator turned bar easing off).
    pub fn walk_bar_drawn(&self, meas: MeasId, now: Instant) -> Option<f32> {
        if !self.prefs.bar_ease {
            return None;
        }
        let topic = Topic::Data {
            meas,
            stream: Stream::Tf,
        };
        let g = self.rig.fill_glide.get(&topic)?;
        let tf = self.rig.data.as_ref()?.latest.get(&topic)?;
        g.drawn(&tf.frame, now)
    }

    /// Some walk's readiness bar still glides at `now`.
    pub fn walk_bar_easing(&self, now: Instant) -> bool {
        self.walks()
            .iter()
            .flat_map(|w| &w.meas)
            .any(|&m| self.walk_bar_drawn(m, now).is_some())
    }
}
