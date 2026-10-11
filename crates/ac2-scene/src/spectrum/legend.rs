//! The caption's shorter forms, the axis unit for the curves shown, and the legend laid
//! out in rows under the plot.

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

/// Legend row pitch.
pub(super) const LEGEND_ROW: f32 = 16.0;
/// Colour swatch of a legend entry and the gap after it.
pub(super) const SWATCH_W: f32 = 12.0;
pub(super) const SWATCH_GAP: f32 = 6.0;
/// Gap between two entries on a row.
const ENTRY_GAP: f32 = 16.0;
/// Most of the pane height the legend may take: the curves are what the pane is for.
pub(super) const LEGEND_SHARE: f32 = 0.25;
pub(super) const LEGEND_MAX_ROWS: usize = 4;

/// Legend line of one curve: its name and what sets it apart — `stopped`, `STALE 3.2 s`,
/// `offset +3.0 dB` (a spread is never read as a level difference).
pub(super) fn legend_entry(t: &SpectrumTrace<'_>) -> LegendEntry {
    let mut tags = Vec::new();
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

/// Width an entry takes on a row: swatch, gap, text.
pub(super) fn entry_width(text: &str, font: f32) -> f32 {
    SWATCH_W + SWATCH_GAP + text_width(text, font)
}

/// Flows `texts` into rows `width` wide; each item's row and x offset. An item wider than
/// a row is cut to fit it.
fn flow(texts: &[String], width: f32, font: f32) -> Vec<(usize, f32, String)> {
    let mut out = Vec::with_capacity(texts.len());
    let (mut row, mut x) = (0usize, 0.0f32);
    for t in texts {
        let t = cut_to(t, width - SWATCH_W - SWATCH_GAP, font);
        let w = entry_width(&t, font);
        if x > 0.0 && x + w > width {
            row += 1;
            x = 0.0;
        }
        out.push((row, x, t));
        x += w + ENTRY_GAP;
    }
    out
}

/// The legend laid out in at most `max_rows` rows `width` wide: every entry in full when
/// that fits, else names only, else the names that fit and `+N more` closing the last row.
/// Items are (entry index or `None` for the `+N more` note, row, x, text).
pub(super) fn legend_layout(
    entries: &[LegendEntry],
    width: f32,
    max_rows: usize,
    font: f32,
) -> Vec<(Option<usize>, usize, f32, String)> {
    if entries.is_empty() || max_rows == 0 {
        return Vec::new();
    }
    let rows = |f: &[(usize, f32, String)]| f.last().map_or(0, |l| l.0 + 1);
    let indexed = |f: Vec<(usize, f32, String)>| {
        f.into_iter()
            .enumerate()
            .map(|(i, (r, x, t))| (Some(i), r, x, t))
            .collect()
    };
    let full: Vec<String> = entries.iter().map(|e| e.text.clone()).collect();
    let f = flow(&full, width, font);
    if rows(&f) <= max_rows {
        return indexed(f);
    }
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let f = flow(&names, width, font);
    if rows(&f) <= max_rows {
        return indexed(f);
    }
    let mut kept: Vec<(usize, f32, String)> = f
        .into_iter()
        .take_while(|(r, _, _)| *r < max_rows)
        .collect();
    // Room on the last row for the note of what is left out.
    loop {
        let more = format!("+{} more", entries.len() - kept.len());
        let end = kept
            .last()
            .map_or(0.0, |(_, x, t)| x + entry_width(t, font) + ENTRY_GAP);
        let last_row = kept.last().map_or(0, |k| k.0);
        if kept.is_empty() || end + text_width(&more, font) <= width {
            let mut out: Vec<_> = indexed(kept);
            out.push((None, last_row, end, more));
            return out;
        }
        kept.pop();
    }
}
