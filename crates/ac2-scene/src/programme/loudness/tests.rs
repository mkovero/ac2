use super::*;
use ac2_proto::units::{Db, Dbtp, Lufs, Seconds};

fn source(source: LoudnessSource) -> SourceLoudness {
    SourceLoudness {
        source,
        momentary: Lufs(-23.06),
        short_term: Lufs(-22.84),
        integrated: Some(Lufs(-23.0)),
        range: Some(Db(6.21)),
        max_momentary: Lufs(-20.0),
        max_short_term: Lufs(-21.0),
        max_true_peak: Dbtp(-1.2),
        duration: Seconds(75.0),
        since: WallNs(1),
    }
}

const MAIN: LoudnessSource = LoudnessSource::Pair { left: 2, right: 3 };

/// Inputs 1 (line) and 3+4 (the line pair Main) captured at 48 kHz; 2 a mic; 5+6 the pair
/// Spare with 6 not captured; 7+8 the pair Talk with 8 a mic.
struct Rig {
    rate_hz: u32,
    pairs: Vec<NamedPair>,
    captured: Vec<u16>,
    sources: Vec<SourceLoudness>,
}

impl Rig {
    fn new() -> Self {
        let pair = |left, right, name: &str| NamedPair {
            left,
            right,
            name: name.into(),
        };
        Self {
            rate_hz: 48_000,
            pairs: vec![pair(2, 3, "Main"), pair(4, 5, "Spare"), pair(6, 7, "Talk")],
            captured: vec![0, 1, 2, 3, 4, 6, 7],
            sources: vec![source(MAIN), source(LoudnessSource::Input { channel: 0 })],
        }
    }

    fn column(&self, shown: &[u16]) -> Option<LoudnessColumn> {
        let is_mic = |c: u16| c == 1 || c == 7;
        let label = |c: u16| format!("In {}", c + 1);
        let own_name = |c: u16| (c == 5).then(|| "Spare R".to_owned());
        let clock = |_: WallNs| "20:14".to_owned();
        loudness_column(
            shown,
            &LoudnessFacts {
                rate_hz: self.rate_hz,
                pairs: &self.pairs,
                captured: &self.captured,
                sources: &self.sources,
                is_mic: &is_mic,
                label: &label,
                own_name: &own_name,
                clock: &clock,
                reset_key: Some("R"),
            },
        )
    }

    fn message(&self, shown: &[u16]) -> String {
        match self.column(shown) {
            Some(LoudnessColumn::Message(m)) => m,
            other => panic!("a message for {shown:?}, got {other:?}"),
        }
    }

    fn readout(&self, shown: &[u16]) -> LoudnessReadout {
        match self.column(shown) {
            Some(LoudnessColumn::Readout(r)) => r,
            other => panic!("a readout for {shown:?}, got {other:?}"),
        }
    }
}

#[test]
fn readout_lines_one_decimal_absolute_scale() {
    let r = Rig::new().readout(&[2, 3]);
    assert_eq!(r.title, "Main");
    assert_eq!(
        r.lines(),
        [
            "Mom −23.1",
            "Short −22.8",
            "Int −23.0 LUFS",
            "LRA 6.2 LU",
            "TP max −1.2 dBTP",
        ]
    );
    assert_eq!(r.since, "since 20:14 · R resets");
}

#[test]
fn readout_while_settling_waiting_and_silent() {
    let mut r = Rig::new().readout(&[2, 3]);
    r.duration_s = 59.9;
    assert_eq!(r.range_text(), "LRA 6.2 LU (settling)");
    r.range = None;
    assert_eq!(r.range_text(), "LRA — (waiting for programme)");
    r.integrated = None;
    assert_eq!(r.integrated_text(), "Int — (waiting for programme)");
    r.momentary = f64::NEG_INFINITY;
    r.short_term = f64::NEG_INFINITY;
    r.max_true_peak = f64::NEG_INFINITY;
    assert_eq!(r.momentary_text(), "Mom —");
    assert_eq!(r.short_term_text(), "Short —");
    assert_eq!(r.true_peak_text(), "TP max —");
    assert_eq!(since_text("20:14", None), "since 20:14");
}

#[test]
fn a_pane_picks_its_source() {
    let rig = Rig::new();
    // A pair pane, in either order, and either side alone: the pair, by its name.
    for shown in [&[2, 3][..], &[3, 2], &[2], &[3]] {
        assert_eq!(rig.readout(shown).title, "Main", "{shown:?}");
    }
    // One input in no pair: its mono source.
    assert_eq!(rig.readout(&[0]).title, "In 1 mono");
}

#[test]
fn messages_say_why_there_is_none() {
    let rig = Rig::new();
    assert_eq!(rig.message(&[1]), "no loudness on a mic input");
    assert_eq!(
        rig.message(&[6]),
        "no loudness on a mic input",
        "the pair's mic"
    );
    assert_eq!(
        rig.message(&[4]),
        "loudness: Spare R (input 6) not captured"
    );
    assert_eq!(rig.message(&[0, 2]), "loudness: one input or a pair");
    assert_eq!(rig.message(&[0, 2, 3]), "loudness: one input or a pair");
    let mut unnamed = Rig::new();
    unnamed.pairs[1].right = 9;
    assert_eq!(unnamed.message(&[4]), "loudness: input 10 not captured");
}

#[test]
fn the_first_message_that_applies_wins() {
    let mut rig = Rig::new();
    // A mic beats the partner: pair 7+8 with 8 a mic and not captured.
    rig.captured.retain(|c| *c != 7);
    assert_eq!(rig.message(&[6]), "no loudness on a mic input");
    // Several inputs say only the last, whatever they are.
    assert_eq!(rig.message(&[1, 4]), "loudness: one input or a pair");
    // The rate beats everything.
    rig.rate_hz = 32_000;
    for shown in [&[0][..], &[1], &[4], &[0, 1, 2]] {
        assert_eq!(
            rig.message(shown),
            "loudness needs a rate of 44.1 to 192 kHz, a multiple of 100 Hz",
            "{shown:?}"
        );
    }
}

#[test]
fn loudness_rates() {
    for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        assert!(rate_has_loudness(hz), "{hz}");
    }
    for hz in [32_000, 47_952, 384_000, 44_000 - 100, 192_100] {
        assert!(!rate_has_loudness(hz), "{hz}");
    }
}

#[test]
fn no_source_in_the_frame_shows_nothing() {
    let mut rig = Rig::new();
    rig.sources.clear();
    assert_eq!(rig.column(&[2, 3]), None);
    assert_eq!(rig.column(&[0]), None);
    assert_eq!(rig.column(&[]), None);
}
