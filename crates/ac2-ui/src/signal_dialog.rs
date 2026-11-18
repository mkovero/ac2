//! The stimulus signal dialog: what the first Space on a live view opens while the operator
//! has not said what the generator plays. A form of the rows Settings › Audio I/O has for
//! the same choice ([`ac2_scene::stimulus::signal_rows`]): the signal, and with a sine its
//! frequency and whether it goes onto an FFT bin. It opens on pink noise, so Enter alone
//! takes the signal every level was for before there was a choice.
//!
//! Pure data: Enter hands the reducer a [`SignalChoice`]; nothing is armed from here.

use ac2_proto::units::Hz;
use ac2_scene::format;
use ac2_scene::stimulus::{
    SINE_FREQ_FORMS, SignalChoice, SignalKind, SignalRow, SineSetting, on_bin_value, signal_rows,
    sine_plays_text, typed_sine_freq,
};

use crate::forms::{Field, FieldId, Form, FormKind};

/// The dialog's own state beside its fields.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalForm {
    /// The sine the rows opened with. Its frequency stands in the frequency row as text of
    /// two decimals, so the text left untouched means this frequency, not its rounding.
    opened: SineSetting,
    /// The sine's rows as last shown, kept while a noise hides them.
    freq_text: String,
    on_bin: bool,
    /// The FFT whose bins a sine goes onto and the sample rate, as the app resolved them
    /// when the dialog opened.
    bins: Option<(usize, f64)>,
    /// The open session's sample rate: a sine must stay below half of it.
    rate: Option<u32>,
}

/// The dialog on pink noise, the sine's rows (hidden until the signal is a sine) holding
/// `sine`. `bins` and `rate` as [`SignalChoice::played`] and
/// [`ac2_scene::stimulus::parse_sine_freq`] take them.
pub fn form(sine: SineSetting, bins: Option<(usize, f64)>, rate: Option<u32>) -> Form {
    let m = SignalForm {
        opened: sine,
        freq_text: format::freq_exact(sine.freq.0),
        on_bin: sine.on_bin,
        bins,
        rate,
    };
    let mut f = Form::new(FormKind::StimulusSignal, fields(&m, SignalKind::Pink));
    f.signal = Some(Box::new(m));
    f
}

/// The frequency the row's `text` stands for.
fn freq(m: &SignalForm, text: &str) -> Result<Hz, String> {
    if text == format::freq_exact(m.opened.freq.0) {
        return Ok(m.opened.freq);
    }
    typed_sine_freq(text, m.rate)
}

/// What stands beside the bin row: the frequency played, while the frequency row reads
/// as one.
fn plays(m: &SignalForm) -> String {
    freq(m, &m.freq_text).map_or_else(
        |_| String::new(),
        |freq| {
            sine_plays_text(
                SineSetting {
                    freq,
                    on_bin: m.on_bin,
                },
                m.bins,
            )
        },
    )
}

/// The rows for `kind`: those Settings shows for it, under the same titles.
fn fields(m: &SignalForm, kind: SignalKind) -> Vec<Field> {
    signal_rows(Some(kind), m.opened, m.bins)
        .into_iter()
        .map(|r| match r.row {
            SignalRow::Signal => {
                let names = SignalKind::ALL.map(SignalKind::name);
                let at = SignalKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
                Field::choice(FieldId::Signal, r.title, &names, at)
            }
            SignalRow::Frequency => Field::text(
                FieldId::SineFrequency,
                r.title,
                m.freq_text.clone(),
                SINE_FREQ_FORMS,
            ),
            SignalRow::OnBin => {
                let (on, off) = (on_bin_value(true, m.bins), on_bin_value(false, m.bins));
                let mut f = Field::choice(
                    FieldId::OnBin,
                    r.title,
                    &[on.as_str(), off.as_str()],
                    usize::from(!m.on_bin),
                );
                f.hint = plays(m);
                f
            }
        })
        .collect()
}

/// The signal the dialog shows.
pub fn kind(f: &Form) -> SignalKind {
    f.choice_index(FieldId::Signal)
        .and_then(|i| SignalKind::ALL.get(i).copied())
        .unwrap_or(SignalKind::Pink)
}

/// The sine's rows read back into the dialog's state, while they are shown.
fn keep(f: &Form, m: &mut SignalForm) {
    if f.field(FieldId::SineFrequency).is_some() {
        m.freq_text = f.text(FieldId::SineFrequency).to_owned();
    }
    if let Some(i) = f.choice_index(FieldId::OnBin) {
        m.on_bin = i == 0;
    }
}

/// The frequency row was typed in: the bin row says what would play now.
pub(crate) fn rehint(f: &mut Form) {
    let Some(mut m) = f.signal.take() else {
        return;
    };
    keep(f, &mut m);
    let text = plays(&m);
    if let Some(bin) = f.fields.iter_mut().find(|x| x.id == FieldId::OnBin) {
        bin.hint = text;
    }
    f.signal = Some(m);
}

/// ←/→ in the dialog. On the signal row: the next signal, around the ends as on the
/// settings row, and the sine's rows shown or hidden with it. On the bin row: on or off.
/// False for any other form or row.
pub(crate) fn cycle(f: &mut Form, d: i32) -> bool {
    let Some(mut m) = f.signal.take() else {
        return false;
    };
    keep(f, &mut m);
    let done = match f.fields.get(f.focus).map(|x| x.id) {
        Some(FieldId::Signal) => {
            let to = SignalKind::step(Some(kind(f)), d);
            f.fields = fields(&m, to);
            true
        }
        Some(FieldId::OnBin) => {
            m.on_bin = !m.on_bin;
            f.fields = fields(&m, SignalKind::Sine);
            true
        }
        _ => false,
    };
    if done {
        f.error = None;
    }
    f.signal = Some(m);
    done
}

/// Enter: the choice, or why the frequency row cannot be played.
pub fn choice(f: &Form) -> Result<SignalChoice, String> {
    let m = f.signal.as_deref().ok_or("not the signal dialog")?;
    Ok(match kind(f) {
        SignalKind::Pink => SignalChoice::Pink,
        SignalKind::White => SignalChoice::White,
        SignalKind::Sine => SignalChoice::Sine {
            freq: freq(m, f.text(FieldId::SineFrequency))?,
            on_bin: f.choice_index(FieldId::OnBin) == Some(0),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_scene::stimulus::DEFAULT_SINE;

    /// 65536 points at 48 kHz: the bins of the default spectrum.
    const BINS: Option<(usize, f64)> = Some((65_536, 48_000.0));

    fn rows(f: &Form) -> Vec<(String, String, String)> {
        f.fields
            .iter()
            .map(|x| (x.label.clone(), x.display(), x.hint.clone()))
            .collect()
    }

    fn row(label: &str, value: &str, hint: &str) -> (String, String, String) {
        (label.into(), value.into(), hint.into())
    }

    /// The dialog asks its question over one row for a noise and three for a sine, and its
    /// footer names the keys that act on what is shown.
    #[test]
    fn a_noise_shows_one_row_a_sine_three() {
        let mut f = form(DEFAULT_SINE, BINS, Some(48_000));
        assert_eq!(f.kind.title(), "What should the generator play?");
        assert_eq!(f.kind.verb(), "Choose");
        assert_eq!(rows(&f), [row("Signal", "pink noise", "")]);
        assert_eq!(
            f.keys_hint(),
            "Enter chooses it; nothing is armed · ←→ choose · Esc closes"
        );
        assert_eq!(choice(&f), Ok(SignalChoice::Pink));

        f.cycle(1);
        assert_eq!(rows(&f), [row("Signal", "white noise", "")]);
        assert_eq!(choice(&f), Ok(SignalChoice::White));

        f.cycle(1);
        assert_eq!(
            rows(&f),
            [
                row("Signal", "sine", ""),
                row("Sine frequency", "1000 Hz", "997, 1k, 1.5 kHz"),
                row(
                    "On an FFT bin",
                    "on · bins of the 65536-point FFT",
                    "plays 1000 Hz → 999.76 Hz"
                ),
            ]
        );
        assert_eq!(
            f.keys_hint(),
            "Enter chooses it; nothing is armed · ↑↓ field · ←→ choose · Esc closes"
        );
        assert_eq!(f.focus, 0, "the focus stays on the row that was changed");
        assert_eq!(
            choice(&f),
            Ok(SignalChoice::Sine {
                freq: Hz(1000.0),
                on_bin: true
            })
        );

        // Around the end, as the settings row goes; ← from pink is the sine.
        f.cycle(1);
        assert_eq!(rows(&f), [row("Signal", "pink noise", "")]);
        f.cycle(-1);
        assert_eq!(kind(&f), SignalKind::Sine);
        assert_eq!(f.fields.len(), 3);
    }

    /// The frequency is typed over the one held; the bin row says what plays as it is
    /// typed, and off plays it as typed. What was set survives a look at the noises.
    #[test]
    fn the_sine_rows_are_typed_and_toggled() {
        let mut f = form(DEFAULT_SINE, BINS, Some(48_000));
        f.cycle(-1);
        f.move_focus(1);
        assert!(f.selected, "typing replaces the frequency held");
        f.type_text("5");
        f.type_text("k");
        assert_eq!(f.text(FieldId::SineFrequency), "5k");
        assert_eq!(f.fields[2].hint, "plays 5000 Hz → 5000.24 Hz");
        f.type_text("x");
        assert_eq!(f.fields[2].hint, "", "no frequency, nothing to promise");
        f.backspace();
        assert_eq!(f.fields[2].hint, "plays 5000 Hz → 5000.24 Hz");

        f.move_focus(1);
        f.cycle(1);
        assert_eq!(
            rows(&f)[2],
            row("On an FFT bin", "off · played as typed", "plays 5000 Hz")
        );
        assert_eq!(
            choice(&f),
            Ok(SignalChoice::Sine {
                freq: Hz(5000.0),
                on_bin: false
            })
        );
        // Either arrow turns it: there are two states.
        f.cycle(1);
        assert_eq!(f.fields[2].display(), "on · bins of the 65536-point FFT");
        f.cycle(-1);
        assert_eq!(f.fields[2].display(), "off · played as typed");

        f.move_focus(1);
        assert_eq!(f.focus, 0);
        f.cycle(1);
        assert_eq!(f.fields.len(), 1);
        f.cycle(-1);
        assert_eq!(
            rows(&f)[1..],
            [
                row("Sine frequency", "5k", "997, 1k, 1.5 kHz"),
                row("On an FFT bin", "off · played as typed", "plays 5000 Hz"),
            ]
        );
    }

    /// With no spectrum running the bin row says so, and the sine plays as typed.
    #[test]
    fn no_spectrum_no_bin() {
        let mut f = form(DEFAULT_SINE, None, None);
        f.cycle(-1);
        assert_eq!(
            rows(&f)[2],
            row(
                "On an FFT bin",
                "on · no spectrum running, played as typed",
                "plays 1000 Hz"
            )
        );
    }

    /// Enter refuses a frequency row that is no playable frequency, in the words the
    /// settings row uses.
    #[test]
    fn a_bad_frequency_is_refused_in_the_settings_rows_words() {
        for (text, error) in [
            ("", "type the sine's frequency (e.g. 1k), then Enter"),
            ("x", "\"x\": expected a number followed by a unit"),
            ("0", "\"0\": frequency must be positive"),
            ("1 m", "\"1 m\": unknown unit \"m\" (expected hz or khz)"),
            ("24k", "a sine must be below 24000 Hz, half the sample rate"),
        ] {
            let mut f = form(DEFAULT_SINE, BINS, Some(48_000));
            f.cycle(-1);
            f.set_text(FieldId::SineFrequency, text);
            assert_eq!(choice(&f), Err(error.to_owned()), "{text}");
        }
    }

    /// A frequency of more decimals than the row prints is kept when the row is left as
    /// it opened.
    #[test]
    fn an_untouched_frequency_is_the_one_held() {
        let held = SineSetting {
            freq: Hz(997.123_456),
            on_bin: false,
        };
        let mut f = form(held, BINS, Some(48_000));
        f.cycle(-1);
        assert_eq!(f.text(FieldId::SineFrequency), "997.12 Hz");
        assert_eq!(f.fields[2].display(), "off · played as typed");
        assert_eq!(
            choice(&f),
            Ok(SignalChoice::Sine {
                freq: Hz(997.123_456),
                on_bin: false
            })
        );
    }
}
