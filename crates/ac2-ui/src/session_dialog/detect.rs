//! Loopback detection: the burst that finds which input the stimulus returns on.

use super::*;

impl SessionDialog {
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
        if self.output_info().is_none() {
            return Err("choose an output device first (Audio & I/O page)".into());
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
        let output_device = self.output_info()?.id.clone();
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
            output_device,
            output: d.output,
            level: Dbfs(level),
        })
    }

    /// The daemon's answer: the loopback input becomes the Reference.
    pub fn detect_result(&mut self, r: Result<LoopbackDetection, String>) {
        let device = self.device_info().map(|x| x.id.clone());
        let output_device = self.output_info().map(|x| x.id.clone());
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
                let same = device.as_ref() == Some(&det.input_device)
                    && output_device.as_ref() == Some(&det.output_device);
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

    /// The default reference as its loopback pair, in words: which input the stimulus output
    /// returns on, and whether that is the open session's already.
    pub fn reference_text(&self) -> String {
        let input = self.inputs.iter().find(|r| r.role == InputRole::Reference);
        let output = self
            .stimulus_output()
            .and_then(|o| self.outputs.iter().find(|r| r.channel == o));
        match (input, output) {
            (Some(i), Some(o)) => {
                let now = self.is_open_output()
                    && self.open.as_ref().and_then(|s| s.config.loopback)
                        == Some(LoopbackRoute {
                            output: o.channel,
                            input: i.channel,
                        });
                format!(
                    "Reference (loopback): input {} · {} ← output {} · {}{}",
                    i.channel + 1,
                    i.label(),
                    o.channel + 1,
                    o.label(),
                    if now {
                        ""
                    } else {
                        " — applies when the session opens (Enter)"
                    }
                )
            }
            (Some(i), None) => format!(
                "Reference: input {} · {} — tick the output that feeds its loopback (S)",
                i.channel + 1,
                i.label()
            ),
            (None, _) => "No reference: R on the input the stimulus's loopback cable returns on \
                          (or D detects it)"
                .to_owned(),
        }
    }
}
