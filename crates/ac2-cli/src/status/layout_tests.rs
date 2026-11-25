use super::*;

fn sec(heading: &str, summary: Option<Line>, rows: Vec<Row>) -> Section {
    Section {
        heading: heading.into(),
        summary,
        rows,
    }
}

#[test]
fn cells_align_in_columns_and_continuations_sit_under_the_value() {
    let s = sec(
        "AUDIO",
        Some(Line::plain("open · jack")),
        vec![
            Row::new(["in", "1–8"]),
            Row::new(["clock", "one clock"]).more(Line::plain("system clock synced")),
        ],
    );
    assert_eq!(
        render(&[s], 100),
        "AUDIO     open · jack\n\
         \x20 in      1–8\n\
         \x20 clock   one clock\n\
         \x20         system clock synced"
    );
}

#[test]
fn a_long_heading_pushes_the_value_column() {
    let s = sec(
        "ac2 on a long host",
        Some(Line::plain("ac2d")),
        vec![Row::new(["id", "7"])],
    );
    assert_eq!(
        render(&[s], 100),
        "ac2 on a long host   ac2d\n  id                 7"
    );
}

#[test]
fn trouble_starts_with_a_bang_in_column_0() {
    let s = sec(
        "RIGS",
        None,
        vec![
            Row::new(["ketunkolo", "CLOCKS NOT LOCKED +7.8 ppm", "link up"])
                .trouble(true)
                .more(Line::plain("in 1 streaming"))
                .more(Line::trouble("in 2 no audio", true)),
            Row::new(["kolo", "clocks agree", "link up"]),
        ],
    );
    let t = render(&[s], 100);
    assert_eq!(
        t,
        "RIGS\n\
         ! ketunkolo   CLOCKS NOT LOCKED +7.8 ppm   link up\n\
         \x20             in 1 streaming\n\
         !             in 2 no audio\n\
         \x20 kolo        clocks agree                 link up"
    );
}

#[test]
fn a_troubled_summary_goes_under_its_heading() {
    let s = sec(
        "AUTOSAVE",
        Some(Line::trouble("FAILED: disk full", true)),
        vec![],
    );
    assert_eq!(render(&[s], 100), "AUTOSAVE\n!          FAILED: disk full");
}

#[test]
fn empty_sections_are_left_out_and_others_are_spaced() {
    let t = render(
        &[
            sec("A", Some(Line::plain("x")), vec![]),
            sec("RIGS", None, vec![]),
            sec("B", Some(Line::plain("y")), vec![]),
        ],
        100,
    );
    assert_eq!(t, "A   x\n\nB   y");
}

#[test]
fn a_wide_line_breaks_at_its_separators() {
    let long = ["alpha"; 30].join(" · ");
    let s = sec(
        "X",
        None,
        vec![Row::new(["label", long.as_str()]).trouble(true)],
    );
    let t = render(&[s], 60);
    for l in t.lines() {
        assert!(l.chars().count() <= 60, "{l}");
    }
    let lines: Vec<&str> = t.lines().collect();
    assert!(lines[1].starts_with("! label   alpha · "), "{t}");
    assert!(lines[2].starts_with("!         alpha"), "{t}");
    assert_eq!(t.matches("alpha").count(), 30);
}
