//! What a measurement keeps about how clients draw it: a spectrum's display settings
//! (`meas.update` in place), its display offset (`meas.offset`) and the curves compared into
//! its panes (`meas.compare`). None of it touches the job; all of it is saved in the
//! session, and a deleted measurement or trace leaves every compare list.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::Path;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode, ReplyBody};
use ac2d::{Daemon, Handle};
use common::*;

fn start(name: &str, dir: &Path) -> Handle {
    init_log();
    let mut cfg = config(manual_rig(), inproc(&unique(name)));
    cfg.session_dir = dir.join("sessions");
    Daemon::start(cfg).unwrap()
}

fn spectrum(name: &str) -> MeasConfig {
    MeasConfig {
        scope: MeasScope::Session,
        name: name.into(),
        kind: MeasKind::Spectrum {
            config: SpectrumConfig::on_input(1),
        },
    }
}

fn created(c: &mut Client, config: MeasConfig) -> Measurement {
    match c.ok(Command::MeasCreate { config }) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    }
}

fn meas_of(c: &mut Client, id: MeasId) -> Measurement {
    let ReplyBody::Snapshot(s) = c.ok(Command::StateSnapshot) else {
        panic!("state.snapshot answered with something else")
    };
    s.state
        .measurements
        .into_iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("no measurement {id}"))
}

fn import(c: &mut Client) -> TraceMeta {
    let name = "rew_export.txt";
    let content = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ac2-traces/tests/fixtures")
            .join(name),
    )
    .unwrap();
    match c.ok(Command::TraceImport {
        file_name: name.into(),
        format: ImportFormat::Auto,
        role: ImportRole::Trace,
        ir: None,
        content: Blob(content),
        owner: ac2_proto::model::TraceOwner::Imported,
    }) {
        ReplyBody::Trace(t) => t,
        other => panic!("{other:?}"),
    }
}

fn save_and_reload(c: &mut Client, name: &str) {
    c.ok(Command::FileSave {
        session: SessionRef::Name { name: name.into() },
        overwrite: true,
    });
    c.ok(Command::FileNew { discard: true });
    c.ok(Command::FileLoad {
        session: SessionRef::Name { name: name.into() },
        discard: true,
    });
}

#[test]
fn display_settings_keep_the_job_and_survive_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("meas-display", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    let a = created(c, spectrum("A"));
    let b = created(c, spectrum("B"));
    let mut config = a.config.clone();
    let d = config.kind.display_mut().unwrap();
    d.style = SpectrumStyle::Fill;
    d.peak = ac2_proto::model::PeakMode::Hold;
    d.spectrograph_span_s = 60;
    d.spectrograph_colours = Colormap::Magma;
    let want = *d;
    let m = match c.update_ok(a.id, config) {
        ReplyBody::Measurement(m) => m,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        m.config_rev, a.config_rev,
        "how it is drawn is not a new config"
    );
    assert_eq!(m.config.kind.display(), Some(&want));
    let e = c
        .call(Command::MeasOffset {
            meas: b.id,
            offset: Db(f64::NAN),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    let e = c
        .call(Command::MeasOffset {
            meas: b.id,
            offset: Db(200.5),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    c.ok(Command::MeasOffset {
        meas: b.id,
        offset: Db(6.0),
    });
    c.ok(Command::MeasCompare {
        meas: a.id,
        compared: vec![Compared::Meas { meas: b.id }],
    });
    assert_eq!(meas_of(c, b.id).config_rev, b.config_rev);

    save_and_reload(c, "display");
    let a = meas_of(c, a.id);
    let b = meas_of(c, b.id);
    assert_eq!(a.config.kind.display(), Some(&want));
    assert_eq!(a.compared, [Compared::Meas { meas: b.id }]);
    assert_eq!(b.offset, Db(6.0));
    assert_eq!(b.config.kind.display(), Some(&SpectrumDisplay::default()));
    h.shutdown();
}

#[test]
fn compare_lists_refuse_bad_entries_and_lose_what_is_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("meas-compare", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    let a = created(c, spectrum("A"));
    let b = created(c, spectrum("B"));
    let spl = created(c, spl("SPL", 1));
    let t = import(c);
    let refused = |c: &mut Client, meas: MeasId, compared: Vec<Compared>| {
        c.call(Command::MeasCompare { meas, compared })
            .unwrap_err()
            .code
    };
    assert_eq!(
        refused(c, a.id, vec![Compared::Meas { meas: a.id }]),
        ErrorCode::Invalid,
        "itself"
    );
    assert_eq!(
        refused(
            c,
            a.id,
            vec![Compared::Meas { meas: b.id }, Compared::Meas { meas: b.id }]
        ),
        ErrorCode::Invalid,
        "twice"
    );
    assert_eq!(
        refused(c, a.id, vec![Compared::Meas { meas: MeasId(99) }]),
        ErrorCode::NotFound
    );
    assert_eq!(
        refused(c, a.id, vec![Compared::Trace { trace: TraceId(99) }]),
        ErrorCode::NotFound
    );
    assert_eq!(
        refused(c, spl.id, vec![Compared::Meas { meas: a.id }]),
        ErrorCode::Invalid,
        "an SPL meter draws no curve"
    );
    c.ok(Command::MeasCompare {
        meas: a.id,
        compared: vec![
            Compared::Trace { trace: t.id },
            Compared::Meas { meas: b.id },
        ],
    });
    c.ok(Command::MeasCompare {
        meas: b.id,
        compared: vec![Compared::Trace { trace: t.id }],
    });
    c.ok(Command::TraceDelete { trace: t.id });
    assert_eq!(meas_of(c, a.id).compared, [Compared::Meas { meas: b.id }]);
    assert!(meas_of(c, b.id).compared.is_empty());
    c.ok(Command::MeasDelete {
        meas: b.id,
        traces: OwnedTraces::Delete,
    });
    assert!(meas_of(c, a.id).compared.is_empty());
    h.shutdown();
}
