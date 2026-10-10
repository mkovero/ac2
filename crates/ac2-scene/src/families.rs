//! Colour families: which colour every measurement curve is drawn in.
//!
//! Each measurement owns a hue ([`Theme::families`]); everything it owns is drawn in that
//! hue, so the eye groups a measurement's curves the way the tree does. Its live curve has
//! the family's base colour; its stored traces (captures, sweep runs) and then its math
//! channels' results take the family's shades in tree order. A shade keeps the base's
//! OKLCH hue and chroma and moves only its lightness, across the theme's whole shade range:
//! each next shade is the one farthest from the curve before it, kept clear of the base and
//! the other shades, so the first capture sits at the far end of the range from the live
//! curve, the second at the other end, then back and forth: curves that follow each other
//! in the tree are never one small step apart. A
//! measurement without a live curve (a sweep) gives its first run the base colour.
//!
//! Traces under no measurement (imports, those of a deleted measurement kept) and math
//! channels listed with them belong to no one family: each takes a colour of its own, so
//! imported curves overlaid on a pane stay as distinct from each other and from the
//! measurements as the palette allows. They walk the import palette ([`import_palette`])
//! in tree order: first the bases of families no measurement holds (whole new hues), then
//! those free families' shades, then the shades of the measurements' families (the last
//! shade first, since a measurement's own traces take them from the first), and the
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
//! A family the operator chose for a measurement (`Measurement::colour`, kept by the
//! daemon) comes first: those families are taken before the others are walked, and a
//! measurement may choose the neutral grey. Stepping a measurement's colour
//! ([`step_colour`]) skips families another measurement holds.
//! Shades follow the tree's order within the measurement (slot, then oldest first, then
//! math channels by id); after the last shade they repeat.
//!
//! Every pane, legend, cursor readout and tree dot takes its colour from
//! [`CurveColours`], so a curve reads the same everywhere. The daemon's per-trace colour
//! (`TraceEdit::color`) is not used for drawing.

use std::collections::BTreeMap;

use ac2_proto::model::{ColourFamily, MeasKind, Measurement, TraceMeta, TraceOwner};
use ac2_proto::units::{MeasId, TraceId};

use crate::meas_list::{group_of, group_order, has_live_curve, math_group, trace_order};
use crate::primitives::Color;
use crate::theme::{FAMILIES, Theme, contrast_ratio};

/// OKLab distance below which two thin lines of one hue no longer read as different curves
/// side by side: no shade is closer than this to the base or another shade.
const SHADE_STEP: f64 = 0.10;
/// OKLab distance between curves that follow each other in a measurement's sequence (live,
/// then its traces, the shades starting over) a shade past the [`MIN_SHADES`]th must keep:
/// well past [`SHADE_STEP`], so a capture is told from the one before it at a glance.
pub const CONSECUTIVE_STEP: f64 = 0.15;
/// Shades every family offers (each at least [`SHADE_STEP`] from the others): a live curve,
/// two captures and a math result of one measurement all differ. Where the lightness range
/// is narrow (a light theme's top is cut by contrast against the plot), the third may sit
/// nearer its neighbours than [`CONSECUTIVE_STEP`].
const MIN_SHADES: usize = 3;
/// Lightness spacing of the candidate shades the family picks from: fine enough that the
/// picks land near the ends of the range and in the middle of the gaps.
const CANDIDATE_STEP: f64 = 0.01;
/// Shades a family offers before they repeat: two on each side of the base, alternating.
/// More would crowd the lightness range and sit too close to each other to tell apart.
const MAX_SHADES: usize = 4;
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
    /// The family of `base` in `theme`: shades of its hue and chroma anywhere in the
    /// theme's shade lightness range, at 3:1 contrast against the plot (WCAG non-text
    /// contrast) and still showing the hue, in the order curves take them: each next shade
    /// is the candidate farthest (OKLab) from both its neighbours in the sequence (the curve
    /// before it, and the first shade, which follows it when the shades start over) that
    /// keeps [`SHADE_STEP`] from the base and every shade already taken; past
    /// [`MIN_SHADES`], none nearer than [`CONSECUTIVE_STEP`] to a neighbour is taken. The first
    /// trace after the live curve so takes the far end of the range, the next the other
    /// end, then back and forth between the two sides; consecutive curves of one
    /// measurement never sit a small step apart, also where the shades start over.
    pub fn of(theme: &Theme, base: Color) -> Self {
        let (l, c, h) = oklch(base);
        let (lo, hi) = theme.shade_lightness;
        let fits = |s: &Color| {
            contrast_ratio(*s, theme.plot_background) >= 3.0
                && (c <= GREY_CHROMA || oklch(*s).1 >= MIN_CHROMA_KEPT * c)
        };
        let n = ((hi - lo) / CANDIDATE_STEP).round().max(1.0) as usize;
        let candidates: Vec<Color> = (0..=n)
            .map(|i| quantize(from_oklch(lo + (hi - lo) * i as f64 / n as f64, c, h)))
            .filter(fits)
            .collect();
        let mut shades = sequence(base, &candidates, SHADE_STEP);
        if shades.len() < MIN_SHADES {
            // A narrow range the far ends crowd: whole steps from the base instead, which
            // always leave room for three, in the same far-from-the-neighbours order.
            let steps: Vec<Color> = (1..=4)
                .flat_map(|k| [1.0, -1.0].map(|sign| l + sign * f64::from(k) * SHADE_STEP))
                .filter(|t| (lo..=hi).contains(t))
                .map(|t| quantize(from_oklch(t, c, h)))
                .filter(fits)
                .collect();
            shades = sequence(base, &steps, 0.0);
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

/// Shades from `candidates` in the order curves take them after `base`: each the candidate
/// farthest from both its neighbours (the curve before it, and the first shade, which
/// follows it when the shades start over) among those at least `clear` from every colour
/// taken. Past [`MIN_SHADES`] a shade must keep [`CONSECUTIVE_STEP`] from its neighbours.
fn sequence(base: Color, candidates: &[Color], clear: f64) -> Vec<Color> {
    let mut shades: Vec<Color> = Vec::new();
    while shades.len() < MAX_SHADES {
        let prev = shades.last().copied().unwrap_or(base);
        let first = shades.first().copied().unwrap_or(base);
        let free = |s: &&Color| {
            std::iter::once(&base)
                .chain(&shades)
                .all(|t| *t != **s && oklab_dist(**s, *t) >= clear)
        };
        let best = candidates
            .iter()
            .filter(free)
            .map(|s| (*s, oklab_dist(*s, prev).min(oklab_dist(*s, first))))
            .fold(None, |acc: Option<(Color, f64)>, (s, d)| match acc {
                Some((_, bd)) if bd >= d => acc,
                _ => Some((s, d)),
            });
        match best {
            Some((s, d)) if d >= CONSECUTIVE_STEP || shades.len() < MIN_SHADES => {
                shades.push(s);
            }
            _ => break,
        }
    }
    shades
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

/// The family of every measurement that is not a math channel, by id (see the module docs
/// for the rule).
pub fn family_indices(meas: &[&Measurement]) -> BTreeMap<MeasId, ColourFamily> {
    let n = FAMILIES;
    let mut auto: Vec<MeasId> = Vec::new();
    let mut taken = vec![false; n];
    let mut out = BTreeMap::new();
    for m in meas
        .iter()
        .filter(|m| !matches!(m.config.kind, MeasKind::Math { .. }))
    {
        match m.colour.filter(|c| c.is_valid()) {
            Some(c) => {
                if let ColourFamily::Hue { index } = c {
                    taken[usize::from(index)] = true;
                }
                out.insert(m.id, c);
            }
            None => auto.push(m.id),
        }
    }
    auto.sort();
    for id in auto {
        let preferred = (id.0 as usize).wrapping_sub(1) % n;
        let i = (0..n)
            .map(|k| (preferred + k) % n)
            .find(|i| !taken[*i])
            .unwrap_or(preferred);
        taken[i] = true;
        out.insert(id, hue(i));
    }
    out
}

/// Hue `i` (below [`FAMILIES`]).
fn hue(i: usize) -> ColourFamily {
    ColourFamily::Hue {
        index: u8::try_from(i).unwrap_or(0),
    }
}

/// The family measurement `id` steps to from its current one, forwards or backwards through
/// the hues and then neutral grey (wrapping), skipping those another measurement holds;
/// `None` when every other is held, or `id` is a math channel or not listed.
pub fn step_colour(meas: &[&Measurement], id: MeasId, forward: bool) -> Option<ColourFamily> {
    let fams = family_indices(meas);
    let current = *fams.get(&id)?;
    let cycle: Vec<ColourFamily> = (0..FAMILIES)
        .map(hue)
        .chain([ColourFamily::Neutral])
        .collect();
    let at = cycle.iter().position(|c| *c == current)?;
    let held = |c: &ColourFamily| fams.iter().any(|(m, f)| *m != id && f == c);
    let len = cycle.len();
    (1..len)
        .map(|k| {
            if forward {
                cycle[(at + k) % len]
            } else {
                cycle[(at + len - k) % len]
            }
        })
        .find(|c| !held(c))
}

/// The base colour of family `c` in `theme`.
pub fn family_base(theme: &Theme, c: ColourFamily) -> Color {
    match c {
        ColourFamily::Hue { index } => theme
            .families
            .get(usize::from(index))
            .copied()
            .unwrap_or(theme.neutral),
        ColourFamily::Neutral => theme.neutral,
    }
}

/// The colours imported traces take in turn, given the measurements' families (see the
/// module docs for the order). Never empty: the neutral base ends it.
pub fn import_palette(theme: &Theme, hues: &BTreeMap<MeasId, ColourFamily>) -> Vec<Color> {
    let mut taken = [false; FAMILIES];
    for c in hues.values() {
        if let ColourFamily::Hue { index } = c
            && let Some(t) = taken.get_mut(usize::from(*index))
        {
            *t = true;
        }
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
                TraceOwner::Meas { meas: id } => hues
                    .get(&id)
                    .map_or(theme.neutral, |c| family_base(theme, *c)),
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

/// Euclidean distance in OKLab between two display-encoded colours.
fn oklab_dist(a: Color, b: Color) -> f64 {
    let (a, b) = (oklab(a), oklab(b));
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
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
