//! What a mic-curve file says besides its points: the incidence angle it was measured at
//! (a capsule has one curve per angle, and using the wrong one is a measurement error of a
//! few dB at high frequencies) and the sensitivity the manufacturer states.
//!
//! Both come from free text, so the reading is conservative: an angle is a number of at
//! most three digits written directly before `°`, `deg`, `degree(s)` or `Grad`; a
//! sensitivity is a number directly before `mV/Pa`. Anything else is left unknown.

/// Facts read from a curve file's header lines and file name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveFileInfo {
    /// Incidence angle named in the header (`90-degree-curve`) or, failing that, in the
    /// file name (`…_90Grad.txt`, `ecm_0deg.frd`), degrees.
    pub angle_deg: Option<u32>,
    /// Sensitivity stated in the header (`Sensitivity: 15.0mV/Pa`), mV/Pa. Information
    /// only: the sensitivity calibration measures the whole chain, preamp gain included,
    /// which a capsule's data sheet value cannot know.
    pub stated_sensitivity_mv_per_pa: Option<f64>,
}

/// Units an angle may be written in, longest first so `degrees` is not read as `deg`.
const ANGLE_UNITS: [&str; 5] = ["degrees", "degree", "deg", "grad", "°"];

/// Reads the header lines (lines that are not data, as [`super::MicCurve::parse`] skips
/// them) and the file name.
pub fn file_info(bytes: &[u8], file_name: &str) -> CurveFileInfo {
    let text = String::from_utf8_lossy(bytes);
    let header: Vec<String> = text
        .split(['\n', '\r'])
        .filter(|l| !is_data(l))
        .map(str::to_lowercase)
        .collect();
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = base
        .rsplit_once('.')
        .map_or(base, |(s, _)| s)
        .to_lowercase();
    CurveFileInfo {
        angle_deg: header
            .iter()
            .find_map(|l| angle(l))
            .or_else(|| angle(&stem)),
        stated_sensitivity_mv_per_pa: header.iter().find_map(|l| sensitivity(l)),
    }
}

/// A data line starts with a number (the parser's own rule).
fn is_data(line: &str) -> bool {
    let first = line
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .find(|t| !t.is_empty())
        .map(|t| t.trim_matches('"'));
    first.is_some_and(|t| t.replace(',', ".").parse::<f64>().is_ok())
}

/// The digits (at most `max` of them, with `.` / `,` when `decimal`) that end at byte
/// `end` of `s`, after skipping up to two separators (` `, `-`, `_`).
fn number_before(s: &str, end: usize, decimal: bool, max: usize) -> Option<&str> {
    let head = &s[..end];
    let trimmed = head.trim_end_matches([' ', '-', '_']);
    if head.len() - trimmed.len() > 2 {
        return None;
    }
    let start = trimmed
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit() || (decimal && (*c == '.' || *c == ',')))
        .last()
        .map(|(i, _)| i)?;
    let n = &trimmed[start..];
    // A longer run of digits is a serial or part number, not an angle.
    let before = trimmed[..start].chars().next_back();
    if n.len() > max || before.is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(n)
}

fn angle(s: &str) -> Option<u32> {
    let mut from = 0;
    while from < s.len() {
        let (at, unit) = ANGLE_UNITS
            .iter()
            .filter_map(|u| s[from..].find(u).map(|i| (from + i, *u)))
            .min_by_key(|(i, u)| (*i, std::cmp::Reverse(u.len())))?;
        let after = s[at + unit.len()..].chars().next();
        if !after.is_some_and(char::is_alphabetic)
            && let Some(n) = number_before(s, at, false, 3)
            && let Ok(v) = n.parse::<u32>()
            && v <= 360
        {
            return Some(v);
        }
        from = at + unit.len();
    }
    None
}

fn sensitivity(s: &str) -> Option<f64> {
    let at = s.find("mv/pa")?;
    let n = number_before(s, at, true, 8)?;
    let v: f64 = n.replace(',', ".").parse().ok()?;
    (v.is_finite() && v > 0.0 && v < 10_000.0).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/mic_curves")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn beyerdynamic_mm1_files() {
        let zero = file_info(&fixture("449350_34804_0Grad.txt"), "449350_34804_0Grad.txt");
        // No angle in the 0° file's header: the file name says it.
        assert_eq!(zero.angle_deg, Some(0));
        assert_eq!(zero.stated_sensitivity_mv_per_pa, Some(15.0));
        let ninety = file_info(&fixture("449350_34804_90Grad.txt"), "/tmp/x/renamed.txt");
        // The header names it (`90-degree-curve`) even when the file was renamed.
        assert_eq!(ninety.angle_deg, Some(90));
        assert_eq!(ninety.stated_sensitivity_mv_per_pa, Some(15.0));
        assert!(super::super::MicCurve::parse(&fixture("449350_34804_90Grad.txt")).is_ok());
    }

    #[test]
    fn angles_and_what_is_not_one() {
        let a = |text: &str, name: &str| file_info(text.as_bytes(), name).angle_deg;
        assert_eq!(a("20 0\n", "ecm_0deg.frd"), Some(0));
        assert_eq!(a("20 0\n", "M30 90 degrees.txt"), Some(90));
        assert_eq!(a("* 90° incidence\n20 0\n", "x.txt"), Some(90));
        assert_eq!(a("20 0\n", "UMIK_7001234_90deg.txt"), Some(90));
        // A phase column header, a serial number, a gradient: no angle.
        assert_eq!(
            a("* Freq(Hz) SPL(dB) Phase(degrees)\n20 0\n", "m30.frd"),
            None
        );
        assert_eq!(a("20 0\n", "7001234deg.txt"), None);
        assert_eq!(a("; 3 gradient\n20 0\n", "x.txt"), None);
        assert_eq!(a("20 0\n", "x.txt"), None);
        // Data lines are never header text.
        assert_eq!(a("20 90deg\n", "x.txt"), None);
    }

    #[test]
    fn sensitivities() {
        let s = |text: &str| file_info(text.as_bytes(), "x.txt").stated_sensitivity_mv_per_pa;
        assert_eq!(s("; Sensitivity: 15.0mV/Pa = -36.5dBV\n20 0\n"), Some(15.0));
        assert_eq!(s("* sens 12,5 mV/Pa\n20 0\n"), Some(12.5));
        assert_eq!(s("\"Sens Factor =-1.378dB\"\n20 0\n"), None);
        assert_eq!(s("20 0\n"), None);
    }
}
