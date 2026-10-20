//! Measurement and delay commands of the fake daemon.

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Change, Command, ErrorCode, GridDef, Patch, ProtoError, ReplyBody};

use super::{FakeFinding, Shared, conflict, err};

impl Shared {
    pub(super) fn meas_cmd(&mut self, cmd: Command) -> Result<ReplyBody, ProtoError> {
        use Command as C;
        Ok(match cmd {
            C::MeasCreate { config } => {
                let id = MeasId(self.next_id);
                self.next_id += 1;
                let (delay, grid_id) = match &config.kind {
                    MeasKind::Transfer { config } => {
                        let g = GridDef::Log {
                            ppo: config.grid().ppo,
                            k_min: config.grid().k_min,
                            k_max: config.grid().k_max,
                        };
                        let gid = g.id();
                        self.grids.insert(gid, g);
                        (
                            Some(DelayState {
                                applied: Seconds(0.0),
                                applied_samples: 0.0,
                                nudged: Seconds(0.0),
                                nudged_samples: 0.0,
                                tracking: false,
                                awaiting_pick: false,
                                last_finding: None,
                            }),
                            Some(gid),
                        )
                    }
                    _ => (None, None),
                };
                let m = Measurement {
                    id,
                    config,
                    config_rev: Rev(self.rev.0 + 1),
                    running: false,
                    delay,
                    grid_id,
                    colour: None,
                    layout: None,
                    compared: Vec::new(),
                    offset: Db(0.0),
                    eq: Vec::new(),
                };
                self.put_meas(m)
            }
            C::MeasUpdate { meas, base, config } => {
                let mut m = self.meas(meas)?;
                if base != m.config_rev {
                    return Err(conflict(m.config_rev));
                }
                if config.kind.display().is_some_and(|d| !d.span_ok()) {
                    return Err(err(ErrorCode::Invalid, "no such spectrograph span"));
                }
                // As the daemon does: how it is drawn is not a new configuration.
                let mut drawn = m.config.clone();
                if let (Some(d), Some(n)) = (drawn.kind.display_mut(), config.kind.display()) {
                    *d = *n;
                }
                if drawn != config {
                    m.config_rev = Rev(self.rev.0 + 1);
                }
                m.config = config;
                self.put_meas(m)
            }
            C::MeasColour { meas, colour } => {
                let mut m = self.meas(meas)?;
                m.colour = colour;
                self.put_meas(m)
            }
            C::MeasLayout { meas, layout } => {
                let mut m = self.meas(meas)?;
                m.layout = layout;
                self.put_meas(m)
            }
            C::MeasCompare { meas, compared } => {
                let mut m = self.meas(meas)?;
                if compared.len() > Compared::MAX
                    || compared.contains(&Compared::Meas { meas })
                    || compared.iter().any(|c| match *c {
                        Compared::Meas { meas } => self.meas(meas).is_err(),
                        Compared::Trace { trace } => {
                            !self.state.traces.iter().any(|t| t.id == trace)
                        }
                    })
                {
                    return Err(err(ErrorCode::Invalid, "no such compare list"));
                }
                m.compared = compared;
                self.put_meas(m)
            }
            C::MeasOffset { meas, offset } => {
                let mut m = self.meas(meas)?;
                if !Measurement::offset_ok(offset) {
                    return Err(err(ErrorCode::Invalid, "no such offset"));
                }
                m.offset = offset;
                self.put_meas(m)
            }
            C::MeasDelete { meas, traces } => {
                self.meas(meas)?;
                // What it owns: kept under the imported group, or deleted with it.
                let owner = TraceOwner::Meas { meas };
                let owned: Vec<TraceMeta> = self
                    .state
                    .traces
                    .iter()
                    .filter(|t| t.edit.owner == owner)
                    .cloned()
                    .collect();
                let maths: Vec<Measurement> = self
                    .state
                    .measurements
                    .iter()
                    .filter(|m| matches!(&m.config.kind, MeasKind::Math { config } if config.owner == owner))
                    .cloned()
                    .collect();
                match traces {
                    OwnedTraces::Keep => {
                        for mut t in owned {
                            t.edit.owner = TraceOwner::Imported;
                            self.commit(Change::Trace(Patch::Set(t)));
                        }
                        for mut m in maths {
                            if let MeasKind::Math { config } = &mut m.config.kind {
                                config.owner = TraceOwner::Imported;
                            }
                            self.commit(Change::Measurement(Patch::Set(m)));
                        }
                    }
                    OwnedTraces::Delete => {
                        for m in maths {
                            self.commit(Change::Measurement(Patch::Deleted(m.id)));
                        }
                        for t in owned {
                            self.traces.data.remove(&t.id);
                            self.traces.sweeps.remove(&t.id);
                            self.traces.irs.remove(&t.id);
                            self.commit(Change::Trace(Patch::Deleted(t.id)));
                        }
                    }
                }
                if self.state.spl_logs.iter().any(|l| l.meas == meas) {
                    self.commit(Change::SplLog(Patch::Deleted(meas)));
                }
                let rev = self.commit(Change::Measurement(Patch::Deleted(meas)));
                let rev = self.prune_compared().unwrap_or(rev);
                ReplyBody::Ack { rev }
            }
            C::MeasStart { meas } => {
                let mut m = self.meas(meas)?;
                m.running = true;
                self.put_meas(m)
            }
            C::MeasStop { meas } => {
                let mut m = self.meas(meas)?;
                m.running = false;
                self.put_meas(m)
            }
            C::MeasReset { meas } => {
                self.meas(meas)?;
                ReplyBody::Ack { rev: self.rev }
            }
            C::DelayFind {
                meas,
                band,
                observation,
            } => {
                let mut m = self.meas(meas)?;
                self.last_find = Some((band, observation));
                let f = finding(self.finding, band, self.now_ns());
                let st = m
                    .delay
                    .as_mut()
                    .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
                st.awaiting_pick = matches!(f.outcome, DelayOutcome::Ambiguous { .. });
                st.last_finding = Some(f.clone());
                self.commit(Change::Measurement(Patch::Set(m)));
                ReplyBody::DelayFinding(f)
            }
            C::DelayInsert { meas, pick } => {
                let mut m = self.meas(meas)?;
                let f = m
                    .delay
                    .as_ref()
                    .and_then(|d| d.last_finding.clone())
                    .ok_or_else(|| err(ErrorCode::Invalid, "no delay finding to insert"))?;
                if let Some(r) = f.no_estimate() {
                    return Err(err(ErrorCode::Refused, format!("no estimate: {r:?}")));
                }
                let d = f
                    .arrival(pick)
                    .ok_or_else(|| err(ErrorCode::NotFound, "no such arrival"))?
                    .delay;
                set_delay(&mut m, d)?;
                self.put_meas(m)
            }
            C::DelaySet { meas, delay } => {
                let mut m = self.meas(meas)?;
                let arrival = m.delay.as_ref().map(|d| d.applied.0 - d.nudged.0);
                set_delay(&mut m, delay)?;
                if let Some(d) = &mut m.delay {
                    // A typed value keeps the arrival and moves the live curve alone, as a
                    // step does (as the daemon does).
                    if let Some(a) = arrival {
                        d.nudged = Seconds(delay.0 - a);
                        d.nudged_samples = d.nudged.0 * 48_000.0;
                    }
                    // An explicit value supersedes the last finding (as the daemon does).
                    d.last_finding = None;
                }
                self.put_meas(m)
            }
            C::DelayNudge { meas, by } => {
                let mut m = self.meas(meas)?;
                let st = m
                    .delay
                    .as_mut()
                    .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
                // A step from the arrival: the daemon keeps what nudges added apart.
                st.awaiting_pick = false;
                st.applied = Seconds(st.applied.0 + by.0);
                st.applied_samples = st.applied.0 * 48_000.0;
                st.nudged = Seconds(st.nudged.0 + by.0);
                st.nudged_samples = st.nudged.0 * 48_000.0;
                self.put_meas(m)
            }
            C::DelayTrack { meas, enabled } => {
                let mut m = self.meas(meas)?;
                let d = m
                    .delay
                    .as_mut()
                    .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
                d.tracking = enabled;
                self.put_meas(m)
            }
            _ => unreachable!("not a meas command"),
        })
    }
}

fn arrival(ms: f64, level: f64) -> DelayArrival {
    DelayArrival {
        delay: Seconds(ms / 1000.0),
        delay_samples: ms * 48.0,
        level: Db(level),
        phase: Degrees(0.0),
        uncertainty_samples: 0.2,
        misfit: 0.01,
        refined: true,
    }
}

fn finding(kind: FakeFinding, band: FinderBand, now: u64) -> DelayFinding {
    let band = match band {
        FinderBand::Full | FinderBand::Auto => DelayBand::Full,
        FinderBand::Mid => DelayBand::Mid,
        FinderBand::Sub => DelayBand::Sub,
        FinderBand::Custom { lo_hz, hi_hz } => DelayBand::Custom { lo_hz, hi_hz },
    };
    let confidence = DelayConfidence {
        psr_db: Some(Db(24.0)),
        psr_acq_db: Some(Db(20.0)),
        band_snr_db: Some(Db(30.0)),
        excited_fraction: Some(1.0),
        uncertainty_samples: Some(0.2),
        pulse_width_samples: Some(4.0),
        period: None,
    };
    let (outcome, candidates, confidence) = match kind {
        FakeFinding::Accepted => (
            DelayOutcome::Accepted {
                first: arrival(12.5, -2.5),
                strongest: arrival(12.7, 0.0),
            },
            vec![arrival(12.5, -2.5), arrival(12.7, 0.0)],
            confidence,
        ),
        FakeFinding::Ambiguous => (
            DelayOutcome::Ambiguous {
                reasons: vec![AmbiguityReason::BorderlineLevel],
                ranked: vec![
                    arrival(12.5, -11.5),
                    arrival(12.7, 0.0),
                    arrival(13.4, -6.0),
                ],
                strongest: arrival(12.7, 0.0),
            },
            vec![
                arrival(12.5, -11.5),
                arrival(12.7, 0.0),
                arrival(13.4, -6.0),
            ],
            confidence,
        ),
        FakeFinding::NoEstimate => (
            DelayOutcome::NoEstimate {
                reasons: vec![NoEstimateReason::LowPsr, NoEstimateReason::LowBandSnr],
            },
            vec![],
            DelayConfidence {
                psr_db: Some(Db(4.0)),
                band_snr_db: Some(Db(2.0)),
                uncertainty_samples: None,
                ..confidence
            },
        ),
    };
    DelayFinding {
        outcome,
        confidence,
        band,
        observation: Seconds(0.25),
        candidates,
        found_at: WallNs(now),
    }
}

/// An operator-set delay (insert or typed), which also resolves an ambiguous finding.
/// Set as a new arrival (nothing nudged from it); a typed value then restores its arrival.
fn set_delay(m: &mut Measurement, d: Seconds) -> Result<(), ProtoError> {
    let st = m
        .delay
        .as_mut()
        .ok_or_else(|| err(ErrorCode::Invalid, "not a transfer measurement"))?;
    st.awaiting_pick = false;
    st.applied = d;
    st.applied_samples = d.0 * 48_000.0;
    st.nudged = Seconds(0.0);
    st.nudged_samples = 0.0;
    Ok(())
}
