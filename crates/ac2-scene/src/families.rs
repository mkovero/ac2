//! Colour families: which colour every measurement curve is drawn in.
//!
//! Each measurement owns a hue ([`Theme::families`]); everything it owns is drawn in that
//! hue, so the eye groups a measurement's curves the way the tree does. Its live curve has
//! the family's base colour; its stored traces (captures, sweep runs) and then its math
//! channels' results take the family's shades in tree order. A shade keeps the base's
//! OKLCH hue and chroma and moves only its lightness, in steps big enough to tell apart
//! side by side, alternating lighter and darker so the first shades are the nearest
//! distinct ones. A measurement without a live curve (a sweep) gives its first run the
//! base colour.
//!
//! Traces under no measurement (imports, those of a deleted measurement kept) and math
//! channels listed with them belong to no one family: each takes a colour of its own, so
//! imported curves overlaid on a pane stay as distinct from each other and from the
//! measurements as the palette allows. They walk the import palette ([`import_palette`])
//! in tree order: first the bases of families no measurement holds (whole new hues), then
//! those free families' shades, then the shades of the measurements' families (farthest
//! shade first, since a measurement's own traces take the nearest ones first), and the
//! neutral grey family only last; past its end the palette repeats. The colour goes by
//! position in the imported group, so deleting one import shifts those listed after it one
//! step along the palette; the colours keep running down the list in the same order, and
//! no stored per-trace colour is needed. Adding or deleting a measurement changes which
//! families are free and so repaints the imports.
//!
//! Hue assignment follows the measurement id, not list position, so deleting one
//! measurement does not repaint the others: measurement `id` prefers family
//! `(id − 1) mod 8`; walking the measurements by id, one whose preferred family is taken
//! moves on to the next free one, and only past eight measurements do families repeat.
//! Shades follow the tree's order within the measurement (slot, then oldest first, then
//! math channels by id); after the last shade they repeat.
//!
//! Every pane, legend, cursor readout and tree dot takes its colour from
//! [`CurveColours`], so a curve reads the same everywhere. The daemon's per-trace colour
//! (`TraceEdit::color`) is not used for drawing.

use std::collections::BTreeMap;

use ac2_proto::model::{MeasKind, Measurement, TraceMeta, TraceOwner};
use ac2_proto::units::{MeasId, TraceId};

use crate::meas_list::{group_of, group_order, has_live_curve, math_group, trace_order};
use crate::primitives::Color;
use crate::theme::{FAMILIES, Theme, contrast_ratio};

/// OKLab lightness between neighbouring shades: about the smallest step two thin lines of
/// one hue still read as different curves side by side.
const SHADE_STEP: f64 = 0.10;
/// Shades a family offers before they repeat. More would crowd the lightness range and
/// sit too close to each other to tell apart.
const MAX_SHADES: usize = 3;
/// A shade whose chroma, after fitting it into sRGB, falls below this share of the base's
/// has lost its hue (washed to white or black) and is skipped.
const MIN_CHROMA_KEPT: f64 = 0.3;
/// Below this chroma a colour is grey: the neutral family, whose shades have no hue to
/// keep.
const GREY_CHROMA: f64 = 0.02;

/// A colour family: the base and its shades, in assignment order.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub base: Color,
    pub shades: Vec<Color>,
}

impl Family {
    /// The family of `base` in `theme`: shades one, two, three steps lighter and darker in
    /// OKLab lightness (nearest first, lighter before darker), kept only inside the
    /// theme's shade range, at 3:1 contrast against the plot (WCAG non-text contrast) and
    /// still showing the hue.
    pub fn of(theme: &Theme, base: Color) -> Self {
        let (l, c, h) = oklch(base);
        let (lo, hi) = theme.shade_lightness;
        let mut shades = Vec::new();
        'steps: for k in 1..=4 {
            for sign in [1.0, -1.0] {
                let target = l + sign * f64::from(k) * SHADE_STEP;
                if !(lo..=hi).contains(&target) {
                    continue;
                }
                let s = quantize(from_oklch(target, c, h));
                if contrast_ratio(s, theme.plot_background) < 3.0 {
                    continue;
                }
                if c > GREY_CHROMA && oklch(s).1 < MIN_CHROMA_KEPT * c {
                    continue;
                }
                shades.push(s);
                if shades.len() == MAX_SHADES {
                    break 'steps;
                }
            }
        }
        Self { base, shades }
    }

    /// The colour of the `k`-th curve after the live one (the shades, repeating).
    fn member(&self, k: usize) -> Color {
        if self.shades.is_empty() {
            self.base
        } else {
            self.shades[k % self.shades.len()]
        }
    }
}

/// The colour of every curve: live curves and math results by measurement, stored traces
/// by id.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveColours {
    meas: BTreeMap<MeasId, Color>,
    traces: BTreeMap<TraceId, Color>,
    /// For a curve not (yet) listed: the neutral base.
    fallback: Color,
}

impl CurveColours {
    /// Measurement `id`'s live curve (or a math channel's result).
    pub fn meas(&self, id: MeasId) -> Color {
        self.meas.get(&id).copied().unwrap_or(self.fallback)
    }

    /// Stored trace `id`.
    pub fn trace(&self, id: TraceId) -> Color {
        self.traces.get(&id).copied().unwrap_or(self.fallback)
    }
}

/// Family index of every measurement that is not a math channel, by id (see the module
/// docs for the rule).
pub fn family_indices(meas: &[&Measurement]) -> BTreeMap<MeasId, usize> {
    let n = FAMILIES;
    let mut ids: Vec<MeasId> = meas
        .iter()
        .filter(|m| !matches!(m.config.kind, MeasKind::Math { .. }))
        .map(|m| m.id)
        .collect();
    ids.sort();
    let mut taken = vec![false; n];
    let mut out = BTreeMap::new();
    for id in ids {
        let preferred = (id.0 as usize).wrapping_sub(1) % n;
        let i = (0..n)
            .map(|k| (preferred + k) % n)
            .find(|i| !taken[*i])
            .unwrap_or(preferred);
        taken[i] = true;
        out.insert(id, i);
    }
    out
}

/// The colours imported traces take in turn, given the measurements' family indices
/// (see the module docs for the order). Never empty: the neutral base ends it.
pub fn import_palette(theme: &Theme, hues: &BTreeMap<MeasId, usize>) -> Vec<Color> {
    let mut taken = [false; FAMILIES];
    for i in hues.values() {
        taken[*i] = true;
    }
    let fams: Vec<Family> = theme
        .families
        .iter()
        .map(|b| Family::of(theme, *b))
        .collect();
    let (free, held): (Vec<usize>, Vec<usize>) = (0..FAMILIES).partition(|i| !taken[*i]);
    let mut out: Vec<Color> = free.iter().map(|i| fams[*i].base).collect();
    for j in 0..MAX_SHADES {
        out.extend(free.iter().filter_map(|i| fams[*i].shades.get(j)));
    }
    for j in (0..MAX_SHADES).rev() {
        out.extend(held.iter().filter_map(|i| fams[*i].shades.get(j)));
    }
    let neutral = Family::of(theme, theme.neutral);
    out.push(neutral.base);
    out.extend(neutral.shades);
    out
}

/// Every curve's colour for `meas` (math channels included) and `traces` in `theme`.
pub fn curve_colours(theme: &Theme, meas: &[&Measurement], traces: &[&TraceMeta]) -> CurveColours {
    let hues = family_indices(meas);
    let mut out = CurveColours {
        meas: BTreeMap::new(),
        traces: BTreeMap::new(),
        fallback: theme.neutral,
    };
    for g in group_order(meas) {
        let owner = g.meas().and_then(|id| meas.iter().find(|m| m.id == id));
        let family = Family::of(
            theme,
            match g {
                TraceOwner::Meas { meas: id } => {
                    hues.get(&id).map_or(theme.neutral, |i| theme.families[*i])
                }
                TraceOwner::Imported => theme.neutral,
            },
        );
        let imports = matches!(g, TraceOwner::Imported).then(|| import_palette(theme, &hues));
        // Without a live curve the base colour would go unused: the first member takes it.
        let base_free = owner.is_none_or(|m| !has_live_curve(&m.config.kind));
        if let Some(m) = owner.filter(|m| has_live_curve(&m.config.kind)) {
            out.meas.insert(m.id, family.base);
        }
        let colour = |k: usize| {
            if let Some(p) = &imports {
                p[k % p.len()]
            } else if base_free {
                if k == 0 {
                    family.base
                } else {
                    family.member(k - 1)
                }
            } else {
                family.member(k)
            }
        };
        let owned: Vec<&TraceMeta> = traces
            .iter()
            .copied()
            .filter(|t| group_of(t, meas) == g)
            .collect();
        let mut k = 0;
        for t in trace_order(meas, &owned) {
            out.traces.insert(t.id, colour(k));
            k += 1;
        }
        let mut maths: Vec<MeasId> = meas
            .iter()
            .filter(|m| math_group(m, meas) == Some(g))
            .map(|m| m.id)
            .collect();
        maths.sort();
        for id in maths {
            out.meas.insert(id, colour(k));
            k += 1;
        }
    }
    out
}

/// OKLab lightness, chroma and hue (radians) of a display-encoded colour.
pub fn oklch(c: Color) -> (f64, f64, f64) {
    let (l, a, b) = oklab(c);
    (l, a.hypot(b), b.atan2(a))
}

/// OKLab of a display-encoded colour (Björn Ottosson's matrices): a space where equal
/// distances look about equally different, which is what shade steps are measured in.
pub fn oklab(c: Color) -> (f64, f64, f64) {
    let (r, g, b) = (
        to_linear(f64::from(c.r)),
        to_linear(f64::from(c.g)),
        to_linear(f64::from(c.b)),
    );
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    (
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    )
}

fn oklab_to_linear(l: f64, a: f64, b: f64) -> [f64; 3] {
    let lp = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let mp = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let sp = (l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    [
        4.076_741_662_1 * lp - 3.307_711_591_3 * mp + 0.230_969_929_2 * sp,
        -1.268_438_004_6 * lp + 2.609_757_401_1 * mp - 0.341_319_396_5 * sp,
        -0.004_196_086_3 * lp - 0.703_418_614_7 * mp + 1.707_614_701_0 * sp,
    ]
}

/// The colour at OKLCH (`l`, `c`, `h`), its chroma reduced as far as needed to fit sRGB:
/// hue and lightness are what make it a shade of its family, so chroma gives way.
fn from_oklch(l: f64, c: f64, h: f64) -> Color {
    let rgb = |c: f64| oklab_to_linear(l, c * h.cos(), c * h.sin());
    let fits = |v: [f64; 3]| v.iter().all(|x| (-1e-9..=1.0 + 1e-9).contains(x));
    let mut chroma = c;
    if !fits(rgb(c)) {
        let (mut lo, mut hi) = (0.0, c);
        for _ in 0..32 {
            let mid = 0.5 * (lo + hi);
            if fits(rgb(mid)) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        chroma = lo;
    }
    let [r, g, b] = rgb(chroma).map(|v| to_encoded(v.clamp(0.0, 1.0)) as f32);
    Color::rgb(r, g, b)
}

/// Rounded to 8 bits, so a curve and its tree dot compare equal however each is stored.
fn quantize(c: Color) -> Color {
    Color::from_rgba8(c.to_rgba8())
}

fn to_linear(v: f64) -> f64 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn to_encoded(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests;
