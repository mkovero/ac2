//! Default configurations every front end shares, and the Leq window presets.

use super::{
    BandFraction, DepthPolicy, LeqConfig, LeqPreset, LeqWindow, LogGridSpec, PeakLimit, PeakLimits,
    PeakQuantity, PeakWeighting, PositionCorrection, RtaConfig, SpecAveraging, SpectrumConfig,
    SplConfig, TfAveraging, TimeWeighting, TransferConfig, Weighting, Window,
};
use crate::units::{Db, DbSpl, Hz, Seconds};

// Defaults every front end shares (`ac2 meas new`, the app's dialogs), so a measurement
// made from either is the same measurement.

impl LogGridSpec {
    /// Ten octaves around 1 kHz (≈ 31 Hz … 32 kHz) at `ppo` points per octave.
    pub fn ten_octaves(ppo: u32) -> Self {
        let p = i64::from(ppo);
        let k = |v: i64| i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX });
        Self {
            ppo,
            k_min: k(-5 * p),
            k_max: k(5 * p - 1),
        }
    }
}

impl TransferConfig {
    /// Default points per octave of the grid.
    pub const DEFAULT_PPO: u32 = 48;
    /// Default FIFO blocks of the full-rate stage.
    pub const DEFAULT_BLOCKS: u32 = 8;

    /// `reference_input` → `measurement_input` with the default grid and averaging, no
    /// smoothing and equal confidence at every frequency.
    pub fn with_inputs(reference_input: u16, measurement_input: u16) -> Self {
        Self {
            reference_input,
            measurement_input,
            averaging: TfAveraging::Fifo {
                blocks: Self::DEFAULT_BLOCKS,
            },
            grid: LogGridSpec::ten_octaves(Self::DEFAULT_PPO),
            smoothing: None,
            depth: DepthPolicy::EqualConfidence,
        }
    }
}

impl DepthPolicy {
    /// Settle cap of [`DepthPolicy::FastLf`] when none is given, seconds.
    pub const DEFAULT_FAST_LF_S: f64 = 1.0;
}

impl SpectrumConfig {
    /// Default FFT length, samples.
    pub const DEFAULT_FFT_LEN: u32 = 65_536;

    /// `input` with the default FFT length, a Hann window and no averaging.
    pub fn on_input(input: u16) -> Self {
        Self {
            input,
            fft_len: Self::DEFAULT_FFT_LEN,
            window: Window::Hann,
            averaging: SpecAveraging::Off,
            smoothing: None,
        }
    }
}

impl RtaConfig {
    /// Default lowest band, Hz.
    pub const DEFAULT_F_LO_HZ: f64 = 20.0;
    /// Default highest band, Hz.
    pub const DEFAULT_F_HI_HZ: f64 = 20_000.0;

    /// `input` in `fraction` bands over 20 Hz … 20 kHz, Z-weighted, no averaging.
    pub fn on_input(input: u16, fraction: BandFraction) -> Self {
        Self {
            input,
            fraction,
            f_lo: Hz(Self::DEFAULT_F_LO_HZ),
            f_hi: Hz(Self::DEFAULT_F_HI_HZ),
            weighting: Weighting::Z,
            averaging: SpecAveraging::Off,
        }
    }
}

impl SplConfig {
    /// `input` with the given weightings, a C-weighted peak and the default Leq windows.
    pub fn on_input(input: u16, weighting: Weighting, time_weighting: TimeWeighting) -> Self {
        Self {
            input,
            weighting,
            time_weighting,
            peak_weighting: PeakWeighting::C,
            leq: LeqConfig::default_windows(),
            position: None,
        }
    }

    /// Why the configuration cannot run, if it cannot.
    pub fn check(&self) -> Result<(), String> {
        self.leq.check()?;
        if self.position.is_some_and(|p| !p.is_valid()) {
            return Err(format!(
                "a position correction is at most ±{} dB",
                PositionCorrection::MAX_DB
            ));
        }
        Ok(())
    }
}

impl LeqWindow {
    /// Longest window, s (one day).
    pub const MAX_SECONDS: u32 = 86_400;
    /// Default warn margin, dB.
    pub const DEFAULT_WARN_MARGIN_DB: f64 = 3.0;

    /// A-weighted, `minutes` long, no limit.
    pub fn minutes(minutes: u32) -> Self {
        Self {
            duration: Seconds(f64::from(minutes) * 60.0),
            weighting: Weighting::A,
            limit: None,
            warn_margin: Db(Self::DEFAULT_WARN_MARGIN_DB),
        }
    }

    /// Length in whole seconds, when it is one in range.
    pub fn seconds(&self) -> Option<u32> {
        let s = self.duration.0;
        (s.is_finite() && s >= 1.0 && s <= f64::from(Self::MAX_SECONDS) && s.fract() == 0.0)
            .then_some(s as u32)
    }

    /// Puts `windows` in display order: shorter first, equal lengths A, C, Z.
    pub fn sort(windows: &mut [LeqWindow]) {
        let rank = |w: Weighting| match w {
            Weighting::A => 0,
            Weighting::C => 1,
            Weighting::Z => 2,
        };
        windows.sort_by(|a, b| {
            a.duration
                .0
                .total_cmp(&b.duration.0)
                .then(rank(a.weighting).cmp(&rank(b.weighting)))
        });
    }

    /// Whether the window is well formed: whole seconds in range, a finite limit and a
    /// finite margin ≥ 0.
    pub fn is_valid(&self) -> bool {
        self.seconds().is_some()
            && self.limit.is_none_or(|l| l.0.is_finite())
            && self.warn_margin.0.is_finite()
            && self.warn_margin.0 >= 0.0
    }
}

impl LeqConfig {
    /// Most windows per meter.
    pub const MAX_WINDOWS: usize = 8;
    /// Default headroom horizon, s.
    pub const DEFAULT_HORIZON_S: f64 = 60.0;
    /// Longest headroom horizon, s.
    pub const MAX_HORIZON_S: f64 = 3600.0;

    /// LAeq over 1, 5, 10, 30 and 60 min, no limits, a one-minute horizon.
    pub fn default_windows() -> Self {
        Self {
            windows: [1, 5, 10, 30, 60].map(LeqWindow::minutes).to_vec(),
            horizon: Seconds(Self::DEFAULT_HORIZON_S),
            peaks: PeakLimits::default(),
        }
    }

    /// Horizon in whole seconds, when it is one in range.
    pub fn horizon_seconds(&self) -> Option<u32> {
        let h = self.horizon.0;
        (h.is_finite() && (1.0..=Self::MAX_HORIZON_S).contains(&h) && h.fract() == 0.0)
            .then_some(h as u32)
    }

    /// Why the configuration cannot run, if it cannot.
    pub fn check(&self) -> Result<(), String> {
        if self.windows.len() > Self::MAX_WINDOWS {
            return Err(format!(
                "at most {} Leq windows per meter",
                Self::MAX_WINDOWS
            ));
        }
        if let Some(w) = self.windows.iter().find(|w| !w.is_valid()) {
            return Err(format!(
                "Leq window of {} s: a window is 1 s … 24 h in whole seconds, with a finite \
                 limit and a warn margin of 0 dB or more",
                w.duration.0
            ));
        }
        if self.horizon_seconds().is_none() {
            return Err("the headroom horizon is 1 s … 1 h in whole seconds".into());
        }
        let bad = |p: &PeakLimit| {
            !(p.limit.0.is_finite() && p.warn_margin.0.is_finite() && p.warn_margin.0 >= 0.0)
        };
        if [self.peaks.lcpeak, self.peaks.lafmax]
            .iter()
            .flatten()
            .any(bad)
        {
            return Err(
                "a peak limit needs a finite limit and a warn margin of 0 dB or more".into(),
            );
        }
        Ok(())
    }
}

impl LeqPreset {
    /// Every preset, in the order the app offers them.
    pub const ALL: [LeqPreset; 18] = [
        LeqPreset::Din15905,
        LeqPreset::Swiss93,
        LeqPreset::Swiss96,
        LeqPreset::Swiss100,
        LeqPreset::Who,
        LeqPreset::France,
        LeqPreset::FranceChildren,
        LeqPreset::Flanders85,
        LeqPreset::Flanders95,
        LeqPreset::Flanders100,
        LeqPreset::Brussels85,
        LeqPreset::Brussels95,
        LeqPreset::Brussels100,
        LeqPreset::NetherlandsCovenant,
        LeqPreset::NetherlandsCovenant16To17,
        LeqPreset::NetherlandsCovenant14To15,
        LeqPreset::NetherlandsCovenantTo13,
        LeqPreset::Finland545,
    ];

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            LeqPreset::Din15905 => "DIN 15905-5",
            LeqPreset::Swiss93 => "Swiss V-NISSG 93 dB",
            LeqPreset::Swiss96 => "Swiss V-NISSG 96 dB",
            LeqPreset::Swiss100 => "Swiss V-NISSG 100 dB",
            LeqPreset::Who => "WHO safe listening",
            LeqPreset::France => "France R1336-1",
            LeqPreset::FranceChildren => "France R1336-1, children up to 6",
            LeqPreset::Flanders85 => "Flanders VLAREM 85 dB",
            LeqPreset::Flanders95 => "Flanders VLAREM 95 dB",
            LeqPreset::Flanders100 => "Flanders VLAREM 100 dB",
            LeqPreset::Brussels85 => "Brussels 85 dB",
            LeqPreset::Brussels95 => "Brussels 95 dB",
            LeqPreset::Brussels100 => "Brussels 100 dB",
            LeqPreset::NetherlandsCovenant => "NL covenant 103 dB",
            LeqPreset::NetherlandsCovenant16To17 => "NL covenant, ages 16–17",
            LeqPreset::NetherlandsCovenant14To15 => "NL covenant, ages 14–15",
            LeqPreset::NetherlandsCovenantTo13 => "NL covenant, ages up to 13",
            LeqPreset::Finland545 => "Finland STM 545/2015",
        }
    }

    /// Where the figures come from.
    pub fn source(self) -> &'static str {
        match self {
            LeqPreset::Din15905 => "DIN 15905-5:2007, loudest audience position",
            LeqPreset::Swiss93 | LeqPreset::Swiss96 | LeqPreset::Swiss100 => {
                "V-NISSG (SR 814.711), by event category"
            }
            LeqPreset::Who => "WHO Global standard for safe listening venues and events (2022)",
            LeqPreset::France => {
                "Code de la santé publique art. R1336-1 II 1° (décret n° 2017-1244), anywhere \
                 the public can be"
            }
            LeqPreset::FranceChildren => {
                "Code de la santé publique art. R1336-1 II 1° (décret n° 2017-1244), events \
                 aimed at children up to six"
            }
            LeqPreset::Flanders85 => {
                "VLAREM II art. 6.7.3 § 1: music in tents, open air and other public places"
            }
            LeqPreset::Flanders95 => {
                "VLAREM II art. 5.32.2.2bis § 1 (and 5.32.3.10): at the measuring position"
            }
            LeqPreset::Flanders100 => {
                "VLAREM II art. 5.32.2.2bis § 2: at the measuring position, LAeq 15 min shown"
            }
            LeqPreset::Brussels85 => {
                "Brussels-Capital arrêté du 26 janvier 2017 (son amplifié) art. 3"
            }
            LeqPreset::Brussels95 => {
                "Brussels-Capital arrêté du 26 janvier 2017 (son amplifié) art. 4"
            }
            LeqPreset::Brussels100 => {
                "Brussels-Capital arrêté du 26 janvier 2017 (son amplifié) art. 5"
            }
            LeqPreset::NetherlandsCovenant => {
                "Vierde convenant preventie gehoorschade versterkte muziek (Stcrt. 2024, 3787) \
                 art. 3.1.2, voluntary"
            }
            LeqPreset::NetherlandsCovenant16To17
            | LeqPreset::NetherlandsCovenant14To15
            | LeqPreset::NetherlandsCovenantTo13 => {
                "Vierde convenant preventie gehoorschade versterkte muziek (Stcrt. 2024, 3787) \
                 art. 3.1.3, voluntary"
            }
            LeqPreset::Finland545 => {
                "Asumisterveysasetus STM 545/2015 §12, to avoid hearing damage, wherever people \
                 are exposed"
            }
        }
    }

    /// The windows the preset sets, shortest first (equal lengths A, C, Z), each with the
    /// rule's limit; one without a limit is a window the rule wants shown.
    pub fn windows(self) -> Vec<LeqWindow> {
        use Weighting::{A, C};
        let w = |minutes: u32, weighting: Weighting, limit: Option<f64>| LeqWindow {
            weighting,
            limit: limit.map(DbSpl),
            ..LeqWindow::minutes(minutes)
        };
        match self {
            LeqPreset::Din15905 => vec![w(30, A, Some(99.0))],
            LeqPreset::Swiss93 => vec![w(60, A, Some(93.0))],
            LeqPreset::Swiss96 => vec![w(60, A, Some(96.0))],
            LeqPreset::Swiss100 => vec![w(60, A, Some(100.0))],
            LeqPreset::Who => vec![w(15, A, Some(100.0))],
            LeqPreset::France => vec![w(15, A, Some(102.0)), w(15, C, Some(118.0))],
            LeqPreset::FranceChildren => vec![w(15, A, Some(94.0)), w(15, C, Some(104.0))],
            LeqPreset::Flanders85 => vec![w(15, A, Some(85.0))],
            LeqPreset::Flanders95 => vec![w(15, A, Some(95.0))],
            LeqPreset::Flanders100 => vec![w(15, A, None), w(60, A, Some(100.0))],
            LeqPreset::Brussels85 => vec![w(15, A, Some(85.0))],
            LeqPreset::Brussels95 => vec![w(15, A, Some(95.0)), w(15, C, Some(110.0))],
            LeqPreset::Brussels100 => vec![w(60, A, Some(100.0)), w(60, C, Some(115.0))],
            LeqPreset::NetherlandsCovenant => vec![w(15, A, Some(103.0))],
            LeqPreset::NetherlandsCovenant16To17 => vec![w(15, A, Some(100.0))],
            LeqPreset::NetherlandsCovenant14To15 => vec![w(15, A, Some(96.0))],
            LeqPreset::NetherlandsCovenantTo13 => vec![w(15, A, Some(91.0))],
            LeqPreset::Finland545 => vec![w(240, A, Some(100.0))],
        }
    }

    /// The peak limits the rule sets (none for most: their texts limit Leq windows only).
    pub fn peaks(self) -> PeakLimits {
        let p = |l: f64| {
            Some(PeakLimit {
                limit: DbSpl(l),
                warn_margin: Db(LeqWindow::DEFAULT_WARN_MARGIN_DB),
            })
        };
        match self {
            LeqPreset::Din15905 => PeakLimits {
                lcpeak: p(135.0),
                lafmax: None,
            },
            LeqPreset::Swiss93 | LeqPreset::Swiss96 | LeqPreset::Swiss100 => PeakLimits {
                lcpeak: None,
                lafmax: p(125.0),
            },
            LeqPreset::Finland545 => PeakLimits {
                lcpeak: p(140.0),
                lafmax: p(115.0),
            },
            _ => PeakLimits::default(),
        }
    }

    /// The peak limits a meter has once `presets` are applied: exactly theirs, the lower
    /// where two set the same quantity (both rules met).
    pub fn peaks_of(presets: &[LeqPreset]) -> PeakLimits {
        let mut out = PeakLimits::default();
        for p in presets.iter().map(|p| p.peaks()) {
            for q in PeakQuantity::ALL {
                let slot = out.get_mut(q);
                *slot = match (*slot, p.get(q)) {
                    (Some(a), Some(b)) => Some(if b.limit.0 < a.limit.0 { b } else { a }),
                    (a, b) => a.or(b),
                };
            }
        }
        out
    }

    /// The windows a meter has once `presets` are applied: exactly theirs, shortest first
    /// (equal lengths A, C, Z), whatever it had before — a rule's limits on windows it does
    /// not define would read as part of it. A window two presets share gets the lower of
    /// their limits, so both rules are met, and a limit wins over a window shown without
    /// one. At most six distinct windows occur across all presets, well within
    /// [`LeqConfig::MAX_WINDOWS`].
    pub fn windows_of(presets: &[LeqPreset]) -> Vec<LeqWindow> {
        let mut out: Vec<LeqWindow> = Vec::new();
        for p in presets.iter().flat_map(|p| p.windows()) {
            match out
                .iter_mut()
                .find(|w| w.duration == p.duration && w.weighting == p.weighting)
            {
                Some(w) => {
                    w.limit = match (w.limit, p.limit) {
                        (Some(a), Some(b)) => Some(if b.0 < a.0 { b } else { a }),
                        (a, b) => a.or(b),
                    }
                }
                None => out.push(p),
            }
        }
        LeqWindow::sort(&mut out);
        out
    }
}
