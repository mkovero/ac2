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
                self.mic_input(input, "a sensitivity calibration")?;
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
                loading,
                uncertainty,
                replace_acoustic,
            } => {
                use ac2_proto::cal;
                use ac2_proto::model::{CalMethod, SensitivitySource};
                self.mic_input(input, "a sensitivity calibration")?;
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
                let loaded = cal::loaded_sensitivity(s, loading.as_ref())
                    .map_err(|e| err(ErrorCode::Invalid, e.to_string()))?;
                let measured = -20.0;
                let full_scale = cal::full_scale_volts(volts.0, measured);
                let e = CalEntry {
                    spl: SplCal {
                        sensitivity: Db(cal::electrical_sensitivity_db(full_scale, loaded.0)),
                        method: CalMethod::Electrical {
                            connection,
                            volts,
                            full_scale: ac2_proto::units::Volts(full_scale),
                            mic_sensitivity: s,
                            mic_sensitivity_from: from,
                            loading,
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
                    if let InputKind::Mic {
                        mic: Some(m),
                        curve: choice,
                    } = &mut r.kind
                        && *m == curve.mic
                        && *choice == old
                    {
                        *choice = CurveChoice::Curve {
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
            C::CalLine { channel, volts } => {
                use ac2_proto::cal;
                let kind = cal::input_kind(&self.state.inputs, channel);
                if kind != InputKind::Line {
                    return Err(err(
                        ErrorCode::Invalid,
                        format!(
                            "input {} is a {}; a line calibration is for a line input",
                            channel + 1,
                            kind.word()
                        ),
                    ));
                }
                if !(volts.0.is_finite() && (1e-3..=100.0).contains(&volts.0)) {
                    return Err(err(ErrorCode::Invalid, "measured voltage out of range"));
                }
                // The fake's inputs read a fixed level, as for `cal.spl_electrical`.
                let measured = Dbfs(-20.0);
                let v_fs = cal::full_scale_volts(volts.0, measured.0);
                let (lo, hi) = cal::LINE_DBU_FS_RANGE;
                if !(lo..=hi).contains(&cal::dbu_fs(v_fs).0) {
                    return Err(err(ErrorCode::Invalid, "full scale outside +4 … +30 dBu"));
                }
                let e = LineCalEntry {
                    key: LineCalKey {
                        device: DeviceId("fake:loop".into()),
                        channel,
                    },
                    line: LineCal {
                        v_fs: Volts(v_fs),
                        volts,
                        measured,
                        calibrated_at: WallNs(1_790_000_000_000_000_000),
                    },
                };
                self.commit(Change::LineCalibration(Patch::Set(e.clone())));
                ReplyBody::LineCalibration(e)
            }
            C::CalList => ReplyBody::Calibrations {
                calibrations: self.state.calibrations.clone(),
                line_calibrations: self.state.line_calibrations.clone(),
                mics: self.state.mics.clone(),
            },
            C::CalDelete { key } => {
                let rev = match key {
                    CalRef::Spl(key) => {
                        if !self.state.calibrations.iter().any(|e| e.key == key) {
                            return Err(err(ErrorCode::NotFound, "no such calibration"));
                        }
                        self.commit(Change::Calibration(Patch::Deleted(key)))
                    }
                    CalRef::Line(key) => {
                        if !self.state.line_calibrations.iter().any(|e| e.key == key) {
                            return Err(err(ErrorCode::NotFound, "no such line calibration"));
                        }
                        self.commit(Change::LineCalibration(Patch::Deleted(key)))
                    }
                };
                ReplyBody::Ack { rev }
            }
            _ => unreachable!("not a cal command"),
        })
    }

    /// A sensitivity calibration or a mic curve needs a microphone on the input, as the
    /// daemon refuses it.
    pub(super) fn mic_input(&self, channel: u16, what: &str) -> Result<(), ProtoError> {
        let kind = ac2_proto::cal::input_kind(&self.state.inputs, channel);
        if matches!(kind, InputKind::Mic { .. }) {
            return Ok(());
        }
        Err(err(
            ErrorCode::Invalid,
            format!(
                "input {} is a {}; {what} needs a microphone",
                channel + 1,
                kind.word()
            ),
        ))
    }

    /// Calibrating binds the input's mic name, as the daemon does; a new mic starts with
    /// no curve chosen.
    fn set_mic(&mut self, channel: u16, mic: &str) {
        let mut all = self.state.inputs.clone();
        let mut row = ac2_proto::cal::input_setup(&all, channel);
        if row.kind.mic() != Some(mic) {
            row.kind = InputKind::Mic {
                mic: Some(mic.to_owned()),
                curve: CurveChoice::NotChosen,
            };
        }
        all.retain(|i| i.channel != channel);
        all.push(row);
        self.set_inputs(all);
    }

    /// Commits the input setup (sorted, the only curve of a one-curve mic chosen where none
    /// is, as the daemon does) when it changed; the pairs stay.
    pub(super) fn set_inputs(&mut self, all: Vec<InputSetup>) -> Option<Rev> {
        let pairs = self.state.pairs.clone();
        self.set_inputs_and_pairs(all, pairs)
    }

    /// Commits the input setup and the pairs as one change when either changed.
    pub(super) fn set_inputs_and_pairs(
        &mut self,
        mut all: Vec<InputSetup>,
        pairs: Vec<Pair>,
    ) -> Option<Rev> {
        all.sort_by_key(|i| i.channel);
        for r in &mut all {
            ac2_proto::cal::settle(r, &self.state.mics);
        }
        (all != self.state.inputs || pairs != self.state.pairs)
            .then(|| self.commit(Change::Inputs(InputSetups { inputs: all, pairs })))
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
                if let Some(InputKind::Mic { curve, .. }) =
                    all.iter_mut().find(|r| r.channel == i).map(|r| &mut r.kind)
                {
                    *curve = CurveChoice::Curve { label };
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
