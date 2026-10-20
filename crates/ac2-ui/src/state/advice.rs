//! Feeds each running transfer measurement's headroom advice ([`ac2_scene::advice`]) from
//! what already arrives: the session's input meters (reference peak, measurement RMS), the
//! transfer frame (coherence, protection flags) and the mirrored generator (level and the
//! system max level). Reads only; no level is ever changed from here.

use super::*;

use ac2_proto::FrameData;
use ac2_proto::topic::{Stream, Topic};
use ac2_scene::advice::{Reading, Stimulus, pane_coherence};

impl AppState {
    /// One step of every running transfer measurement's advice with the newest data; the
    /// advice of a measurement that stopped or went away is dropped.
    pub(super) fn fold_advice(&mut self, d: &DataSnapshot) {
        let settings = self.prefs.advice;
        let meters = d
            .latest
            .get(&Topic::SessionLevels)
            .filter(|f| !f.stale)
            .and_then(|f| match &f.frame.data {
                FrameData::SessionLevels(l) => Some(l),
                _ => None,
            });
        let level = |input: u16, of: fn(&ac2_proto::frame::SessionLevelsFrame) -> &[f32]| {
            let l = meters?;
            let i = l.meta.channels.iter().position(|c| *c == input)?;
            of(l)
                .get(i)
                .map(|v| f64::from(*v))
                .filter(|v| v.is_finite())
        };
        // The stimulus the max level bounds: this rig's generator while it plays.
        let stimulus = self
            .daemon()
            .map(|s| &s.generator)
            .filter(|g| g.firing)
            .and_then(|g| {
                g.settings.as_ref().map(|s| Stimulus {
                    level_dbfs: s.level.0,
                    max_level_dbfs: g.ceiling.0,
                })
            });
        let mut readings: Vec<(MeasId, Reading)> = Vec::new();
        for m in self.measurements() {
            let MeasKind::Transfer { config } = &m.config.kind else {
                continue;
            };
            if !m.running {
                continue;
            }
            let topic = Topic::Data {
                meas: m.id,
                stream: Stream::Tf,
            };
            let Some(tf) = d.latest.get(&topic).filter(|f| !f.stale) else {
                continue;
            };
            let FrameData::Tf(f) = &tf.frame.data else {
                continue;
            };
            readings.push((
                m.id,
                Reading {
                    protection: tf.frame.stamp.protection,
                    ref_peak_dbfs: level(config.reference_input, |l| &l.peak),
                    meas_rms_dbfs: level(config.measurement_input, |l| &l.rms),
                    coherence: pane_coherence(&f.coh, &f.validity),
                    stimulus,
                    // Any SPL calibration applied to the input, verified or borrowed: its
                    // readings rely on the gain staying as it is.
                    meas_calibrated: self.daemon().is_some_and(|s| {
                        ac2_proto::cal::state_input_use(s, config.measurement_input)
                            .sensitivity
                            .entry()
                            .is_some()
                    }),
                },
            ));
        }
        self.advice
            .retain(|id, _| readings.iter().any(|(m, _)| m == id));
        let now = self.now_s;
        for (id, r) in readings {
            self.advice.entry(id).or_default().step(now, &r, &settings);
        }
    }

    /// The headroom advice to show for `m` on a transfer pane.
    pub fn advice_of(&self, m: MeasId) -> Option<ac2_scene::advice::Advice> {
        self.advice.get(&m)?.shown()
    }
}
