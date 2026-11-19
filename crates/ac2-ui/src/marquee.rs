//! A line of text wider than its room scrolls through it: leftwards at a steady pace, the
//! line following itself after a gap, so its start comes round again without a jump back.
//! Pure animation of where the text sits, from its width, the room and the time; what the
//! text says comes from the scene.

/// Slow enough to read while it moves.
pub const SPEED_PX_S: f64 = 30.0;
/// The space between the line's end and its start coming round: wide enough to read as
/// the end of the line, not as a space in it.
pub const GAP_PX: f32 = 48.0;
/// Frame period the scroll asks for on its own: 12.5 frames a second, about 2.4 px a step.
/// A kiosk that draws nothing else then draws at this rate, not the display's; a window
/// already drawing faster (live data) moves the line more smoothly for free.
pub const STEP_S: f64 = 0.08;

/// Where the text sits at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// How far the text is shifted left of the room's start, 0 up to its period.
    pub offset: f32,
    /// The text follows itself this far behind (its width and the gap); `None` while it
    /// fits and is drawn once.
    pub period: Option<f32>,
    /// When it next moves, in seconds from now; `None` while it fits or is held.
    pub wake: Option<f64>,
}

/// One scrolling line's clock: how far it has scrolled, which stands still while the
/// pointer rests on it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Marquee {
    /// Pixels scrolled, within the period. Kept as a distance, not a time: a text whose
    /// width changes under way (a figure gaining a digit) then stays where it is.
    offset: f64,
    last: Option<f64>,
    held: bool,
}

impl Marquee {
    /// A line that has been moving for `motion` seconds; its next frame starts from there.
    pub fn at(motion: f64) -> Self {
        Self {
            offset: motion * SPEED_PX_S,
            ..Self::default()
        }
    }

    /// How far the line was scrolled at its last frame.
    pub fn offset(&self) -> f32 {
        self.offset as f32
    }

    /// The text's place at `now` (seconds, any monotonic origin) for a text `text_w` wide
    /// in `room`; `hovered`: the pointer is over the room. Text that fits sits still, and
    /// starts from its start the next time it does not.
    pub fn frame(&mut self, now: f64, text_w: f32, room: f32, hovered: bool) -> Frame {
        if text_w <= room {
            *self = Self::default();
            return Frame {
                offset: 0.0,
                period: None,
                wake: None,
            };
        }
        let period = f64::from(text_w + GAP_PX);
        // The time since the last pass counts only if the pointer was away for all of it.
        if let Some(last) = self.last
            && !hovered
            && !self.held
        {
            self.offset += (now - last).max(0.0) * SPEED_PX_S;
        }
        self.offset %= period;
        self.last = Some(now);
        self.held = hovered;
        // Narrowed to the drawing's precision, a hair under the period can round up to it.
        let period = period as f32;
        let offset = self.offset as f32;
        Frame {
            offset: if offset < period { offset } else { 0.0 },
            period: Some(period),
            // Held, the pointer leaving wakes the window anyway.
            wake: (!hovered).then_some(STEP_S),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(m: &mut Marquee, t: f64, text_w: f32, room: f32) -> Frame {
        m.frame(t, text_w, room, false)
    }

    #[test]
    fn text_that_fits_stays_still_and_asks_for_no_frames() {
        let mut m = Marquee::default();
        for t in [0.0, 1.0, 5.0, 60.0] {
            let f = run(&mut m, t, 300.0, 300.0);
            assert_eq!(
                f,
                Frame {
                    offset: 0.0,
                    period: None,
                    wake: None
                }
            );
        }
    }

    #[test]
    fn it_scrolls_at_a_steady_pace_and_asks_for_slow_frames() {
        let mut m = Marquee::default();
        let f = run(&mut m, 10.0, 500.0, 300.0);
        assert_eq!(f.offset, 0.0);
        assert_eq!(f.period, Some(500.0 + GAP_PX));
        assert_eq!(f.wake, Some(STEP_S));
        let f = run(&mut m, 12.0, 500.0, 300.0);
        assert!((f.offset - 2.0 * SPEED_PX_S as f32).abs() < 1e-3, "{f:?}");
        assert_eq!(f.wake, Some(STEP_S));
        // No more than 15 frames a second of its own.
        const { assert!(STEP_S >= 1.0 / 15.0) };
    }

    #[test]
    fn it_wraps_around_after_its_width_and_the_gap() {
        let mut m = Marquee::default();
        let period = f64::from(500.0 + GAP_PX);
        run(&mut m, 0.0, 500.0, 300.0);
        let almost = run(&mut m, (period - 3.0) / SPEED_PX_S, 500.0, 300.0);
        assert!(
            (almost.offset - (period as f32 - 3.0)).abs() < 1e-2,
            "{almost:?}"
        );
        let round = run(&mut m, (period + 6.0) / SPEED_PX_S, 500.0, 300.0);
        assert!((round.offset - 6.0).abs() < 1e-2, "{round:?}");
        // Never past the period, however long between two frames.
        for k in 0..2000 {
            let o = run(&mut m, 40.0 + f64::from(k) * 0.37, 500.0, 300.0).offset;
            assert!((0.0..period as f32).contains(&o), "{o}");
        }
    }

    #[test]
    fn hovering_pauses_the_scroll() {
        let mut m = Marquee::default();
        run(&mut m, 0.0, 600.0, 300.0);
        let before = run(&mut m, 3.0, 600.0, 300.0);
        assert!(before.offset > 0.0);
        let held = m.frame(3.1, 600.0, 300.0, true);
        assert_eq!(held.offset, before.offset);
        assert_eq!(held.wake, None);
        let still = m.frame(13.0, 600.0, 300.0, true);
        assert_eq!(still.offset, before.offset);
        // The pointer left somewhere in the last gap: that gap does not count either.
        let left = run(&mut m, 20.0, 600.0, 300.0);
        assert_eq!(left.offset, before.offset);
        let on = run(&mut m, 21.0, 600.0, 300.0);
        assert!(
            (on.offset - before.offset - SPEED_PX_S as f32).abs() < 1e-3,
            "{on:?}"
        );
    }

    #[test]
    fn a_text_changing_width_under_way_stays_where_it_is() {
        let mut m = Marquee::default();
        run(&mut m, 0.0, 600.0, 300.0);
        let a = run(&mut m, 4.0, 600.0, 300.0);
        let b = run(&mut m, 4.0, 640.0, 300.0);
        assert_eq!(a.offset, b.offset);
        // And fitting again resets it to the start.
        assert_eq!(run(&mut m, 5.0, 300.0, 300.0).offset, 0.0);
        let f = run(&mut m, 30.0, 301.0, 300.0);
        assert_eq!(f.offset, 0.0);
    }
}
