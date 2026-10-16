//! SPL log, Leq history and band-transfer commands of the fake daemon.

use std::collections::HashMap;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, Command, ErrorCode, Patch, ProtoError, ReplyBody};

use super::{Shared, err};

impl Shared {
    pub(super) fn spl_cmd(&mut self, cmd: Command) -> Result<ReplyBody, ProtoError> {
        use Command as C;
        Ok(match cmd {
            C::SplLogGet {
                meas,
                log,
                from,
                max,
            } => {
                self.spl_meter(meas)?;
                let rows = match log {
                    SplLogWhich::Current => self.spl_rows.get(&meas).cloned().unwrap_or_default(),
                    SplLogWhich::Previous => {
                        self.spl_prev_rows
                            .get(&meas)
                            .cloned()
                            .ok_or_else(|| ProtoError {
                                code: ErrorCode::NotFound,
                                msg: format!("SPL meter {meas} has no previous log"),
                                detail: None,
                            })?
                    }
                };
                let total = rows.len() as u64;
                let from = from.min(total);
                let n = max.min(SplLogPage::MAX_ROWS) as usize;
                ReplyBody::SplLogPage(SplLogPage {
                    meas,
                    from,
                    total,
                    rows: rows.into_iter().skip(from as usize).take(n).collect(),
                })
            }
            C::SplHistoryGet { meas, span } => {
                // The fake computes no windows: its meters have no history to replay, but
                // a reply says the log's extent and the resolution asked for.
                let m = self.spl_meter(meas)?;
                let step_s = match span {
                    HistorySpan::Newest { .. } => 1,
                    HistorySpan::Range { step_s, .. } => step_s.max(1),
                };
                let rows = self.spl_rows.get(&meas);
                let log_start = rows.and_then(|r| r.first()).map(|r| r.start);
                let log_until = rows
                    .and_then(|r| r.last())
                    .map(|r| WallNs(r.start.0 + 1_000_000_000));
                let windows = match &m.config.kind {
                    MeasKind::Spl { config } => config.leq.windows.clone(),
                    _ => Vec::new(),
                };
                let n = windows.len();
                ReplyBody::SplHistory(Box::new(SplHistory {
                    meas,
                    windows,
                    scale: LevelScale::Dbfs,
                    step_s,
                    log_start,
                    log_until,
                    at: Vec::new(),
                    leq: vec![Vec::new(); n],
                    over: vec![Vec::new(); n],
                }))
            }
            C::SplBandLogGet {
                meas,
                from,
                until,
                step,
            } => {
                self.spl_meter(meas)?;
                let rows = fake_band_rows_in(&self.band_rows, meas, from, until);
                ac2_traces::band_log::span_reply(meas, from, until, step, &rows)
                    .map(|r| ReplyBody::SplBandLog(Box::new(r)))
                    .map_err(|m| err(ErrorCode::Invalid, m))?
            }
            C::SplLogNew { meas } => {
                let m = self.spl_meter(meas)?;
                let rows = self.spl_rows.remove(&meas).unwrap_or_default();
                self.spl_prev_rows.insert(meas, rows);
                // The entity starts over as for a new meter: initial states, no alarms.
                if self.state.spl_logs.iter().any(|l| l.meas == meas) {
                    self.commit(Change::SplLog(Patch::Deleted(meas)));
                }
                self.spl_log_for(&m);
                ReplyBody::Ack { rev: self.rev }
            }
            C::SplBandTransfer {
                meas,
                foh,
                at_place,
                background,
                place,
            } => {
                BandTransferSet::check_place(&place).map_err(|e| err(ErrorCode::Invalid, e))?;
                let mut m = self.spl_meter(meas)?;
                let MeasKind::Spl { config } = &mut m.config.kind else {
                    unreachable!("spl_meter checked the kind");
                };
                let Some(bands) = &mut config.bands else {
                    return Err(err(
                        ErrorCode::Invalid,
                        format!("SPL meter {meas} has no band meter: enable it first"),
                    ));
                };
                if let Some(e) = ac2_proto::model::overlapping_spans(
                    &foh,
                    &at_place,
                    background.as_ref(),
                    &place,
                    |id| {
                        self.meas(id)
                            .map_or_else(|_| format!("SPL meter {id}"), |m| m.config.name)
                    },
                ) {
                    return Err(err(ErrorCode::Invalid, e));
                }
                let foh = fake_band_levels(&foh, &self.band_rows)?;
                let at_place = fake_band_levels(&at_place, &self.band_rows)?;
                let background = background
                    .as_ref()
                    .map(|b| fake_band_levels(b, &self.band_rows))
                    .transpose()?;
                let t = ac2_core::band_leq::Transfer::measure(&foh, &at_place, background.as_ref());
                bands.transfer = Some(BandTransferSet {
                    place,
                    measured_at: WallNs(self.now_ns()),
                    origin: ac2_proto::model::TransferOrigin::Measured,
                    bands: t.bands().map(fake_transfer_band),
                });
                m.config_rev = Rev(self.rev.0 + 1);
                self.put_meas(m)
            }
            _ => unreachable!("not a spl command"),
        })
    }

    /// The `spl_log` entity of an SPL meter follows its windows, as the daemon's does: a
    /// window configured as before keeps its state; others start unjudged (the fake has no
    /// calibration path for SPL, so a limit reads "not calibrated" until a test commits
    /// states of its own).
    pub(super) fn spl_log_for(&mut self, m: &Measurement) {
        let MeasKind::Spl { config } = &m.config.kind else {
            if self.state.spl_logs.iter().any(|l| l.meas == m.id) {
                self.commit(Change::SplLog(Patch::Deleted(m.id)));
            }
            return;
        };
        let prev = self.state.spl_logs.iter().find(|l| l.meas == m.id).cloned();
        let now = WallNs(self.now_ns());
        let windows = config
            .leq
            .windows
            .iter()
            .map(|w| {
                prev.as_ref()
                    .and_then(|p| {
                        p.windows
                            .iter()
                            .find(|s| s.duration == w.duration && s.weighting == w.weighting)
                    })
                    .copied()
                    .filter(|s| (s.judgement == LeqJudgement::NoLimit) == w.limit.is_none())
                    .unwrap_or(LeqWindowState {
                        duration: w.duration,
                        weighting: w.weighting,
                        judgement: if w.limit.is_some() {
                            LeqJudgement::NotCalibrated
                        } else {
                            LeqJudgement::NoLimit
                        },
                        since: now,
                    })
            })
            .collect();
        let peak = |q: ac2_proto::model::PeakQuantity| {
            let judgement = if config.leq.peaks.get(q).is_some() {
                LeqJudgement::NotCalibrated
            } else {
                LeqJudgement::NoLimit
            };
            prev.as_ref()
                .map(|p| p.peaks.get(q))
                .filter(|s| s.judgement == judgement)
                .unwrap_or(ac2_proto::model::LeqPeakState {
                    judgement,
                    since: now,
                })
        };
        let peaks = ac2_proto::model::PeakStates {
            lcpeak: peak(ac2_proto::model::PeakQuantity::LcPeak),
            lafmax: peak(ac2_proto::model::PeakQuantity::LafMax),
        };
        let l = SplLog {
            meas: m.id,
            started_at: prev.as_ref().and_then(|p| p.started_at),
            windows,
            peaks,
            alarms: prev.map(|p| p.alarms).unwrap_or_default(),
        };
        if self.state.spl_logs.iter().find(|x| x.meas == m.id) != Some(&l) {
            self.commit(Change::SplLog(Patch::Set(l)));
        }
    }

    /// `invalid` unless `meas` is an SPL meter.
    fn spl_meter(&self, meas: MeasId) -> Result<Measurement, ProtoError> {
        let m = self.meas(meas)?;
        if !matches!(m.config.kind, MeasKind::Spl { .. }) {
            return Err(ProtoError {
                code: ErrorCode::Invalid,
                msg: format!("measurement {meas} is not an SPL meter"),
                detail: None,
            });
        }
        Ok(m)
    }
}

/// Band levels of a transfer source as the daemon takes them: typed levels, or a span of
/// the band rows a test gave the fake (`FakeDaemon::band_rows`).
fn fake_band_levels(
    src: &BandLevelSource,
    band_rows: &HashMap<MeasId, Vec<ac2_traces::band_log::BandLogRow>>,
) -> Result<[f64; BAND_COUNT], ProtoError> {
    match src {
        BandLevelSource::Levels { levels } => {
            if levels.len() != BAND_COUNT {
                return Err(err(
                    ErrorCode::Invalid,
                    format!("band levels are {BAND_COUNT} values, 20 Hz … 10 kHz"),
                ));
            }
            if levels.iter().flatten().any(|l| !l.0.is_finite()) {
                return Err(err(ErrorCode::Invalid, "a band level must be finite"));
            }
            let mut out = [f64::NAN; BAND_COUNT];
            for (o, l) in out.iter_mut().zip(levels) {
                if let Some(l) = l {
                    *o = l.0;
                }
            }
            Ok(out)
        }
        BandLevelSource::Log { meas, from, until } => {
            if until <= from {
                return Err(err(ErrorCode::Invalid, "the span ends before it starts"));
            }
            let rows = fake_band_rows_in(band_rows, *meas, *from, *until);
            ac2_traces::band_log::span_average(&rows)
                .levels()
                .map_err(|g| err(ErrorCode::Invalid, format!("SPL meter {meas}: {g}")))
        }
    }
}

fn fake_band_rows_in(
    band_rows: &HashMap<MeasId, Vec<ac2_traces::band_log::BandLogRow>>,
    meas: MeasId,
    from: WallNs,
    until: WallNs,
) -> Vec<ac2_traces::band_log::BandLogRow> {
    band_rows
        .get(&meas)
        .map(|r| {
            r.iter()
                .filter(|r| r.start >= from && r.start < until)
                .copied()
                .collect()
        })
        .unwrap_or_default()
}

fn fake_transfer_band(b: ac2_core::band_leq::BandTransfer) -> BandTransferBand {
    use ac2_core::band_leq::BandTransfer as T;
    match b {
        T::Unchecked { attenuation_db } => BandTransferBand::Unchecked {
            attenuation: Db(attenuation_db),
        },
        T::Clean { attenuation_db } => BandTransferBand::Clean {
            attenuation: Db(attenuation_db),
        },
        T::Corrected {
            attenuation_db,
            margin_db,
        } => BandTransferBand::Corrected {
            attenuation: Db(attenuation_db),
            margin: Db(margin_db),
        },
        T::Unusable { at_least_db } => BandTransferBand::Unusable {
            at_least: Db(at_least_db),
        },
        T::Missing => BandTransferBand::Missing,
    }
}
