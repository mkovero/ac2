//! A caret in a line of typed text: where typing inserts and Backspace / Delete remove,
//! moved by ←/→, Home/End or a click. Counted in characters, so a click's column maps to
//! it directly and no edit can land inside a multi-byte character.

/// The glyph drawn at the caret.
pub const GLYPH: char = '▏';

/// A caret: the number of characters before it. Kept clamped to the text it edits, so a
/// text changed under it (a rebase, a preset) never puts it past the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caret(usize);

impl Caret {
    /// At the end of `t`: where entering a field puts it.
    pub fn end_of(t: &str) -> Self {
        Self(t.chars().count())
    }

    /// At character `n` of `t` (a click), clamped to its end.
    pub fn at(t: &str, n: usize) -> Self {
        Self(n.min(t.chars().count()))
    }

    /// Characters before the caret in `t`.
    pub fn index(self, t: &str) -> usize {
        self.0.min(t.chars().count())
    }

    fn byte(self, t: &str) -> usize {
        t.char_indices().nth(self.0).map_or(t.len(), |(b, _)| b)
    }

    /// `s` typed at the caret, which moves past it.
    pub fn insert(&mut self, t: &mut String, s: &str) {
        self.0 = self.index(t);
        t.insert_str(self.byte(t), s);
        self.0 += s.chars().count();
    }

    /// Backspace: the character before the caret removed.
    pub fn backspace(&mut self, t: &mut String) {
        self.0 = self.index(t);
        if self.0 > 0 {
            self.0 -= 1;
            t.remove(self.byte(t));
        }
    }

    /// Delete: the character after the caret removed.
    pub fn delete(&mut self, t: &mut String) {
        self.0 = self.index(t);
        if self.0 < t.chars().count() {
            t.remove(self.byte(t));
        }
    }

    /// ←/→: one character left (`d` < 0) or right, stopping at the ends.
    pub fn step(&mut self, t: &str, d: i32) {
        let i = self.index(t);
        self.0 = if d < 0 {
            i.saturating_sub(1)
        } else {
            (i + 1).min(t.chars().count())
        };
    }

    /// Home.
    pub fn home(&mut self) {
        self.0 = 0;
    }

    /// End.
    pub fn end(&mut self, t: &str) {
        *self = Self::end_of(t);
    }
}

/// A line as its field shows it, `width` characters at most: the caret drawn where it is,
/// and a line too long for the field cut to a window around the caret, `…` where text goes
/// on beyond an edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub text: String,
    /// The first character of the line in the window.
    pub first: usize,
    /// The window starts with a `…`.
    pub cut_left: bool,
}

impl Shown {
    /// `t` with the caret `c`, in at most `width` characters (counting the caret and any
    /// `…`; at least 4).
    pub fn new(t: &str, c: Caret, width: usize) -> Self {
        let chars: Vec<char> = t.chars().collect();
        let n = chars.len();
        let i = c.index(t);
        let width = width.max(4);
        if n < width {
            let mut text: String = chars[..i].iter().collect();
            text.push(GLYPH);
            text.extend(&chars[i..]);
            return Self {
                text,
                first: 0,
                cut_left: false,
            };
        }
        // The caret's own cell and an ellipsis at each cut edge come out of the width: the
        // line's start or end when the caret is near it, else a window centred on the caret.
        let room = width - 1;
        let (first, left, right) = if i < room {
            (0, false, true)
        } else if n - i < room {
            (n - (room - 1), true, false)
        } else {
            let shown = room - 2;
            (i - shown / 2, true, true)
        };
        let shown = room - usize::from(left) - usize::from(right);
        let last = (first + shown).min(n);
        let mut text = String::new();
        if left {
            text.push('…');
        }
        text.extend(&chars[first..i]);
        text.push(GLYPH);
        text.extend(&chars[i..last]);
        if right {
            text.push('…');
        }
        Self {
            text,
            first,
            cut_left: left,
        }
    }

    /// The character of the line a click on column `col` of the shown text lands before:
    /// columns past the caret glyph and the left `…` count one less.
    pub fn index_at(&self, col: usize, caret: usize) -> usize {
        let mut k = col;
        if self.cut_left {
            k = k.saturating_sub(1);
        }
        let caret_col = caret.saturating_sub(self.first);
        if k > caret_col {
            k -= 1;
        }
        self.first + k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_land_at_the_caret() {
        let mut t = "12=255 13=0 14=40".to_owned();
        let mut c = Caret::end_of(&t);
        for _ in 0.."14=40".len() + 1 {
            c.step(&t, -1);
        }
        // Before the space ahead of 14: "0" of 13=0 goes, "128" comes in.
        c.backspace(&mut t);
        c.insert(&mut t, "128");
        assert_eq!(t, "12=255 13=128 14=40");
        c.delete(&mut t);
        assert_eq!(t, "12=255 13=12814=40");
        c.insert(&mut t, " ");
        c.home();
        c.delete(&mut t);
        assert_eq!(t, "2=255 13=128 14=40");
        c.end(&t);
        c.step(&t, 1);
        c.insert(&mut t, "é");
        c.backspace(&mut t);
        c.backspace(&mut t);
        assert_eq!(t, "2=255 13=128 14=4");
        c.home();
        c.backspace(&mut t);
        assert_eq!(c.index(&t), 0);
    }

    #[test]
    fn a_caret_past_a_shortened_text_clamps() {
        let mut t = "/light/red 1".to_owned();
        let mut c = Caret::end_of(&t);
        t.truncate(3);
        c.insert(&mut t, "x");
        assert_eq!(t, "/lix");
    }

    #[test]
    fn a_long_line_shows_a_window_around_the_caret() {
        let t = "12=255 13=0 14=40";
        assert_eq!(
            Shown::new(t, Caret::end_of(t), 30).text,
            "12=255 13=0 14=40▏"
        );
        assert_eq!(
            Shown::new(t, Caret::at(t, 6), 30).text,
            "12=255▏ 13=0 14=40"
        );
        // Cut to 10: the caret stays in view, … where the line goes on.
        assert_eq!(Shown::new(t, Caret::end_of(t), 10).text, "…=0 14=40▏");
        assert_eq!(Shown::new(t, Caret::at(t, 0), 10).text, "▏12=255 1…");
        let s = Shown::new(t, Caret::at(t, 10), 10);
        assert_eq!((s.text.as_str(), s.first), ("…=▏0 14=40", 9));
        let long = "1=1 2=2 3=3 4=4 5=5";
        let s = Shown::new(long, Caret::at(long, 10), 10);
        assert_eq!((s.text.as_str(), s.first), ("… 3=▏3 4=…", 7));
    }

    #[test]
    fn a_click_column_maps_to_the_line() {
        let t = "12=255 13=0";
        let s = Shown::new(t, Caret::end_of(t), 30);
        assert_eq!(s.index_at(2, 11), 2);
        let s = Shown::new(t, Caret::at(t, 3), 30);
        // "12=▏255 13=0": column 5 is the second 5, before it 4 characters of the line.
        assert_eq!(s.index_at(5, 3), 4);
        assert_eq!(s.index_at(1, 3), 1);
    }
}
