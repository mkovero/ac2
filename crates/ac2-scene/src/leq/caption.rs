//! The caption above the windows: the meter, the unit, the run.

use super::*;

/// The caption's left part: the meter, and for columns the unit and the `weighting` their
/// names leave out; the measuring-position correction when the values carry one.
pub(super) fn caption_left(v: &LeqView<'_>, weighting: Option<&str>) -> String {
    if v.tiles.is_empty() {
        return v.meter.clone();
    }
    let unit = v.tiles.first().map_or("", |t| t.unit.as_str());
    let unit = match v.tiles.first().and_then(|t| t.corrected.as_deref()) {
        Some(c) => format!("{unit} {c}"),
        None => unit.to_owned(),
    };
    match weighting {
        Some(w) => format!("{} · {w}, {unit}", v.meter),
        None => format!("{} · {unit}", v.meter),
    }
}

/// Where the run goes in the caption.
pub(super) struct Caption {
    pub(super) height: f32,
    /// Text, position of its top centre (the type's real width is narrower than the
    /// estimate it is fitted with, so it is centred by the renderer), size.
    pub(super) run: Option<(String, [f32; 2], f32)>,
}

/// Lays the run into the caption, only with the history on: in large type between the meter and the calibration
/// when a wording down to the clock and the total fits there, else on a row of its own
/// below them in the longest wording that fits (at the caption's type size when nothing
/// fits large); left out only when not even the clock fits. Large type is a share of the
/// pane's height, so the run reads from a distance on a full-screen pane.
pub(super) fn caption(
    v: &LeqView<'_>,
    left: &str,
    right: &str,
    top: f32,
    size: Viewport,
    theme: &Theme,
) -> Caption {
    let pad = 10.0;
    let gap = theme.font_size;
    let base_h = theme.font_size * 1.6;
    // The run belongs to the look back (Shift+B): without the history the windows and the
    // number are the whole picture, and the row it took is theirs.
    let Some(run) = v.run.as_ref().filter(|_| v.layout.history) else {
        return Caption {
            height: base_h,
            run: None,
        };
    };
    let big = (size.height * 0.04).clamp(theme.font_size, theme.font_size * 2.0);
    let variants = run.variants();
    let left_end = pad + canvas::text_width(left, theme.font_size);
    let right_start = size.width - pad - canvas::text_width(right, theme.font_size);
    // Centred on the pane, the way the meter's number under it is; pushed aside only as far
    // as the meter's name and the calibration need.
    let centred =
        |w: f32, lo: f32, hi: f32| (size.width / 2.0 - w / 2.0).clamp(lo, (hi - w).max(lo));
    // On the meter's row: down to the clock and the total, never the bare clock alone, and
    // only exactly centred, lined up with the meter's number under it; a shorter wording
    // centred beats a longer one pushed aside. When nothing fits centred there, the run gets
    // a row of its own below, centred too.
    let inline = variants.len().saturating_sub(1).max(1);
    let half_room = (size.width / 2.0 - left_end - gap).min(right_start - gap - size.width / 2.0);
    if let Some(t) = variants
        .iter()
        .take(inline)
        .find(|t| canvas::text_width(t, big) <= 2.0 * half_room)
    {
        let w = canvas::text_width(t, big);
        let x = centred(w, left_end + gap, right_start - gap) + w / 2.0;
        return Caption {
            height: (big * 1.35).max(base_h),
            run: Some((t.clone(), [x, top], big)),
        };
    }
    let row_y = top + base_h;
    let width = size.width - 2.0 * pad;
    for s in [big, theme.font_size] {
        if let Some(t) = variants.iter().find(|t| canvas::text_width(t, s) <= width) {
            return Caption {
                height: base_h + (s * 1.35).max(base_h),
                run: Some((
                    t.clone(),
                    [
                        centred(canvas::text_width(t, s), pad, size.width - pad)
                            + canvas::text_width(t, s) / 2.0,
                        row_y,
                    ],
                    s,
                )),
            };
        }
    }
    Caption {
        height: base_h,
        run: None,
    }
}
