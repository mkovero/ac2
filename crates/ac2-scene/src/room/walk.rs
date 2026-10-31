//! A sweep walk's room parameters as a table (`docs/design/spatial-walk.md` §11): per band
//! the mean over the positions and, under it, their least and greatest.
//!
//! A position that does not give a parameter in a band is out of that band's mean; the cell
//! then says how many positions it is of (`1.23 (6)`). ISO 3382-1 asks for at least two
//! source positions, and a walk measures from one: the caption counts both and the legend
//! says so.

use ac2_proto::model::{PositionMean, RoomRefusal, WalkRoom, WalkRoomBand};

use super::{BandSet, Param, RoomCell, RoomRow, RoomTable, band_name};
use crate::format;

/// The parameters a walk averages, in table order.
const WALK_PARAMS: [Param; 4] = [Param::Edt, Param::T20, Param::C80, Param::D50];

/// Source positions ISO 3382-1 asks for at the least.
const ISO_SOURCES: u16 = 2;

/// *mean of 8 positions · 1 source*.
pub fn positions_caption(r: &WalkRoom) -> String {
    format!(
        "mean of {} position{} · {} source{}",
        r.positions,
        if r.positions == 1 { "" } else { "s" },
        r.sources,
        if r.sources == 1 { "" } else { "s" }
    )
}

fn value(p: Param, b: &WalkRoomBand) -> Option<PositionMean> {
    match p {
        Param::Edt => b.edt,
        Param::T20 => b.t20,
        Param::C80 => b.c80,
        Param::D50 => b.d50,
        Param::T30 | Param::C50 | Param::Range => None,
    }
}

fn none() -> RoomCell {
    RoomCell {
        text: format::NO_VALUE.to_owned(),
        refused: Some(RoomRefusal::NoDecay),
    }
}

fn mean_cell(p: Param, m: Option<PositionMean>, positions: u16) -> RoomCell {
    match m {
        Some(m) => {
            let mut text = p.number(m.mean);
            if m.n < positions {
                text.push_str(&format!(" ({})", m.n));
            }
            RoomCell {
                text,
                refused: None,
            }
        }
        None => none(),
    }
}

fn spread_cell(p: Param, m: Option<PositionMean>) -> RoomCell {
    match m {
        Some(m) => RoomCell {
            text: format!("{}–{}", p.number(m.min), p.number(m.max)),
            refused: None,
        },
        None => none(),
    }
}

/// The table of a walk's `set` bands and broadband: a row of means per parameter and the
/// positions' least and greatest under it.
pub fn walk_room_table(r: &WalkRoom, set: BandSet) -> RoomTable {
    means_table(r, set, true)
}

/// [`walk_room_table`], with the ISO 3382-1 source-count note when `iso_note`.
pub(super) fn means_table(r: &WalkRoom, set: BandSet, iso_note: bool) -> RoomTable {
    let bands: Vec<&WalkRoomBand> = match set {
        BandSet::Octave => &r.octave,
        BandSet::Third => &r.third,
    }
    .iter()
    .chain(std::iter::once(&r.broadband))
    .collect();
    let mut rows: Vec<RoomRow> = Vec::with_capacity(2 * WALK_PARAMS.len());
    for p in WALK_PARAMS {
        rows.push(RoomRow {
            param: p,
            spread: false,
            cells: bands
                .iter()
                .map(|b| mean_cell(p, value(p, b), r.positions))
                .collect(),
        });
        rows.push(RoomRow {
            param: p,
            spread: true,
            cells: bands.iter().map(|b| spread_cell(p, value(p, b))).collect(),
        });
    }
    let mut legend = Vec::new();
    let fewer = bands.iter().any(|b| {
        WALK_PARAMS
            .iter()
            .any(|&p| value(p, b).is_some_and(|m| m.n < r.positions))
    });
    if fewer {
        legend.push(
            "(n): the mean of the n positions that give it; the others' decay does not in the \
             band (too little range, or too short)"
                .to_owned(),
        );
    }
    if rows
        .iter()
        .flat_map(|r| &r.cells)
        .any(|c| c.refused.is_some())
    {
        legend.push(format!(
            "{}: no position gives it in the band",
            format::NO_VALUE
        ));
    }
    if iso_note && r.sources < ISO_SOURCES {
        legend.push(format!(
            "ISO 3382-1 asks for at least {ISO_SOURCES} source positions; this walk measured \
             from {}",
            r.sources
        ));
    }
    RoomTable {
        caption: format!(
            "Room (ISO 3382-1) · {} · {}",
            positions_caption(r),
            match set {
                BandSet::Octave => "octave bands",
                BandSet::Third => "⅓-octave bands",
            }
        ),
        bands: bands
            .iter()
            .map(|b| band_name(b.centre.map(|h| h.0)))
            .collect(),
        rows,
        legend,
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use ac2_proto::units::Hz;

    fn mean(mean: f64, min: f64, max: f64, n: u16) -> Option<PositionMean> {
        Some(PositionMean { mean, min, max, n })
    }

    pub(in crate::room) fn walk_room() -> WalkRoom {
        WalkRoom {
            positions: 4,
            sources: 1,
            broadband: WalkRoomBand {
                centre: None,
                edt: mean(0.9, 0.8, 1.0, 4),
                t20: mean(1.1, 1.0, 1.25, 4),
                c80: mean(2.46, 1.0, 3.9, 4),
                d50: mean(0.452, 0.4, 0.5, 4),
            },
            octave: vec![WalkRoomBand {
                centre: Some(Hz(125.0)),
                edt: mean(1.2, 1.1, 1.3, 4),
                t20: mean(1.4, 1.2, 1.6, 3),
                c80: None,
                d50: mean(0.3, 0.25, 0.35, 4),
            }],
            third: Vec::new(),
        }
    }

    #[test]
    fn the_caption_counts_positions_and_sources() {
        let t = walk_room_table(&walk_room(), BandSet::Octave);
        assert_eq!(
            t.caption,
            "Room (ISO 3382-1) · mean of 4 positions · 1 source · octave bands"
        );
        assert_eq!(t.bands, ["125", "All"]);
    }

    #[test]
    fn means_with_their_count_and_range_rows() {
        let t = walk_room_table(&walk_room(), BandSet::Octave);
        let names: Vec<&str> = t.rows.iter().map(RoomRow::name).collect();
        assert_eq!(
            names,
            [
                "EDT (s)",
                "  min–max",
                "T20 (s)",
                "  min–max",
                "C80 (dB)",
                "  min–max",
                "D50 (%)",
                "  min–max"
            ]
        );
        let cells =
            |k: usize| -> Vec<&str> { t.rows[k].cells.iter().map(|c| c.text.as_str()).collect() };
        assert_eq!(cells(2), ["1.40 (3)", "1.10"]);
        assert_eq!(cells(3), ["1.20–1.60", "1.00–1.25"]);
        assert_eq!(cells(4), ["—", "2.5"]);
        assert_eq!(cells(6), ["30", "45"]);
        assert_eq!(cells(7), ["25–35", "40–50"]);
    }

    #[test]
    fn the_legend_says_the_count_and_the_iso_sources() {
        let t = walk_room_table(&walk_room(), BandSet::Octave);
        assert!(t.legend[0].starts_with("(n): the mean of the n positions that give it"));
        assert_eq!(t.legend[1], "—: no position gives it in the band");
        assert_eq!(
            t.legend[2],
            "ISO 3382-1 asks for at least 2 source positions; this walk measured from 1"
        );
        assert!(t.text().contains("  min–max"));
    }

    #[test]
    fn one_position_reads_singular() {
        let mut r = walk_room();
        r.positions = 1;
        assert_eq!(positions_caption(&r), "mean of 1 position · 1 source");
    }
}
