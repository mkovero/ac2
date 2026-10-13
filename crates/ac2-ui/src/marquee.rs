//! A line of text wider than its room scrolls through it: still at the start, slowly to the
//! end, still there, quickly back. Pure animation of where the text sits, from its width,
//! the room and the time; what the text says comes from the scene.

/// Still at each end long enough to read what is there.
pub const PAUSE_S: f64 = 2.0;
/// Slow enough to read while it moves.
pub const SPEED_PX_S: f64 = 35.0;
/// The way back carries nothing new to read.
pub const RETURN_SPEED_PX_S: f64 = 3.0 * SPEED_PX_S;
/// Frame period while the text moves: smooth at this speed (about one pixel per frame)
/// without drawing at the display's full rate.
pub const MOVING_FRAME_S: f64 = 1.0 / 30.0;

/// Where the text sits at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// How far the text is shifted left of the room's start, 0 up to its overflow.
    pub offset: f32,
    /// When it next moves, in seconds from now; `None` while it fits (or is held).
    pub wake: Option<f64>,
}

/// The text's place `t` seconds into its motion, `overflow` pixels wider than its room.
/// The cycle: hold at 0, scroll to `overflow`, hold, return; then again.
pub fn offset_at(overflow: f32, t: f64) -> Frame {
    if overflow <= 0.0 {
        return Frame {
            offset: 0.0,
            wake: None,
        };
    }
    let d = f64::from(overflow);
    let out = d / SPEED_PX_S;
    let back = d / RETURN_SPEED_PX_S;
    let cycle = PAUSE_S + out + PAUSE_S + back;
    let t = t.max(0.0) % cycle;
    let moving = |offset: f64| Frame {
        offset: offset.clamp(0.0, d) as f32,
        wake: Some(MOVING_FRAME_S),
    };
    if t < PAUSE_S {
        Frame {
            offset: 0.0,
            wake: Some(PAUSE_S - t),
        }
    } else if t < PAUSE_S + out {
        moving((t - PAUSE_S) * SPEED_PX_S)
    } else if t < PAUSE_S + out + PAUSE_S {
        Frame {
            offset: overflow,
            wake: Some(PAUSE_S + out + PAUSE_S - t),
        }
    } else {
        moving(d - (t - PAUSE_S - out - PAUSE_S) * RETURN_SPEED_PX_S)
    }
}

/// One scrolling line's clock: the time it has spent moving, which stands still while the
/// pointer rests on it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Marquee {
    motion: f64,
    last: Option<f64>,
    held: bool,
}

impl Marquee {
    /// A line that has been moving for `motion` seconds; its next frame starts from there.
    pub fn at(motion: f64) -> Self {
        Self {
            motion,
            ..Self::default()
        }
    }

    /// The text's place at `now` (seconds, any monotonic origin) for a text `text_w` wide
    /// in `room`. Text that fits sits still and restarts the cycle from its start pause the
    /// next time it does not.
    pub fn frame(&mut self, now: f64, text_w: f32, room: f32, hovered: bool) -> Frame {
        let overflow = text_w - room;
        if overflow <= 0.0 {
            *self = Self::default();
            return offset_at(0.0, 0.0);
        }
        // The time since the last pass counts only if the pointer was away for all of it.
        if let Some(last) = self.last
            && !hovered
            && !self.held
        {
            self.motion += (now - last).max(0.0);
        }
        self.last = Some(now);
        self.held = hovered;
        let f = offset_at(overflow, self.motion);
        if hovered {
            // The pointer leaving wakes the window anyway.
            Frame { wake: None, ..f }
        } else {
            f
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
                    wake: None
                }
            );
        }
    }

    #[test]
    fn the_start_pause_holds_still_and_wakes_once_when_it_ends() {
        let mut m = Marquee::default();
        let f = run(&mut m, 10.0, 500.0, 300.0);
        assert_eq!(f.offset, 0.0);
        assert!((f.wake.expect("a wake") - PAUSE_S).abs() < 1e-9);
        let f = run(&mut m, 11.5, 500.0, 300.0);
        assert_eq!(f.offset, 0.0);
        assert!((f.wake.expect("a wake") - 0.5).abs() < 1e-9);
        let f = run(&mut m, 12.5, 500.0, 300.0);
        assert!((f.offset - 0.5 * SPEED_PX_S as f32).abs() < 1e-3, "{f:?}");
        assert_eq!(f.wake, Some(MOVING_FRAME_S));
    }

    #[test]
    fn the_offset_reaches_the_end_holds_there_and_returns() {
        let overflow = 70.0;
        let out = f64::from(overflow) / SPEED_PX_S;
        let back = f64::from(overflow) / RETURN_SPEED_PX_S;
        let end = offset_at(overflow, PAUSE_S + out + 0.1);
        assert_eq!(end.offset, overflow);
        assert!((end.wake.expect("a wake") - (PAUSE_S - 0.1)).abs() < 1e-9);
        let mid_back = offset_at(overflow, PAUSE_S + out + PAUSE_S + back / 2.0);
        assert!(
            (mid_back.offset - overflow / 2.0).abs() < 1e-3,
            "{mid_back:?}"
        );
        // Then the cycle starts again from its start pause.
        let again = offset_at(overflow, 2.0 * PAUSE_S + out + back + 0.5);
        assert_eq!(again.offset, 0.0);
        // Never past either end.
        for k in 0..2000 {
            let o = offset_at(overflow, f64::from(k) * 0.01).offset;
            assert!((0.0..=overflow).contains(&o), "{o}");
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
    fn one_pixel_too_wide_scrolls_that_pixel() {
        let mut m = Marquee::default();
        let f = run(&mut m, 0.0, 301.0, 300.0);
        assert_eq!(f.offset, 0.0);
        assert!(f.wake.is_some());
        let end = run(&mut m, PAUSE_S + 1.0 / SPEED_PX_S + 0.01, 301.0, 300.0);
        assert_eq!(end.offset, 1.0);
        // And fitting again resets it to the start.
        assert_eq!(run(&mut m, 4.0, 300.0, 300.0).offset, 0.0);
        let f = run(&mut m, 30.0, 301.0, 300.0);
        assert_eq!(f.offset, 0.0);
        assert!((f.wake.expect("a wake") - PAUSE_S).abs() < 1e-9);
    }
}
