//! Command-line grammar: what parses, into what, and what is refused (never a panic).

use ac2_cli::Cli;
use ac2_cli::args::*;
use ac2_cli::units::*;
use ac2_proto::units::{Dbfs, Hz, Seconds};
use clap::Parser;
use proptest::prelude::*;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("ac2").chain(args.iter().copied()))
}

fn ok(args: &[&str]) -> Cli {
    match parse(args) {
        Ok(c) => c,
        Err(e) => panic!("{args:?}: {e}"),
    }
}

#[test]
fn plan_examples_parse() {
    // PLAN §7, as written (minus `delay find --band`, which the protocol does not carry).
    let c = ok(&["gen", "pink", "--out", "1,2", "--level", "-20dbfs"]);
    let Cmd::Gen {
        cmd: GenCmd::Pink(o),
    } = c.cmd
    else {
        panic!("not gen pink");
    };
    assert_eq!(o.outputs, Channels(vec![0, 1]));
    assert_eq!(o.level, LevelDbfs(Dbfs(-20.0)));
    assert!(!o.force);

    assert!(matches!(
        ok(&["gen", "stop"]).cmd,
        Cmd::Gen { cmd: GenCmd::Stop }
    ));

    let c = ok(&[
        "meas", "new", "tf", "--ref", "1", "--meas", "2", "--name", "main-l",
    ]);
    let Cmd::Meas {
        cmd: MeasCmd::New(n),
    } = c.cmd
    else {
        panic!("not meas new");
    };
    assert_eq!(n.kind, MeasKindArg::Tf);
    assert_eq!(n.reference, Some(Channel(0)));
    assert_eq!(n.measurement, Some(Channel(1)));

    let c = ok(&["delay", "find", "main-l", "--insert"]);
    assert!(matches!(
        c.cmd,
        Cmd::Delay { cmd: DelayCmd::Find { meas: MeasRef::Name(ref n), insert: Some(PickArg::First) } } if n == "main-l"
    ));
    let c = ok(&["delay", "find", "3", "--insert", "strongest"]);
    assert!(matches!(
        c.cmd,
        Cmd::Delay {
            cmd: DelayCmd::Find {
                meas: MeasRef::Id(3),
                insert: Some(PickArg::Strongest)
            }
        }
    ));

    let c = ok(&["spl", "watch", "--input", "3", "--weight", "a", "--json"]);
    assert!(c.json);
    assert!(matches!(
        c.cmd,
        Cmd::Spl {
            cmd: SplCmd::Watch(SplWatch {
                input: Some(Channel(2)),
                weight: WeightArg::A,
                ..
            })
        }
    ));

    let c = ok(&["cal", "spl", "--input", "3", "--ref", "94db"]);
    let Cmd::Cal {
        cmd: CalCmd::Spl(a),
    } = c.cmd
    else {
        panic!("not cal spl");
    };
    assert_eq!(a.reference.0.0, 94.0);
    assert_eq!(a.freq, Freq(Hz(1000.0)));

    ok(&["trace", "capture", "main-l", "--name", "l-pre-eq"]);
    let c = ok(&["trace", "export", "l-pre-eq", "--csv", "out.csv"]);
    assert!(matches!(
        c.cmd,
        Cmd::Trace {
            cmd: TraceCmd::Export { .. }
        }
    ));

    let c = ok(&["--remote", "foh-rig.local", "status"]);
    assert_eq!(c.remote.map(|r| r.host), Some("foh-rig.local".to_owned()));
    let c = ok(&["status", "--remote", "10.0.0.2:5000", "--json"]);
    assert_eq!(c.remote.map(|r| r.port), Some(5000));
}

#[test]
fn other_commands_parse() {
    ok(&["devices"]);
    ok(&["daemon", "start"]);
    ok(&["daemon", "stop"]);
    ok(&["daemon", "status", "--json"]);
    let c = ok(&[
        "session",
        "open",
        "--backend",
        "fake",
        "--in",
        "1-4",
        "--rate",
        "48khz",
        "--buffer",
        "256samples",
        "--loopback-out",
        "2",
        "--loopback-in",
        "1",
    ]);
    let Cmd::Session {
        cmd: SessionCmd::Open(o),
    } = c.cmd
    else {
        panic!("not session open");
    };
    assert_eq!(o.backend, BackendArg::Fake);
    assert_eq!(o.inputs, Channels(vec![0, 1, 2, 3]));
    assert_eq!(o.buffer, Some(SampleCount(256)));
    ok(&["session", "close"]);
    ok(&[
        "gen", "sine", "--freq", "1khz", "--out", "1", "--level", "-30dbfs", "--force",
    ]);
    ok(&[
        "gen",
        "periodic-pink",
        "--period",
        "65536samples",
        "--out",
        "1",
        "--level=-25dBFS",
        "--hp",
        "30hz",
        "--lp",
        "18khz",
        "--slope",
        "12",
    ]);
    ok(&["meas", "list", "--watch"]);
    ok(&["meas", "start", "main-l"]);
    ok(&["meas", "stop", "2"]);
    ok(&["meas", "rm", "main-l"]);
    let c = ok(&["delay", "set", "main-l", "-1.5ms"]);
    assert!(matches!(
        c.cmd,
        Cmd::Delay { cmd: DelayCmd::Set { delay: DelayAmount::Time(Seconds(t)), .. } } if (t + 0.0015).abs() < 1e-12
    ));
    ok(&["delay", "set", "main-l", "4.3m", "--temp", "-5c"]);
    ok(&["delay", "set", "main-l", "600samples"]);
    ok(&["delay", "insert", "main-l", "--pick", "2"]);
    ok(&["delay", "track", "main-l", "on"]);
    ok(&["spl", "watch", "--meas", "foh"]);
    ok(&[
        "spl", "cal", "--input", "1", "--ref", "114dbspl", "--freq", "1khz",
    ]);
    ok(&["timing", "--watch"]);
    ok(&["state", "dump"]);
    ok(&["trace", "list", "--json"]);
    ok(&["auth", "pair", "rig.local", "--server-key", "x"]);
    ok(&["auth", "show"]);
    ok(&["--timeout", "300ms", "devices"]);
}

#[test]
fn refusals() {
    let bad: &[&[&str]] = &[
        // The generator never runs without a typed level.
        &["gen", "pink", "--out", "1,2"],
        &["gen", "pink", "--out", "1,2", "--level", "-20"],
        &["gen", "pink", "--out", "1,2", "--level", "-20db"],
        &["gen", "pink", "--out", "1,2", "--level", "6dbfs"],
        &["gen", "pink", "--level", "-20dbfs"],
        &["gen", "pink", "--out", "0", "--level", "-20dbfs"],
        &["gen", "sine", "--out", "1", "--level", "-20dbfs"],
        &[
            "gen", "sine", "--freq", "1000", "--out", "1", "--level", "-20dbfs",
        ],
        // Session backend is never implied.
        &["session", "open", "--in", "1,2"],
        &["session", "open", "--backend", "alsa", "--in", "1"],
        &[
            "session",
            "open",
            "--backend",
            "fake",
            "--in",
            "1",
            "--loopback-out",
            "2",
        ],
        &["meas", "new", "fft", "--name", "x"],
        &[
            "meas", "new", "tf", "--ref", "0", "--meas", "1", "--name", "x",
        ],
        &["delay", "set", "main-l", "12"],
        &["delay", "track", "main-l", "maybe"],
        &["delay", "find", "x", "--insert", "best"],
        &["cal", "spl", "--input", "3", "--ref", "94"],
        &["spl", "watch"],
        &["spl", "watch", "--meas", "a", "--input", "1"],
        &["--timeout", "fast", "devices"],
        &["--remote", "fe80::1", "status"],
        &["status", "--ctrl-endpoint", "tcp://x:1"],
        &["frobnicate"],
    ];
    for args in bad {
        assert!(parse(args).is_err(), "{args:?} parsed");
    }
}

fn arb_token() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[-+]?[0-9]{0,6}(\\.[0-9]{0,4})?(e[-+]?[0-9]{1,3})?[a-zA-Zµ°]{0,7}",
        "[0-9,\\- ]{0,12}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn unit_parsers_never_panic(s in arb_token()) {
        let _ = s.parse::<Freq>();
        let _ = s.parse::<LevelDbfs>();
        let _ = s.parse::<Gain>();
        let _ = s.parse::<SplLevel>();
        let _ = s.parse::<Time>();
        let _ = s.parse::<SampleCount>();
        let _ = s.parse::<Distance>();
        let _ = s.parse::<Celsius>();
        let _ = s.parse::<DelayAmount>();
        let _ = s.parse::<Channel>();
        let _ = s.parse::<Channels>();
        let _ = s.parse::<MeasRef>();
        let _ = s.parse::<PickArg>();
        let _ = s.parse::<ac2_client::RemoteAddr>();
    }

    #[test]
    fn cli_never_panics(args in proptest::collection::vec(arb_token(), 0..6)) {
        let _ = Cli::try_parse_from(std::iter::once("ac2".to_owned()).chain(args));
    }

    #[test]
    fn freq_roundtrip(hz in 0.001f64..1e6) {
        let f: Freq = format!("{hz}hz").parse().map_err(|e: UnitError| TestCaseError::fail(e.0))?;
        prop_assert_eq!(f.0.0, hz);
        let k: Freq = format!("{}khz", hz / 1e3).parse().map_err(|e: UnitError| TestCaseError::fail(e.0))?;
        prop_assert!((k.0.0 - hz).abs() <= hz * 1e-12);
    }

    #[test]
    fn level_roundtrip(db in -150f64..=0.0) {
        let l: LevelDbfs = format!("{db}dbfs").parse().map_err(|e: UnitError| TestCaseError::fail(e.0))?;
        prop_assert_eq!(l.0.0, db);
        let bare = format!("{db}");
        prop_assert!(bare.parse::<LevelDbfs>().is_err());
    }

    #[test]
    fn time_roundtrip(ms in 0f64..10_000.0) {
        let t: Time = format!("{ms}ms").parse().map_err(|e: UnitError| TestCaseError::fail(e.0))?;
        prop_assert!((t.0.0 - ms * 1e-3).abs() < 1e-15);
        let d: DelayAmount = format!("-{}ms", ms.min(9_999.0)).parse().map_err(|e: UnitError| TestCaseError::fail(e.0))?;
        prop_assert!(matches!(d, DelayAmount::Time(_)));
    }

    #[test]
    fn channels_roundtrip(ch in proptest::collection::btree_set(1u16..=256, 1..10)) {
        let text = ch.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
        let parsed: Channels = text.parse().map_err(|e: UnitError| TestCaseError::fail(e.0))?;
        prop_assert_eq!(channels_text(&parsed.0), text);
    }
}
