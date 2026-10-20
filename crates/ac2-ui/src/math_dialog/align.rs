//! The math dialog's alignment rows: the overlap band (asked, prefilled from where the two
//! curves the app already draws are alike) and the longest delay to look at.

use std::sync::Arc;

use ac2_proto::model::{ALIGN_MAX_DELAY_DEFAULT, AlignBand, Operand};
use ac2_proto::units::{Hz, Seconds};
use ac2_scene::grid::GridColumns;
use ac2_scene::unit_text::{Freq, Time, in_field};

use super::{Choice, MathForm, Picks};
use crate::forms::{Field, FieldId, Form};

/// A transfer curve's magnitude as last drawn: what the band is prefilled from.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub operand: Operand,
    pub cols: Arc<GridColumns>,
    pub db: Vec<f32>,
}

/// The band row's hint, when the band is prefilled.
pub const PREFILLED: &str = "where both curves are alike: check it";
/// The band row's hint, when the curves did not say.
pub const ASKED: &str = "type where the two overlap";

/// A frequency as the band rows hold it: `65 Hz`, `1300 Hz`.
pub(super) fn hz_text(hz: f64) -> String {
    format!("{hz} Hz")
}

/// A delay as the max-delay row holds it.
pub(super) fn ms_text(s: Seconds) -> String {
    format!("{} ms", (s.0 * 1e6).round() / 1e3)
}

/// The max-delay row's text for a new alignment.
pub(super) fn default_max_delay() -> String {
    ms_text(ALIGN_MAX_DELAY_DEFAULT)
}

impl MathForm {
    /// The band where A's and B's curves overlap ([`ac2_scene::align::overlap`]), as the
    /// rows hold it; `None` without both curves on one grid or without an overlap.
    pub(super) fn band_prefill(
        &self,
        a: Option<Operand>,
        b: Option<Operand>,
    ) -> Option<(String, String)> {
        let level =
            |o: Option<Operand>| o.and_then(|o| self.levels.iter().find(|l| l.operand == o));
        let (la, lb) = (level(a)?, level(b)?);
        if la.cols.freqs != lb.cols.freqs {
            return None;
        }
        let (lo, hi) = ac2_scene::align::overlap(&la.cols.freqs, &la.db, &lb.db)?;
        Some((hz_text(lo), hz_text(hi)))
    }
}

impl MathForm {
    /// An alignment's rows after B: the band (following A and B while it still holds what
    /// was prefilled) and the max delay.
    pub(super) fn align_fields(&mut self, p: &mut Picks, fields: &mut Vec<Field>) {
        let prefill = self.band_prefill(p.a, p.b);
        if p.band == self.auto_band || (p.band.0.is_empty() && p.band.1.is_empty()) {
            self.auto_band = prefill.clone().unwrap_or_default();
            p.band = self.auto_band.clone();
        }
        let hint = if prefill.as_ref() == Some(&p.band) {
            PREFILLED
        } else if p.band.0.is_empty() && p.band.1.is_empty() {
            ASKED
        } else {
            ""
        };
        fields.push(Field::text(
            FieldId::AlignFrom,
            "Overlap from",
            p.band.0.clone(),
            hint,
        ));
        fields.push(Field::text(
            FieldId::AlignTo,
            "Overlap to",
            p.band.1.clone(),
            "",
        ));
        fields.push(Field::text(
            FieldId::MaxDelay,
            "Max delay",
            p.max_delay.clone(),
            "the longest delay looked at",
        ));
    }
}

impl Form {
    /// Gives the math dialog the curves its alignment band is prefilled from.
    pub fn set_levels(&mut self, levels: Vec<Level>) {
        if let Some(m) = self.math.as_mut() {
            m.levels = levels;
        }
        self.math_changed();
    }

    /// The math dialog's operator set to align (Alt+M), the focus on B.
    pub fn choose_align(&mut self) {
        if let Some(mut p) = self.picks() {
            p.choice = Choice::Align;
            self.lay_out(p, Some(FieldId::OperandB));
        }
    }

    /// The typed overlap band and max delay.
    pub(super) fn align_rows(&self) -> Result<(AlignBand, Seconds), String> {
        let hz = |id: FieldId, what: &str| {
            in_field::<Freq>(self.text(id), "hz")
                .map(|f| f.0)
                .map_err(|e| format!("overlap {what}: {}", e.0))
        };
        let band = AlignBand {
            lo: hz(FieldId::AlignFrom, "from")?,
            hi: hz(FieldId::AlignTo, "to")?,
        };
        if !band.is_valid() {
            return Err("the overlap band needs from below to (Hz)".into());
        }
        let max_delay = in_field::<Time>(self.text(FieldId::MaxDelay), "ms")
            .map(|t| t.0)
            .map_err(|e| format!("max delay: {}", e.0))?;
        Ok((band, max_delay))
    }
}

/// `band` as the rows hold it.
pub(super) fn band_texts(band: AlignBand) -> (String, String) {
    let Hz(lo) = band.lo;
    let Hz(hi) = band.hi;
    (hz_text(lo), hz_text(hi))
}
