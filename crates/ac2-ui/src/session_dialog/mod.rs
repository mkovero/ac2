//! The audio I/O model behind two pages of Settings ([`crate::settings`]): **Audio**
//! (a backend, the input and output devices, rate and buffer; Shift+O) and **Inputs &
//! outputs** (one row per input and output with its name, a live meter and its role). Pure
//! data; the reducer routes keys here and the view draws it.
//!
//! The output device is the input device itself when it has outputs (one device keeps one
//! clock where the host allows it), else the system's default output: hosts that list each
//! direction as its own device, and a USB mic beside an interface, need another
//! device to play the stimulus on.
//!
//! Roles say what a channel is for, so nobody types channel numbers:
//! - **R — Reference**: the input the stimulus returns on through a loopback cable (one).
//! - **M — Measurement mic**: an input with a mic (any number; each can be named, and
//!   ←/→ on its row choose which of the mic's curves applies: off, 0°, 90° …).
//! - **S — Stimulus**: an output that feeds the speakers and the loopback.
//!
//! The session's inputs, outputs and loopback follow from the roles (and the rows put in
//! the session with Space). Choices are remembered per device ([`UiPrefs::sessions`]).
//! Outputs carry the rig's labels (`Main L`), kept by the daemon for every client; N on an
//! output row names it.

use std::collections::BTreeMap;

use ac2_proto::model::{
    Availability, BackendInfo, BackendKind, ClockRelation, CurveChoice, DeviceId, DeviceInfo,
    DeviceSelector, InputSetup, LoopbackDetection, LoopbackRoute, MAX_OUTPUT_LABEL, Mic,
    OpenSession, OutputSetup, SessionConfig,
};
use ac2_proto::units::Dbfs;
use ac2_scene::format;
use ac2_scene::meter::{input_name, output_name};

use crate::forms::{parse_buffer, parse_rate};
use crate::prefs::{DeviceRoles, UiPrefs};

mod detect;
mod devices;
mod edit;
mod plan;

pub use detect::REFERENCE_HELP;

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
    /// Which of the mic's curves applies.
    pub curve: CurveChoice,
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
    /// The rig's label of this output (`Main L`), kept by the daemon.
    pub rig_label: Option<String>,
    pub stimulus: bool,
}

impl OutputRow {
    /// The rig's label, else the backend's name, else `Output N`.
    pub fn label(&self) -> String {
        match &self.rig_label {
            Some(l) => l.clone(),
            None => output_name(self.channel, self.device_name.as_deref()),
        }
    }
}

/// A focusable row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Backend,
    /// The input (capture) device.
    Device,
    /// The output (playback) device.
    OutputDevice,
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
    /// The rig's label of output row `.0`; applied on Enter, dropped on ↑/↓.
    OutputLabel(usize),
    /// The level of the loopback detection burst.
    DetectLevel,
}

/// The device the session plays on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputDevice {
    /// The input device (one device for both directions).
    SameAsInput,
    /// Another device of the backend (an index into its devices).
    Other(usize),
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
    /// Input device id (the roles are remembered per input device).
    pub device_id: String,
    /// Output device id (the stimulus outputs are remembered per output device).
    pub output_device_id: String,
    /// Remembered for this device.
    pub roles: DeviceRoles,
    /// Mic names of the session's inputs (`session.inputs`).
    pub inputs: Vec<InputSetup>,
}

/// A loopback detection to run.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectRequest {
    pub backend: BackendKind,
    pub device: DeviceId,
    pub output_device: DeviceId,
    pub output: u16,
    pub level: Dbfs,
}

/// The dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionDialog {
    /// `None` while `session.devices` is on its way.
    pub backends: Option<Vec<BackendInfo>>,
    pub backend: usize,
    /// The input device (an index into the backend's devices).
    pub device: usize,
    pub out_device: OutputDevice,
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
    /// The output label being typed ([`Edit::OutputLabel`]).
    pub label_text: String,
    /// What just happened (a role moved, a detection answered).
    pub notice: Option<String>,
    pub detect: Option<DetectPanel>,
    /// Why the meters of a device cannot be shown.
    pub preview_error: Option<String>,
    /// The session open when the dialog opened.
    open: Option<OpenSession>,
    /// The daemon's input setup (mic names) when the dialog opened.
    setup: Vec<InputSetup>,
    /// The rig's output labels as last mirrored.
    labels: Vec<OutputSetup>,
}

/// Operator-facing backend name.
pub fn backend_name(b: BackendKind) -> &'static str {
    match b {
        BackendKind::Jack => "JACK",
        BackendKind::CoreAudio => "Core Audio",
        BackendKind::Fake => "Simulated rig",
        BackendKind::Replay => "Recording",
        BackendKind::Asio => "ASIO",
    }
}

/// What P opens for a backend's device: the button text and what is set there.
pub fn panel_label(b: BackendKind) -> (&'static str, &'static str) {
    match b {
        BackendKind::CoreAudio => (
            "Audio MIDI Setup… (P)",
            "rate, format and clock source are set there for every app",
        ),
        _ => (
            "Driver control panel… (P)",
            "rate, buffer, clock and routing are the driver's own settings",
        ),
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
            out_device: OutputDevice::SameAsInput,
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
            label_text: String::new(),
            notice: None,
            detect: None,
            preview_error: None,
            open: open.cloned(),
            setup: setup.to_vec(),
            labels: Vec::new(),
        }
    }

    /// Keeps the focus on a row that exists: rows of a device that is gone move it to the
    /// first row.
    fn keep_focus(&mut self) {
        if !self.rows().contains(&self.focus) {
            self.edit = None;
            if let Some(r) = self.rows().first() {
                self.focus = *r;
            }
        }
    }

    /// The rig's output labels changed (or arrived): the rows follow, an edit in progress
    /// keeps its text.
    pub fn set_labels(&mut self, labels: &[OutputSetup]) {
        self.labels = labels.to_vec();
        for o in &mut self.outputs {
            o.rig_label = rig_label(labels, o.channel);
        }
    }
}

/// The rig's label of output `channel`, if it has one.
fn rig_label(labels: &[OutputSetup], channel: u16) -> Option<String> {
    labels
        .iter()
        .find(|o| o.channel == channel)
        .and_then(|o| o.label.clone())
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
            output_device: None,
        };
    }
    DeviceRoles {
        inputs: (0..n_in.min(2)).collect(),
        outputs: n_out.min(2),
        ..DeviceRoles::default()
    }
}
