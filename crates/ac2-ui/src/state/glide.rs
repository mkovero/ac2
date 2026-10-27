//! The live spectrum and RTA curves glide from one frame to the next
//! ([`ac2_scene::curve_ease`]): what each glides from, and where it has got to.

use super::*;

use std::time::Instant;

use ac2_scene::curve_ease;

/// A live curve's glide to its newest frame.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecGlide {
    /// `seq` of the frame it glides to; a glide to any other frame is over.
    pub to_seq: u64,
    /// The levels drawn when the frame was taken, dB per column.
    pub from: Arc<[f32]>,
    /// When the frame was taken.
    pub start: Instant,
    /// How long it lasts, s: about the time since the frame before
    /// ([`curve_ease::Pacing`]).
    pub duration_s: f64,
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
}

impl AppState {
    /// Some live curve still glides to its newest frame at `now`.
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
    /// Starts each live curve's glide to a new frame from where it is drawn now. A frame
    /// from another session, daemon, configuration or grid is not the same curve moving: it
    /// is drawn as received.
    pub(super) fn keep_spec_glide(&mut self, d: &DataSnapshot, spec_ease: bool) {
        if !spec_ease {
            self.spec_glide.clear();
            self.spec_pacing.clear();
            return;
        }
        let old = self.data.as_deref();
        self.spec_glide.retain(|t, _| d.latest.get(t).is_some());
        self.spec_pacing.retain(|t, _| d.latest.get(t).is_some());
        for tf in d.latest.frames.values() {
            let Some(to) = levels(&tf.frame.data) else {
                continue;
            };
            let Some(prev) = old.and_then(|o| o.latest.get(&tf.topic)) else {
                self.spec_glide.remove(&tf.topic);
                self.spec_pacing.remove(&tf.topic);
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
            let Some(prev_levels) =
                levels(&prev.frame.data).filter(|p| same && p.len() == to.len())
            else {
                self.spec_glide.remove(&tf.topic);
                self.spec_pacing.remove(&tf.topic);
                continue;
            };
            // Mid-glide, the curve moves on from where it is drawn, so it never jumps.
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
            // A refresh carries the newest capture time, not its result's: it never gets
            // here (same levels), so it does not upset the pacing.
            let duration_s = self
                .spec_pacing
                .entry(tf.topic)
                .or_default()
                .glide_s(d.drained, b.capture_wall_ns.0)
                .unwrap_or_else(|| {
                    let interval = tf.received.saturating_duration_since(prev.received);
                    curve_ease::duration_s(interval.as_secs_f64())
                });
            self.spec_glide.insert(
                tf.topic,
                SpecGlide {
                    to_seq: b.seq,
                    from: from.into(),
                    start: d.drained,
                    duration_s,
                },
            );
        }
    }
}
