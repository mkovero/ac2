//! Summaries of a preset, its windows and limits, a transfer, and the predicted level:
//! the words the band settings and the predicted line show.

use super::*;

/// The predicted level at the transfer's place.
pub fn predicted_text(p: &PredictedLeq, place: &str) -> PredictedText {
    let mut parts = vec![format!(
        "predicted LAeq {} in {place} {}",
        length(p.duration.0),
        with_db(p.estimate)
    )];
    // The unusable bands at their bound can only raise it: shown when they do.
    if p.at_most.is_finite() && (!p.estimate.is_finite() || p.at_most - p.estimate >= 0.05) {
        parts.push(format!("at most {} dB", format::level(p.at_most)));
    }
    match p.limit {
        Some(l) => parts.push(format!("limit {} dB", format::level(l.0))),
        None => parts.push("no limit now".to_owned()),
    }
    let state = TileState::from(p.judgement);
    if let Some(s) = state_text(state, false) {
        parts.push(s);
    }
    PredictedText {
        line: parts.join(" · "),
        state,
    }
}

fn with_db(v: f64) -> String {
    if v.is_finite() {
        format!("{} dB", format::level(v))
    } else {
        format::NO_VALUE.to_owned()
    }
}

/// What a band preset sets, as the dialog and the CLI show it.
pub fn preset_summary(p: BandLeqPreset) -> String {
    let c = p.apply(None);
    let mut parts = windows_summary(&c.windows);
    if let Some(pr) = c.predicted {
        let mut pred = Vec::new();
        if let Some(d) = pr.day {
            pred.push(format!("day ≤ {} dB", d.0));
        }
        if let Some(n) = pr.night {
            pred.push(format!("night ≤ {} dB", n.0));
        }
        if !pred.is_empty() {
            parts.push(format!(
                "predicted LAeq {} {}",
                length(pr.duration.0),
                pred.join(", ")
            ));
        }
    }
    format!("{}: {}", p.name(), parts.join("; "))
}

/// Band windows in words, a part per row of the view: `20–200 Hz LZeq 60 min, night 74 …
/// 32 dB, day 5 dB higher`.
pub fn windows_summary(windows: &[BandWindow]) -> Vec<String> {
    let keys: Vec<(f64, _)> = windows
        .iter()
        .map(|w| {
            (
                w.band.0,
                (
                    w.duration.0.to_bits(),
                    w.weighting,
                    w.day_offset.map(|o| o.0.to_bits()),
                ),
            )
        })
        .collect();
    group_rows(&keys)
        .iter()
        .map(|ws| {
            let first = &windows[ws[0]];
            let bands: Vec<usize> = ws.iter().filter_map(|&w| windows[w].index()).collect();
            let limits: Vec<f64> = ws
                .iter()
                .filter_map(|&w| windows[w].limit.map(|l| l.0))
                .collect();
            format!(
                "{} {}, {}",
                bands_text(&bands),
                window_name(first.duration.0, first.weighting),
                limits_summary(&limits, first.day_offset.map(|o| o.0))
            )
        })
        .collect()
}

/// Limits low band to high in words: `night 74 … 32 dB, day 5 dB higher`, `70 … 42 dB`,
/// `no limits`; the first and the last.
pub fn limits_summary(limits: &[f64], day_offset: Option<f64>) -> String {
    match (limits.first(), limits.last(), day_offset) {
        (Some(a), Some(b), Some(o)) if a == b => format!("night {a} dB, day {o} dB higher"),
        (Some(a), Some(b), Some(o)) => format!("night {a} … {b} dB, day {o} dB higher"),
        (Some(a), Some(b), None) if a == b => format!("{a} dB"),
        (Some(a), Some(b), None) => format!("{a} … {b} dB"),
        _ => "no limits".to_owned(),
    }
}

/// Where a band preset's figures come from, with the caveats it carries.
pub fn preset_source(p: BandLeqPreset) -> String {
    format!(
        "{} — informational, not legal advice; a prediction from FOH is not a measurement \
         where the limits apply",
        p.source()
    )
}

/// One band of a transfer: `63 Hz 18.2 dB`, `40 Hz ≥ 35.0 dB (under the background)`.
pub fn transfer_band_text(nominal_hz: f64, b: &BandTransferBand) -> String {
    let hz = format!("{} Hz", band_label(nominal_hz));
    match *b {
        BandTransferBand::Unchecked { attenuation } => {
            format!("{hz} {} dB (no background)", format::level(attenuation.0))
        }
        BandTransferBand::Clean { attenuation } => {
            format!("{hz} {} dB", format::level(attenuation.0))
        }
        BandTransferBand::Corrected {
            attenuation,
            margin,
        } => format!(
            "{hz} {} dB (background subtracted, {} dB over it)",
            format::level(attenuation.0),
            format::level(margin.0)
        ),
        BandTransferBand::Unusable { at_least } => format!(
            "{hz} ≥ {} dB (under the background: a bound)",
            format::level(at_least.0)
        ),
        BandTransferBand::Missing => format!("{hz} not measured"),
    }
}

/// The transfer of the shown bands (indices, low to high) in a line: `transfer from flat 4
/// bedroom, 20–200 Hz: 8 clean, 1 corrected, 1 bound, 1 not measured`; `no band transfer:
/// limits judged at the mic as typed`.
pub fn transfer_summary(t: Option<&BandTransferSet>, shown: &[usize]) -> String {
    let Some(t) = t else {
        return "no band transfer: limits judged at the mic as typed".to_owned();
    };
    let place = &t.place;
    let bands = bands_text(shown);
    let of_shown = || shown.iter().filter_map(|&b| t.bands.get(b));
    if t.origin == TransferOrigin::Estimated {
        let typed = of_shown()
            .filter(|b| !matches!(b, BandTransferBand::Missing))
            .count();
        return format!(
            "estimated transfer from {place}, {bands}: {typed} bands typed, not measured \
             (measure it when you can reach {place})"
        );
    }
    let mut n = [0usize; 5];
    for b in of_shown() {
        n[match b {
            BandTransferBand::Clean { .. } => 0,
            BandTransferBand::Corrected { .. } => 1,
            BandTransferBand::Unchecked { .. } => 2,
            BandTransferBand::Unusable { .. } => 3,
            BandTransferBand::Missing => 4,
        }] += 1;
    }
    let parts: Vec<String> = [
        "clean",
        "background subtracted",
        "no background",
        "bound",
        "not measured",
    ]
    .iter()
    .zip(n)
    .filter(|(_, k)| *k > 0)
    .map(|(w, k)| format!("{k} {w}"))
    .collect();
    format!("transfer from {place}, {bands}: {}", parts.join(", "))
}
