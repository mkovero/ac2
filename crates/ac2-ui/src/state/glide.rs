//! A live spectrum or RTA curve slower than the display eases into its newest frame over
//! one display frame ([`ac2_scene::curve_ease`]): what it eases from, and where it has
//! got to. Frames at the display's rate are drawn as received.

use super::*;

use std::time::Instant;

use ac2_scene::curve_ease;

/// A live curve's ease into its newest frame.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecGlide {
    /// `seq` of the frame it glides to; a glide to any other frame is over.
    pub to_seq: u64,
    /// The levels drawn when the frame was taken, dB per column.
    pub from: Arc<[f32]>,
    /// When the frame was taken.
    pub start: Instant,
    /// How long it lasts, s ([`curve_ease::EASE_S`]).
    pub duration_s: f64,
    /// The capture time `from` stands for, ns on the daemon's wall clock: the frame before's
    /// `capture_wall_ns`, or part way past it when a glide was cut short. A curve drawn `k`
    /// of the way stands for `from + k (to − from)` in time as in level, which is how far
    /// the picture trails the newest frame.
    pub from_capture_ns: f64,
}

/// A live spectrum or RTA frame's levels, dB per column.
fn levels(d: &ac2_proto::FrameData) -> Option<&[f32]> {
    match d {
        ac2_proto::FrameData::Spec(f) => Some(&f.level),
        ac2_proto::FrameData::Rta(f) => Some(&f.level),
        _ => None,
    }
}

impl SpecGlide {
    /// The levels drawn `now` for the frame `to`; `None` when the curve is drawn as received
    /// (another frame, or the glide is over).
    pub fn drawn(&self, to: &ac2_proto::Frame, now: Instant) -> Option<Vec<f32>> {
        if to.stamp.seq != self.to_seq {
            return None;
        }
        let k = curve_ease::progress(
            now.saturating_duration_since(self.start).as_secs_f64(),
            self.duration_s,
        )?;
        Some(curve_ease::eased(&self.from, levels(&to.data)?, k))
    }

    /// How far the glide to `to` has got at `now`, 0–1; `None` when `to` is another frame.
    /// An ended glide is 1: the curve is drawn as received.
    pub fn progress(&self, to: &ac2_proto::FrameStamp, now: Instant) -> Option<f64> {
        (to.seq == self.to_seq).then(|| {
            curve_ease::progress(
                now.saturating_duration_since(self.start).as_secs_f64(),
                self.duration_s,
            )
            .unwrap_or(1.0)
        })
    }

    /// The capture time the curve drawn at `now` stands for, ns on the daemon's wall clock
    /// ([`Self::from_capture_ns`]); `to`'s own when the glide is to another frame.
    pub fn drawn_capture_ns(&self, to: &ac2_proto::FrameStamp, now: Instant) -> f64 {
        let at = to.capture_wall_ns.0 as f64;
        self.progress(to, now).map_or(at, |k| {
            self.from_capture_ns + k * (at - self.from_capture_ns)
        })
    }
}

impl AppState {
    /// Some live curve still eases into its newest frame at `now`.
    pub fn spectrum_gliding(&self, now: Instant) -> bool {
        self.prefs.spec_ease
            && self.rig.spec_glide.iter().any(|(t, g)| {
                self.rig
                    .data
                    .as_ref()
                    .and_then(|d| d.latest.get(t))
                    .is_some_and(|tf| {
                        tf.frame.stamp.seq == g.to_seq
                            && curve_ease::progress(
                                now.saturating_duration_since(g.start).as_secs_f64(),
                                g.duration_s,
                            )
                            .is_some()
                    })
            })
    }
}

impl RigState {
    /// Starts each live curve's ease into a new frame from where it is drawn now, when the
    /// frame came slower than the display ([`curve_ease::eases`]); a faster one is drawn as
    /// received. A frame from another session, daemon, configuration or grid is not the
    /// same curve moving: it is drawn as received.
    pub(super) fn keep_spec_glide(&mut self, d: &DataSnapshot, spec_ease: bool) {
        if !spec_ease {
            self.spec_glide.clear();
            return;
        }
        let old = self.data.as_deref();
        self.spec_glide.retain(|t, _| d.latest.get(t).is_some());
        for tf in d.latest.frames.values() {
            let Some(to) = levels(&tf.frame.data) else {
                continue;
            };
            let Some(prev) = old.and_then(|o| o.latest.get(&tf.topic)) else {
                self.spec_glide.remove(&tf.topic);
                continue;
            };
            let (a, b) = (&prev.frame.stamp, &tf.frame.stamp);
            if a.seq == b.seq {
                continue;
            }
            let same = a.session_epoch == b.session_epoch
                && a.daemon_incarnation == b.daemon_incarnation
                && a.config_rev == b.config_rev
                && a.grid_id == b.grid_id;
            let slow = curve_ease::eases(tf.received.saturating_duration_since(prev.received));
            let Some(prev_levels) =
                levels(&prev.frame.data).filter(|p| same && slow && p.len() == to.len())
            else {
                self.spec_glide.remove(&tf.topic);
                continue;
            };
            // Mid-ease, the curve moves on from where it is drawn, so it never jumps.
            let from_capture_ns = self
                .spec_glide
                .get(&tf.topic)
                .map_or(a.capture_wall_ns.0 as f64, |g| {
                    g.drawn_capture_ns(a, d.drained)
                });
            let from = self
                .spec_glide
                .get(&tf.topic)
                .and_then(|g| g.drawn(&prev.frame, d.drained))
                .unwrap_or_else(|| prev_levels.to_vec());
            if from.iter().zip(to).all(|(x, y)| x.to_bits() == y.to_bits()) {
                // A refresh of a settled result: nothing moves.
                self.spec_glide.remove(&tf.topic);
                continue;
            }
            self.spec_glide.insert(
                tf.topic,
                SpecGlide {
                    to_seq: b.seq,
                    from: from.into(),
                    start: d.drained,
                    duration_s: curve_ease::EASE_S,
                    from_capture_ns,
                },
            );
        }
    }
}
