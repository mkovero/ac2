//! Room parameters (ISO 3382-1) of a sweep trace as a table: one column per band and one
//! for broadband, one row per parameter, every string the app and the CLI show
//! (`docs/design/room-metrics.md`).
//!
//! A refused value is never a number: its cell names why in a word (`noise`: the decay
//! meets the noise too soon; `short`: too short a decay for the band's filter; `—`: no
//! decay), and the legend says what each word means. A T30 whose decay is curved (more
//! than 10 % above T20) carries a `*`.

use ac2_proto::model::{RoomAcoustics, RoomBand, RoomRefusal, RoomValue};

use crate::canvas::{anchor, label, text_width};
use crate::format;
use crate::primitives::{FillRect, HAlign, Layer, Rect, VAlign};
use crate::theme::Theme;

/// Which bands a table shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandSet {
    /// Octave bands.
    Octave,
    /// One-third-octave bands.
    Third,
}

/// A row of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    Edt,
    T20,
    T30,
    C50,
    C80,
    D50,
    /// Decay range: how far the decay falls before it meets the noise.
    Range,
}

impl Param {
    pub const ALL: [Param; 7] = [
        Param::Edt,
        Param::T20,
        Param::T30,
        Param::C50,
        Param::C80,
        Param::D50,
        Param::Range,
    ];

    /// Row name with its unit: `T30 (s)`.
    pub fn name(self) -> &'static str {
        match self {
            Param::Edt => "EDT (s)",
            Param::T20 => "T20 (s)",
            Param::T30 => "T30 (s)",
            Param::C50 => "C50 (dB)",
            Param::C80 => "C80 (dB)",
            Param::D50 => "D50 (%)",
            Param::Range => "Range (dB)",
        }
    }

    /// Key for machine-readable output: `t30_s`.
    pub fn key(self) -> &'static str {
        match self {
            Param::Edt => "edt_s",
            Param::T20 => "t20_s",
            Param::T30 => "t30_s",
            Param::C50 => "c50_db",
            Param::C80 => "c80_db",
            Param::D50 => "d50",
            Param::Range => "decay_range_db",
        }
    }

    fn value(self, b: &RoomBand) -> RoomValue {
        match self {
            Param::Edt => b.edt,
            Param::T20 => b.t20,
            Param::T30 => b.t30,
            Param::C50 => b.c50,
            Param::C80 => b.c80,
            Param::D50 => b.d50,
            Param::Range => match b.decay_range {
                Some(r) => RoomValue::Value { value: r.0 },
                None => RoomValue::Refused {
                    reason: RoomRefusal::NoDecay,
                },
            },
        }
    }

    /// The number as shown: decay times to 10 ms, ratios to 0.1 dB, D50 in whole percent,
    /// the range in whole dB.
    pub fn number(self, v: f64) -> String {
        match self {
            Param::Edt | Param::T20 | Param::T30 => format::fixed(v, 2),
            Param::C50 | Param::C80 => format::fixed(v, 1),
            Param::D50 => format::fixed(100.0 * v, 0),
            Param::Range => format::fixed(v, 0),
        }
    }
}

/// The word a refused cell shows.
pub fn refusal_word(r: RoomRefusal) -> &'static str {
    match r {
        RoomRefusal::InsufficientRange { .. } => "noise",
        RoomRefusal::FilterLimited { .. } => "short",
        RoomRefusal::NoDecay => format::NO_VALUE,
    }
}

/// Why a value is refused, in a sentence.
pub fn refusal_text(r: RoomRefusal) -> String {
    match r {
        RoomRefusal::InsufficientRange { range, needed } => format!(
            "the decay meets the noise after {} dB; this needs {} dB",
            format::fixed(range.0, 0),
            format::fixed(needed.0, 0)
        ),
        RoomRefusal::FilterLimited { .. } => {
            "the decay is too short for this band's filter".to_owned()
        }
        RoomRefusal::NoDecay => "no decay in this band".to_owned(),
    }
}

/// `63`, `125`, `1k`, `1.25k`, `10k`: the nominal IEC name of a mid-band frequency;
/// `All` for broadband.
pub fn band_name(centre: Option<f64>) -> String {
    const NOMINAL: [f64; 31] = [
        20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
        500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0,
        6300.0, 8000.0, 10_000.0, 12_500.0, 16_000.0, 20_000.0,
    ];
    let Some(f) = centre else {
        return "All".to_owned();
    };
    let n = NOMINAL
        .iter()
        .copied()
        .min_by(|a, b| (a / f).ln().abs().total_cmp(&(b / f).ln().abs()))
        .unwrap_or(f);
    if n >= 1000.0 {
        let k = n / 1000.0;
        let s = format!("{k}");
        format!("{s}k")
    } else {
        format!("{n}")
    }
}

/// One cell.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomCell {
    pub text: String,
    /// Refused: the text is a word, not a number.
    pub refused: Option<RoomRefusal>,
}

/// One parameter across the bands.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomRow {
    pub param: Param,
    pub cells: Vec<RoomCell>,
}

/// The table: band columns (broadband last), parameter rows, and the legend lines for the
/// refusal words and marks that occur.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomTable {
    pub caption: String,
    pub bands: Vec<String>,
    pub rows: Vec<RoomRow>,
    pub legend: Vec<String>,
}

/// T30 longer than T20 by more than this is a curved decay, percent.
const CURVED_PCT: f64 = RoomAcoustics::CURVATURE_LIMIT;

fn cell(p: Param, b: &RoomBand) -> RoomCell {
    match p.value(b) {
        RoomValue::Value { value } => {
            let mut text = p.number(value);
            if p == Param::T30 && b.curvature.is_some_and(|c| c > CURVED_PCT) {
                text.push('*');
            }
            RoomCell {
                text,
                refused: None,
            }
        }
        RoomValue::Refused { reason } => RoomCell {
            text: refusal_word(reason).to_owned(),
            refused: Some(reason),
        },
    }
}

/// The table of `r`'s `set` bands and broadband.
pub fn room_table(r: &RoomAcoustics, set: BandSet) -> RoomTable {
    let bands: Vec<&RoomBand> = match set {
        BandSet::Octave => &r.octave,
        BandSet::Third => &r.third,
    }
    .iter()
    .chain(std::iter::once(&r.broadband))
    .collect();
    let rows: Vec<RoomRow> = Param::ALL
        .iter()
        .map(|&param| RoomRow {
            param,
            cells: bands.iter().map(|b| cell(param, b)).collect(),
        })
        .collect();
    let refused = |f: &dyn Fn(RoomRefusal) -> bool| {
        rows.iter()
            .flat_map(|r| &r.cells)
            .any(|c| c.refused.is_some_and(f))
    };
    let mut legend = Vec::new();
    if refused(&|r| matches!(r, RoomRefusal::InsufficientRange { .. })) {
        legend.push(
            "noise: the decay meets the noise too soon (EDT and C/D need 20 dB, T20 35 dB, \
             T30 45 dB of range)"
                .to_owned(),
        );
    }
    if refused(&|r| matches!(r, RoomRefusal::FilterLimited { .. })) {
        legend.push("short: the decay is too short for the band's filter".to_owned());
    }
    if refused(&|r| matches!(r, RoomRefusal::NoDecay)) {
        legend.push(format!("{}: no decay in the band", format::NO_VALUE));
    }
    if bands
        .iter()
        .any(|b| b.curvature.is_some_and(|c| c > CURVED_PCT) && b.t30.value().is_some())
    {
        legend.push("*: curved decay (T30 more than 10 % above T20)".to_owned());
    }
    RoomTable {
        caption: format!(
            "Room (ISO 3382-1) · {} · decay to {}",
            match set {
                BandSet::Octave => "octave bands",
                BandSet::Third => "⅓-octave bands",
            },
            format::ms(r.span_end.0, 0)
        ),
        bands: bands
            .iter()
            .map(|b| band_name(b.centre.map(|h| h.0)))
            .collect(),
        rows,
        legend,
    }
}

impl RoomTable {
    /// The same table with only the band columns `keep` (indices into [`RoomTable::bands`]).
    fn columns(&self, keep: &[usize]) -> RoomTable {
        RoomTable {
            caption: self.caption.clone(),
            bands: keep.iter().map(|&i| self.bands[i].clone()).collect(),
            rows: self
                .rows
                .iter()
                .map(|r| RoomRow {
                    param: r.param,
                    cells: keep.iter().map(|&i| r.cells[i].clone()).collect(),
                })
                .collect(),
            legend: self.legend.clone(),
        }
    }

    /// Plain text: a header line and one line per parameter, columns padded.
    pub fn text(&self) -> String {
        let name_w = Param::ALL
            .iter()
            .map(|p| p.name().chars().count())
            .max()
            .unwrap_or(0);
        let col_w = self
            .bands
            .iter()
            .map(|b| b.chars().count())
            .chain(
                self.rows
                    .iter()
                    .flat_map(|r| r.cells.iter().map(|c| c.text.chars().count())),
            )
            .max()
            .unwrap_or(0)
            + 2;
        let pad =
            |s: &str, w: usize| format!("{}{s}", " ".repeat(w.saturating_sub(s.chars().count())));
        let mut out = format!("{}\n{}", self.caption, " ".repeat(name_w));
        for b in &self.bands {
            out.push_str(&pad(b, col_w));
        }
        out.push('\n');
        for r in &self.rows {
            out.push_str(&format!(
                "{}{}",
                r.param.name(),
                " ".repeat(name_w - r.param.name().chars().count())
            ));
            for c in &r.cells {
                out.push_str(&pad(&c.text, col_w));
            }
            out.push('\n');
        }
        for l in &self.legend {
            out.push_str(l);
            out.push('\n');
        }
        out
    }
}

/// Height the drawn table needs, logical pixels.
pub fn table_height(t: &RoomTable, theme: &Theme) -> f32 {
    table_height_at(t, theme.small_font_size)
}

/// [`table_height`] at font `size`.
pub fn table_height_at(t: &RoomTable, size: f32) -> f32 {
    let line = size * 1.45;
    line * (2 + t.rows.len()) as f32 + size * 1.3 * t.legend.len() as f32 + 6.0
}

/// Width the table needs with every band shown, at font `size`.
pub fn table_width_at(t: &RoomTable, size: f32) -> f32 {
    name_width(size) + column_width(t, size) * t.bands.len() as f32
}

fn name_width(size: f32) -> f32 {
    Param::ALL
        .iter()
        .map(|p| text_width(p.name(), size))
        .fold(0.0f32, f32::max)
        + 8.0
}

fn column_width(t: &RoomTable, size: f32) -> f32 {
    t.bands
        .iter()
        .map(|b| text_width(b, size))
        .chain(
            t.rows
                .iter()
                .flat_map(|r| r.cells.iter().map(|c| text_width(&c.text, size))),
        )
        .fold(0.0f32, f32::max)
        + 10.0
}

/// The table drawn into `rect`: caption, band header, one row per parameter, legend.
/// Columns that do not fit are dropped from the outer bands inwards (broadband stays),
/// and the caption says how many are hidden. Refused cells are dimmed.
pub fn table_layer(t: &RoomTable, rect: Rect, theme: &Theme) -> Layer {
    table_layer_at(t, rect, theme, theme.small_font_size)
}

/// [`table_layer`] at font `size`.
pub fn table_layer_at(t: &RoomTable, rect: Rect, theme: &Theme, size: f32) -> Layer {
    let line = size * 1.45;
    let name_w = name_width(size);
    let col_w = |t: &RoomTable| column_width(t, size);
    // Keep the middle bands: drop the lowest, then the highest, and so on.
    let n = t.bands.len().saturating_sub(1);
    let mut keep: Vec<usize> = (0..=n).collect();
    let mut low = true;
    let mut shown = t.columns(&keep);
    while keep.len() > 2 && name_w + col_w(&shown) * keep.len() as f32 > rect.w {
        let at = if low { 0 } else { keep.len() - 2 };
        keep.remove(at);
        low = !low;
        shown = t.columns(&keep);
    }
    let hidden = t.bands.len() - keep.len();
    let cw = col_w(&shown);
    let mut layer = Layer::default();
    layer.rects.push(FillRect {
        rect,
        color: theme.background,
        clip: None,
    });
    let caption = if hidden > 0 {
        format!("{} · {hidden} bands hidden (narrow pane)", t.caption)
    } else {
        t.caption.clone()
    };
    let mut y = rect.y + 4.0;
    layer.labels.push(label(
        caption,
        [rect.x, y],
        anchor(HAlign::Left, VAlign::Top),
        size,
        theme.text_dim,
    ));
    y += line;
    for (j, b) in shown.bands.iter().enumerate() {
        layer.labels.push(label(
            b.clone(),
            [rect.x + name_w + cw * (j as f32 + 1.0) - 6.0, y],
            anchor(HAlign::Right, VAlign::Top),
            size,
            theme.text_dim,
        ));
    }
    for r in &shown.rows {
        y += line;
        layer.labels.push(label(
            r.param.name(),
            [rect.x, y],
            anchor(HAlign::Left, VAlign::Top),
            size,
            theme.text,
        ));
        for (j, c) in r.cells.iter().enumerate() {
            layer.labels.push(label(
                c.text.clone(),
                [rect.x + name_w + cw * (j as f32 + 1.0) - 6.0, y],
                anchor(HAlign::Right, VAlign::Top),
                size,
                if c.refused.is_some() {
                    theme.text_dim
                } else {
                    theme.text
                },
            ));
        }
    }
    y += line + 2.0;
    for l in &t.legend {
        layer.labels.push(label(
            l.clone(),
            [rect.x, y],
            anchor(HAlign::Left, VAlign::Top),
            size,
            theme.text_dim,
        ));
        y += size * 1.3;
    }
    for l in &mut layer.labels {
        l.clip = Some(rect);
    }
    layer
}

/// The sweep pane's room view: the table alone under the banners, its font as large as the
/// pane allows (up to [`ROOM_FONT_MAX`] × the theme's font) with every band shown, so it
/// reads across a room; a pane too small for even the small font keeps the narrow-pane
/// rules ([`table_layer`]: outer bands dropped, said in the caption).
#[derive(Clone, Debug, PartialEq)]
pub struct RoomScene {
    pub scene: crate::primitives::Scene,
    pub table: Option<RoomTable>,
    /// The table's font size.
    pub font_size: f32,
    /// Where the table is drawn.
    pub rect: Rect,
    /// Why there is no table (no sweep, or one without room parameters).
    pub note: Option<String>,
    pub strip: Rect,
    pub banners: Vec<crate::banner::BannerRow>,
}

/// Largest table font of the room view, times the theme's font size.
pub const ROOM_FONT_MAX: f32 = 2.0;

/// The room view of `room` (a sweep `name`'s parameters; `None`: says why there are none).
pub fn room_scene(
    room: Option<&RoomAcoustics>,
    name: Option<&str>,
    status: &crate::banner::Status,
    theme: &Theme,
    size: crate::primitives::Viewport,
) -> RoomScene {
    use crate::canvas::{Canvas, MARGINS};
    let mut c = Canvas::new(size, theme);
    let w = (size.width - MARGINS.left - MARGINS.right).max(1.0);
    let strip = crate::canvas::banner_strip(&mut c, status, MARGINS.left, w, size, theme);
    let top = strip.rect.bottom() + MARGINS.top;
    let rect = Rect::new(
        MARGINS.left,
        top,
        w,
        (size.height - top - MARGINS.top).max(1.0),
    );
    let table = room.map(|r| {
        let mut t = room_table(r, BandSet::Octave);
        if let Some(n) = name {
            t.caption = format!("{n} · {}", t.caption);
        }
        t
    });
    let small = theme.small_font_size;
    let font_size = table.as_ref().map_or(small, |t| {
        let mut f = (theme.font_size * ROOM_FONT_MAX).floor();
        while f > small
            && (table_height_at(t, f) > rect.h
                || table_width_at(t, f) > rect.w
                || t.legend.iter().any(|l| text_width(l, f * 0.8) > rect.w))
        {
            f -= 0.5;
        }
        f.max(small)
    });
    let note = match (&table, name) {
        (Some(_), _) => None,
        (None, Some(n)) => Some(format!(
            "{n}: no room parameters (a sweep with silence after it measures them)"
        )),
        (None, None) => Some("no sweep results yet: Shift+S sets one up".to_string()),
    };
    if let Some(t) = &table {
        // The legend lines are long sentences: a smaller size than the numbers keeps them on
        // one line each at the sizes the numbers take.
        let mut layer = table_layer_at(t, rect, theme, font_size);
        if font_size > small {
            let legend_size = (font_size * 0.8).max(small);
            for l in &mut layer.labels {
                if t.legend.contains(&l.text) {
                    l.size = legend_size;
                }
            }
        }
        c.data = layer;
    }
    if let Some(n) = &note {
        c.overlay.labels.push(label(
            n.clone(),
            [rect.x + rect.w / 2.0, rect.y + rect.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            small,
            theme.text_dim,
        ));
    }
    RoomScene {
        scene: c.into_scene(size),
        table,
        font_size,
        rect,
        note,
        strip: strip.rect,
        banners: strip.rows,
    }
}

#[cfg(test)]
mod tests;
