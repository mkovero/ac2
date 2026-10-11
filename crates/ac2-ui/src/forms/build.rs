//! The dialogs' constructors and the `meas.create` / sweep configs they build.

use super::*;

impl Form {
    pub(crate) fn new(kind: FormKind, fields: Vec<Field>) -> Self {
        Self {
            kind,
            fields,
            focus: 0,
            error: None,
            selected: false,
            math: None,
            sweep_edit: None,
            meas_edit: None,
            ir_import: None,
            averaging: AVERAGING.to_vec(),
            sample_rate: None,
        }
    }

    /// The resolution (one FFT's size, or the multi-resolution bands) and window fields
    /// of a spectrum dialog showing `r` and `w`, with the resolution's hint stated at the
    /// session's rate (set [`Form::sample_rate`] first).
    pub(super) fn fft_fields(&self, r: SpecResolution, w: Window, level: SpecLevel) -> [Field; 3] {
        let all = spec_resolutions();
        let labels: Vec<String> = all
            .iter()
            .map(|r| ac2_scene::format::spec_resolution(*r))
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let index = all
            .iter()
            .position(|x| *x == r)
            .unwrap_or(FFT_LENS.len() - 1);
        let mut size = Field::choice(FieldId::FftSize, "Resolution", &labels, index);
        size.hint = self.fft_hint(all[index]);
        let names = WINDOWS.map(|w| {
            let l = ac2_scene::spectrum::window_label(w);
            l.strip_suffix(" window").unwrap_or(l)
        });
        let window = Field::choice(
            FieldId::Window,
            "Window",
            &names,
            WINDOWS.iter().position(|x| *x == w).unwrap_or(0),
        );
        let mut lv = Field::choice(
            FieldId::SpecLevel,
            "Level",
            &SPEC_LEVELS.map(|l| l.0),
            SPEC_LEVELS.iter().position(|l| l.1 == level).unwrap_or(0),
        );
        lv.hint = spec_level_hint(level).to_owned();
        [size, window, lv]
    }

    /// The level the spectrum dialog shows (tone when it has no level field).
    pub(super) fn spec_level(&self) -> SpecLevel {
        self.choice_index(FieldId::SpecLevel)
            .and_then(|i| SPEC_LEVELS.get(i))
            .map_or(SpecLevel::Tone, |l| l.1)
    }

    /// Bin width and update rate of an `n`-point spectrum at the session's rate; what the
    /// multi-resolution bands trade.
    pub(super) fn fft_hint(&self, r: SpecResolution) -> String {
        match r {
            SpecResolution::Fixed { fft_len } => self
                .sample_rate
                .map(|fs| ac2_scene::format::fft_resolution(fft_len, fs))
                .unwrap_or_default(),
            SpecResolution::Multi { .. } => ac2_scene::format::MULTIRES_HINT.to_owned(),
        }
    }

    /// The averaging field showing `current` (added to the options when it is none of them,
    /// so an edit never changes it silently).
    pub(super) fn averaging_field(&mut self, current: SpecAveraging) -> Field {
        if !self.averaging.contains(&current) {
            self.averaging.push(current);
        }
        let options: Vec<String> = self
            .averaging
            .iter()
            .map(|a| ac2_scene::format::spec_averaging(*a))
            .collect();
        let index = self
            .averaging
            .iter()
            .position(|a| *a == current)
            .unwrap_or(0);
        Field {
            id: FieldId::Averaging,
            label: "Averaging".into(),
            value: Value::Choice { options, index },
            hint: "on power".into(),
        }
    }

    /// The dialog editing transfer measurement, spectrum or RTA `m` over the session's
    /// captured `inputs`: a transfer's inputs, name, smoothing, depth and resolution; a
    /// spectrum's or RTA's input, name, smoothing or bands (a spectrum's FFT size and
    /// window too, its bin width stated at `sample_rate`), and averaging.
    pub fn edit_spec(
        m: &Measurement,
        inputs: &[(u16, String)],
        sample_rate: Option<u32>,
    ) -> Result<Self, String> {
        if let MeasKind::Transfer { config } = &m.config.kind {
            return Ok(Self::edit_transfer(m, config, inputs));
        }
        let (kind, input, averaging) = match &m.config.kind {
            MeasKind::Spectrum { config } => {
                (FormKind::SpectrumEdit, config.input, config.averaging)
            }
            MeasKind::Rta { config } => (FormKind::RtaEdit, config.input, config.averaging),
            _ => return Err(format!("{} is not a spectrum or an RTA", m.config.name)),
        };
        let mut f = Self::new(kind, Vec::new());
        f.sample_rate = sample_rate;
        // An input the session no longer captures stays listed, so the edit does not move
        // the measurement to another input unasked (Enter then says to reopen the session).
        let mut inputs = inputs.to_vec();
        if !inputs.iter().any(|(c, _)| *c == input) {
            inputs.push((input, format!("In {} (not captured)", input + 1)));
        }
        let mut fields = vec![
            Field::channel(FieldId::Input, "Input", &inputs, Some(input), ""),
            Field::text(FieldId::Name, "Name", m.config.name.clone(), ""),
        ];
        match &m.config.kind {
            MeasKind::Spectrum { config } => {
                fields.push(Field::choice(
                    FieldId::Smoothing,
                    "Smoothing",
                    &SMOOTHING.map(|s| s.0),
                    SMOOTHING
                        .iter()
                        .position(|s| s.1 == config.smoothing)
                        .unwrap_or(0),
                ));
                fields.extend(f.fft_fields(config.resolution, config.window, config.level));
            }
            MeasKind::Rta { config } => fields.push(Field::choice(
                FieldId::Fraction,
                "Bands",
                &FRACTIONS.map(|f| f.0),
                FRACTIONS
                    .iter()
                    .position(|f| f.1 == config.fraction)
                    .unwrap_or(1),
            )),
            _ => {}
        }
        fields.push(f.averaging_field(averaging));
        f.fields = fields;
        f.meas_edit = Some(Box::new(m.clone()));
        Ok(f)
    }

    pub(super) fn edit_transfer(
        m: &Measurement,
        c: &TransferConfig,
        inputs: &[(u16, String)],
    ) -> Self {
        // Inputs the session no longer captures stay listed, as for a spectrum's.
        let mut inputs = inputs.to_vec();
        for i in [c.reference_input, c.measurement_input] {
            if !inputs.iter().any(|(x, _)| *x == i) {
                inputs.push((i, format!("In {} (not captured)", i + 1)));
            }
        }
        let smoothing = SMOOTHING
            .iter()
            .position(|s| s.1 == c.smoothing.map(|s| s.fraction))
            .unwrap_or(0);
        let depth = usize::from(matches!(c.depth, DepthPolicy::FastLf { .. }));
        let fields = vec![
            Field::channel(
                FieldId::Reference,
                "Reference",
                &inputs,
                Some(c.reference_input),
                "the loopback (stimulus copy)",
            ),
            Field::channel(
                FieldId::Measurement,
                "Measurement",
                &inputs,
                Some(c.measurement_input),
                "the mic",
            ),
            Field::text(FieldId::Name, "Name", m.config.name.clone(), ""),
            Field::choice(
                FieldId::Smoothing,
                "Smoothing",
                &SMOOTHING.map(|s| s.0),
                smoothing,
            ),
            Field::choice(FieldId::Depth, "Depth", &DEPTH, depth),
            resolution_field(c.resolution),
        ];
        let mut f = Self::new(FormKind::TransferEdit, fields);
        f.meas_edit = Some(Box::new(m.clone()));
        f
    }

    /// The measurement this dialog edits (a math channel, a spectrum or an RTA), if any.
    pub fn edits(&self) -> Option<MeasId> {
        self.math_edit().or(self.meas_edit.as_ref().map(|m| m.id))
    }

    /// A measurement dialog over the session's captured `inputs` (channel, name). The
    /// reference defaults to the session's loopback input, the measurement input to the
    /// first mic (`mics`: inputs with a mic name), else the first other captured input.
    pub fn measurement(
        kind: FormKind,
        open: Option<&OpenSession>,
        existing: &[&Measurement],
        inputs: &[(u16, String)],
        mics: &[u16],
    ) -> Self {
        let captured: Vec<u16> = inputs.iter().map(|(c, _)| *c).collect();
        let reference = open
            .and_then(|o| o.config.loopback.map(|l| l.input))
            .filter(|r| captured.contains(r))
            .or_else(|| captured.first().copied())
            .unwrap_or(0);
        let measurement = mics
            .iter()
            .copied()
            .find(|m| *m != reference && captured.contains(m))
            .or_else(|| captured.iter().copied().find(|c| *c != reference))
            .unwrap_or(reference);
        let n = existing
            .iter()
            .filter(|m| form_kind(&m.config.kind) == kind)
            .count()
            + 1;
        let name = |base: &str| Field::text(FieldId::Name, "Name", format!("{base} {n}"), "");
        let input_field = Field::channel(FieldId::Input, "Input", inputs, Some(measurement), "");
        let fields = match kind {
            // Built by [`Form::sweep`], [`Form::math`] and [`crate::ir_import_dialog::form`].
            FormKind::Sweep
            | FormKind::SweepEdit
            | FormKind::Math
            | FormKind::MathEdit
            | FormKind::SpectrumEdit
            | FormKind::RtaEdit
            | FormKind::TransferEdit
            | FormKind::IrImport => Vec::new(),
            FormKind::Transfer => vec![
                Field::channel(
                    FieldId::Reference,
                    "Reference",
                    inputs,
                    Some(reference),
                    "the loopback (stimulus copy)",
                ),
                Field::channel(
                    FieldId::Measurement,
                    "Measurement",
                    inputs,
                    Some(measurement),
                    "the mic",
                ),
                name("TF"),
                Field::choice(FieldId::Smoothing, "Smoothing", &SMOOTHING.map(|s| s.0), 0),
                Field::choice(FieldId::Depth, "Depth", &DEPTH, 0),
                resolution_field(Resolution::default()),
            ],
            FormKind::Spectrum => vec![
                input_field,
                name("Spectrum"),
                Field::choice(FieldId::Smoothing, "Smoothing", &SMOOTHING.map(|s| s.0), 0),
            ],
            FormKind::Rta => vec![
                input_field,
                name("RTA"),
                Field::choice(FieldId::Fraction, "Bands", &FRACTIONS.map(|f| f.0), 1),
            ],
            FormKind::Spl => vec![
                input_field,
                name("SPL"),
                // An SPL meter reads A-weighted Fast by default: the level hearing limits and
                // venue rules are written in.
                Field::choice(FieldId::Weighting, "Weighting", &WEIGHTINGS.map(|w| w.0), 1),
                Field::choice(
                    FieldId::TimeWeighting,
                    "Time weighting",
                    &TIME_WEIGHTINGS.map(|w| w.0),
                    0,
                ),
            ],
        };
        let mut f = Self::new(kind, fields);
        f.sample_rate = open.map(|o| o.sample_rate_hz);
        if kind == FormKind::Spectrum {
            let d = SpectrumConfig::on_input(0);
            let fft = f.fft_fields(d.resolution, d.window, d.level);
            f.fields.extend(fft);
        }
        if matches!(kind, FormKind::Spectrum | FormKind::Rta) {
            let a = f.averaging_field(SpecAveraging::Off);
            f.fields.push(a);
        }
        f
    }

    /// The sweep dialog over the session's captured `inputs` and its `outputs` (channel,
    /// name). The reference is the session's loopback input; without one it is left for the
    /// operator to choose, because the analysis divides by it and an input picked by channel
    /// order would give a plausible but meaningless result. Mic and speaker output default
    /// as for a transfer measurement; the loopback output (when the session has one) always
    /// plays the sweep too. `level` is the operator's typed stimulus level, if any: there is
    /// no default.
    pub fn sweep(
        open: Option<&OpenSession>,
        sweeps: usize,
        inputs: &[(u16, String)],
        outputs: &[(u16, String)],
        mics: &[u16],
        level: Option<Dbfs>,
    ) -> Self {
        let base = Self::measurement(FormKind::Transfer, open, &[], inputs, mics);
        let reference = open
            .and_then(|o| o.config.loopback.map(|l| l.input))
            .filter(|r| inputs.iter().any(|(c, _)| c == r));
        let loopback_out = open.and_then(|o| o.config.loopback.map(|l| l.output));
        let speaker = outputs
            .iter()
            .map(|(c, _)| *c)
            .find(|c| Some(*c) != loopback_out)
            .or_else(|| outputs.first().map(|(c, _)| *c))
            .unwrap_or(0);
        let level_text = level.map_or_else(String::new, |l| {
            ac2_scene::format::fixed(l.0, 1).replace(ac2_scene::format::MINUS, "-")
        });
        let hint_out = match loopback_out {
            Some(l) => {
                let name = outputs
                    .iter()
                    .find(|(c, _)| *c == l)
                    .map_or_else(|| format!("output {}", l + 1), |(_, n)| n.clone());
                format!("the speaker; the loopback output, {name}, plays too")
            }
            None => "the speaker".to_owned(),
        };
        let fields = vec![
            Field::channel(
                FieldId::Reference,
                "Reference",
                inputs,
                reference,
                "the loopback (stimulus copy)",
            ),
            Field::channel(
                FieldId::Measurement,
                "Measurement",
                inputs,
                base.channel(FieldId::Measurement),
                "the mic",
            ),
            Field::channel(FieldId::Output, "Output", outputs, Some(speaker), &hint_out),
            Field::text(
                FieldId::Level,
                "Level",
                level_text,
                "dBFS RMS, required (e.g. -50)",
            ),
            Field::text(FieldId::From, "From", "20 Hz", ""),
            Field::text(FieldId::To, "To", "20 kHz", ""),
            Field::choice(
                FieldId::Duration,
                "Duration",
                &DURATIONS.map(|d| d.0),
                DEFAULT_DURATION,
            ),
            Field::choice(FieldId::Repeats, "Repeats", &REPEATS.map(|r| r.0), 0),
            Field::choice(FieldId::Tail, "Silence after", &TAILS.map(|t| t.0), 0),
            Field {
                hint: LF_HARMONICS_HINT.into(),
                ..Field::choice(
                    FieldId::LfHarmonics,
                    "LF harmonics",
                    &LF_HARMONICS.map(|l| l.0),
                    0,
                )
            },
            resolution_field(Resolution::default()),
            Field::text(FieldId::Name, "Name", format!("Sweep {}", sweeps + 1), ""),
        ];
        Self::new(FormKind::Sweep, fields)
    }

    /// The dialog editing sweep measurement `m`: its settings as the fields. The name stays
    /// the measurement's.
    pub fn edit_sweep(
        m: &Measurement,
        open: Option<&OpenSession>,
        inputs: &[(u16, String)],
        outputs: &[(u16, String)],
    ) -> Result<Self, String> {
        let MeasKind::Sweep { config: c } = &m.config.kind else {
            return Err(format!("{} is not a sweep measurement", m.config.name));
        };
        let mut f = Self::sweep(open, 0, inputs, outputs, &[], Some(c.level));
        f.kind = FormKind::SweepEdit;
        f.sweep_edit = Some(m.id);
        let nearest = |v: f64, opts: &[f64]| {
            opts.iter()
                .enumerate()
                .min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs()))
                .map_or(0, |(i, _)| i)
        };
        let durations = DURATIONS.map(|d| d.1);
        let tails = TAILS.map(|t| t.1);
        let repeats = REPEATS.map(|r| f64::from(r.1));
        let speaker = c
            .outputs
            .iter()
            .copied()
            .find(|o| open.and_then(|o| o.config.loopback).map(|l| l.output) != Some(*o))
            .or_else(|| c.outputs.first().copied());
        for field in &mut f.fields {
            match (field.id, &mut field.value) {
                (
                    FieldId::Reference,
                    Value::Channel {
                        channels, index, ..
                    },
                ) => {
                    *index = channels.iter().position(|x| *x == c.reference_input);
                }
                (
                    FieldId::Measurement,
                    Value::Channel {
                        channels, index, ..
                    },
                ) => {
                    *index = channels.iter().position(|x| *x == c.measurement_input);
                }
                (
                    FieldId::Output,
                    Value::Channel {
                        channels, index, ..
                    },
                ) => {
                    *index = speaker.and_then(|s| channels.iter().position(|x| *x == s));
                }
                (FieldId::From, Value::Text(t)) => {
                    *t = ac2_scene::format::freq_readout(c.sweep.start.0)
                }
                (FieldId::To, Value::Text(t)) => {
                    *t = ac2_scene::format::freq_readout(c.sweep.end.0)
                }
                (FieldId::Duration, Value::Choice { index, .. }) => {
                    *index = nearest(c.sweep.duration.0, &durations);
                }
                (FieldId::Repeats, Value::Choice { index, .. }) => {
                    *index = nearest(f64::from(c.repeats), &repeats);
                }
                (FieldId::Tail, Value::Choice { index, .. }) => {
                    *index = nearest(c.tail.map_or(1.0, |t| t.0), &tails);
                }
                (FieldId::LfHarmonics, Value::Choice { index, .. }) => {
                    *index = LF_HARMONICS
                        .iter()
                        .position(|l| l.1 == c.lf_harmonics)
                        .unwrap_or(0);
                }
                (FieldId::Resolution, Value::Choice { index, .. }) => {
                    *index = RESOLUTIONS
                        .iter()
                        .position(|r| r.1 == c.resolution)
                        .unwrap_or(DEFAULT_RESOLUTION);
                }
                (FieldId::Name, Value::Text(t)) => *t = m.config.name.clone(),
                _ => {}
            }
        }
        Ok(f)
    }

    /// The sweep measurement the dialog makes (or its new settings), checked against the
    /// session and the daemon's ceiling.
    pub fn sweep_meas(
        &self,
        open: Option<&OpenSession>,
        ceiling: Option<Dbfs>,
    ) -> Result<MeasConfig, String> {
        let open = open.ok_or("no open audio session")?;
        let captured = &open.config.input_channels;
        let input = |id: FieldId, what: &str| -> Result<u16, String> {
            let c = self
                .channel(id)
                .ok_or_else(|| format!("{what}: the session captures no input"))?;
            if captured.contains(&c) {
                Ok(c)
            } else {
                Err(format!("{what} {} is not captured by the session", c + 1))
            }
        };
        if self.channel(FieldId::Reference).is_none() {
            return Err(
                "choose the reference: the input that records the loopback (a copy of the \
                 stimulus)"
                    .into(),
            );
        }
        let reference = input(FieldId::Reference, "reference input")?;
        let measurement = input(FieldId::Measurement, "measurement input")?;
        if reference == measurement {
            return Err(
                "the reference and the measurement are the same input: pick the mic as the \
                 measurement"
                    .into(),
            );
        }
        let level = self.text(FieldId::Level).trim();
        if level.is_empty() {
            return Err("type the level (dBFS): a sweep has no default level".into());
        }
        let level = crate::state::parse_number(level, &["dbfs", "db"])?;
        if level > 0.0 {
            return Err("level must be ≤ 0 dBFS".into());
        }
        if let Some(c) = ceiling
            && level > c.0
        {
            return Err(format!(
                "above the daemon's ceiling {} dBFS",
                ac2_scene::format::fixed(c.0, 1)
            ));
        }
        let from = parse_freq(self.text(FieldId::From))?;
        let to = parse_freq(self.text(FieldId::To))?;
        let nyquist = f64::from(open.sample_rate_hz) / 2.0;
        if from >= to || to > nyquist {
            return Err(format!(
                "the sweep must run up, within the rate's limit of {}",
                ac2_scene::format::freq_readout(nyquist)
            ));
        }
        let speaker = self
            .channel(FieldId::Output)
            .ok_or("no output to play on")?;
        let mut outputs = vec![speaker];
        if let Some(l) = open.config.loopback
            && l.output != speaker
        {
            outputs.push(l.output);
        }
        let duration = DURATIONS[self
            .choice_index(FieldId::Duration)
            .unwrap_or(DEFAULT_DURATION)]
        .1;
        let repeats = REPEATS[self.choice_index(FieldId::Repeats).unwrap_or(0)].1;
        let tail = TAILS[self.choice_index(FieldId::Tail).unwrap_or(0)].1;
        let lf_harmonics = LF_HARMONICS[self.choice_index(FieldId::LfHarmonics).unwrap_or(0)].1;
        let resolution = self.resolution();
        let name = self.text(FieldId::Name).trim();
        if name.is_empty() {
            return Err("type a name".into());
        }
        Ok(MeasConfig {
            name: name.to_owned(),
            kind: MeasKind::Sweep {
                config: SweepConfig {
                    reference_input: reference,
                    measurement_input: measurement,
                    outputs,
                    level: Dbfs(level),
                    sweep: EssSpec::with_fades(Hz(from), Hz(to), Seconds(duration)),
                    repeats,
                    gate: None,
                    tail: Some(Seconds(tail)),
                    lf_harmonics,
                    resolution,
                },
            },
        })
    }

    /// The Resolution field's choice (the default when the dialog has none).
    pub(crate) fn resolution(&self) -> Resolution {
        RESOLUTIONS[self
            .choice_index(FieldId::Resolution)
            .unwrap_or(DEFAULT_RESOLUTION)
            .min(RESOLUTIONS.len() - 1)]
        .1
    }
}

impl Form {
    /// `meas.create` of the dialog. Inputs must be captured by the open session.
    pub fn meas_config(&self, open: Option<&OpenSession>) -> Result<MeasConfig, String> {
        let captured = open.map(|o| o.config.input_channels.as_slice());
        let input = |id: FieldId, what: &str| -> Result<u16, String> {
            let c = self
                .channel(id)
                .ok_or_else(|| format!("{what}: the session captures no input"))?;
            match captured {
                Some(cap) if !cap.contains(&c) => Err(format!(
                    "{what} {} is not captured by the session: reopen it with that input (Space on its row)",
                    c + 1
                )),
                _ => Ok(c),
            }
        };
        let pick = |id: FieldId| self.choice_index(id).unwrap_or(0);
        let smoothing = SMOOTHING[pick(FieldId::Smoothing).min(SMOOTHING.len() - 1)].1;
        let averaging = self
            .averaging
            .get(pick(FieldId::Averaging))
            .copied()
            .unwrap_or(SpecAveraging::Off);
        let base = self.meas_edit.as_ref().map(|m| &m.config.kind);
        let kind = match self.kind {
            FormKind::Sweep | FormKind::SweepEdit => return Err("a sweep dialog".into()),
            FormKind::IrImport => return Err("an import dialog".into()),
            FormKind::Math | FormKind::MathEdit => return self.math_config(),
            FormKind::Transfer | FormKind::TransferEdit => {
                let r = input(FieldId::Reference, "reference input")?;
                let m = input(FieldId::Measurement, "measurement input")?;
                if r == m {
                    return Err(
                        "the reference and the measurement are the same input: pick the mic as \
                         the measurement"
                            .into(),
                    );
                }
                // An edit keeps what the dialog does not show (averaging, delay tracking).
                let mut config = match base {
                    Some(MeasKind::Transfer { config }) => TransferConfig {
                        reference_input: r,
                        measurement_input: m,
                        ..config.clone()
                    },
                    _ => TransferConfig::with_inputs(r, m),
                };
                config.smoothing = smoothing.map(|f| Smoothing {
                    fraction: f,
                    ..config.smoothing.unwrap_or(Smoothing::of(f))
                });
                config.depth = match (pick(FieldId::Depth), config.depth) {
                    (1, d @ DepthPolicy::FastLf { .. }) => d,
                    (1, _) => DepthPolicy::FastLf {
                        max_settle_s: Seconds(DepthPolicy::DEFAULT_FAST_LF_S),
                    },
                    _ => DepthPolicy::EqualConfidence,
                };
                config.resolution = self.resolution();
                MeasKind::Transfer { config }
            }
            FormKind::Spectrum | FormKind::SpectrumEdit => {
                let input = input(FieldId::Input, "input")?;
                let base = match base {
                    Some(MeasKind::Spectrum { config }) => config.clone(),
                    _ => SpectrumConfig::on_input(input),
                };
                MeasKind::Spectrum {
                    config: SpectrumConfig {
                        input,
                        smoothing,
                        averaging,
                        resolution: self
                            .choice_index(FieldId::FftSize)
                            .and_then(|i| spec_resolutions().get(i).copied())
                            .unwrap_or(base.resolution),
                        window: self
                            .choice_index(FieldId::Window)
                            .map_or(base.window, |i| WINDOWS[i]),
                        level: self
                            .choice_index(FieldId::SpecLevel)
                            .and_then(|i| SPEC_LEVELS.get(i))
                            .map_or(base.level, |l| l.1),
                    },
                }
            }
            FormKind::Rta | FormKind::RtaEdit => {
                let input = input(FieldId::Input, "input")?;
                let fraction = FRACTIONS[pick(FieldId::Fraction).min(FRACTIONS.len() - 1)].1;
                let base = match base {
                    Some(MeasKind::Rta { config }) => config.clone(),
                    _ => RtaConfig::on_input(input, fraction),
                };
                MeasKind::Rta {
                    config: RtaConfig {
                        input,
                        fraction,
                        averaging,
                        ..base
                    },
                }
            }
            FormKind::Spl => MeasKind::Spl {
                config: SplConfig::on_input(
                    input(FieldId::Input, "input")?,
                    WEIGHTINGS[pick(FieldId::Weighting).min(WEIGHTINGS.len() - 1)].1,
                    TIME_WEIGHTINGS[pick(FieldId::TimeWeighting).min(TIME_WEIGHTINGS.len() - 1)].1,
                ),
            },
        };
        let name = self.text(FieldId::Name).trim();
        if name.is_empty() {
            return Err("type a name".into());
        }
        Ok(MeasConfig {
            name: name.to_owned(),
            kind,
        })
    }
}

/// The dialog that makes a measurement of kind `k`.
pub(super) fn form_kind(k: &MeasKind) -> FormKind {
    match k {
        MeasKind::Transfer { .. } => FormKind::Transfer,
        MeasKind::Spectrum { .. } => FormKind::Spectrum,
        MeasKind::Rta { .. } => FormKind::Rta,
        MeasKind::Spl { .. } => FormKind::Spl,
        MeasKind::Math { .. } => FormKind::Math,
        MeasKind::Sweep { .. } => FormKind::Sweep,
    }
}
