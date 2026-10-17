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
    /// Drawn compared over pane `n`, which keeps what it shows.
    Compare(usize),
}

/// The label beside the pointer while a row is dragged, and the name of the drop after:
/// `FOH SPL → pane 2`, `FOH SPL → new pane below`, `FOH SPL → compare in pane 2`.
pub fn drop_label(item: &str, spot: DropSpot) -> String {
    match spot {
        DropSpot::Pane(n) => format!("{item} \u{2192} pane {n}"),
        DropSpot::NewPane(e) => format!("{item} \u{2192} {}", e.new_pane()),
        DropSpot::Compare(n) => format!("{item} \u{2192} compare in pane {n}"),
    }
}

/// Why a row dropped with Shift is not compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareRefusal {
    /// Pane `pane` is an SPL pane: a meter is a number, not a curve to overlay; two meters
    /// are compared side by side, each on a pane of its own.
    MeterPane { pane: usize },
    /// The item has no curve of the kind the pane draws (`what`: `transfer`, `spectrum or
    /// RTA`, `sweep`).
    NoCurve { what: &'static str },
    /// The item is a stored trace under Imported: it belongs to no measurement, so no pane
    /// draws it until it is moved under one.
    Imported,
    /// The item is of the measurement the pane shows: drawn there anyway.
    OwnGroup,
}

/// The toast of a refused Shift-drop: `FOH SPL not compared: pane 2 shows SPL; meters are
/// compared side by side — drop it without Shift on a pane of its own`.
pub fn compare_refused(item: &str, why: CompareRefusal) -> String {
    match why {
        CompareRefusal::MeterPane { pane } => format!(
            "{item} not compared: pane {pane} shows SPL; meters are compared side by side — drop it without Shift on a pane of its own"
        ),
        CompareRefusal::NoCurve { what } => {
            format!("{item} not compared: it has no {what} curve to draw over the pane")
        }
        CompareRefusal::Imported => {
            format!("{item} not compared: it is under Imported — Move to measurement… first")
        }
        CompareRefusal::OwnGroup => {
            format!("{item} not compared: the pane draws it already, as its own")
        }
    }
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
    /// A row dropped on the panes, a chip or row pick: `item` went to `spot`.
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
    /// W or F.
    Screen(ScreenStep),
}

impl PaneEdit {
    /// The edit in a few words: `FOH SPL → pane 2`, `split pane 1`.
    pub fn describe(&self) -> String {
        match self {
            PaneEdit::Put { item, spot } => drop_label(item, *spot),
            PaneEdit::Split(n) => format!("split pane {n}"),
            PaneEdit::Turn(n) => format!("turn the split of pane {n}"),
            PaneEdit::Close(n) => format!("close pane {n}"),
            PaneEdit::Resize => "resize the panes".into(),
            PaneEdit::Stack(n) => format!("transfer stack in pane {n}"),
            PaneEdit::View(n) => format!("view of pane {n}"),
            PaneEdit::Chrome(n) => format!("grid and labels of pane {n}"),
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
            drop_label("FOH SPL", DropSpot::Pane(2)),
            "FOH SPL \u{2192} pane 2"
        );
        assert_eq!(
            drop_label("FOH SPL", DropSpot::NewPane(Edge::Bottom)),
            "FOH SPL \u{2192} new pane below"
        );
        assert_eq!(
            drop_label("FOH SPL", DropSpot::NewPane(Edge::Top)),
            "FOH SPL \u{2192} new pane above"
        );
        assert_eq!(
            drop_label("Main L", DropSpot::NewPane(Edge::Left)),
            "Main L \u{2192} new pane on the left"
        );
        assert_eq!(
            drop_label("FOH SPL", DropSpot::Compare(2)),
            "FOH SPL \u{2192} compare in pane 2"
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
    fn a_refused_compare_says_why() {
        assert_eq!(
            compare_refused("FOH SPL", CompareRefusal::MeterPane { pane: 2 }),
            "FOH SPL not compared: pane 2 shows SPL; meters are compared side by side — drop it without Shift on a pane of its own"
        );
        assert_eq!(
            compare_refused("FOH SPL", CompareRefusal::NoCurve { what: "transfer" }),
            "FOH SPL not compared: it has no transfer curve to draw over the pane"
        );
        assert_eq!(
            compare_refused("1083 94cm", CompareRefusal::Imported),
            "1083 94cm not compared: it is under Imported — Move to measurement… first"
        );
    }
}
