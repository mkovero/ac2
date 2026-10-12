//! Backend and device choice: the device lists, the output device, rows and remembered roles.

use super::*;

impl SessionDialog {
    // ----- backend and device ------------------------------------------------------------

    /// Fills the backends from `session.devices` and selects one: the open session's, else
    /// the first usable real one, else the first usable (a daemon started on the simulated
    /// rig offers only that).
    pub fn set_backends(&mut self, list: Vec<BackendInfo>, prefs: &UiPrefs) {
        let usable =
            |b: &BackendInfo| b.availability == Availability::Available && !b.devices.is_empty();
        let open = self
            .open
            .as_ref()
            .and_then(|o| list.iter().position(|b| b.kind == o.backend && usable(b)));
        self.backend = open
            .or_else(|| {
                list.iter()
                    .position(|b| usable(b) && b.kind != BackendKind::Fake)
            })
            .or_else(|| list.iter().position(usable))
            .unwrap_or(0);
        self.backends = Some(list);
        self.device = self
            .open
            .as_ref()
            .and_then(|o| {
                self.backend_info()?
                    .devices
                    .iter()
                    .position(|d| d.id == o.input_device)
            })
            .or_else(|| self.input_choices().first().copied())
            .unwrap_or(0);
        self.load_device(prefs);
        // The channel rows are new: the focus belongs on one that exists.
        self.keep_focus();
    }

    pub fn backend_info(&self) -> Option<&BackendInfo> {
        self.backends.as_ref()?.get(self.backend)
    }

    pub fn device_info(&self) -> Option<&DeviceInfo> {
        self.backend_info()?.devices.get(self.device)
    }

    /// The device the session plays on, if it has outputs.
    pub fn output_info(&self) -> Option<&DeviceInfo> {
        let d = match self.out_device {
            OutputDevice::SameAsInput => self.device_info(),
            OutputDevice::Other(i) => self.backend_info()?.devices.get(i),
        }?;
        d.output.is_some().then_some(d)
    }

    /// The input device choices: devices with inputs, in listed order (an output-only
    /// endpoint has nothing to capture).
    fn input_choices(&self) -> Vec<usize> {
        self.backend_info().map_or_else(Vec::new, |b| {
            (0..b.devices.len())
                .filter(|&i| b.devices[i].input.is_some())
                .collect()
        })
    }

    /// How many input and output device choices there are.
    pub fn device_counts(&self) -> (usize, usize) {
        (self.input_choices().len(), self.output_choices().len())
    }

    /// The chosen input device's place among the input choices.
    pub fn input_position(&self) -> usize {
        self.input_choices()
            .iter()
            .position(|&i| i == self.device)
            .unwrap_or(0)
    }

    /// The chosen output device's place among the output choices.
    pub fn output_position(&self) -> usize {
        self.output_choices()
            .iter()
            .position(|c| *c == self.out_device)
            .unwrap_or(0)
    }

    /// The output device choices: the input device when it has outputs, then every other
    /// device with outputs.
    pub fn output_choices(&self) -> Vec<OutputDevice> {
        let Some(b) = self.backend_info() else {
            return Vec::new();
        };
        let mut v = Vec::new();
        if self.device_info().is_some_and(|d| d.output.is_some()) {
            v.push(OutputDevice::SameAsInput);
        }
        v.extend(
            (0..b.devices.len())
                .filter(|&i| i != self.device && b.devices[i].output.is_some())
                .map(OutputDevice::Other),
        );
        v
    }

    /// The output device of a device never used before: itself when it has outputs, else
    /// the system's default output, else the first device with outputs.
    fn default_output(&self) -> OutputDevice {
        let Some(b) = self.backend_info() else {
            return OutputDevice::SameAsInput;
        };
        if self.device_info().is_some_and(|d| d.output.is_some()) {
            return OutputDevice::SameAsInput;
        }
        let other = |default: bool| {
            (0..b.devices.len()).find(|&i| {
                i != self.device
                    && b.devices[i]
                        .output
                        .as_ref()
                        .is_some_and(|o| !default || o.system_default)
            })
        };
        other(true)
            .or_else(|| other(false))
            .map_or(OutputDevice::SameAsInput, OutputDevice::Other)
    }

    /// The output device named `id`, when it is listed with outputs.
    fn output_by_id(&self, id: &DeviceId) -> Option<OutputDevice> {
        if self.device_info().is_some_and(|d| &d.id == id) {
            return Some(OutputDevice::SameAsInput);
        }
        self.output_choices().into_iter().find(|c| match c {
            OutputDevice::Other(i) => self
                .backend_info()
                .and_then(|b| b.devices.get(*i))
                .is_some_and(|d| &d.id == id),
            OutputDevice::SameAsInput => false,
        })
    }

    /// The open session plays on the chosen output device.
    pub fn is_open_output(&self) -> bool {
        match (&self.open, self.output_info()) {
            (Some(o), Some(d)) => self.is_open_device() && o.output_device == d.id,
            _ => false,
        }
    }

    /// The chosen backend kind.
    pub fn backend_kind(&self) -> Option<BackendKind> {
        self.backend_info().map(|b| b.kind)
    }

    /// The open session runs on the chosen device.
    pub fn is_open_device(&self) -> bool {
        match (&self.open, self.backend_kind(), self.device_info()) {
            (Some(o), Some(k), Some(d)) => o.backend == k && o.input_device == d.id,
            _ => false,
        }
    }

    /// The device whose driver settings P opens: an ASIO driver's own control panel
    /// (rate, buffer size, clock source, routing), or Audio MIDI Setup for a Core Audio
    /// device (rate, format, clock source). Other backends have none.
    pub fn panel_target(&self) -> Option<(BackendKind, DeviceId)> {
        let k = self.backend_kind()?;
        matches!(k, BackendKind::Asio | BackendKind::CoreAudio).then_some(())?;
        Some((k, self.device_info()?.id.clone()))
    }

    /// The chosen device's clock sources, the one in use marked; ASIO devices only.
    pub fn clock_sources_text(&self) -> Option<String> {
        ac2_scene::audio::clock_sources_text(&self.device_info()?.clock_sources)
    }

    /// The open session, if the dialog opened over one.
    pub fn open_session(&self) -> Option<&OpenSession> {
        self.open.as_ref()
    }

    /// A note when input and output of the chosen device are not known to share a clock:
    /// the open session plays on another device than it captures from, or the device's
    /// directions are separate endpoints. Measurements on the loopback reference are
    /// unaffected; the drift is measured once a stimulus plays
    /// (`docs/design/multi-device.md`).
    pub fn clock_note(&self) -> Option<String> {
        const CHECKED: &str = "the loopback monitor measures their drift while a stimulus plays";
        if self.is_open_device() {
            let o = self.open.as_ref()?;
            return (o.input_device != o.output_device || o.clock == ClockRelation::Unknown).then(
                || {
                    format!(
                        "Output plays on {}, another device than the input: the two may run on \
                         different clocks; {CHECKED}.",
                        o.output_device.0
                    )
                },
            );
        }
        let d = self.device_info()?;
        if let Some(out) = self.output_info()
            && out.id != d.id
        {
            return Some(format!(
                "Output plays on {}, another device than the input: the two may run on \
                 different clocks; {CHECKED}.",
                out.name
            ));
        }
        (d.duplex_clock == ClockRelation::Unknown && d.output.is_some()).then(|| {
            format!(
                "Input and output of this device are separate endpoints and may run on \
                 different clocks; {CHECKED}."
            )
        })
    }

    /// Where meters come from for the chosen device: a capture-only preview, unless the
    /// open session already captures it (its `session/levels` are shown then) or a
    /// loopback detection holds the device.
    pub fn preview_target(&self) -> Option<(BackendKind, DeviceId)> {
        if self.is_open_device() || self.detecting() {
            return None;
        }
        let b = self.backend_info()?;
        if b.availability != Availability::Available {
            return None;
        }
        let d = self.device_info()?;
        d.input.as_ref()?;
        Some((b.kind, d.id.clone()))
    }

    /// Builds the rows of the chosen device and applies its remembered roles: the
    /// preferences, else the open session (on this device), else defaults.
    fn load_device(&mut self, prefs: &UiPrefs) {
        self.detect = None;
        self.notice = None;
        self.error = None;
        self.preview_error = None;
        self.edit = None;
        let Some(d) = self.device_info().cloned() else {
            self.inputs.clear();
            self.outputs.clear();
            self.out_count = 0;
            return;
        };
        let names = |dir: Option<&ac2_proto::model::DirectionInfo>, ch: u16| {
            dir.and_then(|x| x.channel_names.as_ref())
                .and_then(|n| n.get(usize::from(ch)).cloned())
        };
        let n_in = d.input.as_ref().map_or(0, |i| i.max_channels);
        self.inputs = (0..n_in)
            .map(|channel| InputRow {
                channel,
                device_name: names(d.input.as_ref(), channel),
                in_session: false,
                role: InputRole::None,
                mic: String::new(),
                curve: CurveChoice::NotChosen,
            })
            .collect();
        let kind = self.backend_kind().unwrap_or(BackendKind::Fake);
        let key = UiPrefs::device_key(kind, &d.id.0);
        let remembered = prefs
            .sessions
            .get(&key)
            .cloned()
            .or_else(|| self.roles_of_open_session());
        self.out_device = remembered
            .as_ref()
            .and_then(|r| r.output_device.as_ref())
            .and_then(|id| self.output_by_id(&DeviceId(id.clone())))
            .unwrap_or_else(|| self.default_output());
        self.load_outputs();
        let n_out = self.outputs.len() as u16;
        let roles = remembered.unwrap_or_else(|| default_roles(kind, n_in, n_out));
        self.apply_roles(&roles);
        if !self.is_open_device() {
            self.rate.clear();
            self.buffer.clear();
        }
    }

    /// The output rows of the chosen output device, none ticked.
    fn load_outputs(&mut self) {
        let dir = self.output_info().and_then(|d| d.output.clone());
        let n_out = dir.as_ref().map_or(0, |o| o.max_channels);
        self.outputs = (0..n_out)
            .map(|channel| OutputRow {
                channel,
                device_name: dir
                    .as_ref()
                    .and_then(|x| x.channel_names.as_ref())
                    .and_then(|n| n.get(usize::from(channel)).cloned()),
                rig_label: rig_label(&self.labels, channel),
                stimulus: false,
            })
            .collect();
    }

    /// Another output device was chosen: its rows, with the stimulus outputs last used on
    /// it (none on a device never used) and the session's output count clamped to it.
    fn change_output(&mut self, to: OutputDevice, prefs: &UiPrefs) {
        self.out_device = to;
        self.detect = None;
        self.notice = None;
        self.error = None;
        self.load_outputs();
        let n_out = self.outputs.len() as u16;
        let stimulus: Vec<u16> = self
            .output_info()
            .and_then(|d| prefs.outputs_for(&d.id.0))
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|c| *c < n_out)
            .collect();
        let top_stim = stimulus.iter().map(|s| s + 1).max().unwrap_or(0);
        let wanted = if self.out_count == 0 {
            n_out.min(2)
        } else {
            self.out_count
        };
        self.out_count = wanted.max(top_stim).min(n_out);
        for o in &mut self.outputs {
            o.stimulus = stimulus.contains(&o.channel);
        }
    }

    fn roles_of_open_session(&self) -> Option<DeviceRoles> {
        if !self.is_open_device() {
            return None;
        }
        let open = self.open.as_ref()?;
        let c = &open.config;
        let mic_names: BTreeMap<u16, String> = self
            .setup
            .iter()
            .filter(|s| c.input_channels.contains(&s.channel))
            .filter_map(|s| Some((s.channel, s.mic.clone()?)))
            .collect();
        let reference = c.loopback.map(|l| l.input);
        Some(DeviceRoles {
            inputs: c.input_channels.clone(),
            outputs: c.output_channels,
            reference,
            mics: mic_names
                .keys()
                .copied()
                .filter(|m| Some(*m) != reference)
                .collect(),
            stimulus: c.loopback.map(|l| vec![l.output]).unwrap_or_default(),
            mic_names,
            output_device: (open.output_device != open.input_device)
                .then(|| open.output_device.0.clone()),
        })
    }

    fn apply_roles(&mut self, r: &DeviceRoles) {
        for row in &mut self.inputs {
            let c = row.channel;
            row.in_session = r.inputs.contains(&c) || r.reference == Some(c) || r.mics.contains(&c);
            row.role = if r.reference == Some(c) {
                InputRole::Reference
            } else if r.mics.contains(&c) {
                InputRole::Mic
            } else {
                InputRole::None
            };
            row.mic = r.mic_names.get(&c).cloned().unwrap_or_default();
        }
        let curves: Vec<CurveChoice> = self
            .inputs
            .iter()
            .map(|r| self.setup_curve(r.channel, &r.mic))
            .collect();
        for (row, c) in self.inputs.iter_mut().zip(curves) {
            row.curve = c;
        }
        let n_out = self.outputs.len() as u16;
        let top_stim = r.stimulus.iter().map(|s| s + 1).max().unwrap_or(0);
        self.out_count = r.outputs.max(top_stim).min(n_out);
        for o in &mut self.outputs {
            o.stimulus = r.stimulus.contains(&o.channel) && o.channel < self.out_count;
        }
    }

    /// ←/→ on the backend or device row: the next or previous one, stopping at the ends like
    /// every dialog stepper (an end press changes nothing, so the device is not reloaded).
    pub fn cycle(&mut self, d: i32, prefs: &UiPrefs) {
        let step = |i: usize, n: usize| (i as i64 + i64::from(d)).clamp(0, n as i64 - 1) as usize;
        match self.focus {
            Row::Backend => {
                let n = self.backends.as_ref().map_or(0, Vec::len);
                if n > 0 && step(self.backend, n) != self.backend {
                    self.backend = step(self.backend, n);
                    self.device = 0;
                    self.load_device(prefs);
                }
            }
            Row::Device => {
                let choices = self.input_choices();
                let at = self.input_position();
                if let Some(&next) = choices.get(step(at, choices.len().max(1)))
                    && next != self.device
                {
                    self.device = next;
                    self.load_device(prefs);
                }
            }
            Row::OutputDevice => {
                let choices = self.output_choices();
                let at = self.output_position();
                if let Some(&next) = choices.get(step(at, choices.len().max(1)))
                    && next != self.out_device
                {
                    self.change_output(next, prefs);
                }
            }
            _ => {}
        }
    }
}
