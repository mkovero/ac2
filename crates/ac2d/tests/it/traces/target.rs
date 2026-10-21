//! Generated target curves: made from their parameters, filed under the measurement they
//! were made for (or Imported), refused outside their ranges, and kept with their parameters
//! and owner through a session save and load.
use super::*;

#[test]
fn a_generated_target_is_a_target_trace_with_its_parameters_and_survives_a_reload() {
    let mut r = rig("target_curves");
    let c = &mut r.c;
    let curve = TargetCurve::Tilt {
        slope_db_per_oct: -1.0,
        pivot: Hz(1000.0),
        lf_shelf: Some(LfShelf {
            gain: Db(6.0),
            corner: Hz(100.0),
        }),
    };
    let t = trace(c.ok(Command::TraceTarget {
        curve,
        name: "house".into(),
        owner: ac2_proto::model::TraceOwner::Imported,
    }));
    assert_eq!(t.kind, TraceKind::Target);
    assert_eq!(t.source, TraceSource::Generated { curve });
    assert_eq!(t.edit.owner, TraceOwner::Imported);
    assert_eq!(t.edit.name, "house");
    let d = data(c, t.id);
    assert!(d.phase_deg.is_none() && d.coherence.is_none());
    let (first, last) = (d.mag_db[0], *d.mag_db.last().unwrap());
    // About 10 octaves of −1 dB/oct, and the shelf's 6 dB at the bottom.
    assert!(first > 10.0 && last < -4.0, "{first} … {last}");

    let loud = trace(c.ok(Command::TraceTarget {
        curve: TargetCurve::loudness_shift(),
        name: "loudness 85 → 100 phon".into(),
        owner: ac2_proto::model::TraceOwner::Imported,
    }));
    assert!(data(c, loud.id).mag_db[0] > 7.0);

    let e = c
        .call(Command::TraceTarget {
            curve: TargetCurve::LoudnessShift {
                from: Phon(85.0),
                to: Phon(140.0),
            },
            name: "too loud".into(),
            owner: ac2_proto::model::TraceOwner::Imported,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e:?}");
    assert!(e.msg.contains("phon"), "{e:?}");

    c.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "targets".into(),
        },
        overwrite: false,
    });
    c.ok(Command::TraceDelete { trace: t.id });
    c.ok(Command::FileLoad {
        session: SessionRef::Name {
            name: "targets".into(),
        },
        discard: true,
    });
    let back = traces(c)
        .into_iter()
        .find(|x| x.edit.name == "house")
        .expect("reloaded");
    assert_eq!(back.source, TraceSource::Generated { curve });
    assert_eq!(data(c, back.id).mag_db, d.mag_db);
}

#[test]
fn a_target_is_filed_under_the_measurement_it_was_made_for_and_stays_there() {
    let mut r = rig("target_owned");
    let c = &mut r.c;
    let main = TraceOwner::Meas { meas: MeasId(1) };
    let loud = trace(c.ok(Command::TraceTarget {
        curve: TargetCurve::loudness_shift(),
        name: "loudness 85 → 100 phon".into(),
        owner: main,
    }));
    assert_eq!(loud.edit.owner, main);
    // A file loaded as a target goes under the same measurement.
    let file = trace(c.ok(Command::TraceImport {
        file_name: "house.txt".into(),
        format: ImportFormat::Auto,
        role: ImportRole::Target,
        ir: None,
        content: Blob(b"20 6\n1000 0\n20000 -6\n".to_vec()),
        owner: main,
    }));
    assert_eq!(file.edit.owner, main);
    // A measurement that does not exist is refused.
    let e = c
        .call(Command::TraceTarget {
            curve: TargetCurve::Flat,
            name: "flat".into(),
            owner: TraceOwner::Meas { meas: MeasId(99) },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "{e:?}");
    assert!(e.msg.contains("no measurement 99"), "{e:?}");
    c.ok(Command::FileSave {
        session: SessionRef::Name {
            name: "owned".into(),
        },
        overwrite: false,
    });
    c.ok(Command::FileLoad {
        session: SessionRef::Name {
            name: "owned".into(),
        },
        discard: true,
    });
    let back = traces(c);
    let owner_of = |n: &str| back.iter().find(|t| t.edit.name == n).map(|t| t.edit.owner);
    assert_eq!(owner_of("loudness 85 → 100 phon"), Some(main));
    assert_eq!(owner_of("house"), Some(main));
}
