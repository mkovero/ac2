//! Leq limit presets: the windows and limits each sets (`docs/design/leq.md`, where each
//! figure's source is listed), and how they replace a meter's windows.

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

fn rows(w: &[LeqWindow]) -> Vec<(u32, Weighting, Option<f64>)> {
    w.iter()
        .map(|w| {
            (
                w.seconds().expect("whole seconds") / 60,
                w.weighting,
                w.limit.map(|l| l.0),
            )
        })
        .collect()
}

#[test]
fn every_preset_sets_its_published_windows() {
    for p in LeqPreset::ALL {
        assert!(p.windows().iter().all(LeqWindow::is_valid), "{p:?}");
        assert_eq!(rows(&p.windows()), table(p), "{p:?}");
        assert!(!p.name().is_empty() && !p.source().is_empty());
    }
    let mut names: Vec<&str> = LeqPreset::ALL.iter().map(|p| p.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), LeqPreset::ALL.len(), "names are distinct");
}

/// Applied, a preset is the meter's windows: exactly the rule's, shortest first, each with
/// the rule's limit, whatever the meter had before; a valid configuration.
#[test]
fn every_preset_replaces_the_windows_with_its_own() {
    for p in LeqPreset::ALL {
        let w = LeqPreset::windows_of(&[p]);
        assert_eq!(rows(&w), table(p), "{p:?}");
        let lengths: Vec<f64> = w.iter().map(|w| w.duration.0).collect();
        assert!(lengths.is_sorted(), "{p:?}: shortest first");
        let cfg = LeqConfig {
            windows: w,
            ..LeqConfig::default_windows()
        };
        cfg.check().expect("valid");
    }
}

/// Two rules at once: the windows of both; a window both have gets the lower limit (both
/// met), and a limit wins over a window only shown. France and Flanders 100 dB: LAeq 15 min
/// at most 102 (Flanders only shows it), LCeq 15 min at most 118, LAeq 60 min at most 100.
#[test]
fn presets_together_are_the_union_with_the_lower_limit() {
    let w = LeqPreset::windows_of(&[LeqPreset::Flanders100, LeqPreset::France]);
    assert_eq!(
        rows(&w),
        [
            (15, Weighting::A, Some(102.0)),
            (15, Weighting::C, Some(118.0)),
            (60, Weighting::A, Some(100.0)),
        ]
    );
    let w = LeqPreset::windows_of(&[LeqPreset::France, LeqPreset::Who]);
    assert_eq!(w[0].limit, Some(DbSpl(100.0)), "the WHO limit is the lower");
    let all = LeqPreset::windows_of(&LeqPreset::ALL);
    assert!(all.len() <= LeqConfig::MAX_WINDOWS, "{}", all.len());
    assert_eq!(LeqPreset::windows_of(&[]), Vec::<LeqWindow>::new());
}

/// Display order of windows: by length, equal lengths A, C, Z.
#[test]
fn windows_sort_shortest_first() {
    let with = |weighting, w: LeqWindow| LeqWindow { weighting, ..w };
    let mut w = vec![
        with(Weighting::Z, LeqWindow::minutes(15)),
        LeqWindow::minutes(60),
        with(Weighting::C, LeqWindow::minutes(15)),
        LeqWindow::minutes(15),
        LeqWindow {
            duration: Seconds(10.0),
            ..LeqWindow::minutes(1)
        },
    ];
    LeqWindow::sort(&mut w);
    let got: Vec<(f64, Weighting)> = w.iter().map(|w| (w.duration.0, w.weighting)).collect();
    assert_eq!(
        got,
        [
            (10.0, Weighting::A),
            (900.0, Weighting::A),
            (900.0, Weighting::C),
            (900.0, Weighting::Z),
            (3600.0, Weighting::A),
        ]
    );
}
