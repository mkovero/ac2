//! The reducer's side of the stimulus signal rows of Settings › Audio I/O.

use ac2_scene::stimulus::{SignalChoice, SignalKind, SignalRow};

use super::{AppState, Overlay};
use crate::conn::Request;

impl AppState {
    /// A stimulus signal row acted on: ←/→ (`d`) on a choice row, Enter on the frequency
    /// row. The change is this app's and is sent at once while armed or playing.
    pub(super) fn signal_row(&mut self, row: SignalRow, d: i32, out: &mut Vec<Request>) {
        let rate = self
            .daemon()
            .and_then(|s| s.session.open.as_ref())
            .map(|o| o.sample_rate_hz);
        let mut sine = self.rig.stimulus.sine;
        let kind = match row {
            SignalRow::Signal => SignalKind::step(self.rig.stimulus.generator, d),
            SignalRow::Frequency => {
                let Overlay::Settings(s) = &mut self.overlay else {
                    return;
                };
                let Some(freq) = s.signal.enter(rate) else {
                    return;
                };
                sine.freq = freq;
                SignalKind::Sine
            }
            SignalRow::OnBin => {
                sine.on_bin = !sine.on_bin;
                SignalKind::Sine
            }
        };
        let choice = SignalChoice::of(kind, sine);
        self.choose_signal(choice, out);
    }
}
