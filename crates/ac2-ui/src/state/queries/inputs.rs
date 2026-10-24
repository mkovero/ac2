//! The daemon's running operation, the session's inputs and outputs and their meters.

use crate::state::*;

impl AppState {
    /// The multi-step operation running on the daemon (a set of sweeps), as the progress
    /// strip shows it.
    pub fn operation(&self) -> Option<ac2_scene::progress::Progress> {
        let run = self.daemon()?.sweep.as_ref()?;
        let since = self
            .sweep
            .step_seen
            .as_ref()
            .filter(|(i, s, _)| *i == run.id && *s == run.status)
            .map_or(0.0, |(_, _, t)| self.now_s - t);
        ac2_scene::progress::sweep(run, since)
    }

    /// Every input the open session captures, labelled by name and role, with its meter
    /// and what the running sweep (else the selected measurement) uses it as.
    pub fn session_inputs(&self) -> Vec<ac2_scene::meter::InputRow> {
        use ac2_scene::meter::{InputRole, InputRow, InputUse, MeterReading, input_label};
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let meters = if self.overlay.session().is_some() {
            // The dialog's meters may be another device's preview.
            BTreeMap::new()
        } else {
            self.input_meters()
        };
        let device_names = self
            .devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.input_device)
            .and_then(|d| d.input.as_ref())
            .and_then(|i| i.channel_names.clone());
        let ms = self.measurements();
        let loopback = o.config.loopback.map(|l| l.input);
        let run = self
            .daemon()
            .and_then(|s| s.sweep.as_ref())
            .filter(|r| r.active());
        let uses: Vec<(u16, InputUse)> = match (run, self.selected_meas()) {
            (Some(r), _) => vec![
                (r.reference_input, InputUse::Reference),
                (r.measurement_input, InputUse::Measurement),
            ],
            (None, Some(m)) => match &m.config.kind {
                MeasKind::Transfer { config } => vec![
                    (config.reference_input, InputUse::Reference),
                    (config.measurement_input, InputUse::Measurement),
                ],
                MeasKind::Spectrum { config } => vec![(config.input, InputUse::Measurement)],
                MeasKind::Rta { config } => vec![(config.input, InputUse::Measurement)],
                MeasKind::Spl { config } => vec![(config.input, InputUse::Measurement)],
                MeasKind::Sweep { config } => vec![
                    (config.reference_input, InputUse::Reference),
                    (config.measurement_input, InputUse::Measurement),
                ],
                // Every live operand's inputs.
                MeasKind::Math { config } => ms
                    .iter()
                    .filter(|x| config.expr.names(Operand::Meas { meas: x.id }))
                    .flat_map(|x| match &x.config.kind {
                        MeasKind::Transfer { config } => vec![
                            (config.reference_input, InputUse::Reference),
                            (config.measurement_input, InputUse::Measurement),
                        ],
                        MeasKind::Spectrum { config } => {
                            vec![(config.input, InputUse::Measurement)]
                        }
                        MeasKind::Rta { config } => vec![(config.input, InputUse::Measurement)],
                        _ => Vec::new(),
                    })
                    .collect(),
            },
            (None, None) => Vec::new(),
        };
        let now = super::super::sessions_impl::wall_now();
        let offset = self.clock_offset();
        let st = self.daemon();
        let name_of = |c: u16| {
            let mic = self.input_setup(c).mic().map(str::to_owned);
            let dev = device_names
                .as_ref()
                .and_then(|n| n.get(usize::from(c)).cloned());
            ac2_scene::meter::input_name(c, mic.as_deref(), dev.as_deref())
        };
        let kind_of = |c: u16| crate::prefs::KindPref::of(&self.input_setup(c).kind);
        let role_of = |c: u16| {
            if loopback == Some(c) {
                InputRole::Reference
            } else {
                crate::session_dialog::kind_role(kind_of(c))
            }
        };
        // What a line or digital row reads in: dBFS, or dBu with its calibration's age. A
        // mic's calibration is the SPL meter's business; its row names the curve instead.
        let cal_of = |c: u16| {
            if kind_of(c) == crate::prefs::KindPref::Mic {
                return None;
            }
            let status = st.map(|s| ac2_proto::cal::state_input_use(s, c).sensitivity.status());
            Some(match status {
                Some(ac2_proto::model::CalStatus::Line { .. }) => {
                    ac2_scene::spl::cal_text(status?, false, now, offset)
                }
                _ => ac2_scene::meter::DBFS.to_owned(),
            })
        };
        let reading = |c: u16| meters.get(&c).cloned().unwrap_or_else(MeterReading::none);
        let pairs = st.map(|s| s.pairs.as_slice()).unwrap_or_default();
        let captured = &o.config.input_channels;
        let mut rows = Vec::new();
        for &c in captured {
            let pair = pairs
                .iter()
                .find(|p| p.right == c && captured.contains(&p.left));
            if pair.is_some() {
                // Drawn with its left side.
                continue;
            }
            let pair = pairs
                .iter()
                .find(|p| p.left == c && captured.contains(&p.right));
            let used = uses.iter().find(|(i, _)| *i == c).map(|(_, u)| *u);
            let row = match pair {
                Some(p) => {
                    let name = p.name.clone().unwrap_or_else(|| {
                        ac2_scene::meter::pair_name(&name_of(c), &name_of(p.right))
                    });
                    InputRow {
                        channel: c,
                        label: ac2_scene::meter::pair_label(c, p.right, &name, role_of(c)),
                        used,
                        reading: reading(c),
                        cal: cal_of(c),
                        right: Some((p.right, reading(p.right))),
                    }
                }
                None => {
                    let curve = st.and_then(|s| {
                        ac2_scene::cal::curve_short(&ac2_proto::cal::state_input_use(s, c).curve)
                    });
                    InputRow {
                        channel: c,
                        label: input_label(c, &name_of(c), curve.as_deref(), Some(role_of(c))),
                        used,
                        reading: reading(c),
                        cal: cal_of(c),
                        right: None,
                    }
                }
            };
            rows.push(row);
        }
        rows
    }

    /// Subscribes, opens, renews and closes what [`Self::meter_wants`] changed to.
    /// The preview carries the dialog's kinds and pairs as they stand when it is asked for
    /// (a device change or the renewal), not on every edit: the dialog's rows are the
    /// truth while it is open, the frames' declaration follows at the next renewal.
    pub(in crate::state) fn sync_meters(
        &mut self,
        before: (
            bool,
            Option<(ac2_proto::model::BackendKind, ac2_proto::model::DeviceId)>,
        ),
        tick: bool,
        out: &mut Vec<Request>,
    ) {
        let after = self.meter_wants();
        if before.0 != after.0 {
            out.push(Request::Meters(after.0));
        }
        let renew = tick && self.now_s - self.preview_sent_s >= PREVIEW_RENEW_S;
        match (&before.1, &after.1) {
            (_, Some((backend, device))) if before.1 != after.1 || renew => {
                let (inputs, pairs) = self
                    .overlay
                    .session()
                    .map(|d| d.preview_setup())
                    .unwrap_or_default();
                out.push(Request::Preview {
                    backend: *backend,
                    device: device.clone(),
                    inputs,
                    pairs,
                });
                self.preview_sent_s = self.now_s;
            }
            (Some(_), None) => out.push(Request::PreviewStop),
            _ => {}
        }
    }

    /// Input meters of the session dialog's device (its preview, or the session's own
    /// meters when the session captures it) or, for the measurement dialogs, of the
    /// session: device input → reading. Stale frames read as nothing.
    pub fn input_meters(&self) -> BTreeMap<u16, ac2_scene::meter::MeterReading> {
        use ac2_proto::FrameData;
        use ac2_proto::topic::Topic;
        let mut out = BTreeMap::new();
        let Some(d) = &self.data else {
            return out;
        };
        let from_preview = self.overlay.session().and_then(|s| s.preview_target());
        let topic = if from_preview.is_some() {
            Topic::PreviewLevels
        } else {
            Topic::SessionLevels
        };
        let Some(tf) = d.latest.get(&topic).filter(|f| !f.stale) else {
            return out;
        };
        let (channels, peak, rms, clip) = match &tf.frame.data {
            FrameData::SessionLevels(f) => (&f.meta.channels, &f.peak, &f.rms, &f.clip),
            FrameData::PreviewLevels(f) => {
                if from_preview.as_ref() != Some(&(f.meta.backend, f.meta.device.clone())) {
                    return out;
                }
                (&f.meta.channels, &f.peak, &f.rms, &f.clip)
            }
            _ => return out,
        };
        for (i, c) in channels.iter().enumerate() {
            let (Some(p), Some(r), Some(k)) = (peak.get(i), rms.get(i), clip.get(i)) else {
                continue;
            };
            out.insert(
                *c,
                ac2_scene::meter::MeterReading::new(
                    *p,
                    *r,
                    *k != ac2_proto::frame::ClipFlags::NONE,
                ),
            );
        }
        out
    }

    /// The names of the open session's outputs: the device's channel name, else `Output N`.
    pub fn session_output_names(&self) -> Vec<(u16, String)> {
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let device_names = self
            .devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.output_device)
            .and_then(|d| d.output.as_ref())
            .and_then(|i| i.channel_names.clone());
        (0..o.config.output_channels)
            .map(|c| {
                let name = device_names
                    .as_ref()
                    .and_then(|n| n.get(usize::from(c)).cloned())
                    .unwrap_or_else(|| format!("Output {}", c + 1));
                (c, name)
            })
            .collect()
    }

    /// The names of the open session's inputs, as the dialogs show them: mic name, else the
    /// device's channel name, else `Input N`.
    pub fn session_input_names(&self) -> Vec<(u16, String)> {
        self.session_input_labels()
            .into_iter()
            .map(|(c, name)| (c, ac2_scene::meter::channel_choice(c, &name)))
            .collect()
    }

    /// The open session's inputs by name alone (mic, device channel name or `Input N`).
    pub fn session_input_labels(&self) -> Vec<(u16, String)> {
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let device_names = self.input_channel_names();
        o.config
            .input_channels
            .iter()
            .map(|&c| {
                let mic = self.input_setup(c).mic().map(str::to_owned);
                let dev = device_names
                    .as_ref()
                    .and_then(|n| n.get(usize::from(c)).cloned());
                (
                    c,
                    ac2_scene::meter::input_name(c, mic.as_deref(), dev.as_deref()),
                )
            })
            .collect()
    }

    /// The open input device's channel names, as the backend reports them.
    fn input_channel_names(&self) -> Option<Vec<String>> {
        let o = self.open_session()?;
        self.devices
            .iter()
            .flatten()
            .filter(|b| b.kind == o.backend)
            .flat_map(|b| &b.devices)
            .find(|d| d.id == o.input_device)
            .and_then(|d| d.input.as_ref())
            .and_then(|i| i.channel_names.clone())
    }

    /// The name of input `c` of the open device, captured or not: its mic, else the
    /// device's channel name; `None` when it has neither.
    pub fn input_own_name(&self, c: u16) -> Option<String> {
        let mic = self.input_setup(c).mic().map(str::to_owned);
        let dev = self
            .input_channel_names()
            .and_then(|n| n.get(usize::from(c)).cloned());
        [mic, dev]
            .into_iter()
            .flatten()
            .map(|s| s.trim().to_owned())
            .find(|s| !s.is_empty())
    }
}
