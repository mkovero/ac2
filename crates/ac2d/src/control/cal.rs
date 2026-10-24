//! Calibration commands (`docs/design/q7-calibration.md`, `programme-meter.md` §1):
//! sensitivity calibrations (`cal.spl`, `cal.spl_electrical`), line calibrations
//! (`cal.line`), `cal.delete`, the mic library (`cal.curve_import`, `cal.curve_rename`,
//! `cal.curve_delete`), the input setup with its pairs (`session.inputs`) and `cal.list`.
//!
//! Every change goes through [`Control::commit_store`]: the store file is written first
//! (nothing is committed when that fails), then the changed entities are committed and the
//! running jobs get their inputs' new calibration and curve.

use std::sync::Arc;

use ac2_core::mic_curve::{MicCurve, file_info};
use ac2_proto::cal;
use ac2_proto::model::{
    CalEntry, CalKey, CalMethod, CalRef, CurveChoice, ElectricalConnection, InputKind, InputSetup,
    InputSetups, LineCal, LineCalEntry, LineCalKey, MeasConfig, MeasKind, Mic, MicCurveId,
    MicCurveRef, MicLoading, Pair, SensitivitySource, SplCal,
};
use ac2_proto::units::{Blob, Db, DbSpl, Dbfs, Hz, MvPerPa, Rev, Volts, WallNs};

use super::{
    Change, Control, ErrorCode, MAX_CAL_UNSETTLED_DB, Patch, ProtoError, ReplyBody, block_index,
    perr, upsert_inputs,
};
use crate::calstore::{self, Contents, Curves};
use crate::util::wall_ns;

/// `inputs` with `channel`'s mic name set to `mic`; a new mic starts with no curve chosen
/// (the old choice named another capsule's curve). The row is a mic's ([`Control::mic_input`]
/// checked it).
fn set_input_mic(inputs: &[InputSetup], channel: u16, mic: &str) -> Vec<InputSetup> {
    let mut row = cal::input_setup(inputs, channel);
    if row.kind.mic() != Some(mic) {
        row.kind = InputKind::Mic {
            mic: Some(mic.to_owned()),
            curve: CurveChoice::NotChosen,
        };
    }
    upsert_inputs(inputs, vec![row])
}

/// Input `channel` as printed on the interface: 1-based.
fn nth(channel: u16) -> u32 {
    u32::from(channel) + 1
}

/// The fields of `cal.spl_electrical`.
pub(super) struct ElectricalArgs {
    pub(super) input: u16,
    pub(super) mic: String,
    pub(super) connection: ElectricalConnection,
    pub(super) volts: Volts,
    pub(super) freq: Hz,
    pub(super) mic_sensitivity: Option<MvPerPa>,
    pub(super) loading: Option<MicLoading>,
    pub(super) uncertainty: Option<Db>,
    pub(super) replace_acoustic: bool,
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
            line_calibrations: st.line_calibrations.clone(),
            mics: st.mics.clone(),
            inputs: st.inputs.clone(),
            pairs: st.pairs.clone(),
        }
    }

    /// Input `channel` must be a microphone for a sensitivity calibration or a mic curve: a
    /// line or digital input has no capsule to calibrate (`programme-meter.md` §1).
    /// An SPL meter (and the Leq and band Leq inside it) is acoustic by definition: dB SPL
    /// needs a capsule, and a line or digital feed has none.
    pub(super) fn spl_needs_mic(&self, config: &MeasConfig) -> Result<(), ProtoError> {
        match &config.kind {
            MeasKind::Spl { config } => self.mic_input(config.input, "an SPL meter"),
            _ => Ok(()),
        }
    }

    pub(super) fn mic_input(&self, channel: u16, what: &str) -> Result<(), ProtoError> {
        let kind = cal::input_kind(&self.store.state().inputs, channel);
        if matches!(kind, InputKind::Mic { .. }) {
            return Ok(());
        }
        Err(perr(
            ErrorCode::Invalid,
            format!(
                "input {} is a {}; {what} needs a microphone",
                nth(channel),
                kind.word()
            ),
        ))
    }

    /// The whole input setup as mirrored.
    fn input_setups(&self) -> InputSetups {
        let st = self.store.state();
        InputSetups {
            inputs: st.inputs.clone(),
            pairs: st.pairs.clone(),
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
        for e in &new.line_calibrations {
            if !old.line_calibrations.contains(e) {
                changes.push(Change::LineCalibration(Patch::Set(e.clone())));
            }
        }
        for e in &old.line_calibrations {
            if !new.line_calibrations.iter().any(|n| n.key == e.key) {
                changes.push(Change::LineCalibration(Patch::Deleted(e.key.clone())));
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
        if new.inputs != old.inputs || new.pairs != old.pairs {
            changes.push(Change::Inputs(InputSetups {
                inputs: new.inputs,
                pairs: new.pairs,
            }));
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
        let n = nth(input);
        let Some(rt) = self.session.as_ref() else {
            return Err(perr(
                ErrorCode::Invalid,
                "no open session: a calibration is tied to the session's capture device",
            ));
        };
        block_index(&rt.input_map, input)
            .ok_or_else(|| perr(ErrorCode::Invalid, format!("input {n} is not captured")))?;
        Ok(rt.open.input_device.clone())
    }

    /// [`Self::steady_level`] of a microphone input, for a sensitivity calibration.
    fn cal_reading(
        &self,
        input: u16,
        tone: &str,
    ) -> Result<(ac2_proto::model::DeviceId, f64), ProtoError> {
        self.mic_input(input, "a sensitivity calibration")?;
        self.steady_level(input, tone)
    }

    /// The steady, uncorrected broadband level of `input` (τ = 1 s) for a calibration,
    /// with the device it is tied to. Refused without a session, without signal (below
    /// −80 dBFS), when the input reached full scale in the last 2 s (a clipped tone reads
    /// low) and while the level is not steady. `tone` names the signal in the refusals.
    fn steady_level(
        &self,
        input: u16,
        tone: &str,
    ) -> Result<(ac2_proto::model::DeviceId, f64), ProtoError> {
        let n = nth(input);
        let device = self.cal_target(input)?;
        let Some(rt) = self.session.as_ref() else {
            return Err(perr(ErrorCode::Invalid, "no open session"));
        };
        let idx = block_index(&rt.input_map, input)
            .ok_or_else(|| perr(ErrorCode::Invalid, format!("input {n} is not captured")))?;
        // Broadband and uncorrected: the mic curve is 0 dB at the calibration frequency,
        // so the corrected paths read the tone the same (Q7 §3).
        let (ms, fast) = rt.fanout.meters.mean_square(idx).unwrap_or((0.0, 0.0));
        let measured = ac2_core::spectrum::rms_dbfs(ms.sqrt());
        if !(measured.is_finite() && measured > -80.0) {
            return Err(perr(
                ErrorCode::Refused,
                format!("no {tone} signal on input {n} ({measured:.1} dBFS)"),
            ));
        }
        let latest = rt.fanout.latest.load(std::sync::atomic::Ordering::Acquire);
        if let Some(end) = rt.fanout.meters.clipped_until(idx)
            && latest.saturating_sub(end) < 2 * u64::from(rt.sample_rate)
        {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "input {n} clipped in the last 2 s: a clipped {tone} reads low; lower \
                     the level or the gain and retry"
                ),
            ));
        }
        // The 1 s mean square is within 0.05 dB of a steady level once the 0.2 s one agrees
        // with it that closely (about 5 s after the tone went on).
        let unsettled = 10.0 * (fast / ms).log10();
        if unsettled.is_nan() || unsettled.abs() > MAX_CAL_UNSETTLED_DB {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "the {tone} level on input {n} is not steady yet ({unsettled:+.2} dB \
                     over the last second); keep it on and retry in a few seconds"
                ),
            ));
        }
        Ok((device, measured))
    }

    /// Stores `entry` (replacing the key's calibration) and binds the input's mic name.
    fn store_calibration(&mut self, entry: &CalEntry) -> Result<(), ProtoError> {
        let mut new = self.cal_contents();
        new.calibrations.retain(|e| e.key != entry.key);
        new.calibrations.push(entry.clone());
        new.inputs = set_input_mic(&new.inputs, entry.key.channel, &entry.key.mic);
        self.commit_store(new, self.cal.curves())?;
        Ok(())
    }

    pub(super) fn cal_spl(
        &mut self,
        input: u16,
        mic: String,
        calibrator_level: DbSpl,
        calibrator_freq: Hz,
    ) -> Result<ReplyBody, ProtoError> {
        let n = nth(input);
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
        let (device, measured) = self.cal_reading(input, "calibrator")?;
        // A calibrator measures the whole chain, capsule included: it replaces an electrical
        // calibration of the key without asking.
        let entry = CalEntry {
            key: CalKey {
                device,
                channel: input,
                mic: mic.clone(),
            },
            spl: SplCal {
                sensitivity: Db(calibrator_level.0 - measured),
                method: CalMethod::Acoustic { calibrator_level },
                freq: calibrator_freq,
                measured: Dbfs(measured),
                calibrated_at: WallNs(wall_ns()),
            },
        };
        self.store_calibration(&entry)?;
        tracing::info!(
            "input {n} ({mic}) calibrated: {measured:.2} dBFS at {:.1} dB SPL",
            calibrator_level.0
        );
        Ok(ReplyBody::Calibration(entry))
    }

    /// `cal.spl_electrical` (Q7 §11): the voltage the operator measured at the input, the
    /// level read at the same time and the mic's sensitivity give dB SPL of 0 dBFS.
    pub(super) fn cal_spl_electrical(
        &mut self,
        a: ElectricalArgs,
    ) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        calstore::check_mic_name(&a.mic)?;
        let invalid = |m: String| perr(ErrorCode::Invalid, m);
        let input = a.input;
        let n = nth(input);
        // Ranges that catch a unit slip (mV typed as V, V/Pa as mV/Pa) rather than limits of
        // the method.
        if !(a.volts.0.is_finite() && (1e-4..=100.0).contains(&a.volts.0)) {
            return Err(invalid(format!(
                "measured voltage {} V is outside 0.1 mV … 100 V RMS",
                a.volts.0
            )));
        }
        if !(a.freq.0.is_finite() && (20.0..=20_000.0).contains(&a.freq.0)) {
            return Err(invalid(format!(
                "tone frequency {} Hz is outside 20 Hz … 20 kHz (1 kHz is the one mic \
                 sensitivities are stated at)",
                a.freq.0
            )));
        }
        let uncertainty = a.uncertainty.unwrap_or(cal::DEFAULT_ELECTRICAL_UNCERTAINTY);
        let (lo, hi) = cal::ELECTRICAL_UNCERTAINTY_RANGE;
        if !(uncertainty.0.is_finite() && (lo..=hi).contains(&uncertainty.0)) {
            return Err(invalid(format!(
                "stated uncertainty ±{} dB is outside ±{lo} … ±{hi} dB",
                uncertainty.0
            )));
        }
        let (mic_sensitivity, from) = match a.mic_sensitivity {
            Some(s) => (s, SensitivitySource::Typed),
            None => match cal::data_sheet(&self.store.state().mics, &a.mic) {
                cal::DataSheet::One(s, from) => (s, from),
                cal::DataSheet::NoMic => {
                    return Err(invalid(format!(
                        "no mic sensitivity given and {} has no curve file in the mic library \
                         to take a data-sheet value from: give the sensitivity (mV/Pa)",
                        a.mic
                    )));
                }
                cal::DataSheet::NoneStated => {
                    return Err(invalid(format!(
                        "no mic sensitivity given and the curve files of {} state none: give \
                         the sensitivity (mV/Pa) from its data sheet",
                        a.mic
                    )));
                }
                cal::DataSheet::Differ(v) => {
                    let v: Vec<String> = v.iter().map(|x| format!("{x} mV/Pa")).collect();
                    return Err(invalid(format!(
                        "the curve files of {} state different sensitivities ({}): give the \
                         one to use",
                        a.mic,
                        v.join(", ")
                    )));
                }
            },
        };
        if !(mic_sensitivity.0.is_finite() && (0.1..=1000.0).contains(&mic_sensitivity.0)) {
            return Err(invalid(format!(
                "mic sensitivity {} mV/Pa is outside 0.1 … 1000 mV/Pa",
                mic_sensitivity.0
            )));
        }
        // The data sheet's sensitivity is open circuit; the preamp sees it divided down.
        let loaded = cal::loaded_sensitivity(mic_sensitivity, a.loading.as_ref())
            .map_err(|e| invalid(e.to_string()))?;
        let device = self.cal_target(input)?;
        let key = CalKey {
            device,
            channel: input,
            mic: a.mic.clone(),
        };
        if !a.replace_acoustic
            && let Some(e) = self
                .store
                .state()
                .calibrations
                .iter()
                .find(|e| e.key == key)
            && let CalMethod::Acoustic { calibrator_level } = e.spl.method
        {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "input {n} has an acoustic calibration for {} ({:.1} dB SPL calibrator): \
                     it measured the whole chain and is the better one; replace it only on \
                     purpose",
                    a.mic, calibrator_level.0
                ),
            ));
        }
        let (_, measured) = self.cal_reading(input, "tone")?;
        if measured > cal::ELECTRICAL_MAX_DBFS {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "input {n} reads {measured:.1} dBFS, too close to full scale for a \
                     calibration (at most {} dBFS): lower the tone, not the gain",
                    cal::ELECTRICAL_MAX_DBFS
                ),
            ));
        }
        if measured < cal::ELECTRICAL_MIN_DBFS {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "input {n} reads {measured:.1} dBFS, too low for a good reading (at \
                     least {} dBFS): raise the tone, not the gain",
                    cal::ELECTRICAL_MIN_DBFS
                ),
            ));
        }
        let full_scale = cal::full_scale_volts(a.volts.0, measured);
        let sensitivity = cal::electrical_sensitivity_db(full_scale, loaded.0);
        let entry = CalEntry {
            key,
            spl: SplCal {
                sensitivity: Db(sensitivity),
                method: CalMethod::Electrical {
                    connection: a.connection,
                    volts: a.volts,
                    full_scale: Volts(full_scale),
                    mic_sensitivity,
                    mic_sensitivity_from: from,
                    loading: a.loading,
                    uncertainty,
                },
                freq: a.freq,
                measured: Dbfs(measured),
                calibrated_at: WallNs(wall_ns()),
            },
        };
        self.store_calibration(&entry)?;
        tracing::info!(
            "input {n} ({}) calibrated electrically ({:?}): {} V read at {measured:.2} dBFS \
             → {full_scale:.4} V at 0 dBFS, {} mV/Pa ({:.3} mV/Pa loaded) → 0 dBFS = \
             {sensitivity:.2} dB SPL",
            a.mic,
            a.connection,
            a.volts.0,
            mic_sensitivity.0,
            loaded.0
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
        if let Some(i) = input {
            self.mic_input(i, "a mic curve")?;
            if self.session.is_some() {
                self.cal_target(i)?;
            }
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
                        kind: InputKind::Mic {
                            mic: Some(mic.clone()),
                            curve: CurveChoice::Curve {
                                label: label.clone(),
                            },
                        },
                        pair: cal::input_setup(&new.inputs, i).pair,
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
            if let InputKind::Mic {
                mic: Some(m),
                curve: CurveChoice::Curve { label: l },
            } = &mut r.kind
                && *m == id.mic
                && *l == id.label
            {
                l.clone_from(&label);
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

    /// Removes a sensitivity or line calibration, on any device: housekeeping of
    /// calibrations that no longer describe the hardware (a mic sold, a device retired) needs
    /// no open session.
    pub(super) fn cal_delete(&mut self, key: &CalRef) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        let mut new = self.cal_contents();
        let what = match key {
            CalRef::Spl(key) => {
                let before = new.calibrations.len();
                new.calibrations.retain(|e| e.key != *key);
                if new.calibrations.len() == before {
                    return Err(not_found(format!(
                        "no sensitivity calibration for {} on input {} of {}",
                        key.mic,
                        nth(key.channel),
                        key.device.0
                    )));
                }
                format!(
                    "sensitivity calibration of {} on input {} of {}",
                    key.mic,
                    nth(key.channel),
                    key.device.0
                )
            }
            CalRef::Line(key) => {
                let before = new.line_calibrations.len();
                new.line_calibrations.retain(|e| e.key != *key);
                if new.line_calibrations.len() == before {
                    return Err(not_found(format!(
                        "no line calibration of input {} of {}",
                        nth(key.channel),
                        key.device.0
                    )));
                }
                format!(
                    "line calibration of input {} of {}",
                    nth(key.channel),
                    key.device.0
                )
            }
        };
        let rev = self.commit_store(new, self.cal.curves())?;
        tracing::info!("{what} deleted");
        Ok(ReplyBody::Ack { rev })
    }

    /// `cal.line` (`programme-meter.md` §1.1): the operator measured `volts` RMS of a steady
    /// tone at a line input; the daemon measures the tone's level the way
    /// `cal.spl_electrical` does, and the volts at 0 dBFS follow, dBu of 0 dBFS from them.
    pub(super) fn cal_line(&mut self, channel: u16, volts: Volts) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        let invalid = |m: String| perr(ErrorCode::Invalid, m);
        let kind = cal::input_kind(&self.store.state().inputs, channel);
        if kind != InputKind::Line {
            return Err(invalid(format!(
                "input {} is a {}; a line calibration is for a line input ({})",
                nth(channel),
                kind.word(),
                match kind {
                    InputKind::Mic { .. } => "a microphone reads dB SPL through `cal.spl`",
                    _ => "a digital feed has no voltage: 0 dBFS is its full scale",
                }
            )));
        }
        // Ranges that catch a unit slip (mV typed as V) rather than limits of the method.
        if !(volts.0.is_finite() && (1e-3..=100.0).contains(&volts.0)) {
            return Err(invalid(format!(
                "measured voltage {} V is outside 1 mV … 100 V RMS",
                volts.0
            )));
        }
        let (device, measured) = self.steady_level(channel, "tone")?;
        if !(cal::ELECTRICAL_MIN_DBFS..=cal::ELECTRICAL_MAX_DBFS).contains(&measured) {
            return Err(perr(
                ErrorCode::Refused,
                format!(
                    "input {} reads {measured:.1} dBFS, outside {} … {} dBFS for a \
                     calibration: {} the tone, not the gain",
                    nth(channel),
                    cal::ELECTRICAL_MIN_DBFS,
                    cal::ELECTRICAL_MAX_DBFS,
                    if measured > cal::ELECTRICAL_MAX_DBFS {
                        "lower"
                    } else {
                        "raise"
                    }
                ),
            ));
        }
        let v_fs = cal::full_scale_volts(volts.0, measured);
        let dbu_fs = cal::dbu_fs(v_fs);
        let (lo, hi) = cal::LINE_DBU_FS_RANGE;
        if !(lo..=hi).contains(&dbu_fs.0) {
            return Err(invalid(format!(
                "{} V at {measured:.1} dBFS puts full scale at {:+.1} dBu, outside {lo:+} … \
                 {hi:+} dBu: check the voltage (volts RMS, not millivolts) and the level",
                volts.0, dbu_fs.0
            )));
        }
        let entry = LineCalEntry {
            key: LineCalKey { device, channel },
            line: LineCal {
                v_fs: Volts(v_fs),
                volts,
                measured: Dbfs(measured),
                calibrated_at: WallNs(wall_ns()),
            },
        };
        let mut new = self.cal_contents();
        new.line_calibrations.retain(|e| e.key != entry.key);
        new.line_calibrations.push(entry.clone());
        self.commit_store(new, self.cal.curves())?;
        tracing::info!(
            "input {} line calibrated: {} V RMS measured at {:.2} dBFS, full scale {:.3} V = \
             {:+.1} dBu",
            nth(channel),
            volts.0,
            measured,
            v_fs,
            dbu_fs.0
        );
        Ok(ReplyBody::LineCalibration(entry))
    }

    pub(super) fn cal_list(&self) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        let st = self.store.state();
        Ok(ReplyBody::Calibrations {
            calibrations: st.calibrations.clone(),
            line_calibrations: st.line_calibrations.clone(),
            mics: st.mics.clone(),
        })
    }

    /// `session.inputs`: rows upsert, the pair list replaces. The rows are checked as given,
    /// the pairs over the merged setup (a row's kind may change under a pair), and every
    /// row's `pair` side is set from the list.
    pub(super) fn session_inputs(
        &mut self,
        rows: Vec<InputSetup>,
        pairs: Vec<Pair>,
    ) -> Result<ReplyBody, ProtoError> {
        self.cal.check()?;
        let st = self.store.state();
        for (i, r) in rows.iter().enumerate() {
            let n = nth(r.channel);
            if rows[..i].iter().any(|o| o.channel == r.channel) {
                return Err(perr(ErrorCode::Invalid, format!("input {n} listed twice")));
            }
            let InputKind::Mic { mic, curve } = &r.kind else {
                continue;
            };
            if let Some(m) = mic {
                calstore::check_mic_name(m)?;
            }
            // A row as it is may keep a curve no longer stored: it says so where shown.
            if st
                .inputs
                .iter()
                .any(|x| x.channel == r.channel && x.kind == r.kind)
            {
                continue;
            }
            if let CurveChoice::Curve { label } = curve {
                let Some(m) = mic else {
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
        let merged = upsert_inputs(&new.inputs, rows);
        let captured = self.session.as_ref().map(|rt| rt.input_map.clone());
        let captured_fn = |ch: u16| captured.as_ref().is_some_and(|m| m.contains(&ch));
        cal::check_pairs(
            &merged,
            &pairs,
            captured
                .is_some()
                .then_some(&captured_fn as &dyn Fn(u16) -> bool),
        )
        .map_err(|e| perr(ErrorCode::Invalid, e))?;
        let mut pairs = pairs;
        pairs.sort_by_key(|p| p.left);
        new.inputs = cal::apply_pair_sides(&merged, &pairs);
        new.pairs = pairs;
        let mut settled = new.inputs.clone();
        for r in &mut settled {
            cal::settle(r, &new.mics);
        }
        if settled != self.store.state().inputs || new.pairs != self.store.state().pairs {
            self.commit_store(new, self.cal.curves())?;
        }
        Ok(ReplyBody::Inputs(self.input_setups()))
    }
}
