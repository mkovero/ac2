//! The stimulus of the rig shown, fixed at the top bar's right end beside the stop: what
//! it is doing, what plays and where, and what the stimulus keys do next.
//!
//! It never scrolls with the bar's line: the one thing the operator must read before a key
//! makes a room loud is whether something is armed and what Space and Enter will do now.

use crate::top_bar::Tone;

/// The generator of the rig shown, as the bar says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StimState {
    Off,
    /// Arm requested.
    Arming,
    /// Armed and silent.
    Armed,
    /// Fire requested: it may already sound.
    FirePending,
    Firing,
    /// Stop requested.
    Stopping,
}

impl StimState {
    /// The state's word, as the bar has always badged it.
    pub fn badge(self) -> &'static str {
        match self {
            StimState::Off => "STIM OFF",
            StimState::Arming => "ARMING…",
            StimState::Armed => "ARMED",
            StimState::FirePending => "FIRE…",
            StimState::Firing => "FIRING",
            StimState::Stopping => "STOPPING…",
        }
    }

    /// Armed, playing or on the way to either: nothing about it is off.
    pub fn live(self) -> bool {
        !matches!(self, StimState::Off | StimState::Stopping)
    }
}

/// What the item is built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StimInputs<'a> {
    pub state: StimState,
    /// What plays and where, `pink −20.0 dBFS → out 1, 2`: said after the state word while
    /// it is armed or plays, a hover away while it is off.
    pub plays: &'a str,
    /// Another client holds the generator, by name: this app's Space and Enter are not its.
    pub held_by: Option<&'a str>,
    /// Off: what Space (or the level key) does on the focused view, short (`Space arms`,
    /// `L types a level`, `Space: sweep…`); `None` while a window has those keys.
    pub off_keys: Option<&'a str>,
    /// The stimulus keys reach the stimulus: no window has them.
    pub keys: bool,
    /// More than one rig: the rig shown, by name, so the keys are read as acting on it.
    pub rig: Option<&'a str>,
    /// The long form of what Space or Enter starts next, for the hover.
    pub next_long: Option<&'a str>,
    /// Where the level and the signal are set, for the hover.
    pub tip: &'a str,
}

/// The item: the state in its tone, then the keys and who holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StimItem {
    /// `STIM OFF`, `FIRING pink −20.0 dBFS → out 1`, with more rigs `pupu: STIM OFF`.
    pub state: String,
    pub tone: Tone,
    /// Bold while armed or playing.
    pub strong: bool,
    /// What the keys do now: `Space arms`, `Enter fires · Space disarms`, `Space or Enter
    /// stops`.
    pub keys: Option<String>,
    /// `held by alice`.
    pub held: Option<String>,
    pub hover: String,
    /// The state word alone, `ARMED`, `pupu: FIRING`: what a narrow bar keeps.
    pub short: String,
}

impl StimItem {
    /// The item as one line, its parts joined by ` · `.
    pub fn text(&self) -> String {
        [
            Some(self.state.as_str()),
            self.held.as_deref(),
            self.keys.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }

    /// The item for a bar with less room, `step` 0 being all of it: the keys go first, then
    /// who holds it, then what plays, down to the state word; the hover keeps all of it.
    pub fn narrowed(&self, step: usize) -> StimItem {
        let mut n = self.clone();
        if step >= 1 {
            n.keys = None;
        }
        if step >= 2 {
            n.held = None;
        }
        if step >= 3 && n.state != n.short {
            n.hover = format!("{} · {}", self.text(), self.hover);
            n.state = n.short.clone();
        }
        n
    }

    /// The steps [`Self::narrowed`] takes.
    pub const STEPS: usize = 4;
}

/// Armed: what the keys do.
pub const ARMED_KEYS: &str = "Enter fires · Space disarms";
/// Playing or about to: what the keys do.
pub const PLAYING_KEYS: &str = "Space or Enter stops";

/// The item for `i`.
pub fn stim_item(i: &StimInputs<'_>) -> StimItem {
    let s = i.state;
    let word = if s.live() {
        format!("{} {}", s.badge(), i.plays)
    } else {
        s.badge().to_owned()
    };
    let (state, short) = match i.rig {
        Some(rig) => (format!("{rig}: {word}"), format!("{rig}: {}", s.badge())),
        None => (word, s.badge().to_owned()),
    };
    let tone = match s {
        StimState::Off | StimState::Stopping => Tone::Dim,
        StimState::Arming | StimState::Armed | StimState::FirePending => Tone::Warn,
        StimState::Firing => Tone::Fault,
    };
    // Another client's stimulus: Space and Enter arm and fire this app's, so only the stop
    // (beside the item) is said.
    let keys = match (s, i.held_by, i.keys) {
        (StimState::Off, None, _) => i.off_keys.map(str::to_owned),
        // Arming, a second Space does nothing and Enter has nothing to fire yet: only the
        // stop stands beside it.
        (_, Some(_), _) | (_, _, false) | (StimState::Stopping | StimState::Arming, ..) => None,
        (StimState::Armed, ..) => Some(ARMED_KEYS.to_owned()),
        (StimState::FirePending | StimState::Firing, ..) => Some(PLAYING_KEYS.to_owned()),
    };
    let mut hover = Vec::new();
    if !s.live() {
        hover.push(i.plays.to_owned());
    }
    if let Some(l) = i.next_long {
        hover.push(l.to_owned());
    }
    hover.push(i.tip.to_owned());
    StimItem {
        state,
        tone,
        strong: s.live(),
        keys,
        held: i.held_by.map(|w| format!("held by {w}")),
        hover: hover.join(" · "),
        short,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(state: StimState) -> StimInputs<'static> {
        StimInputs {
            state,
            plays: "pink −20.0 dBFS → out 1, 2",
            held_by: None,
            off_keys: Some("Space arms"),
            keys: true,
            rig: None,
            next_long: None,
            tip: "L types the level",
        }
    }

    fn text(i: StimInputs<'_>) -> String {
        stim_item(&i).text()
    }

    #[test]
    fn every_state_says_its_word_and_its_keys() {
        let cases = [
            (StimState::Off, "STIM OFF · Space arms", Tone::Dim),
            (
                StimState::Arming,
                "ARMING… pink −20.0 dBFS → out 1, 2",
                Tone::Warn,
            ),
            (
                StimState::Armed,
                "ARMED pink −20.0 dBFS → out 1, 2 · Enter fires · Space disarms",
                Tone::Warn,
            ),
            (
                StimState::FirePending,
                "FIRE… pink −20.0 dBFS → out 1, 2 · Space or Enter stops",
                Tone::Warn,
            ),
            (
                StimState::Firing,
                "FIRING pink −20.0 dBFS → out 1, 2 · Space or Enter stops",
                Tone::Fault,
            ),
            (StimState::Stopping, "STOPPING…", Tone::Dim),
        ];
        for (s, want, tone) in cases {
            let item = stim_item(&at(s));
            assert_eq!(item.text(), want);
            assert_eq!(item.tone, tone, "{s:?}");
            assert_eq!(item.strong, s.live(), "{s:?}");
        }
    }

    #[test]
    fn off_without_a_level_names_the_level_key() {
        let mut i = at(StimState::Off);
        i.off_keys = Some("L types a level");
        assert_eq!(text(i), "STIM OFF · L types a level");
    }

    #[test]
    fn off_the_hover_says_what_would_play() {
        let mut i = at(StimState::Off);
        i.next_long = Some("Space arms: pink −20 dBFS → out 1, 2");
        assert_eq!(
            stim_item(&i).hover,
            "pink −20.0 dBFS → out 1, 2 · Space arms: pink −20 dBFS → out 1, 2 · L types the \
             level"
        );
        assert_eq!(stim_item(&at(StimState::Firing)).hover, "L types the level");
    }

    #[test]
    fn with_more_rigs_it_names_the_rig_shown() {
        let mut i = at(StimState::Off);
        i.rig = Some("pupu");
        assert_eq!(text(i), "pupu: STIM OFF · Space arms");
        i.state = StimState::Firing;
        assert_eq!(
            text(i),
            "pupu: FIRING pink −20.0 dBFS → out 1, 2 · Space or Enter stops"
        );
    }

    #[test]
    fn another_clients_stimulus_says_who_and_no_keys() {
        let mut i = at(StimState::Firing);
        i.held_by = Some("alice");
        let item = stim_item(&i);
        assert_eq!(
            item.text(),
            "FIRING pink −20.0 dBFS → out 1, 2 · held by alice"
        );
        assert_eq!(item.held.as_deref(), Some("held by alice"));
        i.state = StimState::Armed;
        assert_eq!(text(i), "ARMED pink −20.0 dBFS → out 1, 2 · held by alice");
    }

    #[test]
    fn with_a_window_open_the_keys_are_the_windows() {
        let mut i = at(StimState::Armed);
        i.keys = false;
        assert_eq!(text(i), "ARMED pink −20.0 dBFS → out 1, 2");
        i.state = StimState::Firing;
        assert_eq!(text(i), "FIRING pink −20.0 dBFS → out 1, 2");
        i.state = StimState::Off;
        i.off_keys = None;
        assert_eq!(text(i), "STIM OFF");
    }

    /// A narrow bar keeps the state word to the last; what it drops is in the hover.
    #[test]
    fn a_narrow_bar_drops_the_keys_then_the_holder_then_what_plays() {
        let mut i = at(StimState::Firing);
        i.held_by = Some("alice");
        i.rig = Some("pupu");
        let item = stim_item(&i);
        let texts: Vec<String> = (0..StimItem::STEPS)
            .map(|n| item.narrowed(n).text())
            .collect();
        assert_eq!(
            texts,
            [
                "pupu: FIRING pink −20.0 dBFS → out 1, 2 · held by alice",
                "pupu: FIRING pink −20.0 dBFS → out 1, 2 · held by alice",
                "pupu: FIRING pink −20.0 dBFS → out 1, 2",
                "pupu: FIRING",
            ]
        );
        assert_eq!(
            item.narrowed(3).hover,
            "pupu: FIRING pink −20.0 dBFS → out 1, 2 · held by alice · L types the level"
        );
        let off = stim_item(&at(StimState::Off));
        assert_eq!(off.narrowed(3).text(), "STIM OFF");
        assert_eq!(off.narrowed(3).hover, off.hover);
    }
}
