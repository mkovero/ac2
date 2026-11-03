//! The measurement list as a tree rooted at each rig: one rig is a root too, a rig's branch
//! folds, Tab skips a folded rig's rows, and the fold is remembered by rig id.

use super::*;
use ac2_scene::meas_list::TreeKey;

/// The rows of rig `rig` in the list as drawn.
fn rows_of(t: &T, rig: RigId) -> Vec<ac2_scene::meas_list::TreeRow> {
    t.st.list_rows()
        .into_iter()
        .filter(|(r, _)| *r == rig)
        .map(|(_, row)| row)
        .collect()
}

/// With one rig connected the list is still rooted at it: its heading first, with a fold
/// arrow, and every row of its own a level under it.
#[test]
fn one_rig_is_the_root_of_its_rows() {
    let t = T::new();
    let rows = t.st.list_rows();
    let (rig, head) = &rows[0];
    assert_eq!((*rig, head.key), (RIG, TreeKey::RigName(RIG)));
    assert_eq!((head.depth, head.collapsed), (0, Some(false)));
    let own = t.st.tree_rows();
    assert_eq!(rows.len(), own.len() + 1);
    for ((_, listed), own) in rows[1..].iter().zip(&own) {
        assert_eq!(listed.key, own.key);
        assert_eq!(listed.depth, own.depth + 1, "{listed:?}");
    }
}

/// The arrow on pupu's heading folds its branch: only the heading is listed, saying how
/// many measurements and traces it hides; the shown rig's rows stay. Again, and it unfolds.
#[test]
fn a_rigs_arrow_folds_its_branch() {
    let (mut t, _) = two_lists();
    let open = rows_of(&t, PUPU).len();
    assert!(open > 1, "pupu lists its rows");
    let shown = rows_of(&t, RIG);
    t.st.prefs_dirty = false;
    t.st.apply(Msg::FoldRig(PUPU), &t.keys);
    assert!(t.st.prefs_dirty, "the fold is saved");
    let folded = rows_of(&t, PUPU);
    assert_eq!(folded.len(), 1, "{folded:?}");
    assert_eq!(folded[0].collapsed, Some(true));
    let n = pupu(&t).daemon().expect("mirrored").measurements.len();
    assert!(
        folded[0].details[0].starts_with(&format!("{n} measurements · ")),
        "{:?}",
        folded[0].details
    );
    assert!(folded[0].details[0].ends_with(" · folded"));
    assert_eq!(rows_of(&t, RIG), shown, "the shown rig's rows stay");
    assert_eq!(t.st.active(), RIG, "folding shows no other rig");
    t.st.apply(Msg::FoldRig(PUPU), &t.keys);
    assert_eq!(rows_of(&t, PUPU).len(), open);
}

/// Tab skips a folded rig's rows: with pupu folded it wraps within the rig shown; with the
/// rig shown folded it goes on to pupu's rows.
#[test]
fn tab_skips_a_folded_rig() {
    let (mut t, _) = two_lists();
    let rows = t.st.tree_rows().len();
    t.st.apply(Msg::FoldRig(PUPU), &t.keys);
    for _ in 0..rows + 2 {
        t.key("Tab");
        assert_eq!(
            t.st.active(),
            RIG,
            "pupu folded: Tab stays on the rig shown"
        );
    }
    t.st.apply(Msg::FoldRig(PUPU), &t.keys);
    t.st.apply(Msg::FoldRig(RIG), &t.keys);
    t.key("Tab");
    assert_eq!(
        t.st.active(),
        PUPU,
        "the rig shown folded: Tab goes on to pupu"
    );
    assert!(t.st.rig.selected.is_some() || t.st.live_row_selected());
}

/// The fold is kept in the prefs by rig id: the file written and read back folds the same
/// rig, in a fresh app too.
#[test]
fn a_rigs_fold_survives_a_restart() {
    let (mut t, _) = two_lists();
    t.st.apply(Msg::FoldRig(PUPU), &t.keys);
    let text = t.st.prefs.to_toml();
    assert!(
        text.contains("folded_rigs = [\"0000000000009e9e\"]"),
        "{text}"
    );
    let back = crate::prefs::UiPrefs::from_toml(&text).expect("parses");
    let (mut fresh, _) = two_lists();
    fresh.st.prefs = back;
    assert_eq!(rows_of(&fresh, PUPU).len(), 1, "pupu still folded");
    assert!(rows_of(&fresh, RIG).len() > 1, "the rig shown not");
}

/// A row of pupu still drags onto a pane when pupu's branch was folded and unfolded, and
/// a root is no row to drag.
#[test]
fn rows_of_a_rig_drag_from_the_tree() {
    let (mut t, p) = two_lists();
    t.st.apply(Msg::FoldRig(PUPU), &t.keys);
    t.st.apply(Msg::FoldRig(PUPU), &t.keys);
    drop_pupu(&mut t, p);
    let v = *t.st.layout.view(p).expect("pane");
    assert_eq!((v.rig, v.meas), (PaneRig::Rig(PUPU), Some(MeasId(2))));
    assert!(TreeKey::RigName(PUPU).is_heading());
}
