//! Notifications ("toasts") in the window's bottom-right corner: how a message wraps, how
//! big its box is, where the boxes stack, how long one stays up and its colours.
//!
//! Text is measured by the caller (`measure`: the drawn width of one line), so the boxes fit
//! the font that is actually drawn; the tests measure with a fixed advance per character.
//!
//! Layout: a box is as wide as its longest line, up to [`max_width`] (a share of the window,
//! never wider than the window minus its margins); words wrap at that width, a word longer
//! than a line (a file path) breaks after a `/`, `\`, `_`, `-`, `.` or `:` where it can and
//! anywhere where it cannot. The newest box sits lowest, just above the focused pane's key
//! hints; older ones stack upwards, and those that no longer fit under the top bar are left
//! out (they stay in the notification log).

use crate::banner::Severity;
use crate::primitives::{Color, Rect};
use crate::theme::Theme;

/// Text inset inside a box, horizontal.
pub const PAD_X: f32 = 10.0;
/// Text inset inside a box, vertical.
pub const PAD_Y: f32 = 6.0;
/// Between two stacked boxes.
pub const GAP: f32 = 6.0;
/// Between a box and the window's right edge, and the least room left of the widest box.
pub const MARGIN: f32 = 12.0;
/// Widest a box gets, as a share of the window: wide enough for a sentence a line, narrow
/// enough that a message never covers most of a pane's curves.
pub const MAX_WIDTH_FRACTION: f32 = 0.45;
/// The widest box on a small window, so a line still holds a handful of words.
pub const MIN_MAX_WIDTH: f32 = 320.0;
/// Most lines one box shows; a longer message ends in `…` and is whole in the log.
pub const MAX_LINES: usize = 10;
/// Where a word too long for a line prefers to break: after a path or name separator.
const BREAK_AFTER: &[char] = &['/', '\\', '_', '-', '.', ':', ',', ';', '=', '&', '?'];

/// Time to notice a message before reading it.
pub const READ_BASE_S: f64 = 3.0;
/// Reading time per word: about 200 words a minute, slower than prose reading because the
/// operator looks over from the work.
pub const READ_PER_WORD_S: f64 = 0.3;
/// Longest an information toast stays up: anything longer is reread in the log.
pub const INFO_MAX_S: f64 = 15.0;
/// A warning (a key refused, something missing) stays half as long again as information:
/// it says what to do instead.
pub const WARNING_FACTOR: f64 = 1.5;
pub const WARNING_MAX_S: f64 = 25.0;
/// An error (a command failed, the link or the stimulus broke, an Leq limit went over) is
/// news the operator may have missed while looking at the speakers: it stays long, but not
/// for good, because keyboard-only operation has no key to dismiss it and a stuck box would
/// cover the corner of a pane for the rest of the show.
pub const ERROR_MIN_S: f64 = 20.0;
pub const ERROR_FACTOR: f64 = 3.0;
pub const ERROR_MAX_S: f64 = 60.0;

/// How many notifications the log keeps (newest last).
pub const LOG_LEN: usize = 50;

/// How long a toast of `text` stays up, hover pauses aside: a base time to notice it plus a
/// reading time per word, longer for warnings and errors.
pub fn duration_s(severity: Severity, text: &str) -> f64 {
    let words = text.split_whitespace().count() as f64;
    let read = READ_BASE_S + READ_PER_WORD_S * words;
    match severity {
        Severity::Info => read.min(INFO_MAX_S),
        Severity::Warning => (read * WARNING_FACTOR).min(WARNING_MAX_S),
        Severity::Fault => (read * ERROR_FACTOR).clamp(ERROR_MIN_S, ERROR_MAX_S),
    }
}

/// A toast's fill, text and border.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastColors {
    pub background: Color,
    pub text: Color,
    pub border: Color,
}

/// Information on the raised panel colour (it should not shout); warnings and errors in the
/// banners' warning and fault colours, so a colour means the same at the top and here.
pub fn colors(severity: Severity, theme: &Theme) -> ToastColors {
    let banner = |b: crate::theme::BannerColors| ToastColors {
        background: b.background,
        text: b.text,
        border: b.background,
    };
    match severity {
        Severity::Info => ToastColors {
            background: theme.plot_background,
            text: theme.text,
            border: theme.grid_major.color,
        },
        Severity::Warning => banner(theme.banner_warning),
        Severity::Fault => banner(theme.banner_fault),
    }
}

/// Widest a box gets in a window `window_w` wide.
pub fn max_width(window_w: f32) -> f32 {
    let room = (window_w - 2.0 * MARGIN).max(0.0);
    (window_w * MAX_WIDTH_FRACTION)
        .max(MIN_MAX_WIDTH.min(room))
        .min(room)
}

/// `text` in lines no wider than `width`: words wrap, a newline starts a new line, a word
/// longer than a line breaks (after a separator where one is in the second two thirds of
/// what fits, else where the line is full). Every line keeps at least one character, so a
/// width too small for any still ends.
pub fn wrap(text: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() {
                let joined = format!("{line} {word}");
                if measure(&joined) <= width {
                    line = joined;
                    continue;
                }
                out.push(std::mem::take(&mut line));
            }
            let mut pieces = break_word(word, width, measure);
            line = pieces.pop().unwrap_or_default();
            out.extend(pieces);
        }
        out.push(line);
    }
    out
}

/// `word` in pieces no wider than `width` (at least one character each).
fn break_word(word: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = word;
    while !rest.is_empty() {
        if measure(rest) <= width {
            out.push(rest.to_owned());
            break;
        }
        // The longest prefix that fits, never less than one character.
        let mut ends = rest.char_indices().map(|(i, c)| i + c.len_utf8());
        let mut fit = ends.next().unwrap_or(rest.len());
        for end in ends {
            if measure(&rest[..end]) > width {
                break;
            }
            fit = end;
        }
        // A separator near the end of what fits keeps a path's parts whole; one near the
        // start would leave most of the line empty.
        let cut = rest[..fit]
            .char_indices()
            .filter(|(_, c)| BREAK_AFTER.contains(c))
            .map(|(i, c)| i + c.len_utf8())
            .rfind(|&e| 3 * e >= fit)
            .unwrap_or(fit);
        out.push(rest[..cut].to_owned());
        rest = &rest[cut..];
    }
    out
}

/// One toast on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastBox {
    /// Which of the texts given to [`stack`].
    pub index: usize,
    /// The box, text insets included.
    pub rect: Rect,
    /// Its lines, top to bottom; the first starts at `rect` + ([`PAD_X`], [`PAD_Y`]), each
    /// one `line_h` under the previous.
    pub lines: Vec<String>,
}

/// Where toasts may go in a `window_w` × `window_h` window: under `top` (the top bar's
/// bottom), above the bottom `bottom_reserved` (the focused pane's key-hint line and the
/// panes' margin) with a gap.
pub fn area(window_w: f32, window_h: f32, top: f32, bottom_reserved: f32) -> Rect {
    let top = top + GAP;
    let bottom = (window_h - bottom_reserved - GAP).max(top);
    Rect::new(0.0, top, window_w, bottom - top)
}

/// The boxes of `texts` (oldest first) in `area`: right-aligned [`MARGIN`] from its right
/// edge, the newest lowest, each older one above the previous with a [`GAP`]. Those that
/// do not fit under the area's top are left out. Returned oldest first.
pub fn stack(
    texts: &[&str],
    area: Rect,
    line_h: f32,
    measure: &dyn Fn(&str) -> f32,
) -> Vec<ToastBox> {
    let max_w = max_width(area.w);
    let text_w = (max_w - 2.0 * PAD_X).max(1.0);
    let fit_lines = ((area.h - 2.0 * PAD_Y) / line_h).floor().max(1.0) as usize;
    let max_lines = fit_lines.min(MAX_LINES);
    let mut out = Vec::new();
    let mut bottom = area.bottom();
    for (index, text) in texts.iter().enumerate().rev() {
        let mut lines = wrap(text, text_w, measure);
        if lines.len() > max_lines {
            lines.truncate(max_lines);
            if let Some(last) = lines.last_mut() {
                *last = ellipsize(last, text_w, measure);
            }
        }
        let widest = lines.iter().map(|l| measure(l)).fold(0.0, f32::max);
        let w = (widest + 2.0 * PAD_X).min(max_w);
        let h = lines.len() as f32 * line_h + 2.0 * PAD_Y;
        let y = bottom - h;
        if y < area.y {
            break;
        }
        out.push(ToastBox {
            index,
            rect: Rect::new(area.right() - MARGIN - w, y, w, h),
            lines,
        });
        bottom = y - GAP;
    }
    out.reverse();
    out
}

/// `line` shortened until `line…` fits `width`.
fn ellipsize(line: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> String {
    let mut s = line.trim_end().to_owned();
    loop {
        let t = format!("{s}…");
        if s.is_empty() || measure(&t) <= width {
            return t;
        }
        s.pop();
        s.truncate(s.trim_end().len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed advance per character, about the bundled font's at 14 px.
    const ADV: f32 = 7.0;
    const LINE_H: f32 = 17.0;

    fn measure(s: &str) -> f32 {
        s.chars().count() as f32 * ADV
    }

    const HINT: f32 = 26.0;
    const TOP: f32 = 40.0;

    fn boxes(texts: &[&str], w: f32, h: f32) -> Vec<ToastBox> {
        stack(texts, area(w, h, TOP, HINT), LINE_H, &measure)
    }

    #[test]
    fn a_short_text_is_one_line_as_wide_as_itself() {
        let b = boxes(&["t13 selected"], 1280.0, 800.0);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].lines, vec!["t13 selected"]);
        assert_eq!(b[0].rect.w, measure("t13 selected") + 2.0 * PAD_X);
        assert_eq!(b[0].rect.h, LINE_H + 2.0 * PAD_Y);
        assert_eq!(b[0].rect.right(), 1280.0 - MARGIN);
        assert_eq!(b[0].rect.bottom(), 800.0 - HINT - GAP);
    }

    #[test]
    fn a_long_text_wraps_at_word_boundaries_within_the_maximum_width() {
        let text = "Main L's level −3.0 dBFS is above the daemon's ceiling −6.0 dBFS: edit \
                    it (palette: edit the selected measurement) or raise the ceiling in \
                    Settings › Audio, then arm the sweep again";
        for w in [640.0, 1280.0, 2560.0] {
            let b = boxes(&[text], w, 800.0);
            let max = max_width(w);
            assert!(b[0].rect.w <= max, "{w}: {} > {max}", b[0].rect.w);
            assert!(b[0].lines.len() > 1, "{w}: {:?}", b[0].lines);
            for l in &b[0].lines {
                assert!(measure(l) <= max - 2.0 * PAD_X, "{w}: {l}");
                assert!(!l.starts_with(' ') && !l.ends_with(' '), "{l:?}");
            }
            // Nothing lost: the words in order.
            let words: Vec<&str> = b[0].lines.iter().flat_map(|l| l.split(' ')).collect();
            assert_eq!(words, text.split_whitespace().collect::<Vec<_>>());
            assert_eq!(b[0].rect.h, b[0].lines.len() as f32 * LINE_H + 2.0 * PAD_Y);
        }
        // Wider window, fewer lines.
        assert!(
            boxes(&[text], 2560.0, 800.0)[0].lines.len()
                < boxes(&[text], 640.0, 800.0)[0].lines.len()
        );
    }

    #[test]
    fn a_path_breaks_after_its_separators() {
        let path = "/home/operator/.local/share/ac2/sessions/2026-10-07-festival-main-stage/\
                    traces/main-left-position-3-after-eq.csv";
        let text = format!("import {path}: no such file or directory");
        let width = 300.0;
        let lines = wrap(&text, width, &measure);
        for l in &lines {
            assert!(measure(l) <= width, "{l}");
        }
        assert_eq!(lines[0], "import");
        // The path's pieces end at a separator and join back into the path.
        let pieces: Vec<&String> = lines
            .iter()
            .skip(1)
            .take_while(|l| !l.contains(' '))
            .collect();
        assert!(pieces.len() > 1, "{lines:?}");
        for p in &pieces {
            assert!(p.ends_with(BREAK_AFTER), "{p}");
        }
        assert!(lines.concat().contains(&path[..60]), "{lines:?}");
        // A word without separators breaks where the line is full.
        let blob = "x".repeat(100);
        let lines = wrap(&blob, 70.0, &measure);
        assert_eq!(lines.len(), 10);
        assert!(lines.iter().all(|l| l.len() == 10));
    }

    #[test]
    fn a_newline_starts_a_line_and_tiny_widths_still_end() {
        assert_eq!(wrap("a b\nc", 1000.0, &measure), vec!["a b", "c"]);
        let lines = wrap("abc def", 1.0, &measure);
        assert_eq!(lines, vec!["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn a_message_too_long_for_the_box_ends_in_an_ellipsis() {
        let text = "word ".repeat(400);
        let b = boxes(&[&text], 1280.0, 800.0);
        assert_eq!(b[0].lines.len(), MAX_LINES);
        assert!(b[0].lines[MAX_LINES - 1].ends_with('…'));
        // On a short window the box never grows past the room there is.
        let b = boxes(&[&text], 640.0, 200.0);
        let a = area(640.0, 200.0, TOP, HINT);
        assert!(b[0].rect.y >= a.y && b[0].rect.bottom() <= a.bottom());
    }

    #[test]
    fn stacked_toasts_never_overlap_each_other_or_the_hints_at_any_window_size() {
        let long = "Main L: no delay estimate (no clear peak; the reference and the mic may \
                    not hear the same stimulus — check the routing and the level)";
        let path = "export /home/operator/.local/share/ac2/traces/very-long-trace-name-of-the-\
                    main-left-hang-after-the-high-shelf.csv failed: permission denied";
        let texts = [
            "t13 selected",
            long,
            path,
            "slot 1: Main L S1 captured",
            long,
            "keys act on the live measurement",
        ];
        for (w, h) in [
            (640.0, 400.0),
            (800.0, 600.0),
            (1280.0, 800.0),
            (1920.0, 1080.0),
            (2560.0, 1440.0),
        ] {
            let b = boxes(&texts, w, h);
            assert!(!b.is_empty(), "{w}×{h}");
            // The newest is always shown, lowest.
            assert_eq!(b.last().map(|x| x.index), Some(texts.len() - 1));
            let hint_top = h - HINT;
            for x in &b {
                let r = x.rect;
                assert!(
                    r.x >= MARGIN - 0.01 && r.right() <= w - MARGIN + 0.01,
                    "{w}×{h} {r:?}"
                );
                assert!(
                    r.bottom() <= hint_top - GAP + 0.01,
                    "{w}×{h}: over the hints {r:?}"
                );
                assert!(r.y >= TOP, "{w}×{h}: under the top bar {r:?}");
                assert!(r.w <= max_width(w) + 0.01);
            }
            for pair in b.windows(2) {
                assert!(
                    pair[0].rect.bottom() + GAP <= pair[1].rect.y + 0.01,
                    "{w}×{h}: {:?} over {:?}",
                    pair[0].rect,
                    pair[1].rect
                );
                assert!(pair[0].index < pair[1].index, "oldest first");
            }
        }
        // A tall window shows them all; a small one leaves the oldest out.
        assert_eq!(boxes(&texts, 2560.0, 1440.0).len(), texts.len());
        assert!(boxes(&texts, 640.0, 400.0).len() < texts.len());
    }

    #[test]
    fn the_maximum_width_is_a_share_of_the_window_within_its_margins() {
        assert_eq!(max_width(2000.0), 900.0);
        assert_eq!(max_width(640.0), MIN_MAX_WIDTH);
        assert_eq!(max_width(300.0), 300.0 - 2.0 * MARGIN);
        assert_eq!(max_width(0.0), 0.0);
    }

    #[test]
    fn reading_time_grows_with_the_words_and_the_severity() {
        let short = "t13 selected";
        let long = "word ".repeat(20);
        assert_eq!(
            duration_s(Severity::Info, short),
            READ_BASE_S + 2.0 * READ_PER_WORD_S
        );
        assert!(duration_s(Severity::Info, &long) > duration_s(Severity::Info, short));
        assert_eq!(duration_s(Severity::Info, &"w ".repeat(1000)), INFO_MAX_S);
        assert!(duration_s(Severity::Warning, short) > duration_s(Severity::Info, short));
        assert_eq!(duration_s(Severity::Fault, short), ERROR_MIN_S);
        assert!(duration_s(Severity::Fault, &long) > duration_s(Severity::Warning, &long));
        assert_eq!(duration_s(Severity::Fault, &"w ".repeat(1000)), ERROR_MAX_S);
    }

    #[test]
    fn colours_follow_the_banners_and_text_reads_on_its_fill() {
        for t in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
            assert_eq!(
                colors(Severity::Warning, &t).background,
                t.banner_warning.background
            );
            assert_eq!(
                colors(Severity::Fault, &t).background,
                t.banner_fault.background
            );
            for s in [Severity::Info, Severity::Warning, Severity::Fault] {
                let c = colors(s, &t);
                assert!(
                    crate::theme::contrast_ratio(c.text, c.background) >= 4.5,
                    "{:?} {s:?}: {}",
                    t.name,
                    crate::theme::contrast_ratio(c.text, c.background)
                );
            }
        }
    }
}
