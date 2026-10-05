//! What a Leq limit preset sets, as the dialog and the CLI show it (`docs/design/leq.md`).

use ac2_proto::model::{LeqPreset, PeakQuantity};

/// The preset's windows and limits, then its peak limits: `France R1336-1: LAeq 15 min ≤
/// 102 dB, LCeq 15 min ≤ 118 dB`, `DIN 15905-5: LAeq 30 min ≤ 99 dB, LCpeak ≤ 135 dB`. A
/// window the rule wants shown without a limit of its own reads `… shown`.
pub fn summary(p: LeqPreset) -> String {
    let mut windows: Vec<String> = p
        .windows()
        .iter()
        .map(|w| {
            let name = crate::leq::window_name(w);
            match w.limit {
                Some(l) => format!("{name} ≤ {} dB", l.0),
                None => format!("{name} shown"),
            }
        })
        .collect();
    let peaks = p.peaks();
    for q in PeakQuantity::ALL {
        if let Some(l) = peaks.get(q) {
            windows.push(format!("{} ≤ {} dB", crate::leq::peak_name(q), l.limit.0));
        }
    }
    format!("{}: {}", p.name(), windows.join(", "))
}

/// Where the preset's figures come from, with the caveat every preset carries.
pub fn source(p: LeqPreset) -> String {
    format!("{} — informational, not legal advice", p.source())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_name_every_window() {
        assert_eq!(
            summary(LeqPreset::Din15905),
            "DIN 15905-5: LAeq 30 min ≤ 99 dB, LCpeak ≤ 135 dB"
        );
        assert_eq!(
            summary(LeqPreset::Swiss96),
            "Swiss V-NISSG 96 dB: LAeq 60 min ≤ 96 dB, LAFmax ≤ 125 dB"
        );
        assert_eq!(
            summary(LeqPreset::France),
            "France R1336-1: LAeq 15 min ≤ 102 dB, LCeq 15 min ≤ 118 dB"
        );
        assert_eq!(
            summary(LeqPreset::Flanders100),
            "Flanders VLAREM 100 dB: LAeq 15 min shown, LAeq 60 min ≤ 100 dB"
        );
        assert_eq!(
            summary(LeqPreset::Brussels100),
            "Brussels 100 dB: LAeq 60 min ≤ 100 dB, LCeq 60 min ≤ 115 dB"
        );
        for p in LeqPreset::ALL {
            assert!(source(p).ends_with("not legal advice"), "{p:?}");
        }
    }
}
