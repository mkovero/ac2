//! `ac2 cal` and the input setup: curves, calibrations, mic names.

use super::*;

#[test]
fn calibration_and_input_setup_parse() {
    let c = ok(&[
        "cal",
        "curve",
        "import",
        "m30.frd",
        "--channel",
        "3",
        "--mic",
        "M30 #1",
        "--label",
        "90°",
    ]);
    let Cmd::Cal {
        cmd: CalCmd::Curve(CalCurveCmd::Import(a)),
    } = c.cmd
    else {
        panic!("not cal curve import");
    };
    assert_eq!(a.channel, Some(Channel(2)));
    assert_eq!(a.mic.as_deref(), Some("M30 #1"));
    assert_eq!(a.label.as_deref(), Some("90°"));
    assert_eq!(a.file, std::path::Path::new("m30.frd"));
    // A mic or an input (whose mic it is) is required.
    assert!(parse(&["cal", "curve", "import", "m30.frd"]).is_err());
    let c = ok(&[
        "cal",
        "curve",
        "rename",
        "--mic",
        "MM1 34804",
        "0°",
        "on axis",
    ]);
    assert!(matches!(
        c.cmd,
        Cmd::Cal {
            cmd: CalCmd::Curve(CalCurveCmd::Rename { ref mic, ref label, ref new_label })
        } if mic == "MM1 34804" && label == "0°" && new_label == "on axis"
    ));
    let c = ok(&["cal", "curve", "rm", "--mic", "MM1 34804", "90°"]);
    assert!(matches!(
        c.cmd,
        Cmd::Cal {
            cmd: CalCmd::Curve(CalCurveCmd::Rm { ref label, .. })
        } if label == "90°"
    ));
    let c = ok(&["cal", "use", "2", "90°"]);
    let Cmd::Cal {
        cmd: CalCmd::Use {
            channel: input,
            curve,
        },
    } = c.cmd
    else {
        panic!("not cal use");
    };
    assert_eq!(input, Channel(1));
    assert_eq!(
        curve.0,
        ac2_proto::model::CurveChoice::Curve {
            label: "90°".into()
        }
    );
    let c = ok(&["cal", "use", "2", "OFF"]);
    assert!(matches!(
        c.cmd,
        Cmd::Cal {
            cmd: CalCmd::Use {
                curve: CurveArg(ac2_proto::model::CurveChoice::Off),
                ..
            }
        }
    ));
    let c = ok(&[
        "cal",
        "rm",
        "--channel",
        "3",
        "--mic",
        "M30",
        "--device",
        "hw:1",
    ]);
    let Cmd::Cal { cmd: CalCmd::Rm(a) } = c.cmd else {
        panic!("not cal rm");
    };
    assert_eq!(a.channel, Channel(2));
    assert_eq!(a.device.as_deref(), Some("hw:1"));

    let c = ok(&[
        "stream",
        "inputs",
        "--mic",
        "3=M30",
        "--mic",
        "4= ECM 8000 ",
        "--mic",
        "5=",
        "--curve",
        "3=off",
        "--curve",
        "4=90°",
    ]);
    let Cmd::Stream {
        cmd: StreamCmd::Inputs(i),
    } = c.cmd
    else {
        panic!("not stream inputs");
    };
    assert_eq!(
        i.mics,
        vec![
            MicAssign {
                input: Channel(2),
                mic: Some("M30".into())
            },
            MicAssign {
                input: Channel(3),
                mic: Some("ECM 8000".into())
            },
            MicAssign {
                input: Channel(4),
                mic: None
            },
        ]
    );
    assert_eq!(
        i.curves,
        vec![
            CurveAssign {
                input: Channel(2),
                curve: CurveArg(ac2_proto::model::CurveChoice::Off),
            },
            CurveAssign {
                input: Channel(3),
                curve: CurveArg(ac2_proto::model::CurveChoice::Curve {
                    label: "90°".into()
                }),
            },
        ]
    );
    let c = ok(&[
        "stream",
        "open",
        "--backend",
        "fake",
        "--in",
        "1-4",
        "--mic",
        "3=M30",
        "--mic",
        "4=ECM",
    ]);
    let Cmd::Stream {
        cmd: StreamCmd::Open(o),
    } = c.cmd
    else {
        panic!("not stream open");
    };
    assert_eq!(o.mics.len(), 2);
    assert!(matches!(
        ok(&["stream", "inputs"]).cmd,
        Cmd::Stream {
            cmd: StreamCmd::Inputs(_)
        }
    ));
}
