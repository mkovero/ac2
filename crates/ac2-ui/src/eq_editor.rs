//! The EQ editor (Alt+E on the transfer pane, `docs/design/eq-overlay.md` §3): the selected
//! measurement's set, its sections, and its curves with what each prediction assumes is in.
//! Pure data: the reducer routes keys here, every change is one `meas.eq` `put` of the
//! working copy, and the daemon validates it (its refusal is shown verbatim).

use ac2_proto::model::{
    Alignment, Bandwidth, BesselNorm, EqBody, EqRealisation, EqSection, EqSet, EqShape, MeasKind,
    Measurement, Operand, PeakQ, ShelfFreq, ShelfOrder, ShelfSlope, TraceKind, TraceMeta,
    TraceOwner, TraceSource,
};
use ac2_proto::units::{Db, EqSetId, Hz, MeasId, Seconds};
use ac2_scene::eq::{self as text, FieldKind};

/// Which part of the editor the arrows act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// The set: name / switcher, realisation, default Q reading.
    Header,
    /// A section, by index.
    Section(usize),
    /// A curve of the measurement, by index in [`curves`].
    Curve(usize),
}

/// The header's fields, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderField {
    Set,
    Realisation,
    PeakQ,
}

const HEADER_FIELDS: [HeaderField; 3] = [
    HeaderField::Set,
    HeaderField::Realisation,
    HeaderField::PeakQ,
];

/// The rates a digital set cycles through (←/→ on the realisation), then analog.
const RATES: [f64; 6] = [44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0];

/// The editor.
#[derive(Clone, Debug, PartialEq)]
pub struct EqEditor {
    pub meas: MeasId,
    /// The set shown; `None` until the daemon has made the first one.
    pub set: Option<EqSetId>,
    /// What the editor last sent, while replies are outstanding: the mirror catches up
    /// behind it, so a quick second step builds on the first.
    pub working: Option<EqBody>,
    /// `put`s not answered yet.
    pub pending: u32,
    pub row: Row,
    /// Field index on the header or the selected section.
    pub field: usize,
    /// A number being typed into the field.
    pub typed: Option<String>,
    /// M asked: the mark is confirmed by M (or cleared by Shift+M).
    pub confirm_mark: bool,
    /// The daemon's last refusal, verbatim.
    pub refusal: Option<String>,
}

/// A curve of the measurement the set can predict.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub operand: Operand,
    pub name: String,
}

impl EqEditor {
    pub fn new(meas: MeasId, set: Option<EqSetId>) -> Self {
        Self {
            meas,
            set,
            working: None,
            pending: 0,
            row: Row::Header,
            field: 0,
            typed: None,
            confirm_mark: false,
            refusal: None,
        }
    }

    /// The set shown, from the mirror.
    pub fn set_of<'m>(&self, m: &'m Measurement) -> Option<&'m EqSet> {
        let id = self.set.or_else(|| m.eq.first().map(|s| s.id))?;
        m.eq.iter().find(|s| s.id == id)
    }

    /// The body the editor shows and edits: what it sent while replies are outstanding,
    /// else the daemon's.
    pub fn body(&self, m: &Measurement) -> Option<EqBody> {
        match (&self.working, self.pending) {
            (Some(b), p) if p > 0 => Some(b.clone()),
            _ => self.set_of(m).map(|s| s.body.clone()),
        }
    }

    /// The field kinds of the selected row.
    pub fn row_fields(&self, body: &EqBody) -> &'static [FieldKind] {
        match self.row {
            Row::Section(i) => body.sections.get(i).map_or(&[], |s| text::fields(&s.shape)),
            _ => &[],
        }
    }

    /// The focused field of a section row.
    pub fn section_field(&self, body: &EqBody) -> Option<FieldKind> {
        self.row_fields(body).get(self.field).copied()
    }

    /// The focused field of the header row.
    pub fn header_field(&self) -> Option<HeaderField> {
        (self.row == Row::Header).then(|| HEADER_FIELDS[self.field.min(2)])
    }

    /// ↑/↓ over header, sections, curves.
    pub fn move_row(&mut self, d: i32, sections: usize, curves: usize) {
        let flat = |r: Row| match r {
            Row::Header => 0,
            Row::Section(i) => 1 + i,
            Row::Curve(i) => 1 + sections + i,
        };
        let n = 1 + sections + curves;
        let at = (flat(self.row) as i32 + d).clamp(0, n as i32 - 1) as usize;
        self.row = match at {
            0 => Row::Header,
            i if i <= sections => Row::Section(i - 1),
            i => Row::Curve(i - 1 - sections),
        };
        self.field = if matches!(self.row, Row::Section(_)) {
            1.min(self.field)
        } else {
            0
        };
        self.typed = None;
    }

    /// Tab / Shift+Tab over the row's fields.
    pub fn move_field(&mut self, d: i32, body: &EqBody) {
        let n = match self.row {
            Row::Header => HEADER_FIELDS.len(),
            Row::Section(_) => self.row_fields(body).len(),
            Row::Curve(_) => 1,
        };
        if n > 0 {
            self.field = (self.field as i32 + d).rem_euclid(n as i32) as usize;
        }
        self.typed = None;
    }
}

/// Every transfer curve of the measurement: its live curve, then its stored transfer
/// traces (captures, sweep runs, imports filed under it), not its EQ curves.
pub fn curves(m: &Measurement, traces: &[&TraceMeta]) -> Vec<Curve> {
    let owner = TraceOwner::Meas { meas: m.id };
    std::iter::once(Curve {
        operand: Operand::Meas { meas: m.id },
        name: format!("{} (live)", m.config.name),
    })
    .chain(
        traces
            .iter()
            .filter(|t| {
                t.edit.owner == owner
                    && t.kind == TraceKind::Transfer
                    && !matches!(t.source, TraceSource::Eq { .. })
            })
            .map(|t| Curve {
                operand: Operand::Trace { trace: t.id },
                name: t.edit.name.clone(),
            }),
    )
    .collect()
}

/// Whether `curve` is predicted after the set: a math channel `curve × EQ` exists.
pub fn predicted(ms: &[&Measurement], meas: MeasId, set: EqSetId, curve: &Operand) -> bool {
    ms.iter().any(|p| match &p.config.kind {
        MeasKind::Math { config } => text::prediction(&config.expr) == Some((meas, set, curve)),
        _ => false,
    })
}

/// What Insert adds: a peaking section at 1 kHz, no gain yet, Q 1.
pub fn new_section() -> EqSection {
    EqSection {
        enabled: true,
        shape: EqShape::Peaking {
            f: Hz(1000.0),
            gain: Db(0.0),
            bw: Bandwidth::Q { value: 1.0 },
            q_measured: None,
        },
    }
}

/// How far ←/→ step: plain, Shift (×4 coarser), Alt (finer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Plain,
    Coarse,
    Fine,
}

impl Step {
    fn scale(self) -> f64 {
        match self {
            Step::Plain => 1.0,
            Step::Coarse => 4.0,
            Step::Fine => 0.25,
        }
    }
}

fn freq_of(shape: &mut EqShape) -> Option<&mut Hz> {
    match shape {
        EqShape::Peaking { f, .. }
        | EqShape::LowShelf { f, .. }
        | EqShape::HighShelf { f, .. }
        | EqShape::HighPass { f, .. }
        | EqShape::LowPass { f, .. }
        | EqShape::HighPassQ { f, .. }
        | EqShape::LowPassQ { f, .. }
        | EqShape::Notch { f, .. }
        | EqShape::BandPass { f, .. }
        | EqShape::AllPass1 { f }
        | EqShape::AllPass2 { f, .. } => Some(f),
        _ => None,
    }
}

fn gain_of(shape: &mut EqShape) -> Option<&mut Db> {
    match shape {
        EqShape::Peaking { gain, .. }
        | EqShape::LowShelf { gain, .. }
        | EqShape::HighShelf { gain, .. }
        | EqShape::Gain { gain } => Some(gain),
        _ => None,
    }
}

/// A Q stepped by `oct` octaves of bandwidth (wider for positive steps).
fn step_q(q: f64, oct: f64) -> f64 {
    let n = (text::octaves_of_q(q) + oct).max(1.0 / 96.0);
    text::q_of_octaves(n)
}

/// The shapes ←/→ cycle through on the shape field.
const SHAPES: [&str; 12] = [
    "peaking",
    "low shelf",
    "high shelf",
    "high-pass",
    "low-pass",
    "notch",
    "band-pass",
    "all-pass 2nd order",
    "all-pass 1st order",
    "gain",
    "delay",
    "polarity",
];

/// The shape after `d` steps through [`SHAPES`], keeping f, gain and Q where it has them.
fn cycle_shape(shape: &EqShape, d: i32) -> EqShape {
    let mut s = *shape;
    let f = freq_of(&mut s).map_or(Hz(1000.0), |f| *f);
    let gain = gain_of(&mut s).map_or(Db(0.0), |g| *g);
    let q = match *shape {
        EqShape::Peaking {
            bw: Bandwidth::Q { value },
            ..
        }
        | EqShape::Notch {
            bw: Bandwidth::Q { value },
            ..
        }
        | EqShape::BandPass {
            bw: Bandwidth::Q { value },
            ..
        }
        | EqShape::HighPassQ { q: value, .. }
        | EqShape::LowPassQ { q: value, .. }
        | EqShape::AllPass2 { q: value, .. } => value,
        _ => 1.0,
    };
    let at = SHAPES
        .iter()
        .position(|w| *w == text::shape_word(shape))
        .unwrap_or(0);
    let next = SHAPES[(at as i32 + d).rem_euclid(SHAPES.len() as i32) as usize];
    let bw = Bandwidth::Q { value: q };
    let order = ShelfOrder::Second {
        slope: ShelfSlope::S { value: 1.0 },
    };
    match next {
        "peaking" => EqShape::Peaking {
            f,
            gain,
            bw,
            q_measured: None,
        },
        "low shelf" => EqShape::LowShelf {
            f,
            gain,
            order,
            f_at: ShelfFreq::Midpoint,
        },
        "high shelf" => EqShape::HighShelf {
            f,
            gain,
            order,
            f_at: ShelfFreq::Midpoint,
        },
        "high-pass" => EqShape::HighPass {
            f,
            alignment: Alignment::LinkwitzRiley,
            order: 4,
        },
        "low-pass" => EqShape::LowPass {
            f,
            alignment: Alignment::LinkwitzRiley,
            order: 4,
        },
        "notch" => EqShape::Notch { f, bw },
        "band-pass" => EqShape::BandPass { f, bw },
        "all-pass 2nd order" => EqShape::AllPass2 { f, q },
        "all-pass 1st order" => EqShape::AllPass1 { f },
        "gain" => EqShape::Gain { gain },
        "delay" => EqShape::Delay {
            seconds: Seconds(0.001),
        },
        _ => EqShape::Polarity,
    }
}

fn cycle_alignment(a: Alignment, d: i32) -> Alignment {
    let all = [
        Alignment::Butterworth,
        Alignment::LinkwitzRiley,
        Alignment::Bessel {
            norm: BesselNorm::Phase,
        },
        Alignment::Bessel {
            norm: BesselNorm::Delay,
        },
        Alignment::Bessel {
            norm: BesselNorm::Minus3db,
        },
    ];
    let at = all.iter().position(|x| *x == a).unwrap_or(0);
    all[(at as i32 + d).rem_euclid(all.len() as i32) as usize]
}

fn cycle_peak_q(p: PeakQ, d: i32) -> PeakQ {
    let all = [PeakQ::HalfGain, PeakQ::Minus3dbFromPeak, PeakQ::BandPass];
    let at = all.iter().position(|x| *x == p).unwrap_or(0);
    all[(at as i32 + d).rem_euclid(3) as usize]
}

/// ←/→ on a section's field: one step of `d` (±1).
pub fn step_field(shape: &mut EqShape, field: FieldKind, d: i32, step: Step, peak_q: PeakQ) {
    let d64 = f64::from(d) * step.scale();
    match field {
        FieldKind::Shape => *shape = cycle_shape(shape, d),
        FieldKind::Freq => {
            if let Some(f) = freq_of(shape) {
                f.0 = (f.0 * 2f64.powf(d64 / 24.0)).clamp(1.0, 100_000.0);
            }
        }
        FieldKind::Gain => {
            if let Some(g) = gain_of(shape) {
                g.0 = ((g.0 + 0.5 * d64) * 100.0).round() / 100.0;
            }
        }
        FieldKind::Q => {
            // Wider bandwidth to the left, as a knob's Q turns.
            let oct = -d64 / 12.0;
            match shape {
                EqShape::Peaking { bw, .. }
                | EqShape::Notch { bw, .. }
                | EqShape::BandPass { bw, .. } => match bw {
                    Bandwidth::Q { value } => *value = step_q(*value, oct),
                    Bandwidth::Octaves { n } => *n = (*n + oct).max(1.0 / 96.0),
                },
                EqShape::HighPassQ { q, .. }
                | EqShape::LowPassQ { q, .. }
                | EqShape::AllPass2 { q, .. } => *q = step_q(*q, oct),
                EqShape::LowShelf { order, .. } | EqShape::HighShelf { order, .. } => {
                    if let ShelfOrder::Second { slope } = order {
                        match slope {
                            ShelfSlope::S { value } => {
                                *value = ((*value + 0.1 * d64) * 100.0).round().max(1.0) / 100.0;
                            }
                            ShelfSlope::Q { value } => *value = step_q(*value, oct),
                            ShelfSlope::Octaves { n } => *n = (*n + oct).max(1.0 / 96.0),
                        }
                    }
                }
                _ => {}
            }
        }
        FieldKind::QMeasured => {
            if let EqShape::Peaking { q_measured, .. } = shape {
                *q_measured = Some(cycle_peak_q(q_measured.unwrap_or(peak_q), d));
            }
        }
        FieldKind::Alignment => {
            if let EqShape::HighPass { alignment, .. } | EqShape::LowPass { alignment, .. } = shape
            {
                *alignment = cycle_alignment(*alignment, d);
            }
        }
        FieldKind::Order => {
            if let EqShape::HighPass {
                order, alignment, ..
            }
            | EqShape::LowPass {
                order, alignment, ..
            } = shape
            {
                // Linkwitz-Riley comes in even orders.
                let by = if *alignment == Alignment::LinkwitzRiley {
                    2
                } else {
                    1
                };
                *order = (i32::from(*order) + d * by).clamp(1, 8) as u8;
            }
        }
        FieldKind::Delay => {
            if let EqShape::Delay { seconds } = shape {
                seconds.0 = (seconds.0 + 1e-4 * d64).max(0.0);
            }
        }
    }
}

/// A typed number into a section's field; `None` when it does not read as one (yet).
pub fn type_field(shape: &mut EqShape, field: FieldKind, typed: &str) -> Option<()> {
    let num = || typed.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    match field {
        FieldKind::Freq => {
            let f =
                ac2_scene::unit_text::in_field::<ac2_scene::unit_text::Freq>(typed.trim(), "hz")
                    .ok()?;
            *freq_of(shape)? = f.0;
        }
        FieldKind::Gain => gain_of(shape)?.0 = num()?,
        FieldKind::Q => {
            let q = num()?;
            match shape {
                EqShape::Peaking { bw, .. }
                | EqShape::Notch { bw, .. }
                | EqShape::BandPass { bw, .. } => *bw = Bandwidth::Q { value: q },
                EqShape::HighPassQ { q: v, .. }
                | EqShape::LowPassQ { q: v, .. }
                | EqShape::AllPass2 { q: v, .. } => *v = q,
                EqShape::LowShelf { order, .. } | EqShape::HighShelf { order, .. } => {
                    *order = ShelfOrder::Second {
                        slope: ShelfSlope::Q { value: q },
                    };
                }
                _ => return None,
            }
        }
        FieldKind::Order => {
            let n: u8 = typed.trim().parse().ok()?;
            match shape {
                EqShape::HighPass { order, .. } | EqShape::LowPass { order, .. } => *order = n,
                _ => return None,
            }
        }
        FieldKind::Delay => match shape {
            EqShape::Delay { seconds } => seconds.0 = num()? / 1000.0,
            _ => return None,
        },
        _ => return None,
    }
    Some(())
}

/// ←/→ on the header's realisation: through the digital rates, then analog.
pub fn step_realisation(r: EqRealisation, d: i32) -> EqRealisation {
    let at = match r {
        EqRealisation::Digital { rate } => RATES
            .iter()
            .position(|x| (*x - rate.0).abs() < 0.5)
            .unwrap_or(1),
        EqRealisation::Analog => RATES.len(),
    };
    let next = (at as i32 + d).rem_euclid(RATES.len() as i32 + 1) as usize;
    RATES
        .get(next)
        .map_or(EqRealisation::Analog, |&rate| EqRealisation::Digital {
            rate: Hz(rate),
        })
}

/// ←/→ on the header's default Q reading.
pub fn step_peak_q(p: PeakQ, d: i32) -> PeakQ {
    cycle_peak_q(p, d)
}

/// One shown cell: its text and whether the focus is on it.
pub type Cell = (String, bool);

/// The editor as text, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorView {
    pub title: String,
    /// Set switcher (`EQ 1 | EQ 2`), realisation, default Q reading, mark.
    pub header: Vec<Cell>,
    /// Section rows; the selected one with its fields.
    pub sections: Vec<(Cell, Option<Vec<Cell>>)>,
    pub curves: Vec<Cell>,
    /// The daemon's refusal, the mark question, or the keys.
    pub bottom: String,
    /// `bottom` is a refusal.
    pub refused: bool,
}

/// What the editor shows of `m` (`traces`: every stored trace's meta; `ms`: every
/// measurement, for the predictions).
pub fn view(
    e: &EqEditor,
    m: &Measurement,
    traces: &[&TraceMeta],
    ms: &[&Measurement],
) -> EditorView {
    let title = text::editor_title(&m.config.name);
    let (Some(set), Some(body)) = (e.set_of(m), e.body(m)) else {
        return EditorView {
            title,
            header: vec![("making EQ 1…".to_owned(), false)],
            sections: Vec::new(),
            curves: Vec::new(),
            bottom: text::EDITOR_KEYS.to_owned(),
            refused: false,
        };
    };
    let hf = e.header_field();
    let switcher =
        m.eq.iter()
            .map(|s| {
                if s.id == set.id {
                    body.name.clone()
                } else {
                    s.body.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" | ");
    let mut mark = text::mark_text(set);
    if !body.enabled {
        mark = format!("bypassed · {mark}");
    }
    let header = vec![
        (switcher, hf == Some(HeaderField::Set)),
        (
            text::realisation_text(body.realisation),
            hf == Some(HeaderField::Realisation),
        ),
        (
            format!("Q measured {}", text::peak_q_text(body.peak_q)),
            hf == Some(HeaderField::PeakQ),
        ),
        (mark, false),
    ];
    let trace_name = |id| {
        traces
            .iter()
            .find(|t| t.id == id)
            .map_or_else(|| format!("trace {}", id.0), |t| t.edit.name.clone())
    };
    let sections = body
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = e.row == Row::Section(i);
            let row = (text::section_text(i, s, body.peak_q, &trace_name), selected);
            let fields = selected.then(|| {
                text::fields(&s.shape)
                    .iter()
                    .enumerate()
                    .map(|(k, f)| {
                        let focused = k == e.field;
                        let t = match (&e.typed, focused) {
                            (Some(typed), true) => format!("{typed}▏"),
                            _ => text::field_text(&s.shape, *f, body.peak_q),
                        };
                        (t, focused)
                    })
                    .collect()
            });
            (row, fields)
        })
        .collect();
    let curves = curves(m, traces)
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let basis = text::curve_basis(set, &c.operand, |id| {
                traces.iter().find(|t| t.id == id).copied()
            });
            (
                text::curve_row(
                    &c.name,
                    predicted(ms, m.id, set.id, &c.operand),
                    &body.name,
                    basis,
                ),
                e.row == Row::Curve(i),
            )
        })
        .collect();
    let (bottom, refused) = match (&e.refusal, e.confirm_mark) {
        (_, true) => (text::mark_question(set, &m.config.name), false),
        (Some(r), _) => (r.clone(), true),
        _ => (text::EDITOR_KEYS.to_owned(), false),
    };
    EditorView {
        title,
        header,
        sections,
        curves,
        bottom,
        refused,
    }
}

#[cfg(test)]
mod tests;
