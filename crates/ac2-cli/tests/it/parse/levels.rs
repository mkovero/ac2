//! Grammar of `ac2 levels` (programme meter stage 2): the settings as typed forms, refused
//! slips, and the reset's inputs.

use super::{ok, parse};
use ac2_cli::args::*;
use ac2_proto::model::{Ballistic, CorrelationWindow};
use ac2_proto::units::{Dbfs, Dbtp, Lufs};
use ac2_scene::unit_text::*;

#[test]
fn levels_grammar() {
    assert!(matches!(ok(&["levels"]).cmd, Cmd::Levels { cmd: None }));
    let c = ok(&[
        "levels",
        "set",
        "--ballistic",
        "type1",
        "--window",
        "0.3s",
        "--over-threshold",
        "-1dbtp",
        "--gate",
        "-50dbfs",
        "--loudness-target",
        "-16lufs",
    ]);
    let Cmd::Levels {
        cmd:
            Some(LevelsCmd::Set {
                ballistic,
                window,
                over_threshold,
                gate,
                loudness_target,
            }),
    } = c.cmd
    else {
        panic!("not levels set");
    };
    assert_eq!(ballistic.map(Ballistic::from), Some(Ballistic::Type1));
    assert_eq!(window, Some(CorrelationWindowArg(CorrelationWindow::Short)));
    assert_eq!(over_threshold, Some(OverThresholdArg(Dbtp(-1.0))));
    assert_eq!(gate, Some(LevelDbfs(Dbfs(-50.0))));
    assert_eq!(loudness_target, Some(LoudnessTargetArg(Lufs(-16.0))));
    // Each ballistic word; the window in either unit; nothing given is a valid (empty) set.
    for (word, want) in [
        ("true-peak", Ballistic::TruePeak),
        ("type2", Ballistic::Type2),
        ("vu", Ballistic::Vu),
        ("momentary", Ballistic::Momentary),
        ("short-term", Ballistic::ShortTerm),
    ] {
        let Cmd::Levels {
            cmd: Some(LevelsCmd::Set { ballistic, .. }),
        } = ok(&["levels", "set", "--ballistic", word]).cmd
        else {
            panic!("{word}");
        };
        assert_eq!(ballistic.map(Ballistic::from), Some(want));
    }
    assert_eq!(
        "300ms".parse::<CorrelationWindowArg>().expect("window").0,
        CorrelationWindow::Short
    );
    assert_eq!(
        "1s".parse::<CorrelationWindowArg>().expect("window").0,
        CorrelationWindow::Long
    );
    assert!(matches!(
        ok(&["levels", "set"]).cmd,
        Cmd::Levels {
            cmd: Some(LevelsCmd::Set {
                ballistic: None,
                window: None,
                over_threshold: None,
                gate: None,
                loudness_target: None,
            })
        }
    ));
    // Refused: a window the rig does not offer, a threshold without its unit or outside the
    // range, a gate in the wrong unit, a ballistic that is not one.
    for bad in [
        &["levels", "set", "--window", "2s"][..],
        &["levels", "set", "--window", "fast"],
        &["levels", "set", "--over-threshold", "-1"],
        &["levels", "set", "--over-threshold", "-1dbfs"],
        &["levels", "set", "--over-threshold", "7dbtp"],
        &["levels", "set", "--over-threshold", "-21dbtp"],
        &["levels", "set", "--gate", "-60dbtp"],
        &["levels", "set", "--gate", "1dbfs"],
        &["levels", "set", "--ballistic", "peak"],
        &["levels", "set", "--ballistic", "loudness"],
        &["levels", "set", "--loudness-target", "-23"],
        &["levels", "set", "--loudness-target", "-23db"],
        &["levels", "set", "--loudness-target", "-37lufs"],
        &["levels", "set", "--loudness-target", "-4lufs"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        "+6dbtp".parse::<OverThresholdArg>().expect("threshold"),
        OverThresholdArg(Dbtp(6.0))
    );
    // The target's range ends are allowed; the unit is required and named in the refusal.
    for (typed, want) in [("-36lufs", -36.0), ("-5LUFS", -5.0), ("-23 lufs", -23.0)] {
        assert_eq!(
            typed.parse::<LoudnessTargetArg>().expect("target"),
            LoudnessTargetArg(Lufs(want))
        );
    }
    let e = "-23".parse::<LoudnessTargetArg>().expect_err("no unit");
    assert!(
        e.0.contains("expected a loudness in LUFS (e.g. -23lufs)"),
        "{}",
        e.0
    );
    // Reset: every input, or the inputs given (1-based).
    let Cmd::Levels {
        cmd: Some(LevelsCmd::Reset { inputs }),
    } = ok(&["levels", "reset"]).cmd
    else {
        panic!("not levels reset");
    };
    assert!(inputs.is_empty());
    let Cmd::Levels {
        cmd: Some(LevelsCmd::Reset { inputs }),
    } = ok(&["levels", "reset", "--input", "2", "--input", "4"]).cmd
    else {
        panic!("not levels reset");
    };
    assert_eq!(inputs, [Channel(1), Channel(3)]);
    assert!(parse(&["levels", "reset", "--input", "0"]).is_err());
}
