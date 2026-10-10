//! The help overlay: the focused pane's keys, then the keys that work everywhere grouped by
//! what they act on. Other panes' keys show when one of them is focused; commands without a
//! key are in the palette, set-once choices in Settings.

use eframe::egui::{self, Key, RichText};

use super::overlays::{backdrop, card};
use crate::app::App;
use crate::keys::{Chord, CommandId, Keymap, STOP_ANYWHERE, Scope};
use crate::state::HELP_LINE;
use crate::theme::Chrome;

/// One row of the help overlay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HelpRow {
    Header(String),
    Bind { keys: String, title: String },
}

/// Where a key that works in every pane is listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Stimulus,
    Selection,
    View,
    Panes,
    App,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::Stimulus,
        Section::Selection,
        Section::View,
        Section::Panes,
        Section::App,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Stimulus => "Stimulus",
            Section::Selection => "Measurements, traces & slots",
            Section::View => "Zoom, axes & cursor",
            Section::Panes => "Panes & window",
            Section::App => "Dialogs & app",
        }
    }

    fn of(c: CommandId) -> Section {
        use CommandId as C;
        match c {
            C::StimulusArm
            | C::StimulusFire
            | C::StimulusStop
            | C::StopAnywhere
            | C::LevelUp
            | C::LevelDown
            | C::LevelUpCoarse
            | C::LevelDownCoarse
            | C::StimulusLevel
            | C::StimulusOutputs
            | C::StimulusTakeOver => Section::Stimulus,
            C::NextMeasurementInTree
            | C::PrevMeasurementInTree
            | C::PaneMeasurement
            | C::NextTrace
            | C::PrevTrace
            | C::NextAnyTrace
            | C::PrevAnyTrace
            | C::ToggleSelected
            | C::HideGroup
            | C::ToggleGroup
            | C::SelectLive
            | C::DeleteSelected
            | C::TraceRename
            | C::MoveTrace
            | C::TraceSlot
            | C::TraceExport
            | C::Compare
            | C::ClearCompare
            | C::OffsetUp
            | C::OffsetDown
            | C::OffsetUpCoarse
            | C::OffsetDownCoarse
            | C::OffsetClear
            | C::NewTransfer
            | C::NewSpectrum
            | C::NewRta
            | C::NewSpl
            | C::NewMath
            | C::SweepNew
            | C::EditMeas
            | C::Slot1
            | C::Slot2
            | C::Slot3
            | C::Slot4
            | C::Slot5
            | C::Slot6
            | C::Slot7
            | C::Slot8
            | C::Slot9
            | C::ShowSlot1
            | C::ShowSlot2
            | C::ShowSlot3
            | C::ShowSlot4
            | C::ShowSlot5
            | C::ShowSlot6
            | C::ShowSlot7
            | C::ShowSlot8
            | C::ShowSlot9 => Section::Selection,
            C::ZoomIn
            | C::ZoomOut
            | C::PanLeft
            | C::PanRight
            | C::ResetView
            | C::LevelZoomIn
            | C::LevelZoomOut
            | C::LevelPanUp
            | C::LevelPanDown
            | C::LevelFit
            | C::LevelReset
            | C::ToggleCursor
            | C::CursorLeft
            | C::CursorRight => Section::View,
            C::FocusPane1
            | C::FocusPane2
            | C::FocusPane3
            | C::FocusPane4
            | C::FocusPane5
            | C::FocusPane6
            | C::FocusPane7
            | C::FocusPane8
            | C::FocusPane9
            | C::SplitPane
            | C::ClosePane
            | C::TurnSplit
            | C::GrowPane
            | C::ShrinkPane
            | C::MaximizePane
            | C::PlotChrome
            | C::Fullscreen
            | C::CycleTheme => Section::Panes,
            _ => Section::App,
        }
    }
}

/// Runs of nine commands bound to one modifier + digit 1…9, shown as one row.
const DIGIT_GROUPS: [([CommandId; 9], &str); 3] = {
    use CommandId as C;
    [
        (
            [
                C::Slot1,
                C::Slot2,
                C::Slot3,
                C::Slot4,
                C::Slot5,
                C::Slot6,
                C::Slot7,
                C::Slot8,
                C::Slot9,
            ],
            "Capture selected measurement to slot 1…9",
        ),
        (
            [
                C::ShowSlot1,
                C::ShowSlot2,
                C::ShowSlot3,
                C::ShowSlot4,
                C::ShowSlot5,
                C::ShowSlot6,
                C::ShowSlot7,
                C::ShowSlot8,
                C::ShowSlot9,
            ],
            "Show / hide slot 1…9",
        ),
        (
            [
                C::FocusPane1,
                C::FocusPane2,
                C::FocusPane3,
                C::FocusPane4,
                C::FocusPane5,
                C::FocusPane6,
                C::FocusPane7,
                C::FocusPane8,
                C::FocusPane9,
            ],
            "Focus pane 1…9 (in reading order)",
        ),
    ]
};

const DIGITS: [Key; 9] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
];

/// Opposite steps shown on one row (`↑/↓ Stimulus level +1 / −1 dB`) when their keys fit the
/// key column side by side.
const PAIRS: [(CommandId, CommandId, &str); 16] = {
    use CommandId as C;
    [
        (C::LevelUp, C::LevelDown, "Stimulus level +1 / −1 dB"),
        (
            C::LevelUpCoarse,
            C::LevelDownCoarse,
            "Stimulus level +3 / −3 dB",
        ),
        (C::ZoomIn, C::ZoomOut, "Zoom frequency in / out (IR: time)"),
        (
            C::PanLeft,
            C::PanRight,
            "Pan frequency down / up (IR: time)",
        ),
        (
            C::CursorLeft,
            C::CursorRight,
            "Cursor 1/12 octave down / up (IR: a step)",
        ),
        (
            C::NudgeEarlier,
            C::NudgeLater,
            "Delay −0.1 / +0.1 ms of the measurement or the selected stored trace",
        ),
        (
            C::DelayDown,
            C::DelayUp,
            "Delay of the measurement −1 / +1 sample",
        ),
        (
            C::DelayDownFine,
            C::DelayUpFine,
            "Delay of the measurement −0.1 / +0.1 sample",
        ),
        (
            C::InsertDelay,
            C::InsertStrongest,
            "Delay: find and insert first arrival / strongest peak",
        ),
        (
            C::OffsetUp,
            C::OffsetDown,
            "Display offset +1 / −1 dB of the selected curve",
        ),
        (
            C::OffsetUpCoarse,
            C::OffsetDownCoarse,
            "Display offset +3 / −3 dB of the selected curve",
        ),
        (
            C::LevelZoomIn,
            C::LevelZoomOut,
            "Zoom level axis in / out (vertical; IR: amplitude or dB)",
        ),
        (
            C::LevelPanUp,
            C::LevelPanDown,
            "Pan level axis up / down (IR: amplitude or dB)",
        ),
        (
            C::NextTrace,
            C::PrevTrace,
            "Select next / previous shown stored trace (then live)",
        ),
        (
            C::SmoothCoarser,
            C::SmoothFiner,
            "Smoothing coarser / finer (selected trace or pane's measurement)",
        ),
        (
            C::ToggleSelected,
            C::HideGroup,
            "Show / hide the selected curve / its measurement with every trace",
        ),
    ]
};

/// The widest key text the help's key column holds (`Alt+Shift+V`).
const KEY_COLUMN_CHARS: usize = 11;

/// The focused pane's keys under its own header, then every global key under its section.
pub(crate) fn help_rows(keymap: &Keymap, active: Scope) -> Vec<HelpRow> {
    let mut rows = Vec::new();
    if active != Scope::Global {
        let pane = scope_rows(keymap, active, |_| true);
        if !pane.is_empty() {
            rows.push(HelpRow::Header(format!("{} pane", active.title())));
            rows.extend(pane);
        }
    }
    for section in Section::ALL {
        let binds = scope_rows(keymap, Scope::Global, |c| Section::of(c) == section);
        if !binds.is_empty() {
            rows.push(HelpRow::Header(section.title().into()));
            rows.extend(binds);
        }
    }
    rows
}

/// A row per command of `scope` that `keep` takes, in table order, with digit runs and
/// opposite steps folded.
fn scope_rows(keymap: &Keymap, scope: Scope, keep: impl Fn(CommandId) -> bool) -> Vec<HelpRow> {
    let folded: Vec<bool> = DIGIT_GROUPS
        .iter()
        .map(|(cmds, _)| {
            let chords: Vec<Vec<Chord>> = cmds.iter().map(|c| keymap.chords(*c, scope)).collect();
            chords.iter().zip(DIGITS).all(|(v, d)| {
                v.len() == 1 && v[0].key == d && {
                    let first = &chords[0][0];
                    (v[0].command, v[0].alt, v[0].shift) == (first.command, first.alt, first.shift)
                }
            })
        })
        .collect();
    let mut rows = Vec::new();
    for &c in CommandId::ALL.iter().filter(|c| keep(**c)) {
        let chords = keymap.chords(c, scope);
        if chords.is_empty() {
            continue;
        }
        if let Some(g) = DIGIT_GROUPS
            .iter()
            .zip(&folded)
            .position(|((cmds, _), f)| *f && cmds.contains(&c))
        {
            let (cmds, title) = &DIGIT_GROUPS[g];
            if c == cmds[0] {
                rows.push(HelpRow::Bind {
                    keys: format!("{}…9", chords[0].label()),
                    title: (*title).into(),
                });
            }
            continue;
        }
        if let Some((a, title, keys)) = PAIRS
            .iter()
            .filter(|(a, b, _)| *a == c || *b == c)
            .find_map(|(a, b, t)| pair_keys(keymap, scope, *a, *b).map(|k| (*a, t, k)))
        {
            if a == c {
                rows.push(HelpRow::Bind {
                    keys,
                    title: (*title).into(),
                });
            }
            continue;
        }
        // Keys that do not fit the key column side by side go one per line.
        let labels: Vec<String> = chords.iter().map(|k| k.label()).collect();
        let one_line = labels.join(" ");
        let keys = if one_line.chars().count() <= KEY_COLUMN_CHARS {
            one_line
        } else {
            labels.join("\n")
        };
        rows.push(HelpRow::Bind {
            keys,
            title: short_title(c).into(),
        });
    }
    rows
}

/// The palette's title without its closing explanation in parentheses: the help is read at
/// a glance, the palette (same words, in full) when searching.
fn short_title(c: CommandId) -> &'static str {
    let t = c.title();
    match t.rfind(" (") {
        Some(i) if t.ends_with(')') => &t[..i],
        _ => t,
    }
}

/// `Shift+↑/↓` for a pair bound to one key each with the same modifiers, `V/Shift+V` for
/// two that differ, when short enough for the key column; else `None` and the two get a row
/// each.
fn pair_keys(keymap: &Keymap, scope: Scope, a: CommandId, b: CommandId) -> Option<String> {
    let (ka, kb) = (keymap.chords(a, scope), keymap.chords(b, scope));
    let ([x], [y]) = (ka.as_slice(), kb.as_slice()) else {
        return None;
    };
    let text = if (x.command, x.alt, x.shift) == (y.command, y.alt, y.shift) {
        format!("{}/{}", x.label(), Chord::key(y.key).label())
    } else {
        format!("{}/{}", x.label(), y.label())
    };
    (text.chars().count() <= KEY_COLUMN_CHARS).then_some(text)
}

/// About how many lines a row takes in a column: a long title wraps.
fn lines(r: &HelpRow) -> usize {
    match r {
        HelpRow::Header(_) => 2,
        HelpRow::Bind { keys, title } => {
            keys.lines().count().max(title.chars().count().div_ceil(44))
        }
    }
}

/// Splits rows into at most `n` columns of whole sections (a header and its rows), cut where
/// the longest column is shortest.
pub(crate) fn columns(rows: Vec<HelpRow>, n: usize) -> Vec<Vec<HelpRow>> {
    let mut sections: Vec<Vec<HelpRow>> = Vec::new();
    for r in rows {
        match (&r, sections.last_mut()) {
            (HelpRow::Bind { .. }, Some(s)) => s.push(r),
            _ => sections.push(vec![r]),
        }
    }
    let weights: Vec<usize> = sections.iter().map(|s| s.iter().map(lines).sum()).collect();
    // Few sections and columns: try every way of cutting the run into `n` and keep the best.
    let n = n.clamp(1, sections.len().max(1));
    let mut best: (usize, Vec<usize>) = (usize::MAX, Vec::new());
    let mut cuts = vec![0; n - 1];
    fn search(
        w: &[usize],
        from: usize,
        k: usize,
        cuts: &mut Vec<usize>,
        best: &mut (usize, Vec<usize>),
    ) {
        if k == cuts.len() {
            let mut edges = vec![0];
            edges.extend(cuts.iter().copied());
            edges.push(w.len());
            let worst = edges
                .windows(2)
                .map(|e| w[e[0]..e[1]].iter().sum::<usize>())
                .max()
                .unwrap_or(0);
            if worst < best.0 {
                *best = (worst, cuts.clone());
            }
            return;
        }
        for c in from..w.len() {
            cuts[k] = c;
            search(w, c + 1, k + 1, cuts, best);
        }
    }
    search(&weights, 1, 0, &mut cuts, &mut best);
    let mut out: Vec<Vec<HelpRow>> = vec![Vec::new()];
    for (i, s) in sections.into_iter().enumerate() {
        if best.1.contains(&i) {
            out.push(Vec::new());
        }
        if let Some(c) = out.last_mut() {
            c.extend(s);
        }
    }
    out
}

pub(super) fn help(app: &mut App, ctx: &egui::Context, ch: &Chrome) {
    backdrop(ctx);
    let active = app.state.scope();
    let screen = ctx.content_rect();
    let cols = columns(help_rows(&app.keymap, active), 3);
    let first = |c: CommandId| {
        app.keymap
            .chords(c, Scope::Global)
            .first()
            .map(|k| k.label())
            .unwrap_or_default()
    };
    let (help_key, palette_key, settings_key) = (
        first(CommandId::Help),
        first(CommandId::Palette),
        first(CommandId::Settings),
    );
    // Explicit position and width: centring by anchor uses the previous frame's size, so an
    // overlay wider than its first measurement slid off the left edge.
    let width = (screen.width() - 40.0).clamp(320.0, 1200.0);
    let left = screen.center().x - width / 2.0;
    egui::Area::new(egui::Id::new("ac2-help"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(left.max(screen.min.x), screen.min.y + 40.0))
        .show(ctx, |ui| {
            card(ch).show(ui, |ui| {
                ui.set_width(width - 24.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Keys").strong().size(16.0));
                    ui.label(
                        RichText::new(format!(
                            "↑↓ PgUp PgDn scroll · {help_key} or Esc closes · {} stops the \
                             stimulus from any window",
                            STOP_ANYWHERE.label()
                        ))
                        .color(ch.dim),
                    );
                });
                ui.add_space(6.0);
                // The keys scroll it (the window owns ↑/↓), the wheel too; the offset lives
                // in the state so the reducer moves it and the view keeps it in range.
                let out = egui::ScrollArea::vertical()
                    .max_height((screen.height() - 170.0).max(120.0))
                    .auto_shrink([false, true])
                    .vertical_scroll_offset(app.state.help_scroll)
                    .show(ui, |ui| {
                        ui.columns(cols.len().max(1), |uis| {
                            for (ui, col) in uis.iter_mut().zip(&cols) {
                                let key_w = key_width(ui, col);
                                for row in col {
                                    help_row(ui, row, key_w, ch);
                                }
                            }
                        });
                    });
                let max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                app.state.help_scroll = out.state.offset.y.clamp(0.0, max);
                app.state.help_page = (out.inner_rect.height() - HELP_LINE).max(HELP_LINE);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "Another pane's keys show when it is focused · the palette \
                         ({palette_key}) runs every command, with or without a key · \
                         Settings ({settings_key}) › Display has the set-once choices"
                    ))
                    .color(ch.dim),
                );
                let path = app
                    .keymap_path
                    .as_ref()
                    .map_or_else(|| "no config dir".into(), |p| p.display().to_string());
                ui.label(
                    RichText::new(format!("key overrides: {path}"))
                        .small()
                        .color(ch.dim),
                );
            });
        });
}

/// The key column of one screen column: as wide as its widest key line, so titles line up
/// and no key runs into its title.
fn key_width(ui: &egui::Ui, col: &[HelpRow]) -> f32 {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    col.iter()
        .filter_map(|r| match r {
            HelpRow::Bind { keys, .. } => Some(keys.lines()),
            HelpRow::Header(_) => None,
        })
        .flatten()
        .map(|k| {
            ui.fonts_mut(|f| f.layout_no_wrap(k.to_owned(), font.clone(), egui::Color32::WHITE))
                .size()
                .x
        })
        .fold(60.0, f32::max)
        + 12.0
}

fn help_row(ui: &mut egui::Ui, row: &HelpRow, key_w: f32, ch: &Chrome) {
    match row {
        HelpRow::Header(title) => {
            ui.add_space(6.0);
            ui.label(RichText::new(title).strong().color(ch.text));
        }
        HelpRow::Bind { keys, title } => {
            ui.horizontal(|ui| {
                // Fixed key column so titles line up.
                let lines = keys.lines().count().max(1) as f32;
                let h = ui.text_style_height(&egui::TextStyle::Body) * lines;
                let (r, _) = ui.allocate_exact_size(egui::vec2(key_w, h), egui::Sense::hover());
                ui.painter().text(
                    r.left_center(),
                    egui::Align2::LEFT_CENTER,
                    keys,
                    egui::TextStyle::Monospace.resolve(ui.style()),
                    ch.focus,
                );
                // Wrap within the column so a long title never widens the overlay.
                ui.add(egui::Label::new(RichText::new(title).color(ch.text)).wrap());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binds(rows: &[HelpRow]) -> Vec<(&str, &str)> {
        rows.iter()
            .filter_map(|r| match r {
                HelpRow::Bind { keys, title } => Some((keys.as_str(), title.as_str())),
                HelpRow::Header(_) => None,
            })
            .collect()
    }

    fn headers(rows: &[HelpRow]) -> Vec<&str> {
        rows.iter()
            .filter_map(|r| match r {
                HelpRow::Header(t) => Some(t.as_str()),
                HelpRow::Bind { .. } => None,
            })
            .collect()
    }

    #[test]
    fn focused_pane_first_then_global_sections() {
        crate::keys::set_label_style(crate::keys::LabelStyle::Pc);
        let rows = help_rows(&Keymap::default(), Scope::Transfer);
        assert_eq!(
            headers(&rows),
            [
                "Transfer function pane",
                "Stimulus",
                "Measurements, traces & slots",
                "Zoom, axes & cursor",
                "Panes & window",
                "Dialogs & app",
            ]
        );
        let b = binds(&rows);
        // Only this pane's and the global keys: the spectrum's peak hold is not listed.
        assert!(!b.iter().any(|(_, t)| *t == CommandId::PeakHold.title()));
        assert!(b.contains(&("↑/↓", "Stimulus level +1 / −1 dB")));
        assert!(b.contains(&(
            "X/Shift+X",
            "Delay: find and insert first arrival / strongest peak"
        )));
        assert!(b.contains(&("Ctrl+1…9", "Capture selected measurement to slot 1…9")));
        assert!(b.contains(&("1…9", "Show / hide slot 1…9")));
        assert!(b.contains(&("Alt+1…9", "Focus pane 1…9 (in reading order)")));
        assert!(b.contains(&(
            "V/Shift+V",
            "Select next / previous shown stored trace (then live)"
        )));
        // Too wide side by side: a row each, one key per line when they do not fit.
        assert!(
            b.iter()
                .any(|(_, t)| *t == CommandId::OffsetUpCoarse.title())
        );
        assert!(b.contains(&("Delete\nBackspace", CommandId::DeleteSelected.title())));
        // The explanation in parentheses stays in the palette.
        assert!(b.contains(&("Space", "Stimulus: arm what the view plays")));
    }

    #[test]
    fn every_listed_binding_once() {
        crate::keys::set_label_style(crate::keys::LabelStyle::Pc);
        let k = Keymap::default();
        for scope in Scope::ALL {
            let rows = help_rows(&k, scope);
            // Every bound command of the scope and the global table is on some row: its own,
            // its digit run's or its pair's.
            for b in k.bindings() {
                if b.scope != scope && b.scope != Scope::Global {
                    continue;
                }
                let c = b.command;
                let listed = binds(&rows).iter().any(|(_, t)| {
                    *t == short_title(c)
                        || DIGIT_GROUPS
                            .iter()
                            .any(|(cmds, g)| cmds.contains(&c) && t == g)
                        || PAIRS.iter().any(|(a, z, p)| (*a == c || *z == c) && t == p)
                });
                assert!(listed, "{c:?} in {scope:?}");
            }
            let titles: Vec<&str> = binds(&rows).iter().map(|(_, t)| *t).collect();
            let mut unique = titles.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), titles.len(), "{scope:?}");
        }
    }

    #[test]
    fn short_enough_to_read() {
        // The help is a page of keys, not the whole table: the busiest pane stays under
        // three screen columns of about thirty rows.
        crate::keys::set_label_style(crate::keys::LabelStyle::Pc);
        let k = Keymap::default();
        for scope in Scope::ALL {
            let n = help_rows(&k, scope).len();
            assert!(n <= 90, "{scope:?}: {n} rows");
        }
    }

    #[test]
    fn columns_hold_whole_sections_and_balance() {
        for scope in Scope::ALL {
            let rows = help_rows(&Keymap::default(), scope);
            let n = rows.len();
            let total: usize = rows.iter().map(lines).sum();
            let biggest = columns(rows.clone(), 99)
                .iter()
                .map(|c| c.iter().map(lines).sum::<usize>())
                .max()
                .unwrap_or(0);
            let cols = columns(rows, 3);
            assert_eq!(cols.len(), 3);
            assert_eq!(cols.iter().map(Vec::len).sum::<usize>(), n);
            for c in &cols {
                assert!(matches!(c.first(), Some(HelpRow::Header(_))), "{scope:?}");
                // No column longer than an even share plus the largest section.
                let w: usize = c.iter().map(lines).sum();
                assert!(w <= total.div_ceil(3) + biggest, "{scope:?}: {w}");
            }
        }
    }
}
