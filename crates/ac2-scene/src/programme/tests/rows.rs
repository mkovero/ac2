//! The Inputs list's rows: one quantity per bar with its unit beside it, fixed slots, the
//! header by ballistic (docs/design/programme-meter.md §6.3 *The Inputs list rows*).

use super::super::*;
use crate::meter::MeterState;

fn input(label: &str, scale: Scale) -> ProgrammeInput {
    ProgrammeInput {
        label: label.into(),
        scale,
        ppm: Some(-50.6),
        true_peak_dbtp: Some(-6.4),
        max: Some(-45.7),
        overs: 0,
        clipped: false,
        flying_peak: Some(-48.0),
        offers_vu: false,
    }
}

fn texts(b: &RowBlock) -> Vec<String> {
    let mut v: Vec<String> = b
        .sides
        .iter()
        .map(|s| format!("{} ▮ {}", s.mark, s.reading.text))
        .collect();
    v.extend(b.lines.iter().map(|l| l.text.clone()));
    v.extend(b.correlation.iter().map(|c| c.text.clone()));
    v.extend(b.note.clone());
    v
}

/// The header names what the rows read, from the rig's ballistic: the true peak under a
/// loudness ballistic, since loudness is a source's.
#[test]
fn the_header_follows_the_rig_ballistic() {
    for (b, want) in [
        (Ballistic::Type1, "Type I · max since R"),
        (Ballistic::Type2, "Type II · max since R"),
        (Ballistic::TruePeak, "true peak · max since R"),
        (Ballistic::Vu, "VU · max since R"),
        (Ballistic::Momentary, "true peak · max since R"),
        (Ballistic::ShortTerm, "true peak · max since R"),
    ] {
        assert_eq!(inputs_header(b, "R"), want, "{b:?}");
    }
    assert_eq!(
        INPUTS_LOUDNESS_TIP,
        "loudness is metered per programme in the Programme pane"
    );
}

/// A digital pair under a loudness ballistic: a bar line per side, then a max line per
/// side with its overs, then the correlation; no true-peak line, since the bars read it.
#[test]
fn a_pair_row_reads_true_peak_per_side() {
    let mut l = input("Main L", Scale::Dbfs);
    l.overs = 1;
    let mut r = input("Main R", Scale::Dbfs);
    r.ppm = Some(-48.6);
    r.max = Some(-42.2);
    for ballistic in [
        Ballistic::ShortTerm,
        Ballistic::Momentary,
        Ballistic::TruePeak,
    ] {
        let b = row_block(
            &[l.clone(), r.clone()],
            ballistic,
            Some(Correlation::Value(0.31)),
        );
        assert_eq!(
            texts(&b),
            [
                "L ▮  −50.6 dBTP       ",
                "R ▮  −48.6 dBTP       ",
                "L max        −45.7 dBTP      1 over ",
                "R max        −42.2 dBTP             ",
                "  correlation +0.31      ",
            ],
            "{ballistic:?}"
        );
        assert!(b.lines[0].flagged && !b.lines[1].flagged, "the over is L's");
        assert_eq!(b.sides[0].state, MeterState::Silent);
        assert_eq!(b.sides[0].fill, l.bar_fill());
        assert_eq!(b.sides[0].tick, Some(l.flying_peak_fill()));
    }
    let neg = row_block(&[l, r], Ballistic::TruePeak, Some(Correlation::Value(-0.4)));
    assert!(neg.correlation.expect("a pair's").warning);
}

/// A calibrated line input under Type I: the PPM in dBu, its max, and this frame's true
/// peak in dBTP (a digital quantity on every scale); VU it cannot read says so.
#[test]
fn a_mono_row_under_a_ppm_adds_its_true_peak() {
    let mut v = input("Vox", Scale::Dbu { fs_dbu: 13.0 });
    v.offers_vu = true;
    for ballistic in [Ballistic::Type1, Ballistic::Type2, Ballistic::Vu] {
        assert_eq!(
            texts(&row_block(
                &[v.clone()],
                ballistic,
                Some(Correlation::Value(0.5))
            )),
            [
                "  ▮  −37.6 dBu        ",
                "  max        −32.7 dBu              ",
                "  true peak   −6.4 dBTP             ",
            ],
            "{ballistic:?}: no correlation on a mono row"
        );
    }
    let d = input("In 5", Scale::Dbfs);
    assert_eq!(
        texts(&row_block(&[d], Ballistic::Vu, None)),
        [
            "  ▮  −50.6 dBTP       ",
            "  max        −45.7 dBTP             ",
            "  true peak shown · VU needs a line calibration",
        ]
    );
}

/// A mic pair: true peak in dBTP without a calibration, dB SPL with one (the true-peak line
/// stays dBTP), and no note under a loudness ballistic: the pane's bar says why it has no
/// loudness.
#[test]
fn a_mic_pair_row_follows_the_same_grammar() {
    let l = input("Overheads L", Scale::Dbfs);
    let r = input("Overheads R", Scale::Dbfs);
    let b = row_block(
        &[l, r],
        Ballistic::ShortTerm,
        Some(Correlation::Value(0.62)),
    );
    assert_eq!(b.sides.len(), 2);
    assert!(b.sides[0].reading.text.contains("dBTP"));
    assert_eq!(b.note, None);
    assert_eq!(
        b.correlation.expect("a pair").text,
        "  correlation +0.62      "
    );
    let spl = input("MM1", Scale::DbSpl { fs_db_spl: 120.5 });
    assert_eq!(
        texts(&row_block(&[spl], Ballistic::Type1, None)),
        [
            "  ▮   69.9 dB SPL     ",
            "  max         74.8 dB SPL           ",
            "  true peak   −6.4 dBTP             ",
        ]
    );
}

/// No reading yet: an empty bar, no tick, `—` in every number slot, every slot kept.
#[test]
fn a_row_without_a_reading_keeps_its_slots() {
    let mut i = input("In 1", Scale::Dbfs);
    (i.ppm, i.max, i.true_peak_dbtp, i.flying_peak) = (None, None, None, None);
    let b = row_block(&[i], Ballistic::Type1, None);
    assert_eq!(b.sides[0].state, MeterState::NoData);
    assert_eq!((b.sides[0].fill, b.sides[0].tick), (0.0, None));
    assert_eq!(
        texts(&b),
        [
            "  ▮      — dBFS       ",
            "  max            — dBFS             ",
            "  true peak      — dBTP             ",
        ]
    );
}

/// Every line keeps its character count whatever the levels, the overs and CLIP, so only
/// the bar fill and the tick move.
#[test]
fn row_slots_keep_their_width() {
    for scale in [
        Scale::Dbfs,
        Scale::Dbu { fs_dbu: 16.3 },
        Scale::DbSpl { fs_db_spl: 120.5 },
    ] {
        for ballistic in [
            Ballistic::TruePeak,
            Ballistic::Type1,
            Ballistic::Vu,
            Ballistic::ShortTerm,
        ] {
            let mut widths = None;
            for level in [
                None,
                Some(f32::NEG_INFINITY),
                Some(-120.0),
                Some(-0.1),
                Some(3.2),
            ] {
                for overs in [0, 1, 999, 1000, 123_456] {
                    for clipped in [false, true] {
                        let mut i = input("In 1", scale);
                        i.offers_vu = matches!(scale, Scale::Dbu { .. });
                        (i.ppm, i.max, i.true_peak_dbtp) = (level, level, level);
                        i.overs = overs;
                        i.clipped = clipped;
                        let b = row_block(&[i.clone(), i], ballistic, Some(Correlation::NoData));
                        let w: Vec<usize> = texts(&b).iter().map(|l| l.chars().count()).collect();
                        let first = widths.get_or_insert_with(|| w.clone());
                        assert_eq!(
                            *first, w,
                            "{scale:?} {ballistic:?} {level:?} {overs} {clipped}"
                        );
                    }
                }
            }
        }
    }
}

/// The bar's colour state is the pane's peak bar's: level, hot within 6 dB of full scale,
/// CLIP while clip is up.
#[test]
fn the_bar_state_is_the_pane_bars() {
    let mut i = input("In 1", Scale::Dbfs);
    let state = |i: &ProgrammeInput| {
        row_block(std::slice::from_ref(i), Ballistic::TruePeak, None).sides[0].state
    };
    assert_eq!(state(&i), MeterState::Silent);
    i.ppm = Some(-20.0);
    assert_eq!(state(&i), MeterState::Signal);
    i.ppm = Some(-5.0);
    assert_eq!(state(&i), MeterState::Hot);
    i.clipped = true;
    assert_eq!(state(&i), MeterState::Clip);
    assert!(
        row_block(&[i], Ballistic::TruePeak, None).sides[0]
            .reading
            .text
            .ends_with("CLIP")
    );
}
