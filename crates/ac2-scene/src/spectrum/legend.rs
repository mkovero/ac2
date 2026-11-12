//! The caption's shorter forms, the axis unit for the curves shown, and the legend on its
//! plate.

use super::*;

/// A caption from its longest form to its shortest, for a pane too narrow for the full
/// one: without the parenthesised details (`(data sheet 15.0 mV/Pa)`), then without its
/// first part (the window or band, which the pane's title and the unit also say), then its
/// parts from the end off, last nothing. The calibration stays longest: it says whether
/// the levels are dB SPL to be trusted.
pub fn caption_forms(caption: &str) -> Vec<String> {
    let mut v = vec![caption.to_string()];
    if caption.is_empty() {
        return v;
    }
    let mut bare = String::new();
    let mut depth = 0usize;
    for ch in caption.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => bare.push(ch),
            _ => {}
        }
    }
    let parts: Vec<String> = bare
        .split('·')
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect();
    v.push(parts.join(" · "));
    let rest = parts.get(1..).unwrap_or_default();
    for n in (1..=rest.len()).rev() {
        v.push(rest[..n].join(" · "));
    }
    v.push(String::new());
    v.dedup();
    v
}

/// The axis unit's forms ([`level_unit_forms`]) for the curves shown, and its help for a
/// narrowband spectrum. Curves that differ only in their bin spacing share `per bin, mixed
/// widths`; any other difference is `mixed units`.
pub(super) fn unit_forms(traces: &[SpectrumTrace<'_>]) -> (Vec<String>, Option<String>) {
    let Some(first) = traces.first() else {
        return (vec![String::new()], None);
    };
    if traces
        .iter()
        .any(|t| t.scale != first.scale || t.quantity != first.quantity)
    {
        return (vec!["mixed units".to_string()], None);
    }
    let bin = if traces.iter().all(|t| t.bands.is_some()) {
        BinWidth::MultiRes
    } else if traces
        .iter()
        .all(|t| t.bin_hz == first.bin_hz && t.bands.is_none())
    {
        BinWidth::of(first.bin_hz)
    } else {
        BinWidth::Mixed
    };
    let help = first
        .quantity
        .is_narrow()
        .then(|| narrow_unit_help(first.quantity, bin));
    (level_unit_forms(first.scale, first.quantity, bin), help)
}

/// Legend line of one curve: its name and what sets it apart — `cmp`, `stopped`,
/// `STALE 3.2 s`, `offset +3.0 dB` (a spread is never read as a level difference).
pub(super) fn legend_entry(t: &SpectrumTrace<'_>) -> LegendEntry {
    // A compared curve is not of the pane's group: said first, as the transfer legend does.
    let mut tags: Vec<String> = t
        .compared
        .then(|| crate::trace::COMPARED.to_owned())
        .into_iter()
        .collect();
    if t.offset_db != 0.0 {
        tags.push(format!("offset {}", format::db_readout(t.offset_db)));
    }
    if let Some(tag) = t.freshness.and_then(|f| f.tag()) {
        tags.push(tag);
    }
    let mut text = t.name.clone();
    for tag in &tags {
        text.push_str(" · ");
        text.push_str(tag);
    }
    LegendEntry {
        key: t.key,
        name: t.name.clone(),
        tags,
        text,
        stale: t.is_stale(),
        selected: t.selected,
    }
}

/// The legend plate of `traces` ([`crate::legend::legend_plate`]) in `area`, clipped to
/// `clip`: the spectrum's, and the spectrographs' over the one curve they draw, so a row
/// reads and is moved the same in every view, the transfer view's too. With it, one entry
/// per curve and where each shown row is.
pub(crate) fn legend_of(
    layer: &mut crate::primitives::Layer,
    traces: &[SpectrumTrace<'_>],
    view: &ViewState,
    area: Rect,
    clip: Rect,
    theme: &Theme,
) -> SpectrumLegend {
    let entries: Vec<LegendEntry> = traces.iter().map(legend_entry).collect();
    let styles: Vec<crate::legend::RowStyle> = traces
        .iter()
        .zip(&entries)
        .map(|(t, e)| crate::legend::RowStyle {
            color: t
                .color
                .with_alpha(if e.stale { theme.stale_alpha } else { 1.0 }),
            dim: e.stale,
            selected: e.selected,
        })
        .collect();
    let (b, hits) =
        crate::legend::legend_plate(layer, &entries, &styles, &[], view, area, clip, theme);
    SpectrumLegend {
        entries,
        shown: b
            .as_ref()
            .map_or_else(Vec::new, |b| b.rows.iter().map(|r| r.0.clone()).collect()),
        b,
        hits,
    }
}

/// A spectrum view's legend as drawn ([`legend_of`]).
pub(crate) struct SpectrumLegend {
    /// One entry per curve, in drawing order.
    pub entries: Vec<LegendEntry>,
    /// The shown rows' texts as drawn (cut to the plate's width).
    pub shown: Vec<String>,
    /// The plate; `None` when hidden or empty.
    pub b: Option<crate::legend::LegendBox>,
    /// Where each shown row is, for a click on it.
    pub hits: Vec<crate::legend::LegendHit>,
}
