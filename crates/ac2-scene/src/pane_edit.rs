//! What pane edits read as: the label that follows a list row dragged onto the panes, and
//! the toasts of undo and redo naming the edit they took back or did again. Panes are named
//! by their number in reading order, as each pane's title numbers it.

/// The side of a pane a row is dropped near: the pane splits there, the new half on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    /// Where the new pane goes, as the drag label says it: `new pane below`.
    fn new_pane(self) -> &'static str {
        match self {
            Edge::Top => "new pane above",
            Edge::Bottom => "new pane below",
            Edge::Left => "new pane on the left",
            Edge::Right => "new pane on the right",
        }
    }
}

/// Where a dragged row lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropSpot {
    /// Pane `n` (reading order, from 1) shows it.
    Pane(usize),
    /// A pane split off on that side shows it.
    NewPane(Edge),
}

/// The label beside the pointer while a row is dragged, and the name of the drop after:
/// `FOH SPL → pane 2`, `FOH SPL → new pane below`. `becomes`: the kind of pane (its title)
/// pane `n` turns into to draw the row, said before release because the drop changes it:
/// `FOH SPL → pane 2, becomes SPL`.
pub fn drop_label(item: &str, spot: DropSpot, becomes: Option<&str>) -> String {
    let to = match spot {
        DropSpot::Pane(n) => format!("{item} \u{2192} pane {n}"),
        DropSpot::NewPane(e) => format!("{item} \u{2192} {}", e.new_pane()),
    };
    match becomes {
        Some(k) => format!("{to}, becomes {k}"),
        None => to,
    }
}

/// What a pane with nothing picked says: it follows no selection, a row dropped on it is
/// what it shows.
pub const NOTHING_PICKED: &str = "nothing picked \u{2014} drag a row of the list here";

/// What an SPL command says when the SPL pane it acts on has nothing picked.
pub const SPL_NOTHING_PICKED: &str = "nothing picked in the SPL pane \u{2014} drag a meter onto it";

/// What an SPL command says when no SPL pane is on screen: it acts on the meter a pane
/// shows.
pub const NO_SPL_PANE: &str = "no SPL pane shows a meter \u{2014} drag one onto a pane";

/// What a pane says after its rig deleted what it showed: the name, until the pane is
/// given something else.
pub fn deleted_here(name: &str) -> String {
    format!("{name} deleted \u{2014} drag a row here")
}

/// What a pane says while what it was given is not listed by its rig yet (`what`: its
/// name when known, else `measurement 4`).
pub fn waiting_for(what: &str, rig: &str) -> String {
    format!("{what} \u{b7} waiting for {rig}")
}

/// E on a Programme pane: what it meters is what was dropped on it.
pub const PROGRAMME_EDIT: &str =
    "a Programme pane meters the Inputs row dropped on it: drag an input or a pair here";

/// A new measurement made with no room for a pane of its own: `drag TF 3 onto one`.
pub fn no_room_for_pane(name: &str) -> String {
    format!("no room for a pane: drag {name} onto one")
}

/// A command about a kind of pane none on screen is: `no SPL pane: drag FOH SPL onto one`,
/// or with nothing of that kind selected, which kind to select.
pub fn no_pane_of(kind: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("no {kind} pane: drag {n} onto one"),
        None => format!("no {kind} pane: drag a {kind} measurement onto one"),
    }
}

/// A row dropped onto a row of another rig: math combines one rig's measurements.
pub fn cross_rig_math_refused(rig: &str) -> String {
    format!("not combined: the two rows are of different rigs; math uses one rig's ({rig})")
}

/// How W or F left the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenStep {
    /// Every pane of the layout.
    AllPanes,
    /// Pane `n` alone.
    Alone(usize),
    /// Pane `n` alone, full screen.
    FullScreen(usize),
    /// The window full screen.
    Window,
    /// The panes alone, full screen.
    PanesOnly,
    /// The window as a window.
    Windowed,
}

/// One edit of the panes that undo takes back (and redo does again).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneEdit {
    /// A row dropped on the panes, or a row's own panes brought: `item` went to `spot`.
    Put { item: String, spot: DropSpot },
    /// N: pane `n` split.
    Split(usize),
    /// Shift+N: the split of pane `n` turned.
    Turn(usize),
    /// Q: pane `n` closed.
    Close(usize),
    /// A split dragged, or Alt+←/→: the panes resized.
    Resize,
    /// The transfer stack laid out in pane `n`.
    Stack(usize),
    /// G / Shift+G (or a view key): pane `n`'s view.
    View(usize),
    /// T: pane `n`'s grid and labels.
    Chrome(usize),
    /// A plate over pane `n`'s plot dragged, scaled or put back.
    Plates(usize),
    /// W or F.
    Screen(ScreenStep),
}

impl PaneEdit {
    /// The edit in a few words: `FOH SPL → pane 2`, `split pane 1`.
    pub fn describe(&self) -> String {
        match self {
            PaneEdit::Put { item, spot } => drop_label(item, *spot, None),
            PaneEdit::Split(n) => format!("split pane {n}"),
            PaneEdit::Turn(n) => format!("turn the split of pane {n}"),
            PaneEdit::Close(n) => format!("close pane {n}"),
            PaneEdit::Resize => "resize the panes".into(),
            PaneEdit::Stack(n) => format!("transfer stack in pane {n}"),
            PaneEdit::View(n) => format!("view of pane {n}"),
            PaneEdit::Chrome(n) => format!("grid and labels of pane {n}"),
            PaneEdit::Plates(n) => format!("plates of pane {n}"),
            PaneEdit::Screen(s) => match s {
                ScreenStep::AllPanes => "all panes".into(),
                ScreenStep::Alone(n) => format!("pane {n} alone"),
                ScreenStep::FullScreen(n) => format!("pane {n} full screen"),
                ScreenStep::Window => "window full screen".into(),
                ScreenStep::PanesOnly => "panes only, full screen".into(),
                ScreenStep::Windowed => "windowed".into(),
            },
        }
    }
}

/// The toast of an undo: `Undid: FOH SPL → pane 2`.
pub fn undid(e: &PaneEdit) -> String {
    format!("Undid: {}", e.describe())
}

/// The toast of a redo: `Redid: split pane 1`.
pub fn redid(e: &PaneEdit) -> String {
    format!("Redid: {}", e.describe())
}

pub const NOTHING_TO_UNDO: &str = "Nothing to undo";
pub const NOTHING_TO_REDO: &str = "Nothing to redo";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_labels_name_the_item_and_where_it_lands() {
        assert_eq!(
            drop_label("FOH SPL", DropSpot::Pane(2), None),
            "FOH SPL \u{2192} pane 2"
        );
        assert_eq!(
            drop_label("FOH SPL", DropSpot::NewPane(Edge::Bottom), None),
            "FOH SPL \u{2192} new pane below"
        );
        assert_eq!(
            drop_label("FOH SPL", DropSpot::NewPane(Edge::Top), None),
            "FOH SPL \u{2192} new pane above"
        );
        assert_eq!(
            drop_label("Main L", DropSpot::NewPane(Edge::Left), None),
            "Main L \u{2192} new pane on the left"
        );
        assert_eq!(
            drop_label("FOH SPL", DropSpot::Pane(2), Some("SPL")),
            "FOH SPL \u{2192} pane 2, becomes SPL"
        );
    }

    #[test]
    fn undo_and_redo_toasts_name_the_edit() {
        let put = PaneEdit::Put {
            item: "FOH SPL".into(),
            spot: DropSpot::Pane(2),
        };
        assert_eq!(undid(&put), "Undid: FOH SPL \u{2192} pane 2");
        assert_eq!(redid(&PaneEdit::Split(1)), "Redid: split pane 1");
        assert_eq!(undid(&PaneEdit::Close(3)), "Undid: close pane 3");
        assert_eq!(
            redid(&PaneEdit::Chrome(2)),
            "Redid: grid and labels of pane 2"
        );
        assert_eq!(
            undid(&PaneEdit::Screen(ScreenStep::Alone(2))),
            "Undid: pane 2 alone"
        );
        assert_eq!(undid(&PaneEdit::Resize), "Undid: resize the panes");
        assert_eq!(NOTHING_TO_UNDO, "Nothing to undo");
    }

    #[test]
    fn a_pane_says_what_it_lost_or_waits_for() {
        assert_eq!(
            deleted_here("FOH SPL"),
            "FOH SPL deleted \u{2014} drag a row here"
        );
        assert_eq!(waiting_for("TF 1", "pupu"), "TF 1 \u{b7} waiting for pupu");
    }

    #[test]
    fn an_empty_pane_says_how_it_gets_something() {
        assert_eq!(
            NOTHING_PICKED,
            "nothing picked \u{2014} drag a row of the list here"
        );
        assert_eq!(
            no_pane_of("SPL", Some("FOH SPL")),
            "no SPL pane: drag FOH SPL onto one"
        );
        assert_eq!(
            no_room_for_pane("TF 3"),
            "no room for a pane: drag TF 3 onto one"
        );
    }
}
