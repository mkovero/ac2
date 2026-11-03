//! The measurement list as a tree rooted at each connected rig: the rig's heading, and under
//! it everything that lives on that rig (its Rig monitor meters, its session's measurements
//! with their rows, its imported traces). A rig's branch folds as a measurement's group
//! does; folded, its heading says what it hides.

use crate::meas_list::{TreeKey, TreeRow, heading_row};
use ac2_proto::units::RigId;

/// One rig's root in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RigRoot<'a> {
    pub rig: RigId,
    /// As the drop-down names it: `pupu`, `ketunkolo · ac2d`, `ketunkolo · in this app`.
    pub name: &'a str,
    /// The rig the drop-down shows, where new measurements and the keys go.
    pub shown: bool,
    /// Its branch folded: only the heading is listed.
    pub folded: bool,
    /// Every measurement on the rig, its meters and math channels included.
    pub measurements: u32,
    /// Every stored trace on the rig.
    pub traces: u32,
}

/// Rig `root`'s heading, with its fold arrow.
pub fn rig_heading(root: &RigRoot<'_>) -> TreeRow {
    let name = root.name;
    let (mut details, describe) = if root.folded {
        // The counts read as the Sessions dialog says a session's.
        let all = crate::sessions::counts(root.measurements, root.traces);
        let meas = all.split(" · ").next().unwrap_or_default().to_owned();
        let whose = if root.shown {
            "the rig shown"
        } else {
            "another rig"
        };
        (
            vec![format!("{all} · folded"), format!("{meas} · folded")],
            format!("{name}: {whose}, folded: {all} under it"),
        )
    } else if root.shown {
        (
            vec!["shown: new measurements and the keys go here".to_owned()],
            format!("{name}: the rig shown; new measurements and the keys go to it"),
        )
    } else {
        (
            vec!["click shows this rig; its rows drag onto panes".to_owned()],
            format!("{name}: another rig; a click shows it, its rows drag onto panes as they are"),
        )
    };
    details.dedup();
    let mut row = heading_row(TreeKey::RigName(root.rig), name, details, describe);
    row.collapsed = Some(root.folded);
    row
}

/// Rig `root`'s branch of the list: its heading, then (unfolded) its rows `rows`
/// ([`crate::meas_list::tree_rows`]) a level under it.
pub fn rig_branch(root: &RigRoot<'_>, rows: Vec<TreeRow>) -> Vec<TreeRow> {
    let mut v = vec![rig_heading(root)];
    if !root.folded {
        v.extend(rows.into_iter().map(|mut r| {
            r.depth += 1;
            r
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meas_list::tests::{rig, rows};
    use std::collections::BTreeSet;

    /// The rows as the list reads, each indented two spaces a level, a row under a header
    /// with its tree line.
    fn text(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let fold = match r.collapsed {
                    Some(true) => "▸ ",
                    Some(false) => "▾ ",
                    None if r.key.is_heading() => "  ",
                    None if r.last => "└ ",
                    None => "├ ",
                };
                let pad = "  ".repeat(usize::from(r.depth));
                format!("{pad}{fold}{} · {}", r.name, r.details[0])
            })
            .collect()
    }

    fn root(name: &str, shown: bool, folded: bool) -> RigRoot<'_> {
        RigRoot {
            rig: RigId(if shown { 1 } else { 2 }),
            name,
            shown,
            folded,
            measurements: 4,
            traces: 5,
        }
    }

    /// Two rigs: each is a root over its own rows, a level in; the folded one lists only its
    /// heading, saying how many measurements and traces it hides.
    #[test]
    fn each_rig_is_a_root_and_a_folded_one_says_what_it_hides() {
        let r = rig();
        let none = BTreeSet::new();
        let mut list = rig_branch(
            &root("ketunkolo · ac2d", true, false),
            rows(&r, &none, None, None),
        );
        list.extend(rig_branch(
            &root("pupu", false, true),
            rows(&r, &none, None, None),
        ));
        assert_eq!(
            text(&list),
            [
                "▾ ketunkolo · ac2d · shown: new measurements and the keys go here",
                "  ▾ TF  Main L · running",
                "    ├ Main L (live) · live",
                "    ├ pre-EQ · capture",
                "    ├ post-EQ · capture",
                "    └ pre ÷ post · math (live)",
                "  ▾ SWEEP  Genelec 1 m · 2 runs",
                "    ├ Run 1 · sweep run · 3 s −50.0 dBFS",
                "    └ Run 2 · sweep run · 3 s −50.0 dBFS",
                "  ▾ SPL  FOH SPL · running",
                "  ▾ Imported · 1 trace",
                "    └ 1083 94cm · imported",
                "▸ pupu · 4 measurements · 5 traces · folded",
            ]
        );
        let pupu = list.last().expect("pupu");
        assert_eq!(pupu.key, TreeKey::RigName(RigId(2)));
        assert_eq!(
            pupu.details.last().map(String::as_str),
            Some("4 measurements · folded")
        );
        assert_eq!(
            pupu.describe,
            "pupu: another rig, folded: 4 measurements · 5 traces under it"
        );
        assert!(pupu.key.is_heading(), "a root is no curve: not dragged");
    }

    /// One of each: the counts read as the Sessions dialog's.
    #[test]
    fn a_folded_rig_counts_in_the_singular() {
        let one = RigRoot {
            measurements: 1,
            traces: 1,
            ..root("pupu", true, true)
        };
        assert_eq!(
            rig_heading(&one).details,
            ["1 measurement · 1 trace · folded", "1 measurement · folded"]
        );
        assert_eq!(
            rig_heading(&one).describe,
            "pupu: the rig shown, folded: 1 measurement · 1 trace under it"
        );
    }
}
