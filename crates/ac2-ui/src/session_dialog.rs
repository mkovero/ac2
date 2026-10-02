//! The audio session dialog (Shift+O): a backend, a device, and one row per input and output
//! with its name, a live meter and its role. Pure data; the reducer routes keys here and the
//! view draws it.
//!
//! Roles say what a channel is for, so nobody types channel numbers:
//! - **R — Reference**: the input the stimulus returns on through a loopback cable (one).
//! - **M — Measurement mic**: an input with a mic (any number; each can be named).
//! - **S — Stimulus**: an output that feeds the speakers and the loopback.
//!
//! The session's inputs, outputs and loopback follow from the roles (and the rows put in
//! the session with Space). Choices are remembered per device ([`UiPrefs::sessions`]).

use std::collections::BTreeMap;

use ac2_proto::model::{
    Availability, BackendInfo, BackendKind, DeviceId, DeviceInfo, DeviceSelector, InputSetup,
    LoopbackDetection, LoopbackRoute, MeasConfig, MeasKind, OpenSession, SessionConfig,
    TransferConfig,
};
use ac2_proto::units::Dbfs;
use ac2_scene::format;
use ac2_scene::meter::{input_name, output_name};

use crate::forms::{parse_buffer, parse_rate};
use crate::prefs::{DeviceRoles, UiPrefs};

/// Longest mic name the daemon accepts.
pub const MAX_MIC_NAME: usize = 64;

/// What an input is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputRole {
    None,
    /// The loopback return of the stimulus.
    Reference,
    /// A measurement mic.
    Mic,
}

/// A role key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleKey {
    Reference,
    Mic,
    Stimulus,
}

/// One device input.
#[derive(Clone, Debug, PartialEq)]
pub struct InputRow {
    pub channel: u16,
    /// What the backend calls it, if anything.
    pub device_name: Option<String>,
    pub in_session: bool,
    pub role: InputRole,
    /// Mic name (K8); empty = none.
    pub mic: String,
}

impl InputRow {
    /// The row's name: mic name, else the backend's, else `Input N`.
    pub fn label(&self) -> String {
        let mic = (self.role == InputRole::Mic).then_some(self.mic.as_str());
        input_name(self.channel, mic, self.device_name.as_deref())
    }
}

/// One device output.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputRow {
    pub channel: u16,
    pub device_name: Option<String>,
    pub stimulus: bool,
}

impl OutputRow {
    pub fn label(&self) -> String {
        output_name(self.channel, self.device_name.as_deref())
    }
}

/// A focusable row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Backend,
    Device,
    Input(usize),
    Output(usize),
    Rate,
    Buffer,
}

/// Text being typed into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// The mic name of input row `.0`.
    Mic(usize),
    /// The level of the loopback detection burst.
    DetectLevel,
}

/// Where the loopback detection stands.
#[derive(Clone, Debug, PartialEq)]
pub enum DetectPhase {
    /// Waiting for the operator's level and Enter.
    Confirm,
    /// The burst is playing.
    Running,
    /// Answered.
    Done(Box<LoopbackDetection>),
}

/// The "Detect loopback…" panel.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectPanel {
    /// Output the burst plays on (the stimulus output).
    pub output: u16,
    /// Typed level, dBFS; never defaulted.
    pub level: String,
    pub phase: DetectPhase,
    pub error: Option<String>,
}

/// What the dialog asks the daemon to do on Enter.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionPlan {
    pub config: SessionConfig,
    /// Device display name, for the toast.
    pub device_name: String,
    /// Device id (the stimulus outputs are remembered per output device).
    pub device_id: String,
    /// Remembered for this device.
    pub roles: DeviceRoles,
    /// Mic names of the session's inputs (`session.inputs`).
    pub inputs: Vec<InputSetup>,
    /// One transfer measurement per mic, against the reference.
    pub transfers: Vec<MeasConfig>,
}

/// A loopback detection to run.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectRequest {
    pub backend: BackendKind,
    pub device: DeviceId,
    pub output: u16,
    pub level: Dbfs,
}

/// The dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionDialog {
    /// `None` while `session.devices` is on its way.
    pub backends: Option<Vec<BackendInfo>>,
    pub backend: usize,
    pub device: usize,
    pub inputs: Vec<InputRow>,
    pub outputs: Vec<OutputRow>,
    /// Output channels of the session: outputs `0 .. out_count`.
    pub out_count: u16,
    pub rate: String,
    pub buffer: String,
    pub focus: Row,
    pub edit: Option<Edit>,
    /// Why the last Enter was refused (plain words).
    pub error: Option<String>,
    /// What just happened (a role moved, a detection answered).
    pub notice: Option<String>,
    pub detect: Option<DetectPanel>,
    /// Why the meters of a device cannot be shown.
    pub preview_error: Option<String>,
    /// The session open when the dialog opened.
    open: Option<OpenSession>,
    /// The daemon's input setup (mic names) when the dialog opened.
    setup: Vec<InputSetup>,
}

/// Operator-facing backend name.
pub fn backend_name(b: BackendKind) -> &'static str {
    match b {
        BackendKind::Jack => "JACK",
        BackendKind::Cpal => "System audio",
        BackendKind::Fake => "Simulated rig",
    }
}

/// `4 in / 2 out · 48 kHz · 256 frames`.
pub fn device_summary(d: &DeviceInfo) -> String {
    let ins = d.input.as_ref().map_or(0, |i| i.max_channels);
    let outs = d.output.as_ref().map_or(0, |o| o.max_channels);
    let dir = d.input.as_ref().or(d.output.as_ref());
    let mut s = format!("{ins} in / {outs} out");
    if let Some(r) = dir.and_then(|x| x.default_rate_hz) {
        s.push_str(&format!(" · {}", format::freq_readout(f64::from(r))));
    }
    if let Some(b) = dir.and_then(|x| x.default_buffer_frames) {
        s.push_str(&format!(" · {b} frames"));
    }
    s
}

impl SessionDialog {
    /// The dialog over the open session (if any), until the device list arrives.
    pub fn new(open: Option<&OpenSession>, setup: &[InputSetup]) -> Self {
        let c = open.map(|o| &o.config);
        Self {
            backends: None,
            backend: 0,
            device: 0,
            inputs: Vec::new(),
            outputs: Vec::new(),
            out_count: 0,
            rate: c
                .and_then(|c| c.sample_rate_hz)
                .map(|r| r.to_string())
                .unwrap_or_default(),
            buffer: c
                .and_then(|c| c.buffer_frames)
                .map(|b| b.to_string())
                .unwrap_or_default(),
            focus: Row::Backend,
            edit: None,
            error: None,
            notice: None,
            detect: None,
            preview_error: None,
            open: open.cloned(),
            setup: setup.to_vec(),
        }
    }

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
            .unwrap_or(0);
        self.load_device(prefs);
    }

    pub fn backend_info(&self) -> Option<&BackendInfo> {
        self.backends.as_ref()?.get(self.backend)
    }

    pub fn device_info(&self) -> Option<&DeviceInfo> {
        self.backend_info()?.devices.get(self.device)
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

    /// The open session, if the dialog opened over one.
    pub fn open_session(&self) -> Option<&OpenSession> {
        self.open.as_ref()
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
        let n_out = d.output.as_ref().map_or(0, |o| o.max_channels);
        self.inputs = (0..n_in)
            .map(|channel| InputRow {
                channel,
                device_name: names(d.input.as_ref(), channel),
                in_session: false,
                role: InputRole::None,
                mic: String::new(),
            })
            .collect();
        self.outputs = (0..n_out)
            .map(|channel| OutputRow {
                channel,
                device_name: names(d.output.as_ref(), channel),
                stimulus: false,
            })
            .collect();
        let kind = self.backend_kind().unwrap_or(BackendKind::Fake);
        let key = UiPrefs::device_key(kind, &d.id.0);
        let roles = prefs
            .sessions
            .get(&key)
            .cloned()
            .or_else(|| self.roles_of_open_session())
            .unwrap_or_else(|| default_roles(kind, n_in, n_out));
        self.apply_roles(&roles);
        if !self.is_open_device() {
            self.rate.clear();
            self.buffer.clear();
        }
    }

    fn roles_of_open_session(&self) -> Option<DeviceRoles> {
        if !self.is_open_device() {
            return None;
        }
        let c = &self.open.as_ref()?.config;
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
        let n_out = self.outputs.len() as u16;
        let top_stim = r.stimulus.iter().map(|s| s + 1).max().unwrap_or(0);
        self.out_count = r.outputs.max(top_stim).min(n_out);
        for o in &mut self.outputs {
            o.stimulus = r.stimulus.contains(&o.channel) && o.channel < self.out_count;
        }
    }

    /// ←/→ on the backend or device row.
    pub fn cycle(&mut self, d: i32, prefs: &UiPrefs) {
        match self.focus {
            Row::Backend => {
                let n = self.backends.as_ref().map_or(0, Vec::len) as i32;
                if n > 0 {
                    self.backend = (self.backend as i32 + d).rem_euclid(n) as usize;
                    self.device = 0;
                    self.load_device(prefs);
                }
            }
            Row::Device => {
                let n = self.backend_info().map_or(0, |b| b.devices.len()) as i32;
                if n > 0 {
                    self.device = (self.device as i32 + d).rem_euclid(n) as usize;
                    self.load_device(prefs);
                }
            }
            _ => {}
        }
    }

    // ----- focus -------------------------------------------------------------------------

    /// Focus order.
    pub fn rows(&self) -> Vec<Row> {
        let mut v = vec![Row::Backend, Row::Device];
        v.extend((0..self.inputs.len()).map(Row::Input));
        v.extend((0..self.outputs.len()).map(Row::Output));
        v.extend([Row::Rate, Row::Buffer]);
        v
    }

    /// ↑/↓ (Tab / Shift+Tab), wrapping. Ends a mic-name edit.
    pub fn move_focus(&mut self, d: i32) {
        self.finish_edit();
        let rows = self.rows();
        let i = rows.iter().position(|r| *r == self.focus).unwrap_or(0) as i32;
        let n = rows.len() as i32;
        self.focus = rows[(i + d).rem_euclid(n) as usize];
    }

    /// Focuses `row` (the mouse).
    pub fn focus_row(&mut self, row: Row) {
        if self.rows().contains(&row) {
            self.finish_edit();
            self.focus = row;
        }
    }

    /// The focused row takes typed text (rate, buffer) or an edit is running.
    pub fn text_focus(&self) -> bool {
        self.edit.is_some() || matches!(self.focus, Row::Rate | Row::Buffer)
    }

    // ----- roles -------------------------------------------------------------------------

    /// Space: puts the focused input or output in the session, or takes it out.
    pub fn toggle(&mut self) {
        self.notice = None;
        self.error = None;
        match self.focus {
            Row::Input(i) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    r.in_session = !r.in_session;
                    if !r.in_session {
                        r.role = InputRole::None;
                    }
                }
            }
            Row::Output(o) => {
                let o = o as u16;
                self.out_count = if o < self.out_count { o } else { o + 1 };
                let n = self.out_count;
                for row in &mut self.outputs {
                    if row.channel >= n {
                        row.stimulus = false;
                    }
                }
            }
            _ => {}
        }
    }

    /// R / M / S on the focused row.
    pub fn assign(&mut self, key: RoleKey) {
        self.error = None;
        self.notice = None;
        match (self.focus, key) {
            (Row::Input(i), RoleKey::Reference) => {
                if self
                    .inputs
                    .get(i)
                    .is_some_and(|r| r.role == InputRole::Reference)
                {
                    self.inputs[i].role = InputRole::None;
                    return;
                }
                for r in &mut self.inputs {
                    if r.role == InputRole::Reference {
                        r.role = InputRole::None;
                    }
                }
                if let Some(r) = self.inputs.get_mut(i) {
                    r.role = InputRole::Reference;
                    r.in_session = true;
                }
            }
            (Row::Input(i), RoleKey::Mic) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    r.role = if r.role == InputRole::Mic {
                        InputRole::None
                    } else {
                        r.in_session = true;
                        InputRole::Mic
                    };
                }
            }
            (Row::Output(o), RoleKey::Stimulus) => {
                if let Some(r) = self.outputs.get_mut(o) {
                    r.stimulus = !r.stimulus;
                    if r.stimulus {
                        self.out_count = self.out_count.max(r.channel + 1);
                    }
                }
            }
            (Row::Output(_), _) => {
                self.notice =
                    Some("R and M mark inputs: the loopback return (R) and the mics (M)".into());
            }
            (Row::Input(_), RoleKey::Stimulus) => {
                self.notice = Some(
                    "S marks an output: the one that feeds your speakers and the loopback".into(),
                );
            }
            _ => {
                self.notice = Some("move to an input or output row first (↑↓)".into());
            }
        }
    }

    /// N: types the focused input's mic name (it becomes a mic).
    pub fn start_mic_edit(&mut self) -> bool {
        let Row::Input(i) = self.focus else {
            self.notice = Some("N names the mic on an input row".into());
            return false;
        };
        if self
            .inputs
            .get(i)
            .is_some_and(|r| r.role == InputRole::Reference)
        {
            self.notice = Some("the reference is the loopback cable: it has no mic".into());
            return false;
        }
        if let Some(r) = self.inputs.get_mut(i) {
            r.role = InputRole::Mic;
            r.in_session = true;
        }
        self.edit = Some(Edit::Mic(i));
        true
    }

    /// Ends a mic-name edit (Enter, ↑/↓, Tab).
    pub fn finish_edit(&mut self) {
        if let Some(Edit::Mic(i)) = self.edit
            && let Some(r) = self.inputs.get_mut(i)
        {
            r.mic = r.mic.trim().to_owned();
            self.edit = None;
        }
    }

    /// Typed text into the edit or the focused text row.
    pub fn type_text(&mut self, s: &str) {
        if self.text_focus() {
            self.error = None;
        }
        match self.edit {
            Some(Edit::Mic(i)) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    for c in s.chars().filter(|c| !c.is_control()) {
                        if r.mic.chars().count() < MAX_MIC_NAME {
                            r.mic.push(c);
                        }
                    }
                }
            }
            Some(Edit::DetectLevel) => {
                if let Some(d) = &mut self.detect {
                    d.level.push_str(s);
                    d.error = None;
                }
            }
            None => match self.focus {
                Row::Rate => self.rate.push_str(s),
                Row::Buffer => self.buffer.push_str(s),
                _ => {}
            },
        }
    }

    pub fn backspace(&mut self) {
        match self.edit {
            Some(Edit::Mic(i)) => {
                if let Some(r) = self.inputs.get_mut(i) {
                    r.mic.pop();
                }
            }
            Some(Edit::DetectLevel) => {
                if let Some(d) = &mut self.detect {
                    d.level.pop();
                }
            }
            None => match self.focus {
                Row::Rate => {
                    self.rate.pop();
                }
                Row::Buffer => {
                    self.buffer.pop();
                }
                _ => {}
            },
        }
    }

    // ----- loopback detection --------------------------------------------------------------

    /// The stimulus output, if one is marked (the lowest).
    pub fn stimulus_output(&self) -> Option<u16> {
        self.outputs
            .iter()
            .filter(|o| o.stimulus && o.channel < self.out_count)
            .map(|o| o.channel)
            .min()
    }

    /// D: opens the confirmation (the burst plays only after the level is typed and Enter).
    /// The level field starts with the stimulus level the operator typed, if any; there is
    /// no default level.
    pub fn detect_start(&mut self, typed: Option<Dbfs>) -> Result<(), String> {
        self.finish_edit();
        if self.detecting() {
            return Err("the loopback detection is still playing; wait for its result".into());
        }
        if self
            .backend_info()
            .is_none_or(|b| b.availability != Availability::Available)
            || self.device_info().is_none()
        {
            return Err("pick an available device first".into());
        }
        let Some(output) = self.stimulus_output() else {
            return Err(
                "Pick the stimulus output first: S on the output that feeds your speakers and \
                 the loopback"
                    .into(),
            );
        };
        if self.inputs.is_empty() {
            return Err("this device has no inputs to listen on".into());
        }
        self.detect = Some(DetectPanel {
            output,
            level: typed
                .map(|l| format::fixed(l.0, 1).replace(format::MINUS, "-"))
                .unwrap_or_default(),
            phase: DetectPhase::Confirm,
            error: None,
        });
        self.edit = Some(Edit::DetectLevel);
        Ok(())
    }

    /// Closes the confirmation without playing anything.
    pub fn detect_cancel(&mut self) {
        if self
            .detect
            .as_ref()
            .is_some_and(|d| d.phase != DetectPhase::Running)
        {
            self.detect = None;
        }
        if self.edit == Some(Edit::DetectLevel) {
            self.edit = None;
        }
    }

    /// Enter on the confirmation: the detection to run, or why not.
    pub fn detect_confirm(&mut self, ceiling: Option<Dbfs>) -> Option<DetectRequest> {
        let backend = self.backend_kind()?;
        let device = self.device_info()?.id.clone();
        let d = self.detect.as_mut()?;
        if d.phase != DetectPhase::Confirm {
            return None;
        }
        let level = match crate::state::parse_number(&d.level, &["dbfs", "db"]) {
            Err(_) if d.level.trim().is_empty() => {
                d.error = Some(
                    "type the burst level in dBFS (e.g. -30): there is no default level".into(),
                );
                return None;
            }
            Err(e) => {
                d.error = Some(e);
                return None;
            }
            Ok(v) => v,
        };
        if level > 0.0 {
            d.error = Some("the level must be at or below 0 dBFS".into());
            return None;
        }
        if let Some(c) = ceiling
            && level > c.0
        {
            d.error = Some(format!(
                "above the daemon's ceiling {} dBFS",
                format::signed(c.0, 1)
            ));
            return None;
        }
        d.phase = DetectPhase::Running;
        d.error = None;
        self.edit = None;
        Some(DetectRequest {
            backend,
            device,
            output: d.output,
            level: Dbfs(level),
        })
    }

    /// The daemon's answer: the loopback input becomes the Reference.
    pub fn detect_result(&mut self, r: Result<LoopbackDetection, String>) {
        let device = self.device_info().map(|x| x.id.clone());
        let Some(d) = self.detect.as_mut() else {
            return;
        };
        match r {
            Err(e) => {
                d.phase = DetectPhase::Confirm;
                d.error = Some(e);
                self.edit = Some(Edit::DetectLevel);
            }
            Ok(det) => {
                let same = device.as_ref() == Some(&det.device);
                let found = det.loopback.filter(|_| same);
                d.phase = DetectPhase::Done(Box::new(det.clone()));
                if let Some(input) = found
                    && let Some(i) = self.inputs.iter().position(|r| r.channel == input)
                {
                    for r in &mut self.inputs {
                        if r.role == InputRole::Reference {
                            r.role = InputRole::None;
                        }
                    }
                    let row = &mut self.inputs[i];
                    row.role = InputRole::Reference;
                    row.in_session = true;
                    self.focus = Row::Input(i);
                }
            }
        }
    }

    /// The result in plain words.
    pub fn detect_text(&self) -> Option<String> {
        let d = self.detect.as_ref()?;
        let out = self
            .outputs
            .iter()
            .find(|o| o.channel == d.output)
            .map_or_else(|| output_name(d.output, None), OutputRow::label);
        Some(match &d.phase {
            DetectPhase::Confirm => format!(
                "Detect loopback: plays a 0.5 s noise burst on output {} ({out}) at the level \
                 you type, then finds the input it returns on.",
                d.output + 1
            ),
            DetectPhase::Running => {
                format!("Playing the burst on output {} ({out})…", d.output + 1)
            }
            DetectPhase::Done(det) => {
                let name = |input: u16| {
                    self.inputs
                        .iter()
                        .find(|r| r.channel == input)
                        .map_or_else(|| input_name(input, None, None), InputRow::label)
                };
                match (det.loopback, det.ranked.first()) {
                    (Some(i), Some(best)) => format!(
                        "Loopback found on input {} ({}): {}, correlation {} — set as Reference.",
                        i + 1,
                        name(i),
                        format::ms(best.delay.0, 2),
                        format::fixed(best.correlation.abs(), 3)
                    ),
                    (None, Some(best)) => format!(
                        "No loopback found: the best match, input {} ({}), correlates only {}. \
                         Check the cable from output {} or raise the level.",
                        best.input + 1,
                        name(best.input),
                        format::fixed(best.correlation.abs(), 2),
                        d.output + 1
                    ),
                    _ => "No inputs answered.".into(),
                }
            }
        })
    }

    // ----- the session -------------------------------------------------------------------

    /// A detection burst is playing.
    pub fn detecting(&self) -> bool {
        self.detect
            .as_ref()
            .is_some_and(|d| d.phase == DetectPhase::Running)
    }

    /// Enter: what to open, or why not (in plain words).
    pub fn plan(&self) -> Result<SessionPlan, String> {
        if self.detecting() {
            return Err(
                "The loopback detection is still playing: wait for its result, then Enter.".into(),
            );
        }
        let Some(backends) = &self.backends else {
            return Err("The device list has not arrived yet.".into());
        };
        let Some(b) = backends.get(self.backend) else {
            return Err("The daemon offers no audio backend.".into());
        };
        if let Availability::Unavailable { reason } = &b.availability {
            // The reason carries its own remedy; another backend is a way out only where
            // the daemon offers one.
            let other = if backends.len() > 1 {
                " Or pick another backend (← → on the first row)."
            } else {
                ""
            };
            return Err(format!(
                "{} is not available: {reason}.{other}",
                backend_name(b.kind)
            ));
        }
        let Some(dev) = self.device_info() else {
            return Err(format!("{} lists no device.", backend_name(b.kind)));
        };
        let inputs: Vec<u16> = self
            .inputs
            .iter()
            .filter(|r| r.in_session)
            .map(|r| r.channel)
            .collect();
        if inputs.is_empty() {
            return Err(
                "Choose at least one input: Space adds the focused input to the session.".into(),
            );
        }
        let reference = self
            .inputs
            .iter()
            .find(|r| r.role == InputRole::Reference)
            .map(|r| r.channel);
        let mics: Vec<&InputRow> = self
            .inputs
            .iter()
            .filter(|r| r.role == InputRole::Mic)
            .collect();
        let stimulus: Vec<u16> = self
            .outputs
            .iter()
            .filter(|o| o.stimulus && o.channel < self.out_count)
            .map(|o| o.channel)
            .collect();
        if !mics.is_empty() && reference.is_none() {
            return Err(
                "Pick a reference input: the loopback from your stimulus output (R on its row, \
                 or D to detect it)."
                    .into(),
            );
        }
        if reference.is_some() && stimulus.is_empty() {
            return Err(
                "Pick the stimulus output: the output that feeds your speakers and the loopback \
                 (S on its row)."
                    .into(),
            );
        }
        let mut seen: Vec<&str> = Vec::new();
        for m in &mics {
            if m.mic.is_empty() {
                continue;
            }
            if seen.contains(&m.mic.as_str()) {
                return Err(format!(
                    "Two mics are named {:?}: give each its own name (N on its row).",
                    m.mic
                ));
            }
            seen.push(&m.mic);
        }
        let rate = parse_rate(&self.rate).map_err(|e| format!("Sample rate: {e}."))?;
        let buffer = parse_buffer(&self.buffer).map_err(|e| format!("Buffer: {e}."))?;
        let loopback = reference.and_then(|input| {
            stimulus
                .first()
                .map(|&output| LoopbackRoute { output, input })
        });
        let sel = DeviceSelector::Id { id: dev.id.clone() };
        let config = SessionConfig {
            backend: Some(b.kind),
            input_device: sel.clone(),
            output_device: sel,
            input_channels: inputs.clone(),
            output_channels: self.out_count,
            sample_rate_hz: rate,
            buffer_frames: buffer,
            loopback,
        };
        let mic_curve = |c: u16| {
            self.setup
                .iter()
                .find(|s| s.channel == c)
                .is_none_or(|s| s.mic_curve)
        };
        let setup: Vec<InputSetup> = self
            .inputs
            .iter()
            .filter(|r| r.in_session)
            .map(|r| InputSetup {
                channel: r.channel,
                mic: (r.role == InputRole::Mic && !r.mic.is_empty()).then(|| r.mic.clone()),
                mic_curve: mic_curve(r.channel),
            })
            .collect();
        let transfers = match reference {
            Some(r) if !stimulus.is_empty() => mics
                .iter()
                .map(|m| MeasConfig {
                    name: format!("Reference → {}", m.label()),
                    kind: MeasKind::Transfer {
                        config: TransferConfig::with_inputs(r, m.channel),
                    },
                })
                .collect(),
            _ => Vec::new(),
        };
        let roles = DeviceRoles {
            inputs,
            outputs: self.out_count,
            reference,
            mics: mics.iter().map(|m| m.channel).collect(),
            stimulus,
            mic_names: self
                .inputs
                .iter()
                .filter(|r| !r.mic.is_empty())
                .map(|r| (r.channel, r.mic.clone()))
                .collect(),
        };
        Ok(SessionPlan {
            config,
            device_name: dev.name.clone(),
            device_id: dev.id.0.clone(),
            roles,
            inputs: setup,
            transfers,
        })
    }
}

/// Roles of a device never used before: the simulated rig's own wiring (out 1 → in 1
/// loopback, in 2 the room mic); on a real interface the first two inputs and outputs and
/// no roles — its wiring is the operator's to say.
fn default_roles(kind: BackendKind, n_in: u16, n_out: u16) -> DeviceRoles {
    if kind == BackendKind::Fake && n_in >= 2 && n_out >= 1 {
        return DeviceRoles {
            inputs: vec![0, 1],
            outputs: n_out.min(2),
            reference: Some(0),
            mics: vec![1],
            stimulus: vec![0],
            mic_names: BTreeMap::new(),
        };
    }
    DeviceRoles {
        inputs: (0..n_in.min(2)).collect(),
        outputs: n_out.min(2),
        ..DeviceRoles::default()
    }
}
