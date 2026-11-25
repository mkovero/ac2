//! Reducer tests of the frequency range a view starts on and Home goes back to.

use super::*;
use ac2_scene::view::FreqRange;

fn at_rate(t: &mut T, rate: u32) {
    t.remirror(|s| {
        s.session.open.as_mut().expect("session").sample_rate_hz = rate;
    });
}

/// A 96 kHz session's view runs up to 48 kHz, a 192 kHz one's to 96 kHz; at 48 kHz it is
/// the audio band. Home goes back there, and a range the operator zoomed to stays when
/// the rate changes.
#[test]
fn the_view_runs_up_to_a_high_rate_sessions_nyquist() {
    let mut t = T::new();
    assert_eq!(t.st.nav.target, FreqRange::default());
    at_rate(&mut t, 96_000);
    assert_eq!((t.st.nav.target.lo, t.st.nav.target.hi), (20.0, 48_000.0));
    at_rate(&mut t, 192_000);
    assert_eq!(t.st.nav.target.hi, 96_000.0);
    // Zoomed by hand: Home returns to the session's range, not to 20 kHz.
    t.key("I");
    assert_ne!(t.st.nav.target.hi, 96_000.0);
    t.key("Home");
    assert_eq!(t.st.nav.target.hi, 96_000.0);
    t.key("I");
    let zoomed = t.st.nav.target;
    at_rate(&mut t, 48_000);
    assert_eq!(t.st.nav.target, zoomed, "the operator's range stays");
    t.key("Home");
    assert_eq!(t.st.nav.target, FreqRange::default());
}
