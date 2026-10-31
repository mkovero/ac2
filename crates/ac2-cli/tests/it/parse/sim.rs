//! `ac2 sim …`: points, settings and their defaults, and what is refused.

use super::{ok, parse};
use ac2_cli::args::*;
use ac2_proto::units::Seconds;

#[test]
fn sim_grammar() {
    let c = ok(&[
        "sim",
        "run",
        "--plan",
        "Hall",
        "--source",
        "Main L",
        "--point",
        "5,12,1.2",
        "--point",
        "Bar = 8, 3.5, 1.2",
        "--rays",
        "5000",
        "--order",
        "2",
        "--budget",
        "2.5",
        "--seed",
        "7",
    ]);
    let Cmd::Sim {
        cmd: SimCmd::Run(a),
    } = c.cmd
    else {
        panic!("not sim run");
    };
    assert_eq!(a.sources, ["Main L"]);
    let points = a.point_specs();
    assert_eq!(points[0].label, "P1");
    assert_eq!(points[0].at, [5.0, 12.0, 1.2]);
    assert_eq!(points[1].label, "Bar");
    assert_eq!(points[1].at, [8.0, 3.5, 1.2]);
    let st = a.settings.settings();
    assert_eq!(
        (st.rays_per_band, st.image_order, st.budget, st.seed),
        (5000, 2, Seconds(2.5), 7)
    );
    assert!(!a.detach);
    // Defaults are the protocol's.
    let Cmd::Sim {
        cmd: SimCmd::Run(a),
    } = ok(&[
        "sim", "run", "--plan", "Hall", "--walk", "Mains", "--detach",
    ])
    .cmd
    else {
        panic!("not sim run");
    };
    assert_eq!(
        a.settings.settings(),
        ac2_proto::model::SimSettings::default()
    );
    assert!(a.detach);
    assert!(matches!(
        ok(&["sim", "fit", "--walk", "2"]).cmd,
        Cmd::Sim { cmd: SimCmd::Fit(ref f) } if f.source.is_none()
    ));
    assert!(matches!(
        ok(&["sim", "status"]).cmd,
        Cmd::Sim {
            cmd: SimCmd::Status
        }
    ));
    assert!(matches!(
        ok(&["sim", "cancel"]).cmd,
        Cmd::Sim {
            cmd: SimCmd::Cancel
        }
    ));
    for bad in [
        &["sim", "run", "--plan", "Hall"][..],
        &[
            "sim", "run", "--plan", "Hall", "--walk", "1", "--point", "1,2,1",
        ],
        &["sim", "run", "--plan", "Hall", "--point", "1,2"],
        &["sim", "run", "--plan", "Hall", "--point", "=1,2,3"],
        &["sim", "run", "--plan", "Hall", "--point", "1,2,inf"],
        &[
            "sim", "run", "--plan", "Hall", "--point", "1,2,1", "--order", "9",
        ],
        &[
            "sim", "run", "--plan", "Hall", "--point", "1,2,1", "--rays", "0",
        ],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn sim_map_defaults_and_flags() {
    let c = ok(&["sim", "map", "--plan", "Hall"]);
    let Cmd::Sim {
        cmd: SimCmd::Map(a),
    } = c.cmd
    else {
        panic!("not sim map");
    };
    let r = a.request(vec!["Main L".into()]);
    assert_eq!(r.spacing, 1.0);
    assert_eq!(r.band, None);
    assert_eq!(r.window, Seconds(0.05));
    assert_eq!(r.image_order, 2);
    let c = ok(&[
        "sim",
        "map",
        "--plan",
        "Hall",
        "--source",
        "Main L",
        "--spacing",
        "0.5",
        "--band",
        "1000",
        "--window-ms",
        "80",
        "--order",
        "3",
        "--detach",
    ]);
    let Cmd::Sim {
        cmd: SimCmd::Map(a),
    } = c.cmd
    else {
        panic!("not sim map");
    };
    let r = a.request(a.sources.clone());
    assert_eq!(r.sources, ["Main L"]);
    assert_eq!(r.spacing, 0.5);
    assert_eq!(r.band, Some(ac2_proto::units::Hz(1000.0)));
    assert!((r.window.0 - 0.08).abs() < 1e-12);
    assert_eq!(r.image_order, 3);
    assert!(a.detach);
}
