//! The reducer's value types: panes, smoothing, connection, stimulus, delay finder.

use super::*;

/// The panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneKind {
    /// Live transfer curves; one of its views (G) is the impulse response.
    Transfer,
    Spectrum,
    Spl,
    /// Sweep results: response, harmonic distortion, the sweep's IR. Hidden until a sweep
    /// is stored (or the operator shows it).
    Distortion,
}

impl PaneKind {
    pub const ALL: [PaneKind; 4] = [
        PaneKind::Transfer,
        PaneKind::Spectrum,
        PaneKind::Spl,
        PaneKind::Distortion,
    ];

    pub fn scope(self) -> Scope {
        match self {
            PaneKind::Transfer => Scope::Transfer,
            PaneKind::Spectrum => Scope::Spectrum,
            PaneKind::Spl => Scope::Spl,
            PaneKind::Distortion => Scope::Distortion,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PaneKind::Transfer => "Transfer",
            PaneKind::Spectrum => "Spectrum / RTA",
            PaneKind::Spl => "SPL",
            PaneKind::Distortion => "Sweep / distortion",
        }
    }

    /// Whether the pane shows measurements of kind `k`.
    pub fn shows(self, k: &MeasKind) -> bool {
        match self {
            // A sweep measurement has no live curve, but its runs are transfer curves: the
            // pane draws them when the sweep is its measurement.
            PaneKind::Transfer => k.publishes_tf() || matches!(k, MeasKind::Sweep { .. }),
            PaneKind::Spectrum => k.publishes_levels(),
            PaneKind::Spl => matches!(k, MeasKind::Spl { .. }),
            // Sweep measurements: the pane shows their runs.
            PaneKind::Distortion => matches!(k, MeasKind::Sweep { .. }),
        }
    }

    /// The pane that shows measurements of kind `k`.
    pub fn for_kind(k: &MeasKind) -> PaneKind {
        match k.stream() {
            Some(Stream::Spec | Stream::Rta) => PaneKind::Spectrum,
            Some(Stream::Spl) => PaneKind::Spl,
            None => PaneKind::Distortion,
            _ => PaneKind::Transfer,
        }
    }

    /// What the pane's measurements are, for messages.
    pub fn what(self) -> &'static str {
        match self {
            PaneKind::Transfer => "transfer",
            PaneKind::Spectrum => "spectrum or RTA",
            PaneKind::Spl => "SPL",
            PaneKind::Distortion => "sweep",
        }
    }
}

/// Smoothing steps K / Shift+K walk through, finest first.
pub const SMOOTHING_STEPS: [Option<SmoothingFraction>; 6] = [
    None,
    Some(SmoothingFraction::FortyEighth),
    Some(SmoothingFraction::TwentyFourth),
    Some(SmoothingFraction::Twelfth),
    Some(SmoothingFraction::Sixth),
    Some(SmoothingFraction::Third),
];

/// A spectrum's smoothing as a trace edit: power only (a spectrum has no phase).
pub fn spectrum_smoothing(fraction: SmoothingFraction) -> Smoothing {
    Smoothing {
        fraction,
        mode: SmoothingMode::Magnitude,
    }
}

/// What the smoothing keys change: the selected slot's trace, or the focused pane's
/// measurement (the spectrum pane's when it has focus, else the transfer pane's).
#[derive(Clone, Debug, PartialEq)]
pub enum SmoothTarget {
    Trace(TraceMeta),
    Meas(Measurement),
}

/// What smoothing does to a curve of some kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Smoothable {
    /// A transfer function: magnitude, and phase unless the mode keeps it.
    Transfer,
    /// A narrowband spectrum: power over a fractional-octave kernel.
    Spectrum,
    /// RTA bands: already fractional-octave.
    Rta,
    /// Not smoothed (a target curve, a meter).
    No,
}

impl SmoothTarget {
    pub fn kind(&self) -> Smoothable {
        match self {
            SmoothTarget::Trace(t) => match t.kind {
                TraceKind::Transfer | TraceKind::Sweep => Smoothable::Transfer,
                TraceKind::Spectrum { .. } => Smoothable::Spectrum,
                TraceKind::Rta { .. } => Smoothable::Rta,
                TraceKind::Target => Smoothable::No,
            },
            SmoothTarget::Meas(m) => match &m.config.kind {
                MeasKind::Transfer { .. } => Smoothable::Transfer,
                MeasKind::Spectrum { .. } => Smoothable::Spectrum,
                MeasKind::Rta { .. } => Smoothable::Rta,
                MeasKind::Spl { .. } | MeasKind::Sweep { .. } => Smoothable::No,
                MeasKind::Math { config } => match config.domain {
                    MathDomain::Transfer => Smoothable::Transfer,
                    MathDomain::Spectrum => Smoothable::Spectrum,
                    MathDomain::Rta => Smoothable::Rta,
                },
            },
        }
    }

    /// The pane its curve is drawn in.
    pub fn pane(&self) -> PaneKind {
        match self.kind() {
            Smoothable::Spectrum | Smoothable::Rta => PaneKind::Spectrum,
            Smoothable::Transfer | Smoothable::No => PaneKind::Transfer,
        }
    }

    pub fn smoothing(&self) -> Option<Smoothing> {
        match self {
            SmoothTarget::Trace(t) => t.edit.smoothing,
            SmoothTarget::Meas(m) => match &m.config.kind {
                MeasKind::Transfer { config } => config.smoothing,
                MeasKind::Math { config } => config.smoothing,
                MeasKind::Spectrum { config } => config.smoothing.map(spectrum_smoothing),
                _ => None,
            },
        }
    }

    /// `smoothing 1/6 oct`, `smoothing off`; a transfer function kept at measured phase
    /// says `mag only`, a spectrum (no phase) never does.
    pub fn caption(&self) -> String {
        let s = self.smoothing();
        match (self.kind(), s) {
            (Smoothable::Spectrum, Some(s)) => {
                format!("smoothing {}", format::octave_fraction(s.fraction))
            }
            _ => ac2_scene::tf::smoothing_caption(s),
        }
    }

    /// `slot 3 (Main L S3)`, the trace's name, or the measurement name.
    pub fn label(&self) -> String {
        match self {
            SmoothTarget::Trace(t) => trace_label(t),
            SmoothTarget::Meas(m) => m.config.name.clone(),
        }
    }
}

/// The measurement list a pane's title chip opens: the pane's compatible measurements, one
/// highlighted (Up/Down move, Enter shows it, Esc closes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneMenu {
    pub pane: PaneId,
    pub index: usize,
}

/// Link status as shown in the top bar.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Connecting {
        target: String,
    },
    Connected {
        target: String,
        server: String,
        client_id: ClientId,
    },
    Failed {
        target: String,
        error: String,
    },
}

/// Display edits of a live trace: offset and polarity are display only and go to the
/// scene, which applies them. Its delay is the measurement's, in the daemon.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveEdit {
    pub offset_db: f64,
    pub inverted: bool,
}

/// What a measurement stop's or delete's toast adds when the stimulus stopped with it.
pub(crate) const STIMULUS_STOPPED_TOO: &str =
    " · stimulus stopped (no transfer measurement left running)";

/// Where the operator's stimulus stands, as far as this client knows. The generator's
/// truth is the mirrored `Generator`; this tracks requests in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StimPhase {
    Idle,
    /// Lease + arm requested.
    Arming,
    /// Lease held, armed, silent.
    Armed,
    /// Fire requested.
    FireRequested,
    Firing,
    /// Stop requested.
    Stopping,
}

/// Stimulus settings chosen by the operator.
#[derive(Clone, Debug, PartialEq)]
pub struct Stimulus {
    /// Typed by the operator; `None` until then — nothing arms without it.
    pub level: Option<Dbfs>,
    pub signal: Signal,
    /// Zero-based output channels.
    pub outputs: Vec<u16>,
    pub phase: StimPhase,
    /// Space came while a stop was still on its way: the stimulus arms once that stop has
    /// landed (not into the lease the stop is releasing). Any stop, a lost or failed lease,
    /// a closed window and a new connection drop it.
    pub arm_after_stop: bool,
    /// The stop in flight went out with a measurement stop whose reply already says the
    /// stimulus stopped: its own "stimulus stopped" would say it twice.
    pub stop_announced: bool,
}

impl Default for Stimulus {
    fn default() -> Self {
        Self {
            level: None,
            signal: Signal::Pink,
            outputs: vec![0],
            phase: StimPhase::Idle,
            arm_after_stop: false,
            stop_announced: false,
        }
    }
}

impl Stimulus {
    pub(super) fn settings(&self) -> Option<GeneratorSettings> {
        Some(GeneratorSettings {
            signal: self.signal,
            level: self.level?,
            band: None,
            outputs: self.outputs.clone(),
        })
    }

    /// `pink noise −20.0 dBFS → out 1`.
    pub fn describe(&self) -> String {
        let sig = match self.signal {
            Signal::White => "white noise".to_string(),
            Signal::Pink => "pink noise".to_string(),
            Signal::PeriodicPink { .. } => "periodic pink".to_string(),
            Signal::Sine { freq } => format!("sine {}", format::freq_readout(freq.0)),
            Signal::Ess { sweep } => format!(
                "sweep {} – {} {} s",
                format::freq_readout(sweep.start.0),
                format::freq_readout(sweep.end.0),
                format::fixed(sweep.duration.0, 1)
            ),
        };
        let level = self
            .level
            .map_or_else(|| "no level".to_string(), |l| dbfs(l.0));
        format!("{sig} {level} → out {}", outputs_text(&self.outputs))
    }
}

/// What X / Shift+X ask the delay finder for: the band and the observation (block length).
/// Auto band and automatic observation by default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinderChoice {
    pub band: FinderBand,
    /// `None`: the band's default (decision D2: sub 4 s).
    pub observation: Option<Seconds>,
}

impl Default for FinderChoice {
    fn default() -> Self {
        Self {
            band: FinderBand::Auto,
            observation: None,
        }
    }
}

/// Sub-band observations the finder accepts (decision D2).
pub const SUB_OBSERVATIONS_S: [f64; 3] = [2.0, 4.0, 8.0];
/// Longest observation the finder accepts.
pub const MAX_OBSERVATION_S: f64 = 8.0;

impl FinderChoice {
    /// The band analysed needs a sub-band observation (2, 4 or 8 s): the sub preset, or a
    /// custom band starting below 150 Hz.
    pub(super) fn sub_like(&self) -> bool {
        match self.band {
            FinderBand::Sub => true,
            FinderBand::Custom { lo_hz, .. } => lo_hz.0 < 150.0,
            _ => false,
        }
    }

    /// `auto band · auto observation`, `sub band · 8 s`, `80 Hz – 800 Hz · auto observation`.
    pub fn describe(&self) -> String {
        let band = match self.band {
            FinderBand::Auto => "auto band".to_string(),
            FinderBand::Full => "full band".to_string(),
            FinderBand::Mid => "mid band".to_string(),
            FinderBand::Sub => "sub band".to_string(),
            FinderBand::Custom { lo_hz, hi_hz } => format!(
                "{} – {}",
                format::freq_readout(lo_hz.0),
                format::freq_readout(hi_hz.0)
            ),
        };
        let obs = self.observation.map_or_else(
            || "auto observation".to_string(),
            |o| format!("{} s", format::fixed(o.0, 1).trim_end_matches(".0")),
        );
        format!("{band} · {obs}")
    }
}

pub(super) fn dbfs(v: f64) -> String {
    format!("{} dBFS", format::signed(v, 1))
}

/// One-based channel list: `1, 2`.
pub fn outputs_text(o: &[u16]) -> String {
    o.iter()
        .map(|c| (u32::from(*c) + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The curve `text` names in the mic library: `MM1 34804 90°` (a mic name, then one of its
/// labels), or just the mic name when it has one curve.
pub fn parse_curve(mics: &[ac2_proto::model::Mic], text: &str) -> Result<MicCurveId, String> {
    let t = text.trim();
    // The longest mic name the text starts with: names may contain spaces, and one name may
    // begin another.
    let m = mics
        .iter()
        .filter(|m| t.starts_with(m.name.as_str()))
        .max_by_key(|m| m.name.len())
        .ok_or_else(|| {
            let names: Vec<&str> = mics.iter().map(|m| m.name.as_str()).collect();
            if names.is_empty() {
                "the mic library is empty: import a curve first (palette: Calibrations…)".into()
            } else {
                format!(
                    "no mic of the library starts {t:?} (mics: {})",
                    names.join(", ")
                )
            }
        })?;
    let rest = t[m.name.len()..].trim();
    let label = match (rest, m.curves.as_slice()) {
        ("", [only]) => only.label.clone(),
        ("", _) => {
            return Err(format!(
                "{} has several curves: add the label ({})",
                m.name,
                ac2_scene::cal::labels(&m.curves)
            ));
        }
        (l, curves) if curves.iter().any(|c| c.label == l) => l.to_owned(),
        (l, _) => {
            return Err(format!(
                "{} has no curve {l:?} (stored: {})",
                m.name,
                ac2_scene::cal::labels(&m.curves)
            ));
        }
    };
    Ok(MicCurveId {
        mic: m.name.clone(),
        label,
    })
}

/// The `trace.mic_curve` request of the trace mic-curve prompt: a curve of the mic library
/// (`MM1 34804 90°`), or `none`.
pub(super) fn trace_mic_request(
    mics: &[ac2_proto::model::Mic],
    id: TraceId,
    text: &str,
) -> Result<Request, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("type the mic and curve, e.g. MM1 34804 90° (none removes the curve)".into());
    }
    let curve = if t.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(parse_curve(mics, t)?)
    };
    Ok(Request::Call {
        what: match &curve {
            Some(c) => format!(
                "trace {id}: mic curve {} applied",
                ac2_scene::cal::curve_name(&c.mic, &c.label)
            ),
            None => format!("trace {id}: mic curve removed"),
        },
        cmd: Command::TraceMicCurve { trace: id, curve },
    })
}

/// `2=90°`, `2=off` (1-based input): an input and the curve it applies.
pub fn parse_input_curve(text: &str) -> Result<(u16, CurveChoice), String> {
    let (ch, label) = text.split_once('=').ok_or_else(|| {
        format!(
            "{:?}: expected input=curve, e.g. 2=90° or 2=off",
            text.trim()
        )
    })?;
    let n: u16 = ch
        .trim()
        .parse()
        .map_err(|_| format!("not an input number: {:?}", ch.trim()))?;
    if n == 0 {
        return Err("inputs count from 1".into());
    }
    let l = label.trim();
    let choice = if l.eq_ignore_ascii_case("off") || l.eq_ignore_ascii_case("none") {
        CurveChoice::Off
    } else if l.is_empty() {
        return Err("type the curve label after = (or off)".into());
    } else {
        CurveChoice::Curve {
            label: l.to_owned(),
        }
    };
    Ok((n - 1, choice))
}
