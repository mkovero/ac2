//! Helpers shared by the app's integration tests and `ac2-e2e` (which includes this file):
//! pane layouts as an operator would have saved them, and queries of what the app shows.
#![allow(dead_code)]

fn ir_view() -> ac2_ui::state::View {
    let mut v = ac2_ui::state::View::of(ac2_ui::state::PaneKind::Transfer);
    v.modes.transfer = ac2_ui::state::TransferView::Ir(ac2_scene::view::IrMode::Linear);
    v
}

/// The four panes most tests drive, as an operator would have saved them: transfer across
/// the top, spectrum, a transfer pane in its impulse-response view and SPL side by side
/// below it (Alt+1 … 4 in that order), the transfer pane focused.
pub fn grid_prefs() -> ac2_ui::prefs::PanesPrefs {
    use ac2_ui::state::{PaneKind, View};
    grid_of(&[
        View::of(PaneKind::Spectrum),
        ir_view(),
        View::of(PaneKind::Spl),
    ])
}

/// [`grid_prefs`] with the sweep / distortion pane last in the bottom row (Alt+5).
pub fn grid_with_sweep_prefs() -> ac2_ui::prefs::PanesPrefs {
    use ac2_ui::state::{PaneKind, View};
    grid_of(&[
        View::of(PaneKind::Spectrum),
        ir_view(),
        View::of(PaneKind::Spl),
        View::of(PaneKind::Distortion),
    ])
}

/// Transfer across the top, `row` side by side below it in equal widths.
fn grid_of(row: &[ac2_ui::state::View]) -> ac2_ui::prefs::PanesPrefs {
    use ac2_ui::state::{Axis, PaneId, PaneKind, PaneNode, View};
    fn bottom(first: u32, n: usize) -> PaneNode {
        if n == 1 {
            return PaneNode::Leaf(PaneId(first));
        }
        PaneNode::Split {
            axis: Axis::Row,
            ratio: 1.0 / n as f32,
            a: Box::new(PaneNode::Leaf(PaneId(first))),
            b: Box::new(bottom(first + 1, n - 1)),
        }
    }
    let root = PaneNode::Split {
        axis: Axis::Column,
        ratio: 0.62,
        a: Box::new(PaneNode::Leaf(PaneId(1))),
        b: Box::new(bottom(2, row.len())),
    };
    let views = std::iter::once(View::of(PaneKind::Transfer))
        .chain(row.iter().copied())
        .zip(1..)
        .map(|(v, n)| ac2_ui::prefs::PanePrefs {
            id: PaneId(n),
            kind: v.kind,
            measurement: None,
            curves: Vec::new(),
            meters: v.meters,
            rig: None,
            modes: v.modes,
            levels: Default::default(),
            plates: Default::default(),
        })
        .collect();
    ac2_ui::prefs::PanesPrefs {
        root,
        focus: PaneId(1),
        views,
    }
}

/// [`grid_prefs`] in a fresh app's preferences.
/// The level range the spectrum pane the keys mean draws on.
pub fn spectrum_level(s: &ac2_ui::state::AppState) -> ac2_scene::axis::Range {
    let pane = s
        .layout
        .lead(ac2_ui::state::PaneKind::Spectrum)
        .expect("a spectrum pane");
    s.level_range(pane).expect("level axis")
}

pub fn grid_ui_prefs() -> ac2_ui::prefs::UiPrefs {
    let mut p = ac2_ui::prefs::UiPrefs::default();
    p.layout.panes = Some(grid_prefs());
    // The test publishers send a frame every 200 ms: gliding bars and curves would never
    // rest for a snapshot. The glides are tested headless (`ac2_scene::bar_ease`,
    // `ac2_scene::curve_ease`).
    p.bar_ease = false;
    p.spec_ease = false;
    p
}

/// `p` with each pane given what the operator would have dropped on it from the rig's
/// list (Main L, Mic 1 FFT, FOH SPL): its measurement of the pane's kind, by name as the
/// preferences keep it. A pane follows no selection, so a picture of the rig needs its
/// panes filled.
pub fn rig_picks(mut p: ac2_ui::prefs::PanesPrefs) -> ac2_ui::prefs::PanesPrefs {
    use ac2_ui::state::PaneKind;
    for v in &mut p.views {
        v.measurement = match v.kind {
            PaneKind::Transfer => Some("Main L".into()),
            PaneKind::Spectrum => Some("Mic 1 FFT".into()),
            PaneKind::Spl => Some("FOH SPL".into()),
            _ => None,
        };
    }
    p
}

/// [`grid_with_sweep_prefs`] in a fresh app's preferences: a sweep from the dialog lands
/// on its own pane (Alt+5) and the transfer pane stays Alt+1.
pub fn grid_with_sweep_ui_prefs() -> ac2_ui::prefs::UiPrefs {
    let mut p = grid_ui_prefs();
    p.layout.panes = Some(grid_with_sweep_prefs());
    p
}

/// The pane showing `kind` (the one focused last when several do).
pub fn pane(st: &ac2_ui::state::AppState, kind: ac2_ui::state::PaneKind) -> ac2_ui::state::PaneId {
    st.layout
        .lead(kind)
        .unwrap_or_else(|| panic!("no {kind:?} pane"))
}

/// The transfer pane in its impulse-response view (the first in reading order).
pub fn ir_pane(st: &ac2_ui::state::AppState) -> ac2_ui::state::PaneId {
    st.layout
        .panes()
        .into_iter()
        .find(|id| st.layout.view(*id).is_some_and(|v| v.shows_ir()))
        .expect("a transfer pane in its IR view")
}

/// What the panes drawn now show, in reading order.
pub fn visible(st: &ac2_ui::state::AppState) -> Vec<ac2_ui::state::PaneKind> {
    st.visible_panes()
        .into_iter()
        .map(|id| st.layout.kind(id))
        .collect()
}

/// Pane of `kind` set to show measurement `id`, as a pick from its title chip would.
pub fn show_on(
    st: &mut ac2_ui::state::AppState,
    kind: ac2_ui::state::PaneKind,
    id: ac2_proto::units::MeasId,
) {
    let p = pane(st, kind);
    if let Some(v) = st.layout.view_mut(p) {
        v.meas = Some(id);
    }
}

/// The layout `p` describes, as the app builds it from `ui.toml`.
pub fn layout_of(p: &ac2_ui::prefs::PanesPrefs) -> ac2_ui::state::Layout {
    let views = p
        .views
        .iter()
        .map(|v| {
            let mut view = ac2_ui::state::View::of(v.kind);
            view.modes = v.modes;
            (v.id, view)
        })
        .collect();
    ac2_ui::state::Layout::of(p.root.clone(), views, p.focus)
}

/// How many of a scene's texts an assert message shows.
pub const TEXTS_SHOWN: usize = 20;

/// The first `n` texts and how many there are, for an assert message: a whole pane's labels
/// run to a hundred lines and more, printed again on every failing assert.
pub fn scene_texts_excerpt<S: AsRef<str>>(texts: &[S], n: usize) -> String {
    let shown: Vec<&str> = texts.iter().take(n).map(AsRef::as_ref).collect();
    if texts.len() > n {
        format!("{shown:?} … ({n} of {} texts)", texts.len())
    } else {
        format!("{shown:?} ({} texts)", texts.len())
    }
}

/// `with` drawn behind the lead of the first pane of `kind`, as C ticks them: the pane's own
/// entries, kept on this screen.
pub fn draw_in(
    st: &mut ac2_ui::state::AppState,
    kind: ac2_ui::state::PaneKind,
    with: &[ac2_proto::model::layout::EntryRef],
) {
    use ac2_ui::state::{Pick, With};
    let p = pane(st, kind);
    let picks: Vec<Pick> = with.iter().map(|w| Pick::here(*w)).collect();
    st.layout.view_mut(p).expect("the pane").with = With::of(&picks);
}

/// A trace in each slot 1…9 (index 0 = slot 1), of any source curve: the last in the
/// daemon's list when several curves' captures hold the same number. For tests of a rig
/// with one capture per slot; the keys act on the stored traces by source.
pub fn slots(s: &ac2_ui::state::AppState) -> [Option<&ac2_proto::model::TraceMeta>; 9] {
    let mut out = [None; 9];
    for t in s.daemon().map(|d| d.traces.as_slice()).unwrap_or_default() {
        if let Some(n) = t.edit.slot.filter(|n| (1..=9).contains(n)) {
            out[usize::from(n - 1)] = Some(t);
        }
    }
    out
}
