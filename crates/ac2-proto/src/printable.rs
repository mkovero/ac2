//! Text a peer sends that the daemon logs or keeps: bounded, one line, nothing a terminal or
//! a log reader would act on.
//!
//! A log line is the record an operator reads after the fact; a newline in a peer's string
//! would let it write a line that reads as the daemon's own, and a string of megabytes would
//! rotate the log files until the lines before it are gone.

use serde::{Deserialize, Deserializer};

/// Most characters of a `hello`'s client string: a program name and a version.
pub const MAX_CLIENT_CHARS: usize = 64;

/// `s` in at most `max` characters (an ellipsis takes the last place when it is cut) with
/// every control character (newline, escape, the C1 range) and every bidirectional
/// override replaced by `?`.
pub fn printable(s: &str, max: usize) -> String {
    let cut = s.chars().nth(max).is_some();
    let keep = if cut { max.saturating_sub(1) } else { max };
    let mut out: String = s
        .chars()
        .take(keep)
        .map(|c| {
            if c.is_control() || is_bidi_control(c) {
                '?'
            } else {
                c
            }
        })
        .collect();
    if cut {
        out.push('…');
    }
    out
}

/// The format characters that reorder the text around them when shown: a line that
/// displays differently from the bytes it holds.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
}

/// `hello`'s client string as decoded: [`printable`] to [`MAX_CLIENT_CHARS`].
pub(crate) fn client_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    Ok(printable(&s, MAX_CLIENT_CHARS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_and_one_line() {
        assert_eq!(printable("ac2-ui 0.9", 64), "ac2-ui 0.9");
        assert_eq!(printable("a\nb\r\x1b[2Jc", 64), "a?b??[2Jc");
        assert_eq!(printable("abcdef", 3), "ab…");
        assert_eq!(printable("abc", 3), "abc");
        assert_eq!(printable("x\u{202e}y\u{85}", 64), "x?y?");
        assert_eq!(printable(&"é".repeat(100), 64).chars().count(), 64);
    }
}
