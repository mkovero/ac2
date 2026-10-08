//! The SPL meter and Leq: weightings, folding their frames, the Leq dialog, logs and alarms.

use super::*;

impl AppState {
    /// F / Z in the SPL pane (or the palette's direct choices): the meter the pane shows
    /// takes the next (or the named) time or frequency weighting, in place — its Leq
    /// windows, log and interval carry on. The pane keeps showing what it showed.
    pub(super) fn spl_weightings(&mut self, c: CommandId, out: &mut Vec<Request>) {
        use CommandId as C;
        use ac2_proto::model::{TimeWeighting as T, Weighting as W};
        let Some(mut m) = self.pane_meas(PaneKind::Spl).cloned() else {
            self.warn("no SPL meter: make one first (New SPL meter… in Ctrl+K)");
            return;
        };
        let MeasKind::Spl { config } = &mut m.config.kind else {
            return;
        };
        match c {
            C::SplTimeWeighting => {
                config.time_weighting = match config.time_weighting {
                    T::Fast => T::Slow,
                    T::Slow => T::Impulse,
                    T::Impulse => T::Fast,
                }
            }
            C::SplWeighting => {
                config.weighting = match config.weighting {
                    W::A => W::C,
                    W::C => W::Z,
                    W::Z => W::A,
                }
            }
            C::SplFast => config.time_weighting = T::Fast,
            C::SplSlow => config.time_weighting = T::Slow,
            C::SplImpulse => config.time_weighting = T::Impulse,
            C::SplA => config.weighting = W::A,
            C::SplC => config.weighting = W::C,
            C::SplZ => config.weighting = W::Z,
            _ => return,
        }
        let what = format!(
            "{}: {}",
            m.config.name,
            ac2_scene::spl::metric_name(config.weighting, config.time_weighting)
        );
        // The pane keeps its view: the Leq windows do not follow the meter's weightings, so
        // switching to the meter would read as the windows having changed. The daemon's
        // reply names the new metric either way.
        self.focus(PaneKind::Spl);
        let (meas, config) = (m.id, m.config);
        self.call(out, Command::MeasUpdate { meas, config }, what);
    }

    /// How long the SPL meter's number holds a reading under time weighting `t`: the
    /// preference when set, else the time weighting's own display period.
    pub fn spl_display_period_s(&self, t: ac2_proto::model::TimeWeighting) -> f64 {
        self.prefs.spl_hold_ms.map_or_else(
            || ac2_scene::spl::display_period_s(t),
            |ms| f64::from(ms) / 1e3,
        )
    }

    /// Takes each SPL meter's newest frame as its displayed reading when the display
    /// period has passed ([`ac2_scene::spl::SplHold`]).
    pub(super) fn fold_spl(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        if let Some(st) = self.daemon() {
            let ids: Vec<MeasId> = st.measurements.iter().map(|m| m.id).collect();
            self.spl_hold.retain(|k, _| ids.contains(k));
        }
        for tf in d.latest.frames.values() {
            let FrameData::Spl(f) = &tf.frame.data else {
                continue;
            };
            let period = self.spl_display_period_s(f.meta.time_weighting);
            let mut held = self.spl_hold.remove(&f.meas);
            ac2_scene::spl::SplHold::update(
                &mut held,
                f,
                tf.frame.stamp.capture_wall_ns.0,
                tf.frame.stamp.config_rev,
                period,
            );
            if let Some(h) = held {
                self.spl_hold.insert(f.meas, h);
            }
        }
    }

    /// The run of SPL meter `meas`'s log, from its newest `leq` frame.
    pub(super) fn leq_run(&self, meas: MeasId) -> Option<ac2_proto::frame::LeqRun> {
        use ac2_proto::FrameData;
        self.data
            .as_ref()
            .and_then(|d| {
                d.latest.get(&Topic::Data {
                    meas,
                    stream: Stream::Leq,
                })
            })
            .and_then(|f| match &f.frame.data {
                FrameData::Leq(l) => l.meta.run,
                _ => None,
            })
    }

    /// Whether SPL meter `meas` reads dB SPL (its newest frame says so).
    pub(super) fn leq_calibrated(&self, meas: MeasId) -> bool {
        use ac2_proto::FrameData;
        use ac2_proto::model::LevelScale;
        let scale = |s: Stream| {
            self.data
                .as_ref()
                .and_then(|d| d.latest.get(&Topic::Data { meas, stream: s }))
                .and_then(|f| match &f.frame.data {
                    FrameData::Leq(l) => Some(l.meta.scale),
                    FrameData::Spl(l) => Some(l.meta.scale),
                    _ => None,
                })
        };
        scale(Stream::Leq).or_else(|| scale(Stream::Spl)) == Some(LevelScale::DbSpl)
    }

    pub(super) fn leq_msg(&mut self, m: LeqMsg, out: &mut Vec<Request>) {
        if m == LeqMsg::Submit {
            self.submit_leq(out);
            return;
        }
        let Some(d) = self.overlay.leq_mut() else {
            return;
        };
        match m {
            LeqMsg::Focus(f) => {
                d.focus = f;
                d.select_all();
            }
            LeqMsg::Cycle(f, step) => {
                d.focus = f;
                d.cycle(step);
            }
            LeqMsg::Add(f) => {
                d.focus = f;
                d.add_window();
            }
            LeqMsg::Remove(f) => {
                d.focus = f;
                d.remove_window();
            }
            LeqMsg::Cancel => self.overlay = Overlay::None,
            LeqMsg::Submit => {}
        }
    }

    /// Enter on the Leq dialog: the meter's windows go out (applied in place: its log and
    /// windows carry on) and the SPL pane shows them; or the dialog says what is wrong.
    pub(super) fn submit_leq(&mut self, out: &mut Vec<Request>) {
        let Some(d) = self.overlay.leq_mut() else {
            return;
        };
        match d.meas_config() {
            Ok(config) => {
                let (meas, name) = (d.meas, d.name.clone());
                self.overlay = Overlay::None;
                self.view.spl.mode = self.view.spl.mode.with_leq();
                self.focus(PaneKind::Spl);
                self.call(
                    out,
                    Command::MeasUpdate { meas, config },
                    format!("{name}: Leq windows set"),
                );
            }
            Err(e) => d.error = Some(e),
        }
    }

    /// Folds the newest `leq` frame of each SPL meter into its history.
    pub(super) fn fold_leq(&mut self, d: &DataSnapshot) {
        use ac2_proto::FrameData;
        // Without a synced daemon state (a resync, a short drop) the meter list is unknown,
        // not empty: keep every history until the state says a meter is gone.
        if self.daemon().is_none() {
            return;
        }
        let cfgs: BTreeMap<MeasId, ac2_proto::model::LeqConfig> = self
            .measurements()
            .iter()
            .filter_map(|m| match &m.config.kind {
                MeasKind::Spl { config } => Some((m.id, config.leq.clone())),
                _ => None,
            })
            .collect();
        self.leq_history.retain(|k, _| cfgs.contains_key(k));
        for tf in d.latest.frames.values() {
            let FrameData::Leq(f) = &tf.frame.data else {
                continue;
            };
            let Some(cfg) = cfgs.get(&f.meas) else {
                continue;
            };
            let seq = tf.frame.stamp.seq;
            let e = self.leq_history.entry(f.meas).or_default();
            if seq > e.0 {
                e.0 = seq;
                e.1.push(cfg, f, tf.frame.stamp.capture_wall_ns.0 as f64 / 1e9);
            }
        }
    }

    /// Rebuilds each SPL meter's history from its log when the app first sees the meter
    /// (connected, resynced, created), when its windows change and when a new log starts
    /// (from this app or any other client: the log empties, or its row count starts over);
    /// a new log first clears the history. The frames received go on folding in meanwhile.
    pub(super) fn follow_leq_logs(&mut self, data: Option<&DataSnapshot>, out: &mut Vec<Request>) {
        use ac2_proto::FrameData;
        let Some(st) = self.daemon() else {
            // Resyncing: the daemon may have restarted with other logs; ask again once synced.
            self.leq_logs.clear();
            return;
        };
        let mut logged: BTreeMap<MeasId, u64> = BTreeMap::new();
        if let Some(d) = data {
            for tf in d.latest.frames.values() {
                if let FrameData::Leq(f) = &tf.frame.data {
                    logged.insert(f.meas, f.meta.logged);
                }
            }
        }
        let meters: Vec<(MeasId, ac2_proto::model::LeqConfig, Option<bool>)> = st
            .measurements
            .iter()
            .filter_map(|m| match &m.config.kind {
                MeasKind::Spl { config } => Some((
                    m.id,
                    config.leq.clone(),
                    st.spl_logs
                        .iter()
                        .find(|l| l.meas == m.id)
                        .map(|l| l.started_at.is_some()),
                )),
                _ => None,
            })
            .collect();
        self.leq_logs
            .retain(|k, _| meters.iter().any(|(id, ..)| id == k));
        for (meas, config, started) in meters {
            let logged = logged.get(&meas).copied();
            let seen = self.leq_logs.get(&meas);
            let new_log = seen.is_some_and(|s| {
                logged.is_some_and(|l| l < s.logged) || (s.started && started == Some(false))
            });
            if new_log && let Some((_, h)) = self.leq_history.get_mut(&meas) {
                h.clear();
            }
            match self.leq_logs.get_mut(&meas) {
                Some(s) if !new_log && s.config == config => {
                    s.logged = logged.unwrap_or(s.logged).max(s.logged);
                    s.started = started.unwrap_or(s.started);
                }
                _ => {
                    self.leq_backfill_ask += 1;
                    let ask = self.leq_backfill_ask;
                    self.leq_logs.insert(
                        meas,
                        LeqLogSeen {
                            config,
                            logged: logged.unwrap_or(0),
                            started: started.unwrap_or(false),
                            ask,
                        },
                    );
                    out.push(Request::LeqBackfill { meas, ask });
                }
            }
        }
    }

    /// A meter's history rebuilt from its log: under the frames received, unless a newer
    /// rebuild was asked for meanwhile.
    pub(super) fn leq_backfilled(
        &mut self,
        meas: MeasId,
        ask: u64,
        result: Result<Box<ac2_proto::model::SplHistory>, String>,
    ) {
        if self.leq_logs.get(&meas).is_none_or(|s| s.ask != ask) {
            return;
        }
        match result {
            Ok(h) => self.leq_history.entry(meas).or_default().1.backfill(&h),
            Err(e) => {
                let name = self
                    .measurements()
                    .iter()
                    .find(|m| m.id == meas)
                    .map(|m| m.config.name.clone())
                    .unwrap_or_default();
                self.fault(format!("{name}: Leq history from the log: {e}"));
            }
        }
    }

    /// Toasts each window that went over its limit or came back since the last mirror; the
    /// alarms already there when the app connected are history, not news.
    pub(super) fn follow_leq_alarms(&mut self) {
        let Some(st) = self.daemon() else {
            return;
        };
        let names: BTreeMap<MeasId, String> = st
            .measurements
            .iter()
            .map(|m| (m.id, m.config.name.clone()))
            .collect();
        let mut news = Vec::new();
        let mut seen = BTreeMap::new();
        for l in &st.spl_logs {
            let newest = l.alarms.last().copied();
            match self.leq_alarms_seen.get(&l.meas) {
                None => {}
                Some(prev) => {
                    let from = match prev {
                        None => 0,
                        Some(p) => l.alarms.iter().rposition(|a| a == p).map_or(0, |i| i + 1),
                    };
                    let name = names.get(&l.meas).cloned().unwrap_or_default();
                    news.extend(l.alarms[from..].iter().map(|a| (name.clone(), *a)));
                }
            }
            seen.insert(l.meas, newest);
        }
        self.leq_alarms_seen = seen;
        for (name, a) in news {
            match ac2_scene::leq::alarm_text(&name, &a) {
                (true, text) => self.fault(text),
                (false, text) => self.toast(text),
            }
        }
    }
}
