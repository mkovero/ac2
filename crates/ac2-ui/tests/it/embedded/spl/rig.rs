//! The rig monitor from the app, on an empty daemon: an SPL meter moved onto the rig from its
//! limit editor leads the tree under the Rig header; the Rig monitor page sets where cues go.

use crate::embedded::harness::{Driver, NAME, R, measure_from_empty};
use ac2_proto::model::{MeasKind, MeasScope};
use ac2_scene::meas_list::TreeKey;
use ac2_ui::embedded::{EmbeddedBackend, Setup, start_embedded_with};
use ac2_ui::forms::FormKind;
use ac2_ui::leq_dialog::Focus;
use ac2_ui::settings::Page;
use ac2_ui::state::{LeqMsg, Msg, Overlay};

#[test]
fn a_meter_moves_onto_the_rig_and_the_rig_page_sets_the_target() -> R {
    let daemon = start_embedded_with(EmbeddedBackend::Fake, Setup::Empty)?;
    let ep = daemon.client_config(NAME);
    let mut d = Driver::connect(ep, &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Ctrl+K");
    d.send(Msg::Text("new spl".into()));
    d.key("Enter");
    d.until(
        "the SPL dialog",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Spl),
    )?;
    d.key("Enter");
    d.until("the SPL meter", |s| {
        s.measurements()
            .iter()
            .any(|m| matches!(m.config.kind, MeasKind::Spl { .. }))
    })?;

    // Shift+L, the rig row, → keeps it on the rig, Enter.
    d.key("Shift+L");
    d.send(Msg::Text("L".into()));
    d.until("the limit editor", |s| s.overlay.leq().is_some())?;
    d.send(Msg::Leq(LeqMsg::Focus(Focus::Scope)));
    d.key("ArrowRight");
    d.key("Enter");
    d.until("the meter on the rig", |s| {
        s.measurements().iter().any(|m| {
            matches!(m.config.kind, MeasKind::Spl { .. }) && m.config.scope == MeasScope::Rig
        })
    })?;
    d.until("the Rig header first in the tree", |s| {
        s.tree_rows().first().map(|r| r.key) == Some(TreeKey::Rig)
    })?;

    // The header opens the Rig monitor page; a target typed and applied.
    d.send(Msg::OpenRig);
    d.until("the Rig monitor page", |s| {
        s.overlay.settings().map(|x| x.page) == Some(Page::Rig)
    })?;
    d.send(Msg::Text("127.0.0.1:9".into()));
    d.key("Enter");
    d.until("the target set", |s| {
        s.daemon()
            .and_then(|x| x.rig.osc_target)
            .is_some_and(|a| a.to_string() == "127.0.0.1:9")
    })?;
    d.until("the page showing it", |s| {
        s.overlay
            .settings()
            .is_some_and(|x| x.rig.target == "127.0.0.1:9" && !x.rig.edited)
    })?;
    Ok(())
}
