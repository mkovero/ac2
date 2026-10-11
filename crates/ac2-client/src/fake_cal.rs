//! Calibration and mic-curve commands of the fake daemon.

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, Command, ErrorCode, ErrorDetail, Patch, ProtoError, ReplyBody};

use super::{Shared, err};

impl Shared {
    pub(super) fn cal_cmd(&mut self, cmd: Command) -> Result<ReplyBody, ProtoError> {
        use Command as C;
        Ok(match cmd {
            C::CalSpl {
                input,
                mic,
                calibrator_level,
                calibrator_freq,
            } => {
                let key = fake_cal_key(input, &mic)?;
                let e = CalEntry {
                    spl: SplCal {
                        sensitivity: Db(calibrator_level.0 + 20.0),
                        method: ac2_proto::model::CalMethod::Acoustic { calibrator_level },
                        freq: calibrator_freq,
                        measured: Dbfs(-20.0),
                        calibrated_at: WallNs(1_790_000_000_000_000_000),
                    },
                    key,
                };
                self.commit(Change::Calibration(Patch::Set(e.clone())));
                self.set_mic(input, &mic);
                ReplyBody::Calibration(e)
            }
            // The fake input reads −20 dBFS, as for `cal.spl`.
            C::CalSplElectrical {
                input,
                mic,
                connection,
                volts,
                freq,
                mic_sensitivity,
                uncertainty,
                replace_acoustic,
            } => {
                use ac2_proto::cal;
                use ac2_proto::model::{CalMethod, SensitivitySource};
                let key = fake_cal_key(input, &mic)?;
                let (s, from) = match mic_sensitivity {
                    Some(s) => (s, SensitivitySource::Typed),
                    None => match cal::data_sheet(&self.state.mics, &mic) {
                        cal::DataSheet::One(s, from) => (s, from),
                        _ => {
                            return Err(err(
                                ErrorCode::Invalid,
                                "no mic sensitivity given and no data-sheet value",
                            ));
                        }
                    },
                };
                if !replace_acoustic
                    && self
                        .state
                        .calibrations
                        .iter()
                        .any(|e| e.key == key && matches!(e.spl.method, CalMethod::Acoustic { .. }))
                {
                    return Err(err(ErrorCode::Refused, "input has an acoustic calibration"));
                }
                let measured = -20.0;
                let full_scale = cal::full_scale_volts(volts.0, measured);
                let e = CalEntry {
                    spl: SplCal {
                        sensitivity: Db(cal::electrical_sensitivity_db(full_scale, s.0)),
                        method: CalMethod::Electrical {
                            connection,
                            volts,
                            full_scale: ac2_proto::units::Volts(full_scale),
                            mic_sensitivity: s,
                            mic_sensitivity_from: from,
                            uncertainty: uncertainty.unwrap_or(cal::DEFAULT_ELECTRICAL_UNCERTAINTY),
                        },
                        freq,
                        measured: Dbfs(measured),
                        calibrated_at: WallNs(1_790_000_000_000_000_000),
                    },
                    key,
                };
                self.commit(Change::Calibration(Patch::Set(e.clone())));
                self.set_mic(input, &mic);
                ReplyBody::Calibration(e)
            }
            C::CalCurveImport {
                mic,
                label,
                file_name,
                content,
                input,
            } => self.curve_import(mic, label, &file_name, &content.0, input)?,
            C::CalCurveRename { curve, label } => {
                ac2_proto::cal::check_label(&label).map_err(|e| err(ErrorCode::Invalid, e))?;
                let Some(mut m) = ac2_proto::cal::mic(&self.state.mics, &curve.mic).cloned() else {
                    return Err(err(ErrorCode::NotFound, "no such mic"));
                };
                if label != curve.label && m.curves.iter().any(|c| c.label == label) {
                    return Err(err(ErrorCode::Invalid, "label taken"));
                }
                let Some(c) = m.curves.iter_mut().find(|c| c.label == curve.label) else {
                    return Err(err(ErrorCode::NotFound, "no such curve"));
                };
                c.label.clone_from(&label);
                if let Some(p) = self.traces.curve_points.remove(&curve) {
                    self.traces.curve_points.insert(
                        MicCurveId {
                            mic: curve.mic.clone(),
                            label: label.clone(),
                        },
                        p,
                    );
                }
                self.commit(Change::Mic(Patch::Set(m.clone())));
                let old = CurveChoice::Curve {
                    label: curve.label.clone(),
                };
                let mut all = self.state.inputs.clone();
                for r in &mut all {
                    if r.mic.as_deref() == Some(curve.mic.as_str()) && r.curve == old {
                        r.curve = CurveChoice::Curve {
                            label: label.clone(),
                        };
                    }
                }
                self.set_inputs(all);
                ReplyBody::Mic(m)
            }
            C::CalCurveDelete { curve } => {
                let Some(mut m) = ac2_proto::cal::mic(&self.state.mics, &curve.mic).cloned() else {
                    return Err(err(ErrorCode::NotFound, "no such mic"));
                };
                let n = m.curves.len();
                m.curves.retain(|c| c.label != curve.label);
                if m.curves.len() == n {
                    return Err(err(ErrorCode::NotFound, "no such curve"));
                }
                self.traces.curve_points.remove(&curve);
                let rev = if m.curves.is_empty() {
                    self.commit(Change::Mic(Patch::Deleted(m.name)))
                } else {
                    self.commit(Change::Mic(Patch::Set(m)))
                };
                let all = self.state.inputs.clone();
                let rev = self.set_inputs(all).unwrap_or(rev);
                ReplyBody::Ack { rev }
            }
            C::CalList => ReplyBody::Calibrations {
                calibrations: self.state.calibrations.clone(),
                mics: self.state.mics.clone(),
            },
            C::CalDelete { key } => {
                if !self.state.calibrations.iter().any(|e| e.key == key) {
                    return Err(err(ErrorCode::NotFound, "no such calibration"));
                }
                let rev = self.commit(Change::Calibration(Patch::Deleted(key)));
                ReplyBody::Ack { rev }
            }
            _ => unreachable!("not a cal command"),
        })
    }

    /// Calibrating binds the input's mic name, as the daemon does; a new mic starts with
    /// no curve chosen.
    fn set_mic(&mut self, channel: u16, mic: &str) {
        let mut all = self.state.inputs.clone();
        match all.iter_mut().find(|i| i.channel == channel) {
            Some(i) if i.mic.as_deref() == Some(mic) => {}
            Some(i) => {
                i.mic = Some(mic.to_owned());
                i.curve = CurveChoice::NotChosen;
            }
            None => all.push(InputSetup {
                channel,
                mic: Some(mic.to_owned()),
                curve: CurveChoice::NotChosen,
            }),
        }
        self.set_inputs(all);
    }

    /// Commits the input setup (sorted, the only curve of a one-curve mic chosen where none
    /// is, as the daemon does) when it changed.
    pub(super) fn set_inputs(&mut self, mut all: Vec<InputSetup>) -> Option<Rev> {
        all.sort_by_key(|i| i.channel);
        for r in &mut all {
            ac2_proto::cal::settle(r, &self.state.mics);
        }
        (all != self.state.inputs).then(|| self.commit(Change::Inputs(all)))
    }

    /// `cal.curve_import`, as the daemon (labels and stated sensitivity from the file).
    fn curve_import(
        &mut self,
        mic: String,
        label: Option<String>,
        file_name: &str,
        content: &[u8],
        input: Option<u16>,
    ) -> Result<ReplyBody, ProtoError> {
        ac2_proto::cal::check_mic_name(&mic).map_err(|e| err(ErrorCode::Invalid, e))?;
        let curve = ac2_core::mic_curve::MicCurve::parse(content).map_err(|e| ProtoError {
            code: ErrorCode::Invalid,
            msg: format!("mic curve file refused: {e}"),
            detail: Some(ErrorDetail::MicCurveFile {
                line: e.line().map(|l| u32::try_from(l).unwrap_or(u32::MAX)),
                reason: ac2_proto::MicCurveFileReason::TooFewPoints,
            }),
        })?;
        let info = ac2_core::mic_curve::file_info(content, file_name);
        let label =
            label.unwrap_or_else(|| ac2_proto::cal::default_label(info.angle_deg, file_name));
        ac2_proto::cal::check_label(&label).map_err(|e| err(ErrorCode::Invalid, e))?;
        let r = MicCurveRef {
            label: label.clone(),
            file_name: file_name.to_owned(),
            content_hash: "0000000000000000".into(),
            points: u32::try_from(curve.len()).unwrap_or(u32::MAX),
            f_lo: Hz(curve.f_lo()),
            f_hi: Hz(curve.f_hi()),
            imported_at: WallNs(1_790_000_000_000_000_000),
            stated_sensitivity: info.stated_sensitivity_mv_per_pa,
        };
        let mut m = ac2_proto::cal::mic(&self.state.mics, &mic)
            .cloned()
            .unwrap_or(Mic {
                name: mic.clone(),
                curves: vec![],
            });
        match m.curves.iter_mut().find(|c| c.label == label) {
            Some(c) => *c = r,
            None => m.curves.push(r),
        }
        self.traces.curve_points.insert(
            MicCurveId {
                mic: mic.clone(),
                label: label.clone(),
            },
            curve
                .freqs()
                .iter()
                .zip(curve.gains())
                .map(|(f, g)| [*f, *g])
                .collect(),
        );
        self.commit(Change::Mic(Patch::Set(m.clone())));
        if let Some(i) = input {
            self.set_mic(i, &mic);
            if m.curves.len() == 1 {
                let mut all = self.state.inputs.clone();
                if let Some(row) = all.iter_mut().find(|r| r.channel == i) {
                    row.curve = CurveChoice::Curve { label };
                }
                self.set_inputs(all);
            }
        }
        Ok(ReplyBody::Mic(m))
    }
}

/// The fake's calibration key: its one device.
fn fake_cal_key(input: u16, mic: &str) -> Result<CalKey, ProtoError> {
    if mic.is_empty() {
        return Err(err(ErrorCode::Invalid, "mic name is required"));
    }
    Ok(CalKey {
        device: DeviceId("fake:loop".into()),
        channel: input,
        mic: mic.to_owned(),
    })
}
