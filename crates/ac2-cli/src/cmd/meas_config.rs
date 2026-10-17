//! A measurement's config from the `meas new` / `meas set` flags.

use ac2_proto::model::*;

use crate::CliError;
use crate::args::*;
use crate::units::SampleCount;

pub(super) fn weighting(w: WeightArg) -> Weighting {
    match w {
        WeightArg::A => Weighting::A,
        WeightArg::C => Weighting::C,
        WeightArg::Z => Weighting::Z,
    }
}

pub(super) fn time_weighting(t: TimeWeightArg) -> TimeWeighting {
    match t {
        TimeWeightArg::Fast => TimeWeighting::Fast,
        TimeWeightArg::Slow => TimeWeighting::Slow,
        TimeWeightArg::Impulse => TimeWeighting::Impulse,
    }
}

pub(crate) fn smoothing(f: FractionArg) -> Result<SmoothingFraction, CliError> {
    Ok(match f {
        FractionArg::F3 => SmoothingFraction::Third,
        FractionArg::F6 => SmoothingFraction::Sixth,
        FractionArg::F12 => SmoothingFraction::Twelfth,
        FractionArg::F24 => SmoothingFraction::TwentyFourth,
        FractionArg::F48 => SmoothingFraction::FortyEighth,
        FractionArg::F1 => return Err(CliError::Usage("smoothing is 1/3 … 1/48 octave".into())),
    })
}

fn band_fraction(f: FractionArg) -> Result<BandFraction, CliError> {
    Ok(match f {
        FractionArg::F1 => BandFraction::Octave,
        FractionArg::F3 => BandFraction::Third,
        FractionArg::F6 => BandFraction::Sixth,
        FractionArg::F12 => BandFraction::Twelfth,
        FractionArg::F24 => BandFraction::TwentyFourth,
        FractionArg::F48 => return Err(CliError::Usage("RTA bands are 1/1 … 1/24 octave".into())),
    })
}

/// A spectrum's `--resolution multi[:1/N]`, else one FFT of `--fft`.
fn spec_resolution(
    r: Option<ResolutionArg>,
    fft: Option<SampleCount>,
) -> Result<SpecResolution, CliError> {
    match (r, fft) {
        (Some(r), _) if !r.multi => Err(CliError::Usage(
            "a spectrum takes --resolution multi or multi:1/N (the multi-resolution bands), \
             or --fft for one FFT"
                .into(),
        )),
        (Some(r), _) => Ok(SpecResolution::Multi { grid: r.grid }),
        (None, Some(n)) => Ok(SpecResolution::Fixed {
            fft_len: fft_len(n)?,
        }),
        (None, None) => Err(CliError::Usage("--fft or --resolution multi".into())),
    }
}

/// `--fft`: a length the daemon takes.
fn fft_len(n: SampleCount) -> Result<u32, CliError> {
    u32::try_from(n.0)
        .ok()
        .filter(|n| SpectrumConfig::fft_len_ok(*n))
        .ok_or_else(|| {
            CliError::Usage(format!(
                "--fft must be a power of two, {} … {} samples",
                SpectrumConfig::MIN_FFT_LEN,
                SpectrumConfig::MAX_FFT_LEN
            ))
        })
}

pub(crate) fn spec_level(l: SpecLevelArg) -> SpecLevel {
    match l {
        SpecLevelArg::Tone => SpecLevel::Tone,
        SpecLevelArg::Density => SpecLevel::Density,
    }
}

pub(crate) fn window(w: WindowArg) -> Window {
    match w {
        WindowArg::Hann => Window::Hann,
        WindowArg::Bh4 => Window::BlackmanHarris4,
        WindowArg::Flattop => Window::FlatTop,
        WindowArg::Rect => Window::Rectangular,
    }
}

/// Builds the measurement configuration of `meas new`, checking the kind's required inputs.
pub fn meas_config(n: &MeasNew) -> Result<MeasConfig, CliError> {
    let need = |c: Option<crate::units::Channel>, flag: &str| {
        c.map(|c| c.0)
            .ok_or_else(|| CliError::Usage(format!("{:?} needs --{flag}", n.kind)))
    };
    let refuse = |c: Option<crate::units::Channel>, flag: &str| match c {
        Some(_) => Err(CliError::Usage(format!(
            "--{flag} does not apply to {:?}",
            n.kind
        ))),
        None => Ok(()),
    };
    let sweep_only = [
        ("out", n.outputs.is_some()),
        (
            "level",
            matches!(n.level, Some(crate::units::MeasLevel::Stimulus(_))),
        ),
        ("duration", n.duration.is_some()),
        ("repeats", n.repeats.is_some()),
        ("gate", n.gate.is_some()),
        ("tail", n.tail.is_some()),
        ("lf-harmonics", n.lf_harmonics.is_some()),
    ];
    if n.kind != MeasKindArg::Sweep
        && let Some((flag, _)) = sweep_only.iter().find(|(_, given)| *given)
    {
        let hint = if *flag == "level" && n.kind == MeasKindArg::Spectrum {
            " (a spectrum takes --level tone|density)"
        } else {
            ""
        };
        return Err(CliError::Usage(format!(
            "--{flag} applies to a sweep, not {:?}{hint}",
            n.kind
        )));
    }
    if n.kind != MeasKindArg::Spectrum
        && let Some(crate::units::MeasLevel::Spectrum(l)) = n.level
    {
        return Err(CliError::Usage(format!(
            "--level {} applies to a spectrum, not {:?}",
            ac2_scene::format::spec_level(l),
            n.kind
        )));
    }
    if n.resolution.is_some()
        && !matches!(
            n.kind,
            MeasKindArg::Tf | MeasKindArg::Sweep | MeasKindArg::Spectrum
        )
    {
        return Err(CliError::Usage(format!(
            "--resolution applies to tf, sweep and spectrum, not {:?}",
            n.kind
        )));
    }
    let resolution = match n.kind {
        MeasKindArg::Spectrum => Resolution::default(),
        _ => n
            .resolution
            .map(ResolutionArg::columns)
            .transpose()
            .map_err(CliError::Usage)?
            .unwrap_or_default(),
    };
    if n.average.is_some() && !matches!(n.kind, MeasKindArg::Spectrum | MeasKindArg::Rta) {
        return Err(CliError::Usage(format!(
            "--average applies to spectrum and rta, not {:?}{}",
            n.kind,
            if n.kind == MeasKindArg::Tf {
                " (tf averages over --blocks)"
            } else {
                ""
            }
        )));
    }
    let kind = match n.kind {
        MeasKindArg::Sweep => {
            refuse(n.input, "input")?;
            if n.smooth.is_some() || n.start {
                return Err(CliError::Usage(
                    "a sweep measurement has no smoothing and no job to start: `ac2 sweep run` \
                     plays it"
                        .into(),
                ));
            }
            MeasKind::Sweep {
                config: super::ir::sweep_config(&super::ir::SweepFlags {
                    reference: need(n.reference, "ref")?,
                    mic: need(n.measurement, "meas")?,
                    outputs: n
                        .outputs
                        .as_ref()
                        .map(|o| o.0.clone())
                        .ok_or_else(|| CliError::Usage("sweep needs --out".into()))?,
                    level: match n.level {
                        Some(crate::units::MeasLevel::Stimulus(l)) => l,
                        Some(crate::units::MeasLevel::Spectrum(_)) => {
                            return Err(CliError::Usage(
                                "a sweep's --level is its RMS level, e.g. -50dbfs; tone and \
                                 density are a spectrum's"
                                    .into(),
                            ));
                        }
                        None => {
                            return Err(CliError::Usage(
                                "sweep needs --level: a sweep has no default level".into(),
                            ));
                        }
                    },
                    from: n.from.0,
                    to: n.to.0,
                    duration: n.duration.map_or(
                        ac2_proto::units::Seconds(ac2_proto::model::EssSpec::DEFAULT_DURATION_S),
                        |d| d.0,
                    ),
                    repeats: n.repeats.unwrap_or(1),
                    gate: n.gate.map(|g| g.0),
                    tail: n.tail.map(|t| t.0),
                    lf_harmonics: n.lf_harmonics.unwrap_or_default().into(),
                    resolution,
                })?,
            }
        }
        MeasKindArg::Tf => {
            refuse(n.input, "input")?;
            if n.blocks == 0 {
                return Err(CliError::Usage("--blocks must be at least 1".into()));
            }
            MeasKind::Transfer {
                config: TransferConfig {
                    reference_input: need(n.reference, "ref")?,
                    measurement_input: need(n.measurement, "meas")?,
                    averaging: TfAveraging::Fifo { blocks: n.blocks },
                    resolution,
                    smoothing: n
                        .smooth
                        .map(|f| {
                            smoothing(f).map(|fraction| Smoothing {
                                fraction,
                                mode: if n.smooth_magnitude_only {
                                    SmoothingMode::Magnitude
                                } else {
                                    SmoothingMode::MagnitudePhase
                                },
                            })
                        })
                        .transpose()?,
                    depth: match n.fast_lf {
                        None => DepthPolicy::EqualConfidence,
                        Some(t) if t.0.0 > 0.0 => DepthPolicy::FastLf { max_settle_s: t.0 },
                        Some(_) => {
                            return Err(CliError::Usage("--fast-lf must be longer than 0".into()));
                        }
                    },
                },
            }
        }
        k => {
            refuse(n.reference, "ref")?;
            refuse(n.measurement, "meas")?;
            let input = need(n.input, "input")?;
            if n.smooth_magnitude_only {
                return Err(CliError::Usage(format!(
                    "--smooth-magnitude-only applies to tf, not {:?}",
                    n.kind
                )));
            }
            if n.smooth.is_some() && k != MeasKindArg::Spectrum {
                return Err(CliError::Usage(format!(
                    "--smooth applies to tf and spectrum, not {:?}{}",
                    n.kind,
                    if k == MeasKindArg::Rta {
                        " (RTA bands already are fractional-octave)"
                    } else {
                        ""
                    }
                )));
            }
            match k {
                MeasKindArg::Spectrum => {
                    let resolution = spec_resolution(n.resolution, Some(n.fft))?;
                    MeasKind::Spectrum {
                        config: SpectrumConfig {
                            resolution,
                            window: window(n.window),
                            smoothing: n.smooth.map(smoothing).transpose()?,
                            averaging: n.average.map_or(SpecAveraging::Off, |a| a.0),
                            level: match n.level {
                                Some(crate::units::MeasLevel::Spectrum(l)) => l,
                                _ => SpecLevel::default_for(resolution),
                            },
                            ..SpectrumConfig::on_input(input)
                        },
                    }
                }
                MeasKindArg::Rta => {
                    if n.from.0.0 >= n.to.0.0 {
                        return Err(CliError::Usage("--from must be below --to".into()));
                    }
                    MeasKind::Rta {
                        config: RtaConfig {
                            f_lo: n.from.0,
                            f_hi: n.to.0,
                            weighting: weighting(n.weight.unwrap_or(WeightArg::Z)),
                            averaging: n.average.map_or(SpecAveraging::Off, |a| a.0),
                            ..RtaConfig::on_input(input, band_fraction(n.fraction)?)
                        },
                    }
                }
                _ => MeasKind::Spl {
                    config: SplConfig::on_input(
                        input,
                        weighting(n.weight.unwrap_or(WeightArg::A)),
                        time_weighting(n.time),
                    ),
                },
            }
        }
    };
    if n.name.trim().is_empty() {
        return Err(CliError::Usage("--name must not be empty".into()));
    }
    Ok(MeasConfig {
        scope: ac2_proto::model::MeasScope::Session,
        name: n.name.clone(),
        kind,
    })
}

/// What `meas set` changes.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MeasSet {
    pub(crate) average: Option<SpecAveraging>,
    pub(crate) blocks: Option<u32>,
    pub(crate) fft: Option<SampleCount>,
    pub(crate) window: Option<Window>,
    pub(crate) resolution: Option<ResolutionArg>,
    pub(crate) level: Option<SpecLevel>,
    pub(crate) style: Option<SpectrumStyle>,
    pub(crate) peak_hold: Option<bool>,
    pub(crate) span_s: Option<u32>,
    pub(crate) colours: Option<Colormap>,
}

impl MeasSet {
    /// Nothing of the configuration changes.
    pub(crate) fn is_empty(&self) -> bool {
        self.average.is_none()
            && self.blocks.is_none()
            && self.fft.is_none()
            && self.window.is_none()
            && self.resolution.is_none()
            && self.level.is_none()
            && self.style.is_none()
            && self.peak_hold.is_none()
            && self.span_s.is_none()
            && self.colours.is_none()
    }
}

/// `meas set`: `config` with the averaging or Resolution changed.
pub(crate) fn meas_set(config: &MeasConfig, set: MeasSet) -> Result<MeasConfig, CliError> {
    if set.is_empty() {
        return Err(CliError::Usage(
            "say what changes: --average (spectrum, rta), --blocks (tf), --fft, --window or \
             --level (spectrum), --resolution (tf, sweep; multi[:1/N] for a spectrum), \
             --style, --peak-hold, --span or --colours (spectrum, rta), --colour or --offset"
                .into(),
        ));
    }
    let before = match &config.kind {
        MeasKind::Spectrum { config: c } => Some(c.resolution),
        _ => None,
    };
    let mut config = config.clone();
    let name = config.name.clone();
    let kind = match &config.kind {
        MeasKind::Transfer { .. } => "tf",
        MeasKind::Spectrum { .. } => "spectrum",
        MeasKind::Rta { .. } => "rta",
        MeasKind::Spl { .. } => "spl",
        MeasKind::Math { .. } => "math",
        MeasKind::Sweep { .. } => "sweep",
    };
    let not = |flag: &str, applies: &str| {
        Err(CliError::Usage(format!(
            "--{flag} applies to {applies}; {name} is a {kind} measurement"
        )))
    };
    if let Some(a) = set.average {
        match &mut config.kind {
            MeasKind::Spectrum { config: c } => c.averaging = a,
            MeasKind::Rta { config: c } => c.averaging = a,
            _ => return not("average", "spectrum and rta"),
        }
    }
    if let Some(b) = set.blocks {
        let MeasKind::Transfer { config: c } = &mut config.kind else {
            return not("blocks", "tf");
        };
        if b == 0 {
            return Err(CliError::Usage("--blocks must be at least 1".into()));
        }
        c.averaging = TfAveraging::Fifo { blocks: b };
    }
    if let Some(n) = set.fft {
        let MeasKind::Spectrum { config: c } = &mut config.kind else {
            return not("fft", "spectrum");
        };
        if set.resolution.is_some() {
            return Err(CliError::Usage(
                "--fft is one FFT, --resolution multi the bands: give one".into(),
            ));
        }
        c.resolution = SpecResolution::Fixed {
            fft_len: fft_len(n)?,
        };
    }
    if let Some(w) = set.window {
        let MeasKind::Spectrum { config: c } = &mut config.kind else {
            return not("window", "spectrum");
        };
        c.window = w;
    }
    if let Some(r) = set.resolution {
        match &mut config.kind {
            MeasKind::Transfer { config: c } => {
                c.resolution = r.columns().map_err(CliError::Usage)?;
            }
            MeasKind::Sweep { config: c } => {
                c.resolution = r.columns().map_err(CliError::Usage)?;
            }
            MeasKind::Spectrum { config: c } => c.resolution = spec_resolution(Some(r), None)?,
            _ => return not("resolution", "tf, sweep and spectrum"),
        }
    }
    if let MeasKind::Spectrum { config: c } = &mut config.kind {
        // Between one FFT and the multi-resolution bands the level follows the new
        // resolution's default unless `--level` says otherwise (as the edit dialog does).
        let crossed = matches!(
            (before, c.resolution),
            (
                Some(SpecResolution::Fixed { .. }),
                SpecResolution::Multi { .. }
            ) | (
                Some(SpecResolution::Multi { .. }),
                SpecResolution::Fixed { .. }
            )
        );
        if crossed {
            c.level = SpecLevel::default_for(c.resolution);
        }
    }
    if let Some(l) = set.level {
        let MeasKind::Spectrum { config: c } = &mut config.kind else {
            return not("level", "spectrum");
        };
        c.level = l;
    }
    let display = set.style.is_some()
        || set.peak_hold.is_some()
        || set.span_s.is_some()
        || set.colours.is_some();
    if display {
        let Some(d) = config.kind.display_mut() else {
            return not(
                "style, --peak-hold, --span and --colours",
                "spectrum and rta",
            );
        };
        if let Some(s) = set.style {
            d.style = s;
        }
        if let Some(p) = set.peak_hold {
            d.peak_hold = p;
        }
        if let Some(s) = set.span_s {
            if !SpectrumDisplay::SPANS_S.contains(&s) {
                return Err(CliError::Usage(format!(
                    "--span is one of {:?} seconds",
                    SpectrumDisplay::SPANS_S
                )));
            }
            d.spectrograph_span_s = s;
        }
        if let Some(c) = set.colours {
            d.spectrograph_colours = c;
        }
    }
    Ok(config)
}

/// Refuses a measurement whose job would read an input the open session does not capture,
/// before anything is created: the daemon would refuse its start in the same words.
pub(crate) fn uncaptured(kind: &MeasKind, captured: &[u16]) -> Result<(), CliError> {
    let inputs: &[u16] = match kind {
        MeasKind::Spectrum { config } => &[config.input],
        MeasKind::Rta { config } => &[config.input],
        MeasKind::Spl { config } => &[config.input],
        MeasKind::Transfer { config } => &[config.reference_input, config.measurement_input],
        MeasKind::Math { .. } | MeasKind::Sweep { .. } => &[],
    };
    match inputs.iter().find(|i| !captured.contains(i)) {
        Some(i) => Err(CliError::Usage(format!(
            "input {} is not captured by the session",
            i + 1
        ))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
