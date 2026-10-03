//! Calibration commands (`docs/design/q7-calibration.md`): sensitivity calibrations
//! (`cal.spl`, `cal.delete`), the mic library (`cal.curve_import`, `cal.curve_rename`,
//! `cal.curve_delete`), the input setup (`session.inputs`) and `cal.list`.
//!
//! Every change goes through [`Control::commit_store`]: the store file is written first
//! (nothing is committed when that fails), then the changed entities are committed and the
//! running jobs get their inputs' new calibration and curve.

use std::sync::Arc;

use ac2_core::mic_curve::{MicCurve, file_info};
use ac2_proto::cal;
use ac2_proto::model::{
    CalEntry, CalKey, CurveChoice, InputSetup, Mic, MicCurveId, MicCurveRef, SplCal,
};
use ac2_proto::units::{Blob, Db, DbSpl, Dbfs, Hz, Rev, WallNs};

use super::{
    Change, Control, ErrorCode, MAX_CAL_UNSETTLED_DB, Patch, ProtoError, ReplyBody, block_index,
    perr, upsert_inputs,
};
use crate::calstore::{self, Contents, Curves};
use crate::util::wall_ns;

/// `inputs` with `channel`'s mic name set to `mic`; a new mic starts with no curve chosen
/// (the old choice named another capsule's curve).
fn set_input_mic(inputs: &[InputSetup], channel: u16, mic: &str) -> Vec<InputSetup> {
    let mut row = cal::input_setup(inputs, channel);
    if row.mic.as_deref() != Some(mic) {
        row.mic = Some(mic.to_owned());
        row.curve = CurveChoice::NotChosen;
    }
    upsert_inputs(inputs, vec![row])
}

fn not_found(msg: String) -> ProtoError {
    perr(ErrorCode::NotFound, msg)
}

/// `mic`'s labels, `0°, 90°`.
fn labels(m: &Mic) -> String {
    m.curves
        .iter()
        .map(|c| c.label.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl Control {
    /// The store's contents as mirrored.
    pub(super) fn cal_contents(&self) -> Contents {
        let st = self.store.state();
        Contents {
            calibrations: st.calibrations.clone(),
            mics: st.mics.clone(),
            inputs: st.inputs.clone(),
        }
    }

    /// Persists `new` (with the only curve of a one-curve mic chosen where none is) and
    /// commits what changed, then hands the running jobs their new calibrations. Nothing is
    /// committed if the write fails.
    pub(super) fn commit_store(
        &mut self,
        mut new: Contents,
        curves: Curves,
    ) -> Result<Rev, ProtoError> {
        for r in &mut new.inputs {
            cal::settle(r, &new.mics);
        }
        let old = self.cal_contents();
        self.cal.persist(&new, curves)?;
        let mut changes = Vec::new();
        for e in &new.calibrations {
            if !old.calibrations.contains(e) {
                changes.push(Change::Calibration(Patch::Set(e.clone())));
            }
        }
        for e in &old.calibrations {
            if !new.calibrations.iter().any(|n| n.key == e.key) {
                changes.push(Change::Calibration(Patch::Deleted(e.key.clone())));
            }
        }
        for m in &new.mics {
            if !old.mics.contains(m) {
                changes.push(Change::Mic(Patch::Set(m.clone())));
            }
        }
        for m in &old.mics {
            if !new.mics.iter().any(|n| n.name == m.name) {
                changes.push(Change::Mic(Patch::Deleted(m.name.clone())));
            }
        }
        if new.inputs != old.inputs {
            changes.push(Change::Inputs(new.inputs));
        }
        let mut rev = self.store.rev();
        for c in changes {
            rev = self.commit(c);
        }
        self.refresh_cal();
        Ok(rev)
    }

    /// The open session's capture device, with `input` captured by it.
    fn cal_target(&self, input: u16) -> Result<ac2_proto::model::DeviceId, ProtoError> {
        let Some(rt) = self.session.as_ref() else {
            return Err(perr(
                ErrorCode::Invalid,
                "no open session: a calibration is tied to the session's capture device",
            ));
        };
        block_index(&rt.input_map, input)
            .ok_or_else(|| perr(ErrorCode::Invalid, format!("input {input} is not captured")))?;
        Ok(rt.open.input_device.clone())
    }

    pub(super) fn cal_spl(
        &mut self,
        input: u16,
        mic: String,
        calibrator_level: DbSpl,
        calibrator_freq: Hz,
    ) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        calstore::check_mic_name(&mic)?;
        if !(calibrator_level.0.is_finite()
            && calibrator_freq.0.is_finite()
            && calibrator_freq.0 > 0.0)
        {
            return Err(perr(
                ErrorCode::Invalid,
                "calibrator level and frequency must be finite, the frequency above 0 Hz",
            ));
        }
        let device = self.cal_target(input)?;
        let Some(rt) = self.session.as_ref() else {
            return Err(perr(ErrorCode::Invalid, "no open session"));
        };
        let idx = block_index(&rt.input_map, input)
            .ok_or_else(|| perr(ErrorCode::Invalid, format!("input {input} is not captured")))?;
        // Broadband and uncorrected: the mic curve is 0 dB at the calibrator frequency, so
        // the corrected paths read the calibrator the same (Q7 §3).
        let (ms, fast) = rt.fanout.meters.mean_square(idx).unwrap_or((0.0, 0.0));
        let measured = ac2_core::spectrum::rms_dbfs(ms.sqrt());
        if !(measured.is_finite() && measured > -80.0) {
            return Err(perr(
                ErrorCode::Refused,
                format!("no calibrator signal on input {input} ({measured:.1} dBFS)"),
            ));
        }
        // The 1 s mean square is within 0.05 dB of a steady level once the 0.2 s one agrees
        // with it that closely (about 5 s after the calibrator went on).
        let unsettled = 10.0 * (fast / ms).log10();
        if unsettled.is_nan() || unsettled.abs() > MAX_CAL_UNSETTLED_DB {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "the calibrator level on input {input} is not steady yet ({unsettled:+.2} dB \
                     over the last second); keep it on and retry in a few seconds"
                ),
            ));
        }
        let entry = CalEntry {
            key: CalKey {
                device,
                channel: input,
                mic: mic.clone(),
            },
            spl: SplCal {
                sensitivity: Db(calibrator_level.0 - measured),
                calibrator_level,
                calibrator_freq,
                measured: Dbfs(measured),
                calibrated_at: WallNs(wall_ns()),
            },
        };
        let mut new = self.cal_contents();
        new.calibrations.retain(|e| e.key != entry.key);
        new.calibrations.push(entry.clone());
        new.inputs = set_input_mic(&new.inputs, input, &mic);
        self.commit_store(new, self.cal.curves())?;
        tracing::info!(
            "input {input} ({mic}) calibrated: {measured:.2} dBFS at {:.1} dB SPL",
            calibrator_level.0
        );
        Ok(ReplyBody::Calibration(entry))
    }

    pub(super) fn cal_curve_import(
        &mut self,
        mic: String,
        label: Option<String>,
        file_name: String,
        content: Blob,
        input: Option<u16>,
    ) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        calstore::check_mic_name(&mic)?;
        if let (Some(i), Some(_)) = (input, self.session.as_ref()) {
            self.cal_target(i)?;
        }
        let base = file_name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_owned();
        if base.is_empty() || base.chars().count() > 255 {
            return Err(perr(
                ErrorCode::Invalid,
                "file name must be 1 … 255 characters",
            ));
        }
        let curve = MicCurve::parse(&content.0).map_err(calstore::curve_error)?;
        let info = file_info(&content.0, &base);
        let label = label.unwrap_or_else(|| cal::default_label(info.angle_deg, &base));
        calstore::check_label(&label)?;
        let reference = MicCurveRef {
            label: label.clone(),
            file_name: base.clone(),
            content_hash: calstore::content_hash(&content.0),
            points: u32::try_from(curve.len()).unwrap_or(u32::MAX),
            f_lo: Hz(curve.f_lo()),
            f_hi: Hz(curve.f_hi()),
            imported_at: WallNs(wall_ns()),
            stated_sensitivity: info.stated_sensitivity_mv_per_pa,
        };
        let mut new = self.cal_contents();
        let replaced = match new.mics.iter_mut().find(|m| m.name == mic) {
            Some(m) => match m.curves.iter_mut().find(|c| c.label == label) {
                Some(c) => {
                    *c = reference;
                    true
                }
                None => {
                    m.curves.push(reference);
                    false
                }
            },
            None => {
                new.mics.push(Mic {
                    name: mic.clone(),
                    curves: vec![reference],
                });
                false
            }
        };
        let mut curves = self.cal.curves();
        curves.insert(
            MicCurveId {
                mic: mic.clone(),
                label: label.clone(),
            },
            Arc::new(curve),
        );
        let only = new
            .mics
            .iter()
            .find(|m| m.name == mic)
            .is_some_and(|m| m.curves.len() == 1);
        if let Some(i) = input {
            new.inputs = set_input_mic(&new.inputs, i, &mic);
            // Importing a mic's only curve on an input is choosing it there.
            if only {
                new.inputs = upsert_inputs(
                    &new.inputs,
                    vec![InputSetup {
                        channel: i,
                        mic: Some(mic.clone()),
                        curve: CurveChoice::Curve {
                            label: label.clone(),
                        },
                    }],
                );
            }
        }
        let m = new
            .mics
            .iter()
            .find(|m| m.name == mic)
            .cloned()
            .ok_or_else(|| perr(ErrorCode::Internal, "imported mic missing"))?;
        self.commit_store(new, curves)?;
        tracing::info!(
            "mic curve {label:?} of {mic} {} from {base}",
            if replaced { "replaced" } else { "imported" }
        );
        Ok(ReplyBody::Mic(m))
    }

    pub(super) fn cal_curve_rename(
        &mut self,
        id: MicCurveId,
        label: String,
    ) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        calstore::check_label(&label)?;
        let mut new = self.cal_contents();
        let m = new
            .mics
            .iter_mut()
            .find(|m| m.name == id.mic)
            .ok_or_else(|| not_found(format!("no mic {:?} in the mic library", id.mic)))?;
        if label != id.label && m.curves.iter().any(|c| c.label == label) {
            return Err(perr(
                ErrorCode::Invalid,
                format!("{} already has a curve {label:?}", id.mic),
            ));
        }
        let all = labels(m);
        let c = m
            .curves
            .iter_mut()
            .find(|c| c.label == id.label)
            .ok_or_else(|| {
                not_found(format!(
                    "{} has no curve {:?} (stored: {all})",
                    id.mic, id.label
                ))
            })?;
        c.label.clone_from(&label);
        let renamed = m.clone();
        for r in &mut new.inputs {
            if r.mic.as_deref() == Some(id.mic.as_str())
                && r.curve
                    == (CurveChoice::Curve {
                        label: id.label.clone(),
                    })
            {
                r.curve = CurveChoice::Curve {
                    label: label.clone(),
                };
            }
        }
        let mut curves = self.cal.curves();
        if let Some(p) = curves.remove(&id) {
            curves.insert(
                MicCurveId {
                    mic: id.mic.clone(),
                    label: label.clone(),
                },
                p,
            );
        }
        self.commit_store(new, curves)?;
        tracing::info!("mic curve {:?} of {} renamed {label:?}", id.label, id.mic);
        Ok(ReplyBody::Mic(renamed))
    }

    pub(super) fn cal_curve_delete(&mut self, id: MicCurveId) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        let mut new = self.cal_contents();
        let m = new
            .mics
            .iter_mut()
            .find(|m| m.name == id.mic)
            .ok_or_else(|| not_found(format!("no mic {:?} in the mic library", id.mic)))?;
        let before = m.curves.len();
        let all = labels(m);
        m.curves.retain(|c| c.label != id.label);
        if m.curves.len() == before {
            return Err(not_found(format!(
                "{} has no curve {:?} (stored: {all})",
                id.mic, id.label
            )));
        }
        new.mics.retain(|m| !m.curves.is_empty());
        let mut curves = self.cal.curves();
        curves.remove(&id);
        let rev = self.commit_store(new, curves)?;
        tracing::info!("mic curve {:?} of {} deleted", id.label, id.mic);
        Ok(ReplyBody::Ack { rev })
    }

    /// Removes a sensitivity calibration, on any device: housekeeping of calibrations that
    /// no longer describe the hardware (a mic sold, a device retired) needs no open session.
    pub(super) fn cal_delete(&mut self, key: &CalKey) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        let mut new = self.cal_contents();
        let before = new.calibrations.len();
        new.calibrations.retain(|e| e.key != *key);
        if new.calibrations.len() == before {
            return Err(not_found(format!(
                "no sensitivity calibration for {} on input {} of {}",
                key.mic,
                u32::from(key.channel) + 1,
                key.device.0
            )));
        }
        let rev = self.commit_store(new, self.cal.curves())?;
        tracing::info!(
            "sensitivity calibration of {} on input {} of {} deleted",
            key.mic,
            u32::from(key.channel) + 1,
            key.device.0
        );
        Ok(ReplyBody::Ack { rev })
    }

    pub(super) fn cal_list(&self) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        let st = self.store.state();
        Ok(ReplyBody::Calibrations {
            calibrations: st.calibrations.clone(),
            mics: st.mics.clone(),
        })
    }

    pub(super) fn session_inputs(
        &mut self,
        rows: Vec<InputSetup>,
    ) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        let st = self.store.state();
        for (i, r) in rows.iter().enumerate() {
            let n = u32::from(r.channel) + 1;
            if rows[..i].iter().any(|o| o.channel == r.channel) {
                return Err(perr(ErrorCode::Invalid, format!("input {n} listed twice")));
            }
            if let Some(m) = &r.mic {
                calstore::check_mic_name(m)?;
            }
            // A row as it is may keep a curve no longer stored: it says so where shown.
            if st.inputs.contains(r) {
                continue;
            }
            if let CurveChoice::Curve { label } = &r.curve {
                let Some(m) = &r.mic else {
                    return Err(perr(
                        ErrorCode::Invalid,
                        format!("input {n}: a curve is chosen for a mic; name the mic first"),
                    ));
                };
                if cal::curve(&st.mics, m, label).is_none() {
                    let stored = cal::mic(&st.mics, m).map_or_else(|| "none".to_owned(), labels);
                    return Err(perr(
                        ErrorCode::Invalid,
                        format!("input {n}: no curve {label:?} stored for {m} (stored: {stored})"),
                    ));
                }
            }
        }
        let mut new = self.cal_contents();
        new.inputs = upsert_inputs(&new.inputs, rows);
        let mut settled = new.inputs.clone();
        for r in &mut settled {
            cal::settle(r, &new.mics);
        }
        if settled != self.store.state().inputs {
            self.commit_store(new, self.cal.curves())?;
        }
        Ok(ReplyBody::Inputs(self.store.state().inputs.clone()))
    }
}
