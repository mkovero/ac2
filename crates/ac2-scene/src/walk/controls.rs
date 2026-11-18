//! The walk pane's control bar: a button per walk action, each with its words, whether it
//! can run now and, when it cannot, why. The reasons here are the ones the app refuses the
//! action with, so a dimmed button and a refused command never disagree.

use ac2_proto::model::{PointState, WalkState};

use crate::canvas::{cut_to, text_width};
use crate::primitives::Rect;

/// What a walk's operator does at a step, in the bar's order: paging on the left, the
/// capture in the middle, the corrections on the right.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Prev,
    Next,
    Capture,
    Skip,
    Redo,
    Swap,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::Prev,
        Action::Next,
        Action::Capture,
        Action::Skip,
        Action::Redo,
        Action::Swap,
    ];

    /// The action in full, as its command is titled.
    pub fn label(self) -> &'static str {
        match self {
            Action::Prev => "Previous step",
            Action::Next => "Next step",
            Action::Capture => "Capture here and go on",
            Action::Skip => "Skip point",
            Action::Redo => "Redo point and reset",
            Action::Swap => "Mics trade places",
        }
    }

    /// The leading words of [`Action::label`], for a bar too narrow for it.
    pub fn short(self) -> &'static str {
        match self {
            Action::Prev => "Previous",
            Action::Next => "Next",
            Action::Capture => "Capture",
            Action::Skip => "Skip",
            Action::Redo => "Redo",
            Action::Swap => "Trade places",
        }
    }

    /// The capture is what the operator presses at every position, standing at the mic.
    pub fn primary(self) -> bool {
        self == Action::Capture
    }
}

/// The captured points a redo acts on: those of the step the mics are at when it holds a
/// capture, else of the last step before it that does (from the last step when the walk is
/// done). Empty when nothing is captured.
pub fn redo_points(w: &WalkState) -> Vec<usize> {
    let captured = |s: u16| -> Vec<usize> {
        w.step_points(s)
            .into_iter()
            .filter(|&i| matches!(w.points[i].state, PointState::Captured { .. }))
            .collect()
    };
    let n = w.steps().max(1);
    let start = w.step.unwrap_or(n - 1);
    (0..n)
        .map(|k| (start + n - k) % n)
        .map(captured)
        .find(|c| !c.is_empty())
        .unwrap_or_default()
}

/// Why `action` cannot run on `w` now; `None` when it can. `stimulus_playing`: a sweep
/// walk's capture is a run of its sweep, which waits for the stimulus that plays now.
pub fn refusal(w: &WalkState, action: Action, stimulus_playing: bool) -> Option<String> {
    let standing = w.standing();
    match action {
        // The steps wrap round: there is always a next and a previous one.
        Action::Prev | Action::Next => None,
        Action::Capture if standing.is_empty() => {
            let why = if w.step.is_none() {
                "every point is captured or skipped"
            } else {
                "this step is captured"
            };
            Some(format!(
                "{}: {why} ({} redoes one)",
                w.name,
                Action::Redo.short()
            ))
        }
        Action::Capture if stimulus_playing => Some(format!(
            "the stimulus is playing: Esc stops it, then {} arms the walk's sweep",
            Action::Capture.short()
        )),
        Action::Capture => None,
        Action::Skip if standing.is_empty() => Some(format!("{}: no point to skip", w.name)),
        Action::Skip => None,
        Action::Redo if redo_points(w).is_empty() => {
            Some("no point is captured yet: nothing to redo".to_owned())
        }
        Action::Redo => None,
        Action::Swap if standing.len() < 2 => {
            Some("one mic stands at this step: no other to trade places with".to_owned())
        }
        Action::Swap => None,
    }
}

/// One button of the bar, placed.
#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub action: Action,
    /// The words on it: [`Action::label`], shortened where the bar is narrow.
    pub label: String,
    pub font_size: f32,
    /// Why it is dimmed and does nothing; `None`: it runs its action.
    pub blocked: Option<String>,
    /// Where it sits, from the bar's top left.
    pub rect: Rect,
}

impl Button {
    /// What the button says under the pointer: why it is dimmed, else its action in full
    /// when the words on it are shortened.
    pub fn tip(&self) -> Option<String> {
        self.blocked
            .clone()
            .or_else(|| (self.label != self.action.label()).then(|| self.action.label().to_owned()))
    }
}

/// The bar: its buttons in [`Action::ALL`] order and the height it takes at its width.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub buttons: Vec<Button>,
    pub height: f32,
}

impl Bar {
    pub fn button(&self, action: Action) -> Option<&Button> {
        self.buttons.iter().find(|b| b.action == action)
    }
}

/// A button's height: a fingertip on a touch screen, as the session window's buttons.
pub const BUTTON_H: f32 = 40.0;
/// The capture's height: the one pressed without looking, mic stand in the other hand.
pub const PRIMARY_H: f32 = 48.0;
/// The narrowest button a fingertip still hits alone.
const MIN_W: f32 = 64.0;
/// How much wider than the widest other button the capture is at least.
const PRIMARY_WIDER: f32 = 1.25;
/// How much larger the capture's words are.
const PRIMARY_FONT: f32 = 1.2;
const GAP: f32 = 6.0;
const PAD: f32 = 4.0;
/// Room each side of a button's words.
const INSET: f32 = 10.0;

fn font_of(a: Action, fs: f32) -> f32 {
    if a.primary() { fs * PRIMARY_FONT } else { fs }
}

/// Each action's width with `label`'s words whole.
fn natural(label: fn(Action) -> &'static str, fs: f32) -> [f32; 6] {
    let text = |a: Action| (text_width(label(a), font_of(a, fs)) + 2.0 * INSET).max(MIN_W);
    let widest = Action::ALL
        .into_iter()
        .filter(|a| !a.primary())
        .map(text)
        .fold(0.0_f32, f32::max);
    Action::ALL.map(|a| {
        if a.primary() {
            text(a).max(PRIMARY_WIDER * widest)
        } else {
            text(a)
        }
    })
}

fn sum(widths: &[f32]) -> f32 {
    widths.iter().sum::<f32>() + GAP * widths.len().saturating_sub(1) as f32
}

/// The bar of walk `w` at `width`, text size `fs`: one row with the labels in full where
/// they fit, with the short ones where those do; narrower still, the capture takes a row of
/// its own across the bar and the others wrap under it, sharing each row's width.
pub fn bar(w: &WalkState, stimulus_playing: bool, width: f32, fs: f32) -> Bar {
    let room = (width - 2.0 * PAD).max(1.0);
    let button = |a: Action, label: &str, rect: Rect| Button {
        action: a,
        // Half a pixel of slack: a width made from these very words must not cut them.
        label: cut_to(
            label,
            rect.w - 2.0 * INSET.min(rect.w / 4.0) + 0.5,
            font_of(a, fs),
        ),
        font_size: font_of(a, fs),
        blocked: refusal(w, a, stimulus_playing),
        rect,
    };
    let labels: [fn(Action) -> &'static str; 2] = [Action::label, Action::short];
    for label in labels {
        let widths = natural(label, fs);
        let total = sum(&widths);
        if total > room {
            continue;
        }
        // Centred: the capture stays near the middle of the pane at any width.
        let mut x = PAD + (room - total) / 2.0;
        let mut buttons = Vec::with_capacity(widths.len());
        for (a, bw) in Action::ALL.into_iter().zip(widths) {
            let h = if a.primary() { PRIMARY_H } else { BUTTON_H };
            let rect = Rect::new(x, PAD + (PRIMARY_H - h) / 2.0, bw, h);
            buttons.push(button(a, label(a), rect));
            x += bw + GAP;
        }
        return Bar {
            buttons,
            height: PRIMARY_H + 2.0 * PAD,
        };
    }
    let widths = natural(Action::short, fs);
    let mut placed: Vec<(Action, Rect)> =
        vec![(Action::Capture, Rect::new(PAD, PAD, room, PRIMARY_H))];
    let mut y = PAD + PRIMARY_H + GAP;
    let rest: Vec<(Action, f32)> = Action::ALL
        .into_iter()
        .zip(widths)
        .filter(|(a, _)| !a.primary())
        .collect();
    let mut row: Vec<(Action, f32)> = Vec::new();
    let mut flush = |row: &mut Vec<(Action, f32)>, y: &mut f32| {
        if row.is_empty() {
            return;
        }
        let w_row: Vec<f32> = row.iter().map(|(_, bw)| *bw).collect();
        let gaps = GAP * (row.len() - 1) as f32;
        // Each row's buttons share its width in proportion: no ragged right edge, and the
        // targets only grow.
        let scale = (room - gaps).max(1.0) / w_row.iter().sum::<f32>();
        let mut x = PAD;
        for (a, bw) in row.drain(..) {
            placed.push((a, Rect::new(x, *y, bw * scale, BUTTON_H)));
            x += bw * scale + GAP;
        }
        *y += BUTTON_H + GAP;
    };
    for (a, bw) in rest {
        let with: Vec<f32> = row.iter().map(|(_, x)| *x).chain([bw]).collect();
        if !row.is_empty() && sum(&with) > room {
            flush(&mut row, &mut y);
        }
        row.push((a, bw));
    }
    flush(&mut row, &mut y);
    let mut buttons: Vec<Button> = placed
        .into_iter()
        .map(|(a, rect)| {
            // The capture's own row is usually wide enough to say it in full.
            let whole = text_width(a.label(), font_of(a, fs)) + 2.0 * INSET <= rect.w;
            let words = if a.primary() && whole {
                a.label()
            } else {
                a.short()
            };
            button(a, words, rect)
        })
        .collect();
    buttons.sort_by_key(|b| Action::ALL.iter().position(|a| *a == b.action));
    Bar {
        buttons,
        height: y - GAP + PAD,
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{captured, pending, two_mics, walk};
    use super::*;

    const FS: f32 = 14.0;

    fn blocked(w: &WalkState, playing: bool) -> Vec<(Action, Option<String>)> {
        bar(w, playing, 1200.0, FS)
            .buttons
            .into_iter()
            .map(|b| (b.action, b.blocked))
            .collect()
    }

    fn dimmed(w: &WalkState) -> Vec<Action> {
        blocked(w, false)
            .into_iter()
            .filter_map(|(a, b)| b.map(|_| a))
            .collect()
    }

    /// The bar names its six actions as their commands are titled, the capture in the
    /// middle and the largest.
    #[test]
    fn the_bar_has_a_button_per_action_and_the_capture_is_the_largest() {
        let w = walk(
            &[captured(1), PointState::Pending, PointState::Pending],
            Some(1),
        );
        let b = bar(&w, false, 1200.0, FS);
        let labels: Vec<&str> = b.buttons.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Previous step",
                "Next step",
                "Capture here and go on",
                "Skip point",
                "Redo point and reset",
                "Mics trade places"
            ]
        );
        let capture = b.button(Action::Capture).expect("capture");
        for x in b.buttons.iter().filter(|x| !x.action.primary()) {
            assert!(capture.rect.w >= PRIMARY_WIDER * x.rect.w, "{x:?}");
            assert!(capture.rect.h > x.rect.h && capture.font_size > x.font_size);
            assert!(x.rect.h >= BUTTON_H && x.rect.w >= MIN_W, "{x:?}");
            assert_eq!(x.tip(), x.blocked, "words in full need no tip");
        }
        assert!(
            b.buttons
                .windows(2)
                .all(|p| p[0].rect.x + p[0].rect.w <= p[1].rect.x)
        );
        assert_eq!(b.height, PRIMARY_H + 2.0 * PAD);
    }

    /// A walk under way with a capture behind it and one mic: everything but the trade of
    /// places runs.
    #[test]
    fn a_walk_under_way_dims_only_what_one_mic_cannot_do() {
        let w = walk(
            &[captured(1), PointState::Pending, PointState::Pending],
            Some(1),
        );
        assert_eq!(
            blocked(&w, false),
            [
                (Action::Prev, None),
                (Action::Next, None),
                (Action::Capture, None),
                (Action::Skip, None),
                (Action::Redo, None),
                (
                    Action::Swap,
                    Some("one mic stands at this step: no other to trade places with".into())
                ),
            ]
        );
    }

    /// Nothing captured yet: no point to redo. The steps wrap round, so the first step has
    /// a previous one and the last a next.
    #[test]
    fn the_first_and_last_steps_page_round_and_a_fresh_walk_has_nothing_to_redo() {
        let first = walk(&pending(3), Some(0));
        assert_eq!(dimmed(&first), [Action::Redo, Action::Swap]);
        assert_eq!(
            refusal(&first, Action::Redo, false).as_deref(),
            Some("no point is captured yet: nothing to redo")
        );
        let last = walk(&[captured(1), captured(2), PointState::Pending], Some(2));
        assert_eq!(dimmed(&last), [Action::Swap]);
        assert_eq!(redo_points(&last), [1], "the last capture before the mic");
    }

    /// At a step already captured, and with every point done, the capture and the skip say
    /// so and name the redo.
    #[test]
    fn a_captured_step_and_a_finished_walk_refuse_the_capture() {
        let at_captured = walk(&[captured(1), PointState::Pending], Some(0));
        assert_eq!(
            dimmed(&at_captured),
            [Action::Capture, Action::Skip, Action::Swap]
        );
        assert_eq!(
            refusal(&at_captured, Action::Capture, false).as_deref(),
            Some("Mains: this step is captured (Redo redoes one)")
        );
        assert_eq!(redo_points(&at_captured), [0]);
        let done = walk(&[captured(1), PointState::Skipped], None);
        assert_eq!(
            refusal(&done, Action::Capture, false).as_deref(),
            Some("Mains: every point is captured or skipped (Redo redoes one)")
        );
        assert_eq!(
            refusal(&done, Action::Skip, false).as_deref(),
            Some("Mains: no point to skip")
        );
        assert_eq!(refusal(&done, Action::Redo, false), None);
    }

    /// Two mics standing can trade places; one of them alone at its step cannot.
    #[test]
    fn two_mics_standing_trade_places() {
        let w = two_mics(&pending(4), Some(0));
        assert_eq!(refusal(&w, Action::Swap, false), None);
        let mut one_left = two_mics(&pending(4), Some(0));
        let at = one_left.standing()[0];
        one_left.points[at].state = captured(1);
        assert!(refusal(&one_left, Action::Swap, false).is_some());
        assert_eq!(refusal(&one_left, Action::Capture, false), None);
    }

    /// A sweep walk's capture waits for the stimulus that plays now.
    #[test]
    fn a_playing_stimulus_holds_the_capture() {
        let w = walk(&pending(2), Some(0));
        assert_eq!(
            refusal(&w, Action::Capture, true).as_deref(),
            Some("the stimulus is playing: Esc stops it, then Capture arms the walk's sweep")
        );
        assert_eq!(refusal(&w, Action::Skip, true), None);
    }

    /// Narrower, the labels shorten (each saying its action in full under the pointer);
    /// narrower still the capture takes its own row across the bar and the rest wrap under
    /// it. No button leaves the bar or covers another, and none is smaller than a fingertip.
    #[test]
    fn a_narrow_bar_shortens_its_labels_then_wraps() {
        let w = walk(&pending(3), Some(0));
        let short = bar(&w, false, 700.0, FS);
        let labels: Vec<&str> = short.buttons.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Previous",
                "Next",
                "Capture",
                "Skip",
                "Redo",
                "Trade places"
            ]
        );
        assert_eq!(short.height, PRIMARY_H + 2.0 * PAD);
        assert_eq!(
            short.button(Action::Next).and_then(Button::tip).as_deref(),
            Some("Next step")
        );
        assert_eq!(
            short.button(Action::Redo).and_then(Button::tip).as_deref(),
            Some("no point is captured yet: nothing to redo")
        );
        for width in [420.0_f32, 300.0, 200.0, 120.0, 40.0] {
            let b = bar(&w, false, width, FS);
            assert_eq!(b.buttons.len(), 6);
            let capture = b.button(Action::Capture).expect("capture");
            assert_eq!(capture.rect.y, PAD, "{width}");
            assert!(b.height > PRIMARY_H + 2.0 * PAD, "{width}: wrapped");
            for (i, x) in b.buttons.iter().enumerate() {
                let r = x.rect;
                assert!(r.x >= 0.0 && r.x + r.w <= width.max(2.0 * PAD + 1.0) + 0.01);
                assert!(
                    r.y + r.h <= b.height + 0.01 && r.h >= BUTTON_H,
                    "{width} {x:?}"
                );
                assert!(capture.rect.w >= r.w - 0.01, "{width} {x:?}");
                for o in &b.buttons[i + 1..] {
                    let apart = r.x + r.w <= o.rect.x + 0.01
                        || o.rect.x + o.rect.w <= r.x + 0.01
                        || r.y + r.h <= o.rect.y + 0.01
                        || o.rect.y + o.rect.h <= r.y + 0.01;
                    assert!(apart, "{width}: {x:?} over {o:?}");
                }
            }
        }
        // 300 px: the capture's row (wide enough for its words in full), then three and two.
        let b = bar(&w, false, 300.0, FS);
        assert_eq!(b.height, 2.0 * PAD + PRIMARY_H + 2.0 * (GAP + BUTTON_H));
        let labels: Vec<&str> = b.buttons.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Previous",
                "Next",
                "Capture here and go on",
                "Skip",
                "Redo",
                "Trade places"
            ]
        );
        let narrow = bar(&w, false, 200.0, FS);
        assert_eq!(
            narrow.button(Action::Capture).map(|x| x.label.as_str()),
            Some("Capture")
        );
    }
}
