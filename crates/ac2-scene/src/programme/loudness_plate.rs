//! The loudness plate (docs/design/programme-meter.md §6.2 *The loudness plate*): one
//! source's readouts in fixed cells, Int large because it is the delivery figure and the one
//! thing a bar cannot show (it has no window).
//!
//! Nothing on the plate moves when a value changes: every value is right-aligned in a cell
//! sized from [`VALUE_TEMPLATE`] at its slot's size, every status cell from its longest
//! word, captions and units at fixed x either side, and a slot keeps its height when its
//! text is blank. The plate's size follows the font, the scale and which slots show, never
//! the values; the title and the since line are cut to the width the cells set.

use crate::canvas::{anchor, cut_to, label, text_width};
use crate::plate::{Metrics, PlateHover};
use crate::primitives::{HAlign, Layer, Rect, VAlign};
use crate::theme::Theme;

use super::loudness::{LoudnessReadout, RANGE_SETTLED_S, value_text};

/// The widest value a cell holds: integrated is never below −70 (its gate) and the
/// momentary floor is −150.7.
pub const VALUE_TEMPLATE: &str = "−188.8";
/// Under Int before a block passed its gate (the value cell shows its own —).
pub const WAITING_STATUS: &str = "waiting for programme";
/// Beside LRA before a value passed its gate.
pub const RANGE_WAITING: &str = "waiting";
/// Beside LRA for the first minute: Tech 3342 asks for 60 s of programme before a range is
/// more than a first impression.
pub const RANGE_SETTLING: &str = "settling";
/// Steps of the drop order: the foot, the maxima, the range and peak, the live values.
pub const MAX_DROPPED: u8 = 4;

/// A row of the plate, top down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Title,
    Hero,
    HeroStatus,
    Live,
    Maxima,
    RangePeak,
    Foot,
}

impl Slot {
    pub const ALL: [Self; 7] = [
        Self::Title,
        Self::Hero,
        Self::HeroStatus,
        Self::Live,
        Self::Maxima,
        Self::RangePeak,
        Self::Foot,
    ];

    /// Its text size, times the plate's font (a hero's or live row's captions and units
    /// stay at the font itself).
    pub fn size(self) -> f32 {
        match self {
            Self::Hero => 2.5,
            Self::Live => 1.5,
            Self::HeroStatus | Self::Foot => 0.8,
            Self::Title | Self::Maxima | Self::RangePeak => 1.0,
        }
    }

    /// Whether it shows with `dropped` steps of the drop order taken; title, hero and hero
    /// status never drop.
    pub fn shown(self, dropped: u8) -> bool {
        let step = match self {
            Self::Foot => 1,
            Self::Maxima => 2,
            Self::RangePeak => 3,
            Self::Live => 4,
            Self::Title | Self::Hero | Self::HeroStatus => return true,
        };
        dropped < step
    }
}

/// What a cell holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    IntCaption,
    Int,
    IntUnit,
    IntStatus,
    MomCaption,
    Mom,
    MomUnit,
    LiveDot,
    ShortCaption,
    Short,
    ShortUnit,
    MaxMomCaption,
    MaxMom,
    MaximaDot,
    MaxShortCaption,
    MaxShort,
    LraCaption,
    Lra,
    LraUnit,
    LraStatus,
    RangeDot,
    TpCaption,
    Tp,
    TpUnit,
    Foot,
}

impl Field {
    /// A number: drawn with tabular digits, right-aligned in its cell.
    pub fn is_value(self) -> bool {
        matches!(
            self,
            Self::Int
                | Self::Mom
                | Self::Short
                | Self::MaxMom
                | Self::MaxShort
                | Self::Lra
                | Self::Tp
        )
    }

    /// Drawn in the text colour (else the dim one).
    fn bright(self) -> bool {
        self == Self::Title || self.is_value()
    }
}

/// Where a cell's text goes: its anchor point on the row's baseline, its alignment, size,
/// and the width it is cut to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub field: Field,
    pub pos: [f32; 2],
    pub align: HAlign,
    pub size: f32,
    pub width: f32,
}

/// One item of a row as laid out left to right.
enum Item {
    /// Fixed text, left-aligned.
    Text(Field, &'static str, f32),
    /// A value cell, right-aligned, [`VALUE_TEMPLATE`] wide.
    Value(Field, f32),
    /// A status cell, left-aligned, as wide as its longest word.
    Status(Field, &'static str, f32),
}

impl Item {
    fn width(&self) -> f32 {
        match *self {
            Item::Text(_, t, s) | Item::Status(_, t, s) => text_width(t, s),
            Item::Value(_, s) => text_width(VALUE_TEMPLATE, s),
        }
    }
}

/// The items of `slot` at font `f`; the title and foot are whole-row text, cut.
fn items(slot: Slot, f: f32) -> Vec<Item> {
    use Field as F;
    let big = slot.size() * f;
    match slot {
        Slot::Title | Slot::Foot => vec![],
        Slot::Hero => vec![
            Item::Text(F::IntCaption, "Int", f),
            Item::Value(F::Int, big),
            Item::Text(F::IntUnit, "LUFS", f),
        ],
        Slot::HeroStatus => vec![Item::Status(F::IntStatus, WAITING_STATUS, big)],
        Slot::Live => vec![
            Item::Text(F::MomCaption, "Mom", f),
            Item::Value(F::Mom, big),
            Item::Text(F::MomUnit, "LUFS", f),
            Item::Text(F::LiveDot, "·", f),
            Item::Text(F::ShortCaption, "Short", f),
            Item::Value(F::Short, big),
            Item::Text(F::ShortUnit, "LUFS", f),
        ],
        Slot::Maxima => vec![
            Item::Text(F::MaxMomCaption, "max Mom", f),
            Item::Value(F::MaxMom, f),
            Item::Text(F::MaximaDot, "·", f),
            Item::Text(F::MaxShortCaption, "max Short", f),
            Item::Value(F::MaxShort, f),
        ],
        Slot::RangePeak => vec![
            Item::Text(F::LraCaption, "LRA", f),
            Item::Value(F::Lra, f),
            Item::Text(F::LraUnit, "LU", f),
            Item::Status(F::LraStatus, RANGE_SETTLING, f),
            Item::Text(F::RangeDot, "·", f),
            Item::Text(F::TpCaption, "TP max", f),
            Item::Value(F::Tp, f),
            Item::Text(F::TpUnit, "dBTP", f),
        ],
    }
}

/// The gap between a row's items: a space at the font.
fn gap(f: f32) -> f32 {
    text_width(" ", f)
}

fn row_width(items: &[Item], f: f32) -> f32 {
    let n = items.len();
    items.iter().map(Item::width).sum::<f32>() + n.saturating_sub(1) as f32 * gap(f)
}

/// The plate's rect with its top-left at `origin` and every cell, at the plot font `font`
/// and text scale `scale` with `dropped` steps of the drop order taken.
pub fn cells(origin: [f32; 2], font: f32, scale: f32, dropped: u8) -> (Rect, Vec<Cell>) {
    let m = Metrics::of(font, scale);
    let f = m.font;
    let shown: Vec<Slot> = Slot::ALL.into_iter().filter(|s| s.shown(dropped)).collect();
    let inner = shown
        .iter()
        .map(|&s| row_width(&items(s, f), f))
        .fold(0.0f32, f32::max);
    let x0 = origin[0] + m.pad_x;
    let mut y = origin[1] + m.pad_y;
    let mut out = Vec::new();
    for slot in shown {
        let size = slot.size() * f;
        // Every cell of a row sits on one baseline, the row's own size's.
        let base = y + 0.975 * size;
        match slot {
            Slot::Title | Slot::Foot => out.push(Cell {
                field: if slot == Slot::Title {
                    Field::Title
                } else {
                    Field::Foot
                },
                pos: [x0, base],
                align: HAlign::Left,
                size,
                width: inner,
            }),
            _ => {
                let mut x = x0;
                for it in items(slot, f) {
                    let w = it.width();
                    let (field, align, at, size) = match it {
                        Item::Text(field, _, s) | Item::Status(field, _, s) => {
                            (field, HAlign::Left, x, s)
                        }
                        Item::Value(field, s) => (field, HAlign::Right, x + w, s),
                    };
                    out.push(Cell {
                        field,
                        pos: [at, base],
                        align,
                        size,
                        width: w,
                    });
                    x += w + gap(f);
                }
            }
        }
        y += 1.3 * size;
    }
    let rect = Rect::new(
        origin[0],
        origin[1],
        inner + 2.0 * m.pad_x,
        y + m.pad_y - origin[1],
    );
    (rect, out)
}

/// The plate's size, `(w, h)`, at the plot font `font`, text scale `scale` and `dropped`
/// steps of the drop order: what the dock's fit tests use, so the scene and the drawing
/// agree.
pub fn size(font: f32, scale: f32, dropped: u8) -> (f32, f32) {
    let (r, _) = cells([0.0, 0.0], font, scale, dropped);
    (r.w, r.h)
}

/// What `field` reads on the plate of `r`; blank where the slot is empty this frame.
pub fn text(r: &LoudnessReadout, field: Field) -> String {
    use Field as F;
    match field {
        F::Title => r.title.clone(),
        F::IntCaption => "Int".into(),
        F::Int => value_text(r.integrated),
        F::IntUnit | F::MomUnit | F::ShortUnit => "LUFS".into(),
        F::IntStatus => match r.integrated {
            None => WAITING_STATUS.into(),
            Some(_) => String::new(),
        },
        F::MomCaption => "Mom".into(),
        F::Mom => value_text(Some(r.momentary)),
        F::LiveDot | F::MaximaDot | F::RangeDot => "·".into(),
        F::ShortCaption => "Short".into(),
        F::Short => value_text(Some(r.short_term)),
        F::MaxMomCaption => "max Mom".into(),
        F::MaxMom => value_text(Some(r.max_momentary)),
        F::MaxShortCaption => "max Short".into(),
        F::MaxShort => value_text(Some(r.max_short_term)),
        F::LraCaption => "LRA".into(),
        F::Lra => value_text(r.range),
        F::LraUnit => "LU".into(),
        F::LraStatus => match r.range {
            None => RANGE_WAITING.into(),
            Some(_) if r.duration_s < RANGE_SETTLED_S => RANGE_SETTLING.into(),
            Some(_) => String::new(),
        },
        F::TpCaption => "TP max".into(),
        F::Tp => value_text(Some(r.max_true_peak)),
        F::TpUnit => "dBTP".into(),
        F::Foot => r.since.clone(),
    }
}

/// Draws the plate of `r` at `rect` (from [`cells`]): its fill through
/// [`crate::legend::plate`], so it honours the theme's plate background and the hover
/// opacity as every plate does, its cells, and its grip.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    layer: &mut Layer,
    r: &LoudnessReadout,
    rect: Rect,
    cells: &[Cell],
    hover: Option<PlateHover>,
    clip: Rect,
    theme: &Theme,
) {
    crate::legend::plate(layer, rect, hover.is_some(), clip, theme);
    for c in cells {
        let t = text(r, c.field);
        if t.is_empty() {
            continue;
        }
        let t = match c.field {
            Field::Title | Field::Foot => cut_to(&t, c.width, c.size),
            _ => t,
        };
        let color = if c.field.bright() {
            theme.text
        } else {
            theme.text_dim
        };
        let mut l = label(t, c.pos, anchor(c.align, VAlign::Baseline), c.size, color);
        l.clip = Some(clip);
        if c.field.is_value() {
            l = l.tabular();
        }
        layer.labels.push(l);
    }
    crate::plate::grip(layer, rect, hover.is_some_and(|h| h.grip), clip, theme);
}
