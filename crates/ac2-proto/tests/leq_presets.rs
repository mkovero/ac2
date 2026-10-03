//! Leq limit presets: the windows and limits each sets (`docs/design/leq.md`, where each
//! figure's source is listed), and how they land on a meter's windows.

use ac2_proto::model::{LeqConfig, LeqPreset, LeqWindow, Weighting};
use ac2_proto::units::{DbSpl, Seconds};

/// (minutes, weighting, limit) of each preset, as the sources give them.
fn table(p: LeqPreset) -> Vec<(u32, Weighting, Option<f64>)> {
    use Weighting::{A, C};
    match p {
        LeqPreset::Din15905 => vec![(30, A, Some(99.0))],
        LeqPreset::Swiss93 => vec![(60, A, Some(93.0))],
        LeqPreset::Swiss96 => vec![(60, A, Some(96.0))],
        LeqPreset::Swiss100 => vec![(60, A, Some(100.0))],
        LeqPreset::Who => vec![(15, A, Some(100.0))],
        LeqPreset::France => vec![(15, A, Some(102.0)), (15, C, Some(118.0))],
        LeqPreset::FranceChildren => vec![(15, A, Some(94.0)), (15, C, Some(104.0))],
        LeqPreset::Flanders85 => vec![(15, A, Some(85.0))],
        LeqPreset::Flanders95 => vec![(15, A, Some(95.0))],
        LeqPreset::Flanders100 => vec![(15, A, None), (60, A, Some(100.0))],
        LeqPreset::Brussels85 => vec![(15, A, Some(85.0))],
        LeqPreset::Brussels95 => vec![(15, A, Some(95.0)), (15, C, Some(110.0))],
        LeqPreset::Brussels100 => vec![(60, A, Some(100.0)), (60, C, Some(115.0))],
        LeqPreset::NetherlandsCovenant => vec![(15, A, Some(103.0))],
        LeqPreset::NetherlandsCovenant16To17 => vec![(15, A, Some(100.0))],
        LeqPreset::NetherlandsCovenant14To15 => vec![(15, A, Some(96.0))],
        LeqPreset::NetherlandsCovenantTo13 => vec![(15, A, Some(91.0))],
    }
}

#[test]
fn every_preset_sets_its_published_windows() {
    for p in LeqPreset::ALL {
        let got: Vec<(u32, Weighting, Option<f64>)> = p
            .windows()
            .iter()
            .map(|w| {
                assert!(w.is_valid(), "{p:?}");
                (
                    w.seconds().expect("whole seconds") / 60,
                    w.weighting,
                    w.limit.map(|l| l.0),
                )
            })
            .collect();
        assert_eq!(got, table(p), "{p:?}");
        assert!(!p.name().is_empty() && !p.source().is_empty());
    }
    let mut names: Vec<&str> = LeqPreset::ALL.iter().map(|p| p.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), LeqPreset::ALL.len(), "names are distinct");
}

/// Every preset fits on the default windows, and the result is a valid configuration.
#[test]
fn every_preset_fits_the_default_windows() {
    for p in LeqPreset::ALL {
        let mut cfg = LeqConfig::default_windows();
        p.apply(&mut cfg.windows).expect("fits");
        cfg.check().expect("valid");
        let lengths: Vec<f64> = cfg.windows.iter().map(|w| w.duration.0).collect();
        assert!(lengths.is_sorted(), "{p:?}: in order of length");
    }
}

/// France: both windows, A then C at 15 min, between 10 and 30 min.
#[test]
fn a_two_window_preset_adds_both() {
    let mut w = LeqConfig::default_windows().windows;
    LeqPreset::France.apply(&mut w).expect("fits");
    let got: Vec<(f64, Weighting, Option<f64>)> = w
        .iter()
        .map(|w| (w.duration.0, w.weighting, w.limit.map(|l| l.0)))
        .collect();
    assert_eq!(
        got,
        [
            (60.0, Weighting::A, None),
            (300.0, Weighting::A, None),
            (600.0, Weighting::A, None),
            (900.0, Weighting::A, Some(102.0)),
            (900.0, Weighting::C, Some(118.0)),
            (1800.0, Weighting::A, None),
            (3600.0, Weighting::A, None),
        ]
    );
    // Again, for children: the same windows, lower limits, nothing added.
    LeqPreset::FranceChildren.apply(&mut w).expect("fits");
    assert_eq!(w.len(), 7);
    assert_eq!(w[3].limit, Some(DbSpl(94.0)));
    assert_eq!(w[4].limit, Some(DbSpl(104.0)));
}

/// A window a rule wants shown without a limit is added bare, and an existing one keeps
/// its own limit.
#[test]
fn a_shown_window_keeps_its_limit() {
    let mut w = LeqConfig::default_windows().windows;
    LeqPreset::Flanders100.apply(&mut w).expect("fits");
    assert_eq!(w[3].duration, Seconds(900.0));
    assert_eq!(w[3].limit, None);
    assert_eq!(w[5].limit, Some(DbSpl(100.0)));
    LeqPreset::Flanders95.apply(&mut w).expect("fits");
    LeqPreset::Flanders100.apply(&mut w).expect("fits");
    assert_eq!(w[3].limit, Some(DbSpl(95.0)), "kept");
}

/// No room: refused with the count, the windows unchanged.
#[test]
fn a_full_meter_refuses_a_preset_that_adds() {
    let mut w: Vec<LeqWindow> = [1, 2, 5, 10, 20, 30, 60, 120]
        .map(LeqWindow::minutes)
        .to_vec();
    assert_eq!(w.len(), LeqConfig::MAX_WINDOWS);
    let before = w.clone();
    let e = LeqPreset::Brussels95.apply(&mut w).expect_err("no room");
    assert!(e.contains("Brussels 95 dB needs 2 more windows"), "{e}");
    assert_eq!(w, before);
    assert_eq!(LeqPreset::Who.missing(&w), 1);
    // A preset on windows the meter has still applies.
    LeqPreset::Swiss100.apply(&mut w).expect("has 60 min");
    assert_eq!(w[6].limit, Some(DbSpl(100.0)));
}
