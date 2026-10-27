use super::*;

fn inputs() -> Vec<(u16, String)> {
    [
        "MM1 (in 1)",
        "Loop (in 2)",
        "Main L (in 3)",
        "Main R (in 4)",
    ]
    .iter()
    .enumerate()
    .map(|(c, n)| (c as u16, (*n).to_owned()))
    .collect()
}

fn main_pair() -> Vec<((u16, u16), String)> {
    vec![((2, 3), "Main · line · pair (in 3, 4)".to_owned())]
}

/// Each field as `label: value`, a heading as `## heading`.
fn rows(f: &Form) -> Vec<String> {
    let mut v = Vec::new();
    for x in &f.fields {
        if let Some(h) = f.heading(x.id) {
            v.push(format!("## {h}"));
        }
        v.push(format!("{}: {}", x.label, x.display()));
    }
    v
}

fn focus(f: &mut Form, id: FieldId) {
    let i = f.fields.iter().position(|x| x.id == id).expect("field");
    f.focus_field(i);
}

const RIG_ROWS: [&str; 6] = [
    "## How it reads · Whose: the rig — all clients",
    "Programme meters read: true peak",
    "Loudness target: −23 LUFS",
    "Correlation window: 1 s",
    "An over is above: 0.0 dBTP",
    "Correlation gate: −60 dBFS",
];

#[test]
fn a_new_meter_starts_on_the_first_pair_and_shows_the_rigs_settings() {
    let f = form(main_pair(), inputs(), None, LevelsSettings::default(), None);
    assert_eq!(f.kind, FormKind::Programme);
    assert_eq!(f.kind.title(), "New programme meter");
    let mut want = vec![
        "## What to meter".to_owned(),
        "Meter: Main · line · pair (in 3, 4)".to_owned(),
    ];
    want.extend(RIG_ROWS.map(str::to_owned));
    assert_eq!(rows(&f), want);
    // The pair's meters are drawn beside it, both sides.
    assert_eq!(f.field_meters(0), vec![2, 3]);
    assert_eq!(metered(&f), Ok(Metered::Pair { left: 2, right: 3 }));
    assert_eq!(
        f.keys_hint(),
        "Enter opens it beside the focused one: meters, and a goniometer pane for a pair · ↑↓ field · ←→ choose · Esc closes"
    );
}

#[test]
fn inputs_are_picked_as_bars_by_name_each_once() {
    let mut f = form(main_pair(), inputs(), None, LevelsSettings::default(), None);
    // From the pair to inputs: its sides are the first two bars, a third waits empty.
    f.cycle(1);
    assert_eq!(
        rows(&f)[..5],
        [
            "## What to meter",
            "Meter: inputs side by side",
            "Input 1: Main L (in 3)",
            "Input 2: Main R (in 4)",
            "Input 3: none",
        ]
    );
    // An empty bar takes the first input no other bar meters; the next empty one follows.
    focus(&mut f, FieldId::Bar(2));
    f.cycle(1);
    assert_eq!(rows(&f)[4..6], ["Input 3: MM1 (in 1)", "Input 4: none"]);
    f.cycle(1);
    assert_eq!(
        rows(&f)[4],
        "Input 3: Loop (in 2)",
        "in 3 and in 4 are taken"
    );
    f.cycle(1);
    assert_eq!(rows(&f)[4], "Input 3: Loop (in 2)", "the last free input");
    assert_eq!(
        metered(&f),
        Ok(Metered::inputs_of(&[2, 3, 1]).expect("three inputs"))
    );
    // ← past the first free input: the bar meters nothing, and the empty bar after it goes.
    f.cycle(-1);
    f.cycle(-1);
    assert_eq!(rows(&f)[4], "Input 3: none");
    assert_eq!(rows(&f)[5], RIG_ROWS[0]);
    assert_eq!(f.fields[f.focus].id, FieldId::Bar(2));
    // Bar 1 always meters an input.
    focus(&mut f, FieldId::Bar(0));
    for _ in 0..5 {
        f.cycle(-1);
    }
    assert_eq!(rows(&f)[2], "Input 1: MM1 (in 1)");
    // Back to the pair: no bars.
    focus(&mut f, FieldId::Metered);
    f.cycle(-1);
    assert_eq!(
        rows(&f)[1..3],
        ["Meter: Main · line · pair (in 3, 4)", RIG_ROWS[0]]
    );
}

#[test]
fn eight_bars_at_most() {
    let many: Vec<(u16, String)> = (0..10).map(|c| (c, format!("in {}", c + 1))).collect();
    let all = Metered::inputs_of(&[0, 1, 2, 3, 4, 5, 6, 7]).expect("eight");
    let f = form(Vec::new(), many, Some(all), LevelsSettings::default(), None);
    let bars = f
        .fields
        .iter()
        .filter(|x| matches!(x.id, FieldId::Bar(_)))
        .count();
    assert_eq!(bars, 8, "no ninth, empty bar");
    assert_eq!(metered(&f), Ok(all));
}

#[test]
fn without_pairs_there_is_no_meter_field() {
    let f = form(Vec::new(), inputs(), None, LevelsSettings::default(), None);
    assert_eq!(
        rows(&f)[..3],
        ["## What to meter", "Input 1: MM1 (in 1)", "Input 2: none"]
    );
    assert_eq!(f.field_meters(0), vec![0]);
}

#[test]
fn an_edit_shows_what_the_pane_meters_and_the_rigs_odd_values() {
    let rig = LevelsSettings {
        ballistic: Ballistic::Type2,
        correlation_window: CorrelationWindow::Short,
        over_threshold: Dbtp(-1.3),
        correlation_gate: Dbfs(-70.0),
        loudness_target: ac2_proto::units::Lufs(-23.0),
    };
    let m = Metered::inputs_of(&[3, 0]).expect("two");
    let f = form(main_pair(), inputs(), Some(m), rig, Some(PaneId(4)));
    assert_eq!(f.kind.title(), "Edit programme meter");
    assert_eq!(f.kind.verb(), "Apply");
    assert_eq!(
        rows(&f),
        [
            "## What to meter",
            "Meter: inputs side by side",
            "Input 1: Main R (in 4)",
            "Input 2: MM1 (in 1)",
            "Input 3: none",
            RIG_ROWS[0],
            "Programme meters read: Type II",
            "Loudness target: −23 LUFS",
            "Correlation window: 0.3 s",
            "An over is above: −1.3 dBTP",
            "Correlation gate: −70 dBFS",
        ]
    );
    assert_eq!(levels(&f), Some(rig));
    // A pane of the pair opens on it.
    let p = form(
        main_pair(),
        inputs(),
        Some(Metered::Pair { left: 2, right: 3 }),
        rig,
        Some(PaneId(4)),
    );
    assert_eq!(metered(&p), Ok(Metered::Pair { left: 2, right: 3 }));
}

#[test]
fn only_what_was_changed_here_is_asked_of_the_rig() {
    let opened = LevelsSettings::default();
    let shown = LevelsSettings {
        ballistic: Ballistic::Vu,
        ..opened
    };
    // Another client moved the gate meanwhile: it stays.
    let now = LevelsSettings {
        correlation_gate: Dbfs(-50.0),
        ..opened
    };
    assert_eq!(
        wanted(opened, shown, now),
        LevelsSettings {
            ballistic: Ballistic::Vu,
            correlation_gate: Dbfs(-50.0),
            ..opened
        }
    );
    assert_eq!(wanted(opened, opened, now), now);
}

#[test]
fn nothing_metered_is_said() {
    let f = form(
        Vec::new(),
        Vec::new(),
        None,
        LevelsSettings::default(),
        None,
    );
    assert_eq!(metered(&f), Err("choose input 1".to_owned()));
}
