//! What a measurement keeps about how clients draw it: a spectrum's display settings
//! (`meas.update` in place), its display offset (`meas.offset`) and the curves each pane of
//! its own layout draws (`meas.layout`). None of it touches the job; all of it is saved in
//! the session, and a deleted measurement, trace or walk leaves every stored layout.
#![allow(clippy::unwrap_used)]

use crate::common;

use std::path::Path;

use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_proto::{Command, ErrorCode};
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
    c.measurement(Command::MeasCreate { config })
}

fn meas_of(c: &mut Client, id: MeasId) -> Measurement {
    let s = c.snapshot(Command::StateSnapshot);
    s.state
        .measurements
        .into_iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("no measurement {id}"))
}

fn import(c: &mut Client) -> TraceMeta {
    import_under(c, TraceOwner::Imported)
}

fn import_under(c: &mut Client, owner: TraceOwner) -> TraceMeta {
    import_fixture(c, "rew_export.txt", "rew_export.txt", owner)
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
    let m = c.update_ok(a.id, config).measurement();
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
    let drawn = curves(vec![group(a.id), live(b.id)]);
    c.ok(Command::MeasLayout {
        meas: a.id,
        layout: Some(one_pane(layout::PaneKind::Spectrum, drawn.clone())),
    });
    assert_eq!(meas_of(c, b.id).config_rev, b.config_rev);

    save_and_reload(c, "display");
    let a = meas_of(c, a.id);
    let b = meas_of(c, b.id);
    assert_eq!(a.config.kind.display(), Some(&want));
    assert_eq!(
        a.layout.map(|l| l.panes()[0].content.clone()),
        Some(drawn),
        "a pane's curves come back with the session"
    );
    assert_eq!(b.offset, Db(6.0));
    assert_eq!(b.config.kind.display(), Some(&SpectrumDisplay::default()));
    h.shutdown();
}

fn entry(what: layout::EntryRef) -> layout::PaneEntry {
    layout::PaneEntry {
        rig: layout::RigRef::ThisRig,
        what,
    }
}

fn group(meas: MeasId) -> layout::PaneEntry {
    entry(layout::EntryRef::Group { meas })
}

fn live(meas: MeasId) -> layout::PaneEntry {
    entry(layout::EntryRef::Live { meas })
}

fn trace(trace: TraceId) -> layout::PaneEntry {
    entry(layout::EntryRef::Trace { trace })
}

fn curves(entries: Vec<layout::PaneEntry>) -> layout::PaneContent {
    layout::PaneContent::Curves { entries }
}

fn one_pane(kind: layout::PaneKind, content: layout::PaneContent) -> PaneLayout {
    let mut l = ac2_proto::samples::pane_layout();
    let mut p = l.panes_mut()[0].clone();
    p.kind = kind;
    p.content = content;
    PaneLayout::Pane { pane: Box::new(p) }
}

fn content_of(c: &mut Client, id: MeasId) -> Vec<layout::PaneContent> {
    meas_of(c, id)
        .layout
        .map(|l| l.panes().iter().map(|p| p.content.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn pane_entries_refuse_what_their_group_draws_and_lose_what_is_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let h = start("meas-pane-entries", dir.path());
    let (mut c, _sub) = connect(&h, &[]);
    let c = &mut c;
    let a = created(c, spectrum("A"));
    let b = created(c, spectrum("B"));
    let spl = created(c, spl("SPL", 1));
    let t = import(c);
    let under_a = import_under(c, TraceOwner::Meas { meas: a.id });
    let refused = |c: &mut Client, content: layout::PaneContent| {
        c.call(Command::MeasLayout {
            meas: a.id,
            layout: Some(one_pane(layout::PaneKind::Spectrum, content)),
        })
        .unwrap_err()
    };
    let e = refused(c, curves(vec![group(a.id), trace(under_a.id)]));
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(e.msg.contains("drawn by the group"), "{}", e.msg);
    assert_eq!(
        refused(c, curves(vec![group(a.id), live(a.id)])).code,
        ErrorCode::Invalid,
        "its own live curve is the group's"
    );
    assert_eq!(
        refused(c, curves(vec![group(b.id), group(b.id)])).code,
        ErrorCode::Invalid,
        "twice"
    );
    // Another rig's ids are its own: the same number is another curve.
    let other = layout::PaneEntry {
        rig: layout::RigRef::Rig {
            id: RigId(7),
            name: "ketunkolo".into(),
        },
        what: layout::EntryRef::Group { meas: b.id },
    };
    c.ok(Command::MeasLayout {
        meas: a.id,
        layout: Some(one_pane(
            layout::PaneKind::Spectrum,
            curves(vec![group(a.id), trace(t.id), group(b.id), other.clone()]),
        )),
    });
    c.ok(Command::MeasLayout {
        meas: b.id,
        layout: Some(one_pane(
            layout::PaneKind::Spectrum,
            curves(vec![trace(t.id)]),
        )),
    });
    c.ok(Command::MeasLayout {
        meas: spl.id,
        layout: Some(one_pane(
            layout::PaneKind::Spl,
            layout::PaneContent::Meter {
                rig: layout::RigRef::ThisRig,
                meas: b.id,
            },
        )),
    });
    c.ok(Command::TraceDelete { trace: t.id });
    assert_eq!(
        content_of(c, a.id),
        [curves(vec![group(a.id), group(b.id), other.clone()])]
    );
    assert_eq!(
        content_of(c, b.id),
        [layout::PaneContent::Nothing],
        "the last entry gone leaves nothing picked"
    );
    c.ok(Command::MeasDelete {
        meas: b.id,
        traces: OwnedTraces::Delete,
    });
    assert_eq!(
        content_of(c, a.id),
        [curves(vec![group(a.id), other])],
        "another rig's reference is that rig's to prune"
    );
    assert_eq!(content_of(c, spl.id), [layout::PaneContent::Nothing]);
    h.shutdown();
}
