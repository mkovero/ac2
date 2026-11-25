//! A rig that authorized this app as a viewer: edits are refused before they are sent, with
//! one plain warning; reads and stopping still go.

use super::*;
use ac2_proto::model::ClientRole;

fn as_viewer(t: &mut T) {
    let mut m = (**t.st.rig.mirror.as_ref().expect("a mirror")).clone();
    m.role = Some(ClientRole::Viewer);
    t.conn(ConnEvent::Mirror(Arc::new(m)));
}

#[test]
fn a_viewer_is_refused_an_edit_before_it_is_sent() {
    let mut t = T::new();
    t.st.apply(Msg::SelectMeas(MeasId(1)), &t.keys);
    assert!(!t.st.view_only());
    assert!(!t.key("Shift+C").is_empty(), "an operator's edit goes");

    as_viewer(&mut t);
    assert!(t.st.view_only());
    assert!(t.key("Shift+C").is_empty(), "a viewer's edit is not sent");
    assert!(
        t.last_toast()
            .starts_with("not sent: This rig authorized this app as a viewer"),
        "{}",
        t.last_toast()
    );
}
