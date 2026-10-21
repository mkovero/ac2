//! The built-in target curves, as the Target chooser, its dialogs and the trace list name
//! them: flat, a house tilt, and the loudness shift between two listening levels.

use ac2_proto::model::{LfShelf, TargetCurve};
use ac2_proto::units::{Db, Hz};

use crate::format;
use crate::meas_list::Choice;
use crate::unit_text::{Freq, Gain, LoudnessLevel, Slope, in_field};

/// A row of the Target chooser, in its rows' order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TargetChoice {
    File,
    Flat,
    Tilt,
    Loudness,
}

impl TargetChoice {
    /// The rows, top to bottom: a curve from elsewhere first, then the generated ones from
    /// the plainest.
    pub const ALL: [TargetChoice; 4] = [
        TargetChoice::File,
        TargetChoice::Flat,
        TargetChoice::Tilt,
        TargetChoice::Loudness,
    ];

    /// Its name, as the chooser and the palette say it.
    pub fn name(self) -> &'static str {
        match self {
            TargetChoice::File => "Load a file…",
            TargetChoice::Flat => "Flat",
            TargetChoice::Tilt => "House tilt…",
            TargetChoice::Loudness => "Loudness shift…",
        }
    }

    /// What it is, in one line.
    pub fn what(self) -> &'static str {
        match self {
            TargetChoice::File => "a curve exported from another analyzer, or typed by hand",
            TargetChoice::Flat => "0 dB at every frequency",
            TargetChoice::Tilt => "dB per octave about a pivot, with an optional LF shelf",
            TargetChoice::Loudness => "how a level change is heard, not an EQ target",
        }
    }

    /// Its row: `Flat — 0 dB at every frequency`.
    pub fn row(self) -> String {
        format!("{} — {}", self.name(), self.what())
    }
}

/// The chooser's title.
pub const TITLE: &str = "Target curve";

/// What Enter does in the tilt and loudness dialogs.
pub const SUBMIT: &str = "Enter adds it as a target trace";

/// The chooser's text: one row per [`TargetChoice::ALL`], the file row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetChoices {
    pub title: String,
    pub lines: Vec<String>,
    pub choices: Vec<Choice>,
    pub hint: String,
}

pub fn choices() -> TargetChoices {
    TargetChoices {
        title: TITLE.to_owned(),
        lines: vec![
            "A stored trace to compare the response with, drawn in the transfer pane.".to_owned(),
        ],
        choices: TargetChoice::ALL
            .iter()
            .map(|c| Choice {
                label: c.row(),
                blocked: None,
            })
            .collect(),
        hint: "↑/↓ choose · Enter takes it · Esc cancels".to_owned(),
    }
}

/// A frequency as a name says it, as few digits as it needs: `1 kHz`, `100 Hz`, `31.5 Hz`.
fn freq_name(hz: Hz) -> String {
    let t = format::freq_tick(hz.0);
    match t.strip_suffix('k') {
        Some(k) => format!("{k} kHz"),
        None => format!("{t} Hz"),
    }
}

/// The trace's name: `flat`, `tilt −1.0 dB/oct @ 1 kHz`,
/// `tilt −1.0 dB/oct @ 1 kHz, LF +6.0 dB @ 100 Hz`, `loudness 85 → 100 phon`.
pub fn name(c: &TargetCurve) -> String {
    match c {
        TargetCurve::Flat => "flat".to_owned(),
        TargetCurve::Tilt {
            slope_db_per_oct,
            pivot,
            lf_shelf,
        } => {
            let mut s = format!(
                "tilt {} dB/oct @ {}",
                format::signed(*slope_db_per_oct, 1),
                freq_name(*pivot)
            );
            if let Some(sh) = lf_shelf {
                s.push_str(&format!(
                    ", LF {} @ {}",
                    format::db_readout(sh.gain.0),
                    freq_name(sh.corner)
                ));
            }
            s
        }
        TargetCurve::LoudnessShift { from, to } => format!(
            "loudness {} → {} phon",
            format::fixed(from.0, needed(from.0)),
            format::fixed(to.0, needed(to.0))
        ),
    }
}

/// Decimals a phon level needs: none for a whole number, one otherwise.
fn needed(v: f64) -> usize {
    usize::from((v - v.round()).abs() > 1e-9)
}

/// The legend note of a generated curve. A loudness shift describes how a level change is
/// heard: equalising a system to it would bake one listening level's perception into the
/// response at every level.
pub fn note(c: &TargetCurve) -> Option<&'static str> {
    match c {
        TargetCurve::LoudnessShift { .. } => Some("perceived shift, not an EQ target"),
        _ => None,
    }
}

/// The house tilt dialog's title, fields and hints.
pub mod tilt {
    pub const TITLE: &str = "House tilt target";
    pub const SLOPE: &str = "Slope";
    pub const SLOPE_HINT: &str = "dB per octave, negative falls toward HF (−6 … 6)";
    pub const PIVOT: &str = "Pivot";
    pub const PIVOT_HINT: &str = "where the tilt is 0 dB (20 Hz … 20 kHz)";
    pub const SHELF_GAIN: &str = "LF shelf";
    pub const SHELF_GAIN_HINT: &str = "off, or the low-frequency lift in dB (±20): a 2nd-order shelf, half its gain at the corner";
    pub const SHELF_CORNER: &str = "Shelf corner";
    pub const SHELF_CORNER_HINT: &str = "where the shelf has half its gain (20 Hz … 1 kHz)";
    /// The fields' first text.
    pub const DEFAULT_SLOPE: &str = "−1.0 dB/oct";
    pub const DEFAULT_PIVOT: &str = "1 kHz";
    pub const DEFAULT_SHELF_GAIN: &str = "off";
    pub const DEFAULT_SHELF_CORNER: &str = "100 Hz";
}

/// The loudness shift dialog's title, fields and hints.
pub mod loudness {
    pub const TITLE: &str = "Loudness shift (not an EQ target)";
    pub const FROM: &str = "From";
    pub const FROM_HINT: &str = "the level the balance was judged at (20 … 100 phon)";
    pub const TO: &str = "To";
    pub const TO_HINT: &str = "the level it is played at: shows how the same response is heard there (above 90 phon the contours rest on few data)";
    pub const DEFAULT_FROM: &str = "85 phon";
    pub const DEFAULT_TO: &str = "100 phon";
}

/// The tilt the dialog's text asks for, or what is wrong with it. `shelf_gain` `off`, empty
/// or 0 dB means no shelf.
pub fn tilt_from_text(
    slope: &str,
    pivot: &str,
    shelf_gain: &str,
    shelf_corner: &str,
) -> Result<TargetCurve, String> {
    let Slope(slope) = in_field(slope, "db/oct").map_err(|e| format!("slope: {e}"))?;
    let Freq(pivot) = in_field(pivot, "hz").map_err(|e| format!("pivot: {e}"))?;
    let g = shelf_gain.trim();
    let lf_shelf = if g.is_empty() || g.eq_ignore_ascii_case("off") {
        None
    } else {
        let Gain(gain) = in_field(g, "db").map_err(|e| format!("LF shelf: {e}"))?;
        if gain == Db(0.0) {
            None
        } else {
            let Freq(corner) =
                in_field(shelf_corner, "hz").map_err(|e| format!("shelf corner: {e}"))?;
            Some(LfShelf { gain, corner })
        }
    };
    let c = TargetCurve::Tilt {
        slope_db_per_oct: slope,
        pivot,
        lf_shelf,
    };
    c.check()?;
    Ok(c)
}

/// The loudness shift the dialog's text asks for, or what is wrong with it.
pub fn loudness_from_text(from: &str, to: &str) -> Result<TargetCurve, String> {
    let LoudnessLevel(from) = in_field(from, "phon").map_err(|e| format!("from: {e}"))?;
    let LoudnessLevel(to) = in_field(to, "phon").map_err(|e| format!("to: {e}"))?;
    let c = TargetCurve::LoudnessShift { from, to };
    c.check()?;
    Ok(c)
}

#[cfg(test)]
mod tests;
