//! The band meter's section of the Leq dialog (`docs/design/band-leq.md`): on or off with
//! the limits of a rule, the window length, the §13 corrections in force, and the measured
//! FOH → dwelling transfer as it stands (measured with `ac2 spl bands transfer`).

use ac2_proto::model::{
    BAND_NOMINAL_HZ, BandCorrection, BandLeqConfig, BandLeqPreset, BandTransferSet,
    ImpulseCorrection, LF_BAND_COUNT, TonalCorrection,
};
use ac2_proto::units::Seconds;

use super::LENGTHS;

/// A setting of the band section, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandField {
    /// Off, or the limits of a preset (or as set from the CLI).
    Limits,
    /// Window length.
    Duration,
    /// §13 impulse correction.
    Impulse,
    /// §13 narrowband correction.
    Tonal,
}

impl BandField {
    pub const ALL: [BandField; 4] = [
        BandField::Limits,
        BandField::Duration,
        BandField::Impulse,
        BandField::Tonal,
    ];

    pub(super) fn index(self) -> usize {
        match self {
            BandField::Limits => 0,
            BandField::Duration => 1,
            BandField::Impulse => 2,
            BandField::Tonal => 3,
        }
    }

    /// Its label in the dialog.
    pub fn title(self) -> &'static str {
        match self {
            BandField::Limits => "Band meter",
            BandField::Duration => "Band window",
            BandField::Impulse => "§13 impulse",
            BandField::Tonal => "§13 narrowband",
        }
    }
}

/// What the band meter judges against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandLimits {
    Off,
    /// The limits the meter has, set from the CLI (no preset's).
    Kept,
    Preset(BandLeqPreset),
}

/// The band section being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct BandSection {
    pub limits: BandLimits,
    pub seconds: u32,
    pub impulse: ImpulseCorrection,
    pub tonal: TonalCorrection,
    /// The meter's band configuration when it is not a preset's (kept while chosen).
    kept: Option<BandLeqConfig>,
    /// The meter's measured transfer: every preset keeps it.
    transfer: Option<BandTransferSet>,
    /// The meter's warn margin.
    base: Option<BandLeqConfig>,
}

fn preset_of(c: &BandLeqConfig) -> Option<BandLeqPreset> {
    BandLeqPreset::ALL.into_iter().find(|p| {
        let pc = p.config(c.transfer);
        pc.day == c.day && pc.night == c.night && pc.predicted == c.predicted
    })
}

impl BandSection {
    pub fn new(base: Option<&BandLeqConfig>) -> Self {
        let limits = match base {
            None => BandLimits::Off,
            Some(c) => preset_of(c).map_or(BandLimits::Kept, BandLimits::Preset),
        };
        let seconds = base.and_then(BandLeqConfig::seconds).unwrap_or(3600);
        let correction = base.map(|c| c.correction).unwrap_or_default();
        Self {
            limits,
            seconds,
            impulse: correction.impulse,
            tonal: correction.tonal,
            kept: base.filter(|c| preset_of(c).is_none()).cloned(),
            transfer: base.and_then(|c| c.transfer),
            base: base.cloned(),
        }
    }

    fn choices(&self) -> Vec<BandLimits> {
        let mut out = vec![BandLimits::Off];
        if self.kept.is_some() {
            out.push(BandLimits::Kept);
        }
        out.extend(BandLeqPreset::ALL.map(BandLimits::Preset));
        out
    }

    /// ←/→ on `f`: the next or previous option, stopping at the ends.
    pub fn cycle(&mut self, f: BandField, d: i32) {
        fn step<T: Copy + PartialEq>(all: &[T], cur: T, d: i32) -> T {
            let i = all.iter().position(|x| *x == cur).unwrap_or(0);
            all[(i as i64 + i64::from(d)).clamp(0, all.len() as i64 - 1) as usize]
        }
        match f {
            BandField::Limits => self.limits = step(&self.choices(), self.limits, d),
            // The rest set nothing while the meter is off.
            _ if self.limits == BandLimits::Off => {}
            BandField::Duration => {
                self.seconds = if d > 0 {
                    LENGTHS
                        .iter()
                        .copied()
                        .find(|&s| s > self.seconds)
                        .unwrap_or(self.seconds)
                } else {
                    LENGTHS
                        .iter()
                        .rev()
                        .copied()
                        .find(|&s| s < self.seconds)
                        .unwrap_or(self.seconds)
                };
            }
            BandField::Impulse => {
                use ImpulseCorrection as I;
                self.impulse = step(&[I::None, I::Plus5, I::Plus10], self.impulse, d);
            }
            BandField::Tonal => {
                use TonalCorrection as T;
                self.tonal = step(&[T::None, T::Plus3, T::Plus6], self.tonal, d);
            }
        }
    }

    /// The value of `f` as shown.
    pub fn text(&self, f: BandField) -> String {
        let off = self.limits == BandLimits::Off;
        match f {
            BandField::Limits => match self.limits {
                BandLimits::Off => "off".into(),
                BandLimits::Kept => "the limits set from the CLI".into(),
                BandLimits::Preset(p) => p.name().into(),
            },
            _ if off => "—".into(),
            BandField::Duration => ac2_scene::band_leq::meter_name(f64::from(self.seconds)),
            BandField::Impulse => match self.impulse {
                ImpulseCorrection::None => "none".into(),
                ImpulseCorrection::Plus5 => "+5 dB".into(),
                ImpulseCorrection::Plus10 => "+10 dB".into(),
            },
            BandField::Tonal => match self.tonal {
                TonalCorrection::None => "none".into(),
                TonalCorrection::Plus3 => "+3 dB".into(),
                TonalCorrection::Plus6 => "+6 dB".into(),
            },
        }
    }

    /// What `f` does, beside it.
    pub fn note(&self, f: BandField) -> String {
        match f {
            BandField::Limits => match self.limits {
                BandLimits::Preset(p) => {
                    let summary = ac2_scene::band_leq::preset_summary(p);
                    let what = summary
                        .split_once(": ")
                        .map_or(summary.as_str(), |(_, w)| w);
                    format!("{what}. {}", ac2_scene::band_leq::preset_source(p))
                }
                _ => "unweighted 1/3-octave Leq 20 … 200 Hz against limits in a neighbour's \
                      dwelling; informational, not legal advice"
                    .into(),
            },
            BandField::Duration => "the decree's is 1 h".into(),
            BandField::Impulse | BandField::Tonal => {
                "added from now while the character is heard (ac2 does not detect it)".into()
            }
        }
    }

    /// The transfer as it stands, in a line.
    pub fn transfer_text(&self) -> String {
        ac2_scene::band_leq::transfer_summary(self.transfer.as_ref())
    }

    /// The transfer per limited band, when there is one.
    pub fn transfer_bands(&self) -> Vec<String> {
        self.transfer.as_ref().map_or_else(Vec::new, |t| {
            t.bands[..LF_BAND_COUNT]
                .iter()
                .enumerate()
                .map(|(i, b)| ac2_scene::band_leq::transfer_band_text(BAND_NOMINAL_HZ[i], b))
                .collect()
        })
    }

    /// The band meter as chosen, checked; `None`: off.
    pub fn config(&self) -> Result<Option<BandLeqConfig>, String> {
        let mut c = match self.limits {
            BandLimits::Off => return Ok(None),
            BandLimits::Kept => match &self.kept {
                Some(k) => k.clone(),
                None => return Ok(self.base.clone()),
            },
            BandLimits::Preset(p) => {
                let mut c = p.config(self.transfer);
                // A margin set before stays.
                if let Some(b) = &self.base {
                    c.warn_margin = b.warn_margin;
                }
                c
            }
        };
        c.duration = Seconds(f64::from(self.seconds));
        c.correction = BandCorrection {
            impulse: self.impulse,
            tonal: self.tonal,
        };
        c.check()?;
        Ok(Some(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_to_a_preset_and_the_corrections() {
        let mut s = BandSection::new(None);
        assert_eq!(s.text(BandField::Limits), "off");
        assert_eq!(s.text(BandField::Duration), "—");
        // Off: the other settings stay as they are.
        s.cycle(BandField::Impulse, 1);
        assert_eq!(s.impulse, ImpulseCorrection::None);
        assert_eq!(s.config(), Ok(None));
        s.cycle(BandField::Limits, 1);
        assert_eq!(s.limits, BandLimits::Preset(BandLeqPreset::Finland545Lf));
        assert_eq!(
            s.text(BandField::Limits),
            "Finland STM 545/2015, low frequencies (bedroom)"
        );
        assert!(
            s.note(BandField::Limits)
                .starts_with("band Leq 60 min, unweighted 20 … 200 Hz")
        );
        assert!(s.note(BandField::Limits).contains("not legal advice"));
        s.cycle(BandField::Limits, 1);
        assert_eq!(
            s.limits,
            BandLimits::Preset(BandLeqPreset::Finland545LivingRoom)
        );
        s.cycle(BandField::Limits, 1);
        assert_eq!(
            s.limits,
            BandLimits::Preset(BandLeqPreset::Finland545LivingRoom),
            "stops at the end"
        );
        s.cycle(BandField::Limits, -1);
        s.cycle(BandField::Duration, -1);
        s.cycle(BandField::Impulse, 1);
        s.cycle(BandField::Tonal, 2);
        assert_eq!(s.text(BandField::Duration), "band Leq 30 min, unweighted");
        assert_eq!(s.text(BandField::Impulse), "+5 dB");
        assert_eq!(s.text(BandField::Tonal), "+6 dB");
        let c = s.config().expect("test value").expect("test value");
        assert_eq!(c.duration, Seconds(1800.0));
        assert_eq!(c.correction.db(), 11.0);
        assert_eq!(c.night, BandLeqPreset::Finland545Lf.config(None).night);
        assert_eq!(
            s.transfer_text(),
            "no transfer: dwelling limits judged at the mic"
        );
    }

    #[test]
    fn a_measured_transfer_and_cli_limits_are_kept() {
        let measured = ac2_proto::samples::band_leq_config();
        let s = BandSection::new(Some(&measured));
        let c = s.config().expect("test value").expect("test value");
        assert_eq!(c.transfer, measured.transfer);
        assert_eq!(s.transfer_bands().len(), LF_BAND_COUNT);
        assert!(s.transfer_text().starts_with("transfer 20–200 Hz: "));
        // Limits typed from the CLI read as such and stay until another choice.
        let mut custom = measured.clone();
        custom.night[5] = Some(ac2_proto::units::DbSpl(40.0));
        let mut s = BandSection::new(Some(&custom));
        assert_eq!(s.limits, BandLimits::Kept);
        assert_eq!(
            s.config().expect("test value").expect("test value").night[5],
            custom.night[5]
        );
        s.cycle(BandField::Limits, 1);
        assert_eq!(s.limits, BandLimits::Preset(BandLeqPreset::Finland545Lf));
        let c = s.config().expect("test value").expect("test value");
        assert_eq!(c.transfer, measured.transfer, "a preset keeps the transfer");
        s.cycle(BandField::Limits, -2);
        assert_eq!(s.config(), Ok(None));
    }
}
