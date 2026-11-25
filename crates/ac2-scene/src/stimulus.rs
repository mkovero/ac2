//! What the stimulus keys start next, as the top bar says it.
//!
//! The focused view decides what Space arms: the sweep view a run of the selected sweep
//! measurement with its settings (the new-sweep-measurement dialog when there is none),
//! every other view the noise generator for live measuring. Enter fires what is armed. The hint names it with its
//! level, so the operator reads what will play before it plays.

use ac2_proto::model::{OutputSetup, Signal};
use ac2_proto::units::{ClientId, Dbfs, Hz};

use crate::format;

/// What the stimulus keys would play.
#[derive(Clone, Debug, PartialEq)]
pub enum Stimulus {
    /// The generator: a noise (or tone) at a level on outputs (zero-based), named by the
    /// rig's `labels` where it has them.
    Generator {
        signal: Signal,
        level: Option<Dbfs>,
        outputs: Vec<u16>,
        labels: Vec<OutputSetup>,
    },
    /// A run of the sweep measurement named `meas`, with its settings.
    Sweep {
        meas: String,
        duration_s: f64,
        level: Option<Dbfs>,
    },
    /// The sweep view with no sweep measurement yet: Space opens the dialog that makes one.
    SweepDialog,
    /// A live view before the operator chose the generator's signal: Space opens the
    /// dialog that asks for it.
    SignalDialog,
}

/// Which key the hint is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Idle: Space arms.
    Space,
    /// Armed: Enter fires.
    Enter,
}

/// The top bar's hint, long and short: `Enter fires: sweep Genelec 1 m · 3 s −50 dBFS`,
/// `Space arms: pink −50 dBFS → out 1`. `level_key` names the key that types a level, for
/// the generator without one.
pub fn hint(next: Next, what: &Stimulus, level_key: &str) -> (String, String) {
    match (next, what) {
        (Next::Space, Stimulus::SweepDialog) => (
            "Space sets up a sweep measurement".into(),
            "Space: sweep…".into(),
        ),
        (Next::Space, Stimulus::SignalDialog) => (
            "Space asks what to play: pink, white or a sine".into(),
            "Space: signal…".into(),
        ),
        (Next::Space, Stimulus::Generator { level: None, .. }) => {
            let s = format!("{level_key} types a level");
            (s.clone(), s)
        }
        (Next::Space, w) => (format!("Space arms: {}", describe(w)), "Space arms".into()),
        (Next::Enter, w) => (
            format!("Enter fires: {}", describe(w)),
            "Enter fires".into(),
        ),
    }
}

/// `pink −50 dBFS → out 1`, `sweep Genelec 1 m · 3 s −50 dBFS`.
pub fn describe(what: &Stimulus) -> String {
    match what {
        Stimulus::Generator {
            signal,
            level,
            outputs,
            labels,
        } => format!(
            "{} {} → {}",
            signal_name(signal),
            level_text(*level),
            crate::rig::stimulus_outputs(outputs, labels)
        ),
        Stimulus::Sweep {
            meas,
            duration_s,
            level,
        } => format!(
            "sweep {meas} · {} s {}",
            short_number(*duration_s),
            level_text(*level)
        ),
        Stimulus::SweepDialog => "new sweep measurement".into(),
        Stimulus::SignalDialog => "no signal chosen".into(),
    }
}

/// What this app's own stimulus is doing, for a transfer measurement whose reference
/// carries nothing (NO REFERENCE): with nothing armed or playing here the likely cause is
/// that nobody started it, and the texts name the keys that start it; with it playing, the
/// patch between output and reference input is what is left to check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Drive {
    /// Playing, a request in flight, or another client's stimulus: the loopback patch is
    /// the suspect.
    #[default]
    Playing,
    /// Nothing armed or playing: `arm` arms the generator on this view, `fire` plays it.
    Idle { arm: String, fire: String },
    /// The generator armed and silent: `fire` plays it.
    Armed { fire: String },
    /// Nothing playing and the sweep view focused: there the arm key arms a sweep, so the
    /// generator is armed from a transfer pane.
    SweepView,
}

/// NO REFERENCE's detail: `stimulus off: Space arms, Enter starts it`. Every variant is
/// short enough to sit beside NO REFERENCE in a banner row in every theme (a longer one is
/// left out of the row, and then the operator reads only the fault).
pub fn no_reference_detail(d: &Drive) -> String {
    match d {
        Drive::Playing => "reference silent: check the loopback cable".into(),
        Drive::Idle { arm, fire } => format!("stimulus off: {arm} arms, {fire} starts it"),
        Drive::Armed { fire } => format!("stimulus armed: {fire} starts it"),
        Drive::SweepView => "stimulus off: arm it from a transfer pane".into(),
    }
}

/// The empty IR pane's reason without a reference, which has the room to say it whole:
/// `no reference: nothing is playing — Space arms, Enter starts the stimulus` (it breaks
/// at the dash in a narrow pane).
pub fn no_reference_note(d: &Drive) -> String {
    match d {
        Drive::Playing => "no reference: nothing is driving the loopback".into(),
        Drive::Idle { arm, fire } => {
            format!("no reference: nothing is playing — {arm} arms, {fire} starts the stimulus")
        }
        Drive::Armed { fire } => format!("no reference: armed — {fire} starts the stimulus"),
        Drive::SweepView => {
            "no reference: nothing is playing — arm the stimulus from a transfer pane".into()
        }
    }
}

/// Which signal the generator plays for the live views: a noise or a sine, never a sweep
/// (a sweep is a measurement's, armed from the sweep view).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalKind {
    Pink,
    White,
    Sine,
}

impl SignalKind {
    pub const ALL: [SignalKind; 3] = [SignalKind::Pink, SignalKind::White, SignalKind::Sine];

    /// ←/→ on the signal row from `now` (`None`: not chosen yet, so → starts at the first
    /// and ← at the last).
    pub fn step(now: Option<Self>, d: i32) -> Self {
        let n = Self::ALL.len() as i32;
        let i = match now.and_then(|k| Self::ALL.iter().position(|x| *x == k)) {
            Some(i) => i as i32 + d,
            None if d < 0 => n - 1,
            None => 0,
        };
        Self::ALL[i.rem_euclid(n) as usize]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Pink => "pink noise",
            Self::White => "white noise",
            Self::Sine => "sine",
        }
    }
}

/// The sine as the operator set it: the frequency typed, and whether it is moved onto an
/// FFT bin when it is played.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SineSetting {
    pub freq: Hz,
    pub on_bin: bool,
}

/// The sine before the operator types another: 1 kHz, where distortion is quoted unless
/// said otherwise, on a bin.
pub const DEFAULT_SINE: SineSetting = SineSetting {
    freq: Hz(1000.0),
    on_bin: true,
};

/// The generator's signal as the operator chose it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SignalChoice {
    Pink,
    White,
    Sine { freq: Hz, on_bin: bool },
}

/// A sine's frequency as its settings row and the signal dialog read it: as every
/// frequency field does (`997`, `1k`, `1.5 kHz`). `rate_hz` is the open stream's sample
/// rate when known: a sine at or above half of it cannot be played.
pub fn parse_sine_freq(text: &str, rate_hz: Option<u32>) -> Result<Hz, String> {
    let hz = crate::unit_text::in_field::<crate::unit_text::Freq>(text, "hz")?
        .0
        .0;
    if let Some(fs) = rate_hz
        && hz >= f64::from(fs) / 2.0
    {
        return Err(format!(
            "a sine must be below {}, half the sample rate",
            format::freq_exact(f64::from(fs) / 2.0)
        ));
    }
    Ok(Hz(hz))
}

/// The frequency row's text when Enter takes it, in Settings and in the signal dialog: a
/// frequency ([`parse_sine_freq`]), or what to type instead.
pub fn typed_sine_freq(text: &str, rate_hz: Option<u32>) -> Result<Hz, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(SINE_FREQ_EMPTY.into());
    }
    parse_sine_freq(text, rate_hz)
}

impl SignalChoice {
    /// `kind` with the sine as set.
    pub fn of(kind: SignalKind, sine: SineSetting) -> Self {
        match kind {
            SignalKind::Pink => Self::Pink,
            SignalKind::White => Self::White,
            SignalKind::Sine => Self::Sine {
                freq: sine.freq,
                on_bin: sine.on_bin,
            },
        }
    }

    pub fn kind(self) -> SignalKind {
        match self {
            Self::Pink => SignalKind::Pink,
            Self::White => SignalKind::White,
            Self::Sine { .. } => SignalKind::Sine,
        }
    }

    /// What the generator is asked to play. `bins`: the FFT length whose bins a sine goes
    /// onto and the sample rate (`None`: no spectrum is running to read it). A sine set to
    /// be on a bin is moved to the nearest one; any other is played as typed.
    pub fn played(self, bins: Option<(usize, f64)>) -> Signal {
        match (self, bins) {
            (Self::Pink, _) => Signal::Pink,
            (Self::White, _) => Signal::White,
            (Self::Sine { freq, on_bin: true }, Some((n, fs))) => Signal::Sine {
                freq: Hz(ac2_proto::model::nearest_bin(freq.0, fs, n)),
            },
            (Self::Sine { freq, .. }, _) => Signal::Sine { freq },
        }
    }

    /// What the app says once the choice is made, with the frequency really played and
    /// what was done about bins (`bins` as in [`Self::played`]).
    pub fn chosen_text(self, bins: Option<(usize, f64)>) -> String {
        let name = signal_name(&self.played(bins));
        match (self, bins) {
            (Self::Pink, _) => "pink noise".into(),
            (Self::White, _) => "white noise".into(),
            (Self::Sine { on_bin: true, .. }, Some(_)) => {
                format!("{name} · on an FFT bin of the spectrum")
            }
            (Self::Sine { on_bin: true, .. }, None) => {
                format!("{name} · no spectrum running, played as typed")
            }
            (Self::Sine { on_bin: false, .. }, _) => format!("{name} · as typed"),
        }
    }
}

/// A row of the stimulus signal choice, in Settings › Audio I/O and in the dialog the
/// first arm opens: both show [`signal_rows`], so they name and word the rows alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalRow {
    /// Pink noise, white noise or a sine: ←/→.
    Signal,
    /// The sine's frequency, typed. Shown with a sine.
    Frequency,
    /// Whether the sine is moved onto an FFT bin: ←/→. Shown with a sine.
    OnBin,
}

impl SignalRow {
    /// The rows shown: the sine's own only while the signal is a sine, since a noise has
    /// no frequency to set.
    pub fn shown(sine: bool) -> &'static [SignalRow] {
        if sine {
            &[SignalRow::Signal, SignalRow::Frequency, SignalRow::OnBin]
        } else {
            &[SignalRow::Signal]
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            SignalRow::Signal => SIGNAL_ROW,
            SignalRow::Frequency => SINE_FREQ_ROW,
            SignalRow::OnBin => ON_BIN_ROW,
        }
    }

    /// The hint line under the row in Settings.
    pub fn settings_hint(self) -> &'static str {
        match self {
            SignalRow::Signal => SIGNAL_ROW_HINT,
            SignalRow::Frequency => SINE_FREQ_HINT,
            SignalRow::OnBin => ON_BIN_HINT,
        }
    }
}

/// A row of the signal choice as shown: its title and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalRowText {
    pub row: SignalRow,
    pub title: &'static str,
    pub value: String,
}

/// The rows of the signal choice for `kind` (`None`: not chosen yet) and the sine as set.
/// `bins` as in [`SignalChoice::played`].
pub fn signal_rows(
    kind: Option<SignalKind>,
    sine: SineSetting,
    bins: Option<(usize, f64)>,
) -> Vec<SignalRowText> {
    SignalRow::shown(kind == Some(SignalKind::Sine))
        .iter()
        .map(|&row| SignalRowText {
            row,
            title: row.title(),
            value: match row {
                SignalRow::Signal => signal_row_value(kind).to_owned(),
                SignalRow::Frequency => sine_freq_value(sine, bins),
                SignalRow::OnBin => on_bin_value(sine.on_bin, bins),
            },
        })
        .collect()
}

/// The settings section of the rows.
pub const SIGNAL_SECTION: &str = "Stimulus signal";
/// The signal row: title, the value before a choice is made, its hint line.
pub const SIGNAL_ROW: &str = "Signal";
pub const SIGNAL_UNCHOSEN: &str = "not chosen · asked at the first arm";
pub const SIGNAL_ROW_HINT: &str = "What the generator plays on the live views (a sweep plays \
     its own signal). Changed while armed or playing, it takes effect at once.";
/// The sine frequency row.
pub const SINE_FREQ_ROW: &str = "Sine frequency";
pub const SINE_FREQ_HINT: &str = "Type a frequency (997, 1k, 1.5 kHz), then Enter.";
pub const SINE_FREQ_EMPTY: &str = "type the sine's frequency (e.g. 1k), then Enter";
/// The bin row.
pub const ON_BIN_ROW: &str = "On an FFT bin";
pub const ON_BIN_HINT: &str = "On: the sine is moved to the nearest bin of the shortest FFT \
     among the running spectrum measurements, where no window leaks it into THD+N.";

/// The dialog the first arm opens: its title, what Enter does, its button.
pub const SIGNAL_DIALOG_TITLE: &str = "What should the generator play?";
pub const SIGNAL_DIALOG_SUBMIT: &str = "Enter chooses it; nothing is armed";
pub const SIGNAL_DIALOG_VERB: &str = "Choose";
/// Beside the dialog's frequency row: the forms a frequency is typed in.
pub const SINE_FREQ_FORMS: &str = "997, 1k, 1.5 kHz";

/// Beside the dialog's bin row: the frequency typed and, when a bin moves it, the one
/// played, `plays 1000 Hz → 996.09 Hz`, so the operator sees it before choosing.
pub fn sine_plays_text(sine: SineSetting, bins: Option<(usize, f64)>) -> String {
    format!("plays {}", sine_freq_value(sine, bins))
}

/// The signal row's value.
pub fn signal_row_value(kind: Option<SignalKind>) -> &'static str {
    kind.map_or(SIGNAL_UNCHOSEN, SignalKind::name)
}

/// The sine frequency row's value: the frequency typed, and after an arrow the one
/// played when a bin moved it: `1000 Hz → 1001.95 Hz`.
pub fn sine_freq_value(sine: SineSetting, bins: Option<(usize, f64)>) -> String {
    let typed = format::freq_exact(sine.freq.0);
    let choice = SignalChoice::Sine {
        freq: sine.freq,
        on_bin: sine.on_bin,
    };
    match choice.played(bins) {
        Signal::Sine { freq } if format::freq_exact(freq.0) != typed => {
            format!("{typed} → {}", format::freq_exact(freq.0))
        }
        _ => typed,
    }
}

/// The bin row's value, with why it moves nothing when it cannot.
pub fn on_bin_value(on: bool, bins: Option<(usize, f64)>) -> String {
    match (on, bins) {
        (false, _) => "off · played as typed".into(),
        (true, Some((n, _))) => format!("on · bins of the {n}-point FFT"),
        (true, None) => "on · no spectrum running, played as typed".into(),
    }
}

/// What stands before the level on the top bar's stimulus line: nothing for pink noise,
/// what every level is for unless said otherwise (nor before a signal is chosen); `white `,
/// `sine 999.76 Hz ` for another, so the line never shows a level without saying it is a
/// tone's. `played` is the signal as it is or would be sent.
pub fn level_prefix(played: Option<&Signal>) -> String {
    match played {
        None | Some(Signal::Pink) => String::new(),
        Some(s) => format!("{} ", signal_name(s)),
    }
}

/// The tip of the top bar's level: where what plays is set.
pub fn level_tip(level_key: &str) -> String {
    format!(
        "{level_key} types the level; the signal (pink or white noise, or a sine) is set in \
         Settings › Audio I/O"
    )
}

/// A signal by name, a sine with its frequency to the hundredth of a hertz as the
/// harmonics plate prints it: a sine on an FFT bin (`999.76 Hz`) must not read `1.00 kHz`
/// here and its true frequency there.
pub fn signal_name(s: &Signal) -> String {
    match s {
        Signal::White => "white".into(),
        Signal::Pink => "pink".into(),
        Signal::PeriodicPink { .. } => "periodic pink".into(),
        Signal::Sine { freq } => format!("sine {}", format::freq_exact(freq.0)),
        Signal::Ess { sweep } => format!("sweep {} s", short_number(sweep.duration.0)),
    }
}

fn level_text(l: Option<Dbfs>) -> String {
    l.map_or_else(
        || "no level".into(),
        |l| format!("{} dBFS", short_number(l.0)),
    )
}

/// Whole numbers without decimals (`−50`, `3`), else one (`−50.5`).
fn short_number(v: f64) -> String {
    let one = format::fixed(v, 1);
    match one.strip_suffix(".0") {
        Some(whole) => whole.to_string(),
        None => one,
    }
}

/// [`hint`] with more than one rig: the key line names the rig whose stimulus the key
/// arms or fires, `arm pupu's stimulus · Space: pink −50 dBFS → out 1`.
pub fn rig_hint(rig: &str, next: Next, what: &Stimulus, level_key: &str) -> (String, String) {
    match (next, what) {
        (
            Next::Space,
            Stimulus::SweepDialog
            | Stimulus::SignalDialog
            | Stimulus::Generator { level: None, .. },
        ) => {
            let (long, short) = hint(next, what, level_key);
            (format!("{rig}: {long}"), format!("{rig}: {short}"))
        }
        (Next::Space, w) => {
            let short = format!("arm {rig}'s stimulus · Space");
            (format!("{short}: {}", describe(w)), short)
        }
        (Next::Enter, w) => {
            let short = format!("fire {rig}'s stimulus · Enter");
            (format!("{short}: {}", describe(w)), short)
        }
    }
}

/// The stimulus of a rig not shown, armed or playing: sound from a system the operator is
/// not looking at is never silent in the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct Elsewhere {
    /// The rig, by name.
    pub rig: String,
    /// Playing (else armed).
    pub firing: bool,
    /// Its level, as the rig's generator has it.
    pub level: Option<Dbfs>,
    /// Who holds its lease when not this app.
    pub holder: Option<String>,
}

/// Who holds `rig`'s generator, for "held by …": a key's admitted name, or for a client
/// on the rig's own host (`local`, as the rig saw its connection; its id is only a routing
/// tag) the words `an app on pupu`.
pub fn holder(owner: &ClientId, local: bool, rig: &str) -> String {
    if local {
        format!("an app on {rig}")
    } else {
        owner.0.clone()
    }
}

/// The top bar's chip for `e`, beside the stop that stops every rig's: `▶ pupu −20 dBFS`,
/// `pupu armed −20 dBFS`, `▶ pupu −20 dBFS · held by alice`.
pub fn elsewhere_chip(e: &Elsewhere) -> String {
    let level = level_text(e.level);
    let mut s = if e.firing {
        format!("▶ {} {level}", e.rig)
    } else {
        format!("{} armed {level}", e.rig)
    };
    if let Some(h) = &e.holder {
        s.push_str(&format!(" · held by {h}"));
    }
    s
}

/// The stop's tip with more than one rig: it stops them all.
pub fn stop_every_rig_tip(stop: &str) -> String {
    format!(
        "{stop} stops and disarms every rig's stimulus from anywhere, also with a window open; \
         with no window open Esc stops the rig shown"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::units::Hz;

    fn sine(hz: f64, on_bin: bool) -> SignalChoice {
        SignalChoice::Sine {
            freq: Hz(hz),
            on_bin,
        }
    }

    /// 8192 points at 48 kHz: bins 5.859375 Hz apart, the 171st nearest 1 kHz.
    const BINS: Option<(usize, f64)> = Some((8192, 48_000.0));
    const BIN_1K: f64 = 171.0 * 48_000.0 / 8192.0;

    /// A sine reads to the hundredth of a hertz wherever the stimulus is named, as the
    /// harmonics plate prints it: one on a bin is told from the round number beside it.
    #[test]
    fn a_sine_is_named_with_its_exact_frequency() {
        let on_bin = Signal::Sine {
            freq: Hz(999.755_859_375),
        };
        let what = Stimulus::Generator {
            signal: on_bin,
            level: Some(Dbfs(-20.0)),
            outputs: vec![0],
            labels: Vec::new(),
        };
        assert_eq!(
            hint(Next::Enter, &what, "L").0,
            "Enter fires: sine 999.76 Hz −20 dBFS → out 1"
        );
        assert_eq!(
            rig_hint("pupu", Next::Space, &what, "L").0,
            "arm pupu's stimulus · Space: sine 999.76 Hz −20 dBFS → out 1"
        );
        assert_eq!(
            signal_name(&on_bin),
            format!("sine {}", format::freq_exact(999.755_859_375))
        );
        // The top bar's level: pink is what a level means unless it says otherwise.
        assert_eq!(level_prefix(None), "");
        assert_eq!(level_prefix(Some(&Signal::Pink)), "");
        assert_eq!(level_prefix(Some(&Signal::White)), "white ");
        assert_eq!(level_prefix(Some(&on_bin)), "sine 999.76 Hz ");
        assert_eq!(
            level_tip("L"),
            "L types the level; the signal (pink or white noise, or a sine) is set in Settings › \
             Audio I/O"
        );
    }

    /// Before a signal is chosen the hint says that Space asks, and names no signal.
    #[test]
    fn an_unchosen_signal_is_asked_not_named() {
        assert_eq!(
            hint(Next::Space, &Stimulus::SignalDialog, "L"),
            (
                "Space asks what to play: pink, white or a sine".into(),
                "Space: signal…".into()
            )
        );
        assert_eq!(
            rig_hint("pupu", Next::Space, &Stimulus::SignalDialog, "L").0,
            "pupu: Space asks what to play: pink, white or a sine"
        );
    }

    /// The frequency row reads a frequency as every frequency field does, refuses one the
    /// stream cannot play, and says what to type when Enter finds it empty.
    #[test]
    fn the_frequency_row_reads_a_frequency() {
        for (text, hz) in [
            ("997", 997.0),
            ("1k", 1000.0),
            ("997 Hz", 997.0),
            ("1.5 kHz", 1500.0),
            ("1,5khz", 1500.0),
            (" 1000 Hz ", 1000.0),
            ("23999.99", 23_999.99),
        ] {
            assert_eq!(typed_sine_freq(text, Some(48_000)), Ok(Hz(hz)), "{text}");
        }
        for (text, error) in [
            ("", "type the sine's frequency (e.g. 1k), then Enter"),
            ("  ", "type the sine's frequency (e.g. 1k), then Enter"),
            ("x", "\"x\": expected a number followed by a unit"),
            ("0", "\"0\": frequency must be positive"),
            ("1 m", "\"1 m\": unknown unit \"m\" (expected hz or khz)"),
            (
                "24000",
                "a sine must be below 24000 Hz, half the sample rate",
            ),
            (
                "22.05k",
                "a sine must be below 22050 Hz, half the sample rate",
            ),
        ] {
            let rate = if text == "22.05k" { 44_100 } else { 48_000 };
            assert_eq!(
                typed_sine_freq(text, Some(rate)),
                Err(error.to_string()),
                "{text}"
            );
        }
        // The rate unknown: only the frequency field's own bound holds.
        assert_eq!(typed_sine_freq("30k", None), Ok(Hz(30_000.0)));
        assert!(typed_sine_freq("2000k", None).is_err());
    }

    /// The rows of the choice, as Settings and the dialog both show them: the sine's own
    /// only with a sine.
    #[test]
    fn a_noise_has_one_row_a_sine_three() {
        let rows = |kind| -> Vec<(&'static str, String)> {
            signal_rows(kind, DEFAULT_SINE, BINS)
                .into_iter()
                .map(|r| (r.title, r.value))
                .collect()
        };
        assert_eq!(
            rows(None),
            [("Signal", "not chosen · asked at the first arm".to_owned())]
        );
        assert_eq!(
            rows(Some(SignalKind::Pink)),
            [("Signal", "pink noise".to_owned())]
        );
        assert_eq!(
            rows(Some(SignalKind::White)),
            [("Signal", "white noise".to_owned())]
        );
        assert_eq!(
            rows(Some(SignalKind::Sine)),
            [
                ("Signal", "sine".to_owned()),
                ("Sine frequency", "1000 Hz → 1001.95 Hz".to_owned()),
                (
                    "On an FFT bin",
                    "on · bins of the 8192-point FFT".to_owned()
                ),
            ]
        );
        assert_eq!(
            sine_plays_text(DEFAULT_SINE, BINS),
            "plays 1000 Hz → 1001.95 Hz"
        );
        assert_eq!(sine_plays_text(DEFAULT_SINE, None), "plays 1000 Hz");
        assert_eq!(
            SignalChoice::of(SignalKind::Sine, DEFAULT_SINE),
            sine(1000.0, true)
        );
        assert_eq!(
            SignalChoice::of(SignalKind::White, DEFAULT_SINE),
            SignalChoice::White
        );
    }

    /// A sine set to be on a bin is played on the nearest bin of the FFT it is given, and
    /// as typed without one or with the bin row off; the toast says which.
    #[test]
    fn a_sine_is_played_on_a_bin_or_as_typed() {
        let s = sine(1000.0, true);
        assert_eq!(s.played(BINS), Signal::Sine { freq: Hz(BIN_1K) });
        assert_eq!(s.played(None), Signal::Sine { freq: Hz(1000.0) });
        assert_eq!(
            sine(1000.0, false).played(BINS),
            Signal::Sine { freq: Hz(1000.0) }
        );
        assert_eq!(SignalChoice::Pink.played(BINS), Signal::Pink);
        assert_eq!(SignalChoice::White.played(None), Signal::White);
        assert_eq!(
            s.chosen_text(BINS),
            "sine 1001.95 Hz · on an FFT bin of the spectrum"
        );
        assert_eq!(
            s.chosen_text(None),
            "sine 1000 Hz · no spectrum running, played as typed"
        );
        assert_eq!(
            sine(1000.0, false).chosen_text(BINS),
            "sine 1000 Hz · as typed"
        );
        assert_eq!(SignalChoice::Pink.chosen_text(BINS), "pink noise");
        assert_eq!(SignalChoice::White.chosen_text(None), "white noise");
    }

    /// The three settings rows: the signal by name (or that it is asked), the frequency
    /// typed with the one played after an arrow when a bin moved it, the bin row with what
    /// it does now.
    #[test]
    fn the_settings_rows_say_what_is_set_and_what_plays() {
        assert_eq!(
            signal_row_value(None),
            "not chosen · asked at the first arm"
        );
        assert_eq!(signal_row_value(Some(SignalKind::Pink)), "pink noise");
        assert_eq!(signal_row_value(Some(SignalKind::White)), "white noise");
        assert_eq!(signal_row_value(Some(SignalKind::Sine)), "sine");
        assert_eq!(
            DEFAULT_SINE,
            SineSetting {
                freq: Hz(1000.0),
                on_bin: true
            }
        );
        assert_eq!(sine_freq_value(DEFAULT_SINE, BINS), "1000 Hz → 1001.95 Hz");
        assert_eq!(sine_freq_value(DEFAULT_SINE, None), "1000 Hz");
        let off = SineSetting {
            on_bin: false,
            ..DEFAULT_SINE
        };
        assert_eq!(sine_freq_value(off, BINS), "1000 Hz");
        // Typed on a bin already: nothing to show after it.
        let on = SineSetting {
            freq: Hz(BIN_1K),
            on_bin: true,
        };
        assert_eq!(sine_freq_value(on, BINS), "1001.95 Hz");
        assert_eq!(on_bin_value(true, BINS), "on · bins of the 8192-point FFT");
        assert_eq!(
            on_bin_value(true, None),
            "on · no spectrum running, played as typed"
        );
        assert_eq!(on_bin_value(false, BINS), "off · played as typed");
        // ←/→ on the signal row, from nothing chosen too.
        assert_eq!(SignalKind::step(None, 1), SignalKind::Pink);
        assert_eq!(SignalKind::step(None, -1), SignalKind::Sine);
        assert_eq!(
            SignalKind::step(Some(SignalKind::Pink), 1),
            SignalKind::White
        );
        assert_eq!(
            SignalKind::step(Some(SignalKind::Sine), 1),
            SignalKind::Pink
        );
        assert_eq!(
            SignalKind::step(Some(SignalKind::Pink), -1),
            SignalKind::Sine
        );
    }

    #[test]
    fn a_holder_on_the_rigs_own_host_is_named_by_the_rig() {
        assert_eq!(
            holder(&ClientId::local("006b8b4567"), true, "pupu"),
            "an app on pupu"
        );
        assert_eq!(
            holder(&ClientId("ketunkolo".into()), false, "pupu"),
            "ketunkolo"
        );
    }

    #[test]
    fn a_chip_names_the_rig_its_state_and_level() {
        let e = |firing, level: Option<f64>, holder: Option<&str>| Elsewhere {
            rig: "pupu".into(),
            firing,
            level: level.map(Dbfs),
            holder: holder.map(Into::into),
        };
        assert_eq!(
            elsewhere_chip(&e(true, Some(-20.0), None)),
            "▶ pupu −20 dBFS"
        );
        assert_eq!(
            elsewhere_chip(&e(false, Some(-12.5), None)),
            "pupu armed −12.5 dBFS"
        );
        assert_eq!(
            elsewhere_chip(&e(true, None, Some("alice"))),
            "▶ pupu no level · held by alice"
        );
    }

    fn pink(level: Option<f64>) -> Stimulus {
        Stimulus::Generator {
            signal: Signal::Pink,
            level: level.map(Dbfs),
            outputs: vec![0],
            labels: Vec::new(),
        }
    }

    fn resweep() -> Stimulus {
        Stimulus::Sweep {
            meas: "Genelec 1 m".into(),
            duration_s: 3.0,
            level: Some(Dbfs(-50.0)),
        }
    }

    #[test]
    fn enter_names_what_fires() {
        assert_eq!(
            hint(Next::Enter, &resweep(), "L"),
            (
                "Enter fires: sweep Genelec 1 m · 3 s −50 dBFS".into(),
                "Enter fires".into()
            )
        );
        assert_eq!(
            hint(Next::Enter, &pink(Some(-50.0)), "L").0,
            "Enter fires: pink −50 dBFS → out 1"
        );
        let first = Stimulus::Sweep {
            meas: "Sweep 1".into(),
            duration_s: 1.0,
            level: Some(Dbfs(-20.5)),
        };
        assert_eq!(
            hint(Next::Enter, &first, "L").0,
            "Enter fires: sweep Sweep 1 · 1 s −20.5 dBFS"
        );
    }

    fn idle() -> Drive {
        Drive::Idle {
            arm: "Space".into(),
            fire: "Enter".into(),
        }
    }

    /// NO REFERENCE says what to press while nothing here plays, and points at the patch
    /// once something does.
    #[test]
    fn no_reference_names_the_keys_by_phase() {
        let armed = Drive::Armed {
            fire: "Enter".into(),
        };
        assert_eq!(
            no_reference_detail(&idle()),
            "stimulus off: Space arms, Enter starts it"
        );
        assert_eq!(
            no_reference_detail(&armed),
            "stimulus armed: Enter starts it"
        );
        assert_eq!(
            no_reference_detail(&Drive::SweepView),
            "stimulus off: arm it from a transfer pane"
        );
        assert_eq!(
            no_reference_detail(&Drive::Playing),
            "reference silent: check the loopback cable"
        );
        assert_eq!(
            no_reference_note(&idle()),
            "no reference: nothing is playing — Space arms, Enter starts the stimulus"
        );
        assert_eq!(
            no_reference_note(&armed),
            "no reference: armed — Enter starts the stimulus"
        );
        assert_eq!(
            no_reference_note(&Drive::SweepView),
            "no reference: nothing is playing — arm the stimulus from a transfer pane"
        );
        assert_eq!(
            no_reference_note(&Drive::Playing),
            "no reference: nothing is driving the loopback"
        );
    }

    #[test]
    fn space_names_what_arms() {
        assert_eq!(
            hint(Next::Space, &resweep(), "L"),
            (
                "Space arms: sweep Genelec 1 m · 3 s −50 dBFS".into(),
                "Space arms".into()
            )
        );
        let two = Stimulus::Generator {
            signal: Signal::Sine { freq: Hz(1000.0) },
            level: Some(Dbfs(-12.0)),
            outputs: vec![0, 1],
            labels: Vec::new(),
        };
        assert_eq!(
            hint(Next::Space, &two, "L").0,
            "Space arms: sine 1000 Hz −12 dBFS → out 1, 2"
        );
        // Outputs the rig has named are named.
        let named = Stimulus::Generator {
            signal: Signal::Pink,
            level: Some(Dbfs(-50.0)),
            outputs: vec![0, 1],
            labels: vec![OutputSetup {
                channel: 0,
                label: Some("Main L".into()),
            }],
        };
        assert_eq!(
            hint(Next::Enter, &named, "L").0,
            "Enter fires: pink −50 dBFS → Main L, out 2"
        );
        assert_eq!(
            hint(Next::Space, &pink(None), "L"),
            ("L types a level".into(), "L types a level".into())
        );
        assert_eq!(
            hint(Next::Space, &Stimulus::SweepDialog, "L"),
            (
                "Space sets up a sweep measurement".into(),
                "Space: sweep…".into()
            )
        );
    }
}
