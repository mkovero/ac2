//! The recording indicator and the replay label the top bar shows (text from
//! `ac2_scene::recording`).

use ac2_scene::recording::{RecordingLabel, recording_label, replay_label};

use crate::state::AppState;

impl AppState {
    /// The latest recording as shown, with the inputs named as the session names them;
    /// `None` when nothing was recorded (or nothing is connected).
    pub fn recording_label(&self) -> Option<RecordingLabel> {
        let r = self.daemon()?.recording.as_ref()?;
        let names = self.session_input_labels();
        Some(recording_label(r, |input| {
            names
                .iter()
                .find(|(c, _)| *c == input)
                .map_or_else(|| format!("Input {}", input + 1), |(_, n)| n.clone())
        }))
    }

    /// `REPLAY <name> · <length>` while the session plays a recording.
    pub fn replay_label(&self) -> Option<String> {
        replay_label(self.open_session()?, None)
    }
}
