//! A transfer's resolution on the wire: a grid or the bins, tagged by `type`, the same in
//! JSON (the CLI, session files) and MessagePack (the control socket).

use ac2_proto::model::{
    LogGridSpec, MeasConfig, MeasKind, MeasScope, Resolution, TransferConfig, TransferResolution,
};
use serde_json::json;

fn transfer(resolution: TransferResolution) -> MeasConfig {
    MeasConfig {
        scope: MeasScope::Session,
        name: "Main".into(),
        kind: MeasKind::Transfer {
            config: TransferConfig {
                resolution,
                ..TransferConfig::with_inputs(0, 1)
            },
        },
    }
}

#[test]
fn both_shapes_round_trip_in_json_and_msgpack() {
    for (r, shape) in [
        (
            TransferResolution::Grid {
                grid: Resolution::NinetySixth,
            },
            json!({ "type": "grid", "grid": "ninety_sixth" }),
        ),
        (TransferResolution::Bins, json!({ "type": "bins" })),
    ] {
        assert_eq!(serde_json::to_value(r).expect("json"), shape);
        assert_eq!(
            serde_json::from_value::<TransferResolution>(shape).expect("json"),
            r
        );
        let c = transfer(r);
        let j = serde_json::to_string(&c).expect("json");
        assert_eq!(serde_json::from_str::<MeasConfig>(&j).expect("json"), c);
        let m = rmp_serde::to_vec_named(&c).expect("msgpack");
        assert_eq!(rmp_serde::from_slice::<MeasConfig>(&m).expect("msgpack"), c);
    }
    // The bare grid name of the shared `Resolution` is not a transfer's resolution.
    assert!(serde_json::from_value::<TransferResolution>(json!("ninety_sixth")).is_err());
}

#[test]
fn the_default_is_the_forty_eighth_grid_and_bins_store_on_the_ninety_sixth() {
    assert_eq!(
        TransferResolution::default(),
        TransferResolution::Grid {
            grid: Resolution::FortyEighth
        }
    );
    assert_eq!(
        TransferConfig::with_inputs(0, 1).resolution,
        TransferResolution::default()
    );
    assert_eq!(TransferResolution::Bins.grid(), Resolution::NinetySixth);
    let grids: Vec<Resolution> = TransferResolution::ALL.iter().map(|r| r.grid()).collect();
    assert_eq!(
        grids,
        [
            Resolution::Twelfth,
            Resolution::TwentyFourth,
            Resolution::FortyEighth,
            Resolution::NinetySixth,
            Resolution::NinetySixth
        ]
    );
    assert_eq!(TransferResolution::ALL[4], TransferResolution::Bins);
    let at = |r| match transfer(r).kind {
        MeasKind::Transfer { config } => config.grid(),
        _ => unreachable!(),
    };
    assert_eq!(
        at(TransferResolution::Bins),
        at(Resolution::NinetySixth.into())
    );
    assert_eq!(at(TransferResolution::Bins), LogGridSpec::transfer(96));
}
