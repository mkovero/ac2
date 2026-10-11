//! The session the dialog opens: Enter turns the rows into a plan.

use super::*;

impl SessionDialog {
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
        let out_dev = self.output_info().unwrap_or(dev);
        if self.out_count > 0 && out_dev.output.is_none() {
            return Err(format!(
                "{} has no outputs: choose the output device on the Audio & I/O page.",
                out_dev.name
            ));
        }
        let config = SessionConfig {
            backend: Some(b.kind),
            input_device: DeviceSelector::Id { id: dev.id.clone() },
            output_device: DeviceSelector::Id {
                id: out_dev.id.clone(),
            },
            input_channels: inputs.clone(),
            output_channels: self.out_count,
            sample_rate_hz: rate,
            buffer_frames: buffer,
            loopback,
        };
        let setup: Vec<InputSetup> = self
            .inputs
            .iter()
            .enumerate()
            .filter(|(_, r)| r.in_session)
            .filter_map(|(i, _)| self.row_setup(i))
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
            output_device: (out_dev.id != dev.id).then(|| out_dev.id.0.clone()),
        };
        Ok(SessionPlan {
            config,
            device_name: if out_dev.id == dev.id {
                dev.name.clone()
            } else {
                format!("{} (output {})", dev.name, out_dev.name)
            },
            device_id: dev.id.0.clone(),
            output_device_id: out_dev.id.0.clone(),
            roles,
            inputs: setup,
            transfers,
        })
    }
}
