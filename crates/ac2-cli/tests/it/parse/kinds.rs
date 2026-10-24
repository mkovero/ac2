//! Grammar of the input kinds, stereo pairs and line calibration (programme meter stage 1).

use super::{ok, parse};
use ac2_cli::args::*;
use ac2_scene::unit_text::*;

/// `session inputs --kind / --pair`, `cal line`, `cal rm --line`: typed forms, refused slips.
#[test]
fn input_kinds_pairs_and_line_cal_grammar() {
    let c = ok(&[
        "session",
        "inputs",
        "--kind",
        "3=line",
        "--kind",
        "1=Mic",
        "--kind",
        "5=digital",
        "--pair",
        "3,4=Main",
        "--pair",
        "5,6",
    ]);
    let Cmd::Session {
        cmd: SessionCmd::Inputs(a),
    } = c.cmd
    else {
        panic!("not session inputs");
    };
    assert_eq!(
        a.kinds,
        vec![
            KindAssign {
                input: Channel(2),
                kind: KindArg::Line
            },
            KindAssign {
                input: Channel(0),
                kind: KindArg::Mic
            },
            KindAssign {
                input: Channel(4),
                kind: KindArg::Digital
            },
        ]
    );
    assert_eq!(
        a.pairs,
        vec![
            PairArg {
                left: Channel(2),
                right: Channel(3),
                name: Some("Main".into())
            },
            PairArg {
                left: Channel(4),
                right: Channel(5),
                name: None
            },
        ]
    );
    assert!(!a.no_pairs);
    assert!(!a.is_empty());
    assert_eq!(a.kinds[0].kind.to_kind(), ac2_proto::model::InputKind::Line);
    assert_eq!(
        a.pairs[0].to_pair(),
        ac2_proto::model::Pair {
            left: 2,
            right: 3,
            name: Some("Main".into())
        }
    );
    // `=` with nothing after it: an unnamed pair, not a pair named "".
    assert_eq!("3,4=".parse::<PairArg>().map(|p| p.name), Ok(None));
    // Listing only.
    let Cmd::Session {
        cmd: SessionCmd::Inputs(a),
    } = ok(&["session", "inputs"]).cmd
    else {
        panic!("not session inputs");
    };
    assert!(a.is_empty());
    assert!(ok(&["session", "inputs", "--no-pairs"]).cmd.is_session());
    // A kind is one of three words; a pair is two channels; the two pair forms exclude.
    assert!(parse(&["session", "inputs", "--kind", "3=aes"]).is_err());
    assert!(parse(&["session", "inputs", "--kind", "line"]).is_err());
    assert!(parse(&["session", "inputs", "--kind", "0=line"]).is_err());
    assert!(parse(&["session", "inputs", "--pair", "3"]).is_err());
    assert!(parse(&["session", "inputs", "--pair", "3,x"]).is_err());
    assert!(parse(&["session", "inputs", "--pair", "3,4", "--no-pairs"]).is_err());

    let c = ok(&["cal", "line", "--input", "3", "--volts", "1.228v"]);
    let Cmd::Cal {
        cmd: CalCmd::Line(a),
    } = c.cmd
    else {
        panic!("not cal line");
    };
    assert_eq!(a.input, Channel(2));
    assert!((a.volts.0.0 - 1.228).abs() < 1e-9);
    assert!(
        ok(&["cal", "line", "--channel", "3", "--volts", "775mv"])
            .cmd
            .is_cal()
    );
    // The voltage is required and carries its unit; the level is the daemon's to measure.
    assert!(parse(&["cal", "line", "--input", "3"]).is_err());
    assert!(parse(&["cal", "line", "--input", "3", "--volts", "1.228"]).is_err());
    assert!(
        parse(&[
            "cal", "line", "--input", "3", "--volts", "1.228v", "--read", "-9dbfs"
        ])
        .is_err()
    );

    let Cmd::Cal { cmd: CalCmd::Rm(a) } = ok(&["cal", "rm", "--input", "3", "--line"]).cmd else {
        panic!("not cal rm");
    };
    assert!(a.line);
    assert_eq!(a.mic, None);
    // A line calibration has no mic to name.
    assert!(parse(&["cal", "rm", "--input", "3", "--line", "--mic", "M30"]).is_err());
}

trait CmdKind {
    fn is_session(&self) -> bool;
    fn is_cal(&self) -> bool;
}

impl CmdKind for Cmd {
    fn is_session(&self) -> bool {
        matches!(self, Cmd::Session { .. })
    }
    fn is_cal(&self) -> bool {
        matches!(self, Cmd::Cal { .. })
    }
}
