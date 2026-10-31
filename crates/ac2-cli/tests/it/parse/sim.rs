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
