//! Generic delimited-table parsing shared by every text import: delimiters, numbers,
//! column layout, row validation and the grid the rows land on.

use ac2_proto::GridDef;
use ac2_proto::ImportProblem;
use ac2_proto::frame::MAX_N;

use crate::columns::{Columns, frequencies, resample, wrap_deg};

use super::{ImportError, fail};

pub(super) fn is_comment(l: &str) -> bool {
    ["#", "*", ";", "%", "!", "//"]
        .iter()
        .any(|p| l.starts_with(p))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Delim {
    Semicolon,
    Tab,
    Comma,
    Space,
}

pub(super) fn number(s: &str) -> Option<f64> {
    let t = s.trim().trim_matches('"').trim();
    if t.eq_ignore_ascii_case("nan") {
        return Some(f64::NAN);
    }
    let v: f64 = t.parse().ok()?;
    // `inf` parses but is never a measured value.
    (!v.is_infinite()).then_some(v)
}

/// [`number`] parsed straight to f32 (already checked by [`number`]).
pub(super) fn number32(s: &str) -> f32 {
    let t = s.trim().trim_matches('"').trim();
    if t.eq_ignore_ascii_case("nan") {
        return f32::NAN;
    }
    t.parse().unwrap_or(f32::NAN)
}

pub(super) fn split(l: &str, d: Delim) -> Vec<String> {
    let fields: Vec<String> = match d {
        Delim::Semicolon => l.split(';').map(|f| f.replace(',', ".")).collect(),
        Delim::Tab => l.split('\t').map(|f| f.replace(',', ".")).collect(),
        Delim::Comma => l.split(',').map(str::to_owned).collect(),
        Delim::Space => l.split_whitespace().map(|f| f.replace(',', ".")).collect(),
    };
    let mut fields: Vec<String> = fields.into_iter().map(|f| f.trim().to_owned()).collect();
    // A trailing separator leaves an empty last field.
    while fields.last().is_some_and(String::is_empty) {
        fields.pop();
    }
    fields
}

/// The delimiter that splits `l` into at least two numbers, if any.
pub(super) fn detect(l: &str) -> Option<Delim> {
    let numeric = |d| {
        let f = split(l, d);
        f.len() >= 2 && f.iter().all(|x| number(x).is_some())
    };
    let mut order = Vec::new();
    if l.contains(';') {
        order.push(Delim::Semicolon);
    }
    if l.contains('\t') {
        order.push(Delim::Tab);
    }
    if l.contains(',') {
        order.push(Delim::Comma);
    }
    order.push(Delim::Space);
    order.into_iter().find(|d| numeric(*d))
}

/// Column roles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Layout {
    pub(super) freq: usize,
    pub(super) mag: usize,
    pub(super) phase: Option<usize>,
    pub(super) coh: Option<usize>,
}

pub(super) fn positional(n: usize) -> Layout {
    Layout {
        freq: 0,
        mag: 1,
        phase: (n > 2).then_some(2),
        coh: (n > 3).then_some(3),
    }
}

/// Maps a header row onto columns by name; `None` unless it names frequency and magnitude.
pub(super) fn named(header: &[String]) -> Option<Layout> {
    let find = |pred: &dyn Fn(&str) -> bool| {
        header
            .iter()
            .position(|h| pred(&h.to_lowercase().replace('"', "")))
    };
    let freq = find(&|h| h.contains("freq") || h == "hz" || h == "f")?;
    let coh = find(&|h| h.contains("coh"));
    let phase = find(&|h| h.contains("phase") || h.contains("deg"));
    let mag = find(&|h| {
        ["mag", "spl", "db", "level", "gain", "amp", "response"]
            .iter()
            .any(|k| h.contains(k))
            && !h.contains("phase")
            && !h.contains("freq")
    })?;
    let distinct = [Some(freq), Some(mag), phase, coh];
    for (i, a) in distinct.iter().enumerate() {
        if a.is_some() && distinct[..i].contains(a) {
            return None;
        }
    }
    Some(Layout {
        freq,
        mag,
        phase,
        coh,
    })
}

pub(super) struct Table {
    /// Data rows as checked numeric fields; parsed per column type later, so magnitudes go
    /// straight to f32 (one rounding, exact for what `export_csv` wrote).
    pub(super) rows: Vec<(usize, Vec<String>)>,
    pub(super) layout: Layout,
}

/// Reads the numeric table of `lines` (1-based line numbers).
pub(super) fn table(lines: &[(usize, &str)], delim: Option<Delim>) -> Result<Table, ImportError> {
    let mut header: Option<Vec<String>> = None;
    let mut delim = delim;
    let mut rows: Vec<(usize, Vec<String>)> = Vec::new();
    let mut width = 0usize;
    for &(no, l) in lines {
        let l = l.trim();
        if l.is_empty() || is_comment(l) {
            continue;
        }
        if rows.is_empty() {
            let d = match delim {
                Some(d) => {
                    let f = split(l, d);
                    (f.len() >= 2 && f.iter().all(|x| number(x).is_some())).then_some(d)
                }
                None => detect(l),
            };
            let Some(d) = d else {
                // Before the first data row a non-numeric line is a header.
                header = Some(
                    [Delim::Semicolon, Delim::Tab, Delim::Comma]
                        .into_iter()
                        .find(|d| split(l, *d).len() >= 2)
                        .map_or_else(|| split(l, Delim::Space), |d| split(l, d)),
                );
                continue;
            };
            delim = Some(d);
        }
        let Some(d) = delim else { continue };
        let fields = split(l, d);
        if rows.is_empty() {
            width = fields.len();
        } else if fields.len() != width {
            return Err(fail(
                Some(no),
                ImportProblem::ColumnCount,
                format!("{} columns, the first data row has {width}", fields.len()),
            ));
        }
        if let Some(f) = fields.iter().find(|f| number(f).is_none()) {
            return Err(fail(
                Some(no),
                ImportProblem::BadNumber,
                format!("{f:?} is not a number"),
            ));
        }
        rows.push((no, fields));
        if rows.len() > MAX_N as usize {
            return Err(fail(
                Some(no),
                ImportProblem::TooManyRows,
                format!("more than {MAX_N} rows"),
            ));
        }
    }
    if rows.is_empty() {
        return Err(fail(None, ImportProblem::NoData, "no data rows"));
    }
    if width < 2 {
        return Err(fail(
            Some(rows[0].0),
            ImportProblem::TooFewColumns,
            "frequency and magnitude columns are required",
        ));
    }
    let layout = header
        .as_deref()
        .filter(|h| h.len() == width)
        .and_then(named)
        .unwrap_or_else(|| positional(width));
    Ok(Table { rows, layout })
}

pub(super) struct Raw {
    pub(super) freqs: Vec<f64>,
    columns: Columns,
}

pub(super) fn validate(t: &Table) -> Result<Raw, ImportError> {
    let lay = t.layout;
    let mut freqs = Vec::with_capacity(t.rows.len());
    let mut mag = Vec::with_capacity(t.rows.len());
    let mut phase = lay.phase.map(|_| Vec::with_capacity(t.rows.len()));
    let mut coh = lay.coh.map(|_| Vec::with_capacity(t.rows.len()));
    for (no, r) in &t.rows {
        let f = number(&r[lay.freq]).unwrap_or(f64::NAN);
        // 0 Hz is accepted as the first row only (the DC bin of an FFT grid).
        if !(f.is_finite() && (f > 0.0 || (f == 0.0 && freqs.is_empty()))) {
            return Err(fail(
                Some(*no),
                ImportProblem::OutOfRange,
                format!("frequency {f} is not a positive number"),
            ));
        }
        if freqs.last().is_some_and(|p| f <= *p) {
            return Err(fail(
                Some(*no),
                ImportProblem::NotAscending,
                format!("frequency {f} Hz does not ascend"),
            ));
        }
        freqs.push(f);
        let m = number32(&r[lay.mag]);
        // A finite f64 text can still be ±∞ as f32, or beyond what later math keeps finite.
        if !crate::check::db_ok(m) {
            return Err(fail(
                Some(*no),
                ImportProblem::OutOfRange,
                "magnitude out of range",
            ));
        }
        mag.push(m);
        if let (Some(p), Some(i)) = (phase.as_mut(), lay.phase) {
            let v = number32(&r[i]);
            // In-range values pass through exactly; unwrapped exports are wrapped.
            p.push(if !v.is_finite() {
                f32::NAN
            } else if v > -180.0 && v <= 180.0 {
                v
            } else {
                wrap_deg(f64::from(v)) as f32
            });
        }
        if let (Some(c), Some(i)) = (coh.as_mut(), lay.coh) {
            c.push(number32(&r[i]));
        }
    }
    if mag.iter().filter(|v| v.is_finite()).count() < 2 {
        return Err(fail(
            None,
            ImportProblem::NoData,
            "fewer than two rows with a magnitude",
        ));
    }
    let coherence = match coh {
        None => None,
        Some(c) => {
            // Percent when most values are above 1 (a coherence in 0…1 never is).
            let finite = c.iter().filter(|v| v.is_finite()).count();
            let above = c.iter().filter(|v| v.is_finite() && **v > 1.0).count();
            let percent = above * 2 > finite;
            let mut out = Vec::with_capacity(c.len());
            for (v, (no, _)) in c.iter().zip(&t.rows) {
                let s = if percent { v / 100.0 } else { *v };
                if s.is_finite() && !(0.0..=1.0).contains(&s) {
                    return Err(fail(
                        Some(*no),
                        ImportProblem::BadCoherence,
                        format!("coherence {v} is outside 0…1 (or 0…100 %)"),
                    ));
                }
                out.push(s);
            }
            Some(out)
        }
    };
    Ok(Raw {
        freqs,
        columns: Columns {
            mag_db: mag,
            phase_deg: phase,
            coherence,
        },
    })
}

/// Points per octave of the log grid an off-grid import is resampled onto: 48 (the
/// default measurement grid, so A−B against a capture needs no second resampling), 96
/// when the file is denser than that.
fn import_grid(freqs: &[f64]) -> Result<GridDef, ImportError> {
    let lo = freqs.iter().copied().find(|f| *f > 0.0).unwrap_or(1.0);
    let hi = freqs[freqs.len() - 1];
    let octaves = (hi / lo).log2();
    let density = if octaves > 0.0 {
        (freqs.len() - 1) as f64 / octaves
    } else {
        0.0
    };
    let ppo: u32 = if density > 48.0 { 96 } else { 48 };
    let p = f64::from(ppo);
    // A small tolerance keeps a file written on this very grid on it.
    let k_min = (p * (lo / 1000.0).log2() - 1e-6).ceil() as i32;
    let k_max = (p * (hi / 1000.0).log2() + 1e-6).floor() as i32;
    if k_max <= k_min {
        return Err(fail(
            None,
            ImportProblem::NoData,
            format!("the data spans less than two columns of a 1/{ppo}-octave grid"),
        ));
    }
    Ok(GridDef::Log { ppo, k_min, k_max })
}

pub(super) fn on_import_grid(
    raw: Raw,
    known: Option<GridDef>,
) -> Result<(GridDef, Columns), ImportError> {
    if let Some(g) = known {
        let gf = frequencies(&g);
        let same = gf.len() == raw.freqs.len()
            && gf
                .iter()
                .zip(&raw.freqs)
                .all(|(a, b)| (a - b).abs() <= 1e-9 * a.abs().max(1.0));
        if same {
            return Ok((g, raw.columns));
        }
    }
    let g = import_grid(&raw.freqs)?;
    let c = resample(&raw.freqs, &raw.columns, &frequencies(&g));
    Ok((g, c))
}
