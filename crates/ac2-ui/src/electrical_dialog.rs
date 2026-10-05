//! The electrical calibration dialog of the calibrations view (`docs/design/q7-calibration.md`
//! §11): a voltage measured at an input with a meter while the daemon reads the input's
//! level, and the mic's sensitivity, give the input's sensitivity calibration without an
//! acoustic calibrator.
//!
//! Keyboard-first like the other dialogs: ↑/↓ (Tab) move between the fields, ←/→ choose
//! where the voltage is measured (in-line or injected), typing edits the focused field,
//! Enter reads and stores, Esc closes. The sensitivity starts as the data sheet's (the mic's
//! curve files), named as such; editing it makes it typed.
//!
//! Pure data: the reducer turns Enter into a `cal.spl_electrical` request and hands the
//! reply back through [`ElectricalDialog::reply`].

use ac2_proto::Command;
use ac2_proto::cal::{self, DataSheet};
use ac2_proto::model::{CalMethod, ElectricalConnection, State};
use ac2_proto::units::{Hz, MvPerPa, Volts};

/// The fields, top to bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Field {
    /// In-line or injected.
    Method,
    /// The voltage measured.
    #[default]
    Volts,
    /// The tone's frequency.
    Freq,
    /// The mic's sensitivity.
    Sensitivity,
}

const FIELDS: [Field; 4] = [Field::Method, Field::Volts, Field::Freq, Field::Sensitivity];

/// The `what` of the dialog's request: its reply comes back to the dialog by it.
pub const WHAT_PREFIX: &str = "electrical calibration of input ";

#[derive(Clone, Debug, PartialEq)]
pub struct ElectricalDialog {
    pub input: u16,
    pub mic: String,
    pub connection: ElectricalConnection,
    pub focus: Field,
    pub volts: String,
    pub freq: String,
    pub sensitivity: String,
    /// The data sheet's value as prefilled, with where it comes from (`MM1 34804 0°`), when
    /// the mic's curve files state one.
    pub data_sheet: Option<(String, String)>,
    /// Why there is no data-sheet value, when there is none.
    pub no_data_sheet: Option<String>,
    /// The input has an acoustic calibration of this mic: Enter asks first.
    pub acoustic: Option<String>,
    /// Enter was pressed once on an input with an acoustic calibration.
    pub confirm_replace: bool,
    /// A request is out.
    pub pending: Option<String>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

fn input_no(c: u16) -> u32 {
    u32::from(c) + 1
}

/// A number followed by a unit, the unit lowercased without spaces.
fn split(t: &str) -> Option<(f64, String)> {
    let t = t.trim().replace(',', ".");
    let end = t
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0)))
        .map_or(t.len(), |(i, _)| i);
    let n: f64 = t[..end].parse().ok()?;
    n.is_finite()
        .then(|| (n, t[end..].replace(' ', "").to_lowercase()))
}

/// `15.03 mV`, `0.015 V`, `250 µV` → volts. The unit is required: mV and V differ by 1000.
pub fn parse_volts(t: &str) -> Result<Volts, String> {
    let (n, u) = split(t).ok_or("type the voltage the meter shows, e.g. 15.03 mV")?;
    let v = match u.as_str() {
        "v" => n,
        "mv" => n * 1e-3,
        "uv" | "µv" | "μv" => n * 1e-6,
        "" => return Err("give the unit: mV or V".into()),
        _ => return Err(format!("unknown unit {u:?}: mV or V")),
    };
    if v > 0.0 {
        Ok(Volts(v))
    } else {
        Err("the voltage must be above 0".into())
    }
}

/// `1 kHz`, `1000 Hz`, `400` (Hz) → Hz.
pub fn parse_freq(t: &str) -> Result<Hz, String> {
    let (n, u) = split(t).ok_or("type the tone's frequency, e.g. 1 kHz")?;
    let f = match u.as_str() {
        "" | "hz" => n,
        "k" | "khz" => n * 1e3,
        _ => return Err(format!("unknown unit {u:?}: Hz or kHz")),
    };
    if f > 0.0 {
        Ok(Hz(f))
    } else {
        Err("the frequency must be above 0".into())
    }
}

/// `15.0 mV/Pa`, `0.015 V/Pa`, `-36.5 dBV/Pa` (as data sheets state it) → mV/Pa.
pub fn parse_sensitivity(t: &str) -> Result<MvPerPa, String> {
    let (n, u) = split(t).ok_or("type the mic's sensitivity, e.g. 15.0 mV/Pa")?;
    let mv = match u.as_str() {
        "mv/pa" | "mv" => n,
        "v/pa" => n * 1e3,
        "dbv/pa" | "dbv" => 1e3 * 10f64.powf(n / 20.0),
        "" => return Err("give the unit: mV/Pa or dBV/Pa".into()),
        _ => return Err(format!("unknown unit {u:?}: mV/Pa or dBV/Pa")),
    };
    if mv > 0.0 && mv.is_finite() {
        Ok(MvPerPa(mv))
    } else {
        Err("the sensitivity must be above 0".into())
    }
}

impl ElectricalDialog {
    /// The dialog for `input`, whose mic name is `mic`: in-line, 1 kHz, the data-sheet
    /// sensitivity when the mic's files state exactly one.
    pub fn new(s: &State, input: u16, mic: &str) -> Self {
        let (data_sheet, no_data_sheet) = match cal::data_sheet(&s.mics, mic) {
            DataSheet::One(v, ac2_proto::model::SensitivitySource::DataSheet { label, .. }) => (
                Some((
                    ac2_scene::cal::mv_per_pa(v),
                    ac2_scene::cal::curve_name(mic, &label),
                )),
                None,
            ),
            DataSheet::One(..) => (None, None),
            DataSheet::NoMic => (
                None,
                Some(format!(
                    "no curve file of {mic} in the mic library: type the data sheet's value"
                )),
            ),
            DataSheet::NoneStated => (
                None,
                Some(format!(
                    "the curve files of {mic} state none: type the data sheet's value"
                )),
            ),
            DataSheet::Differ(v) => (
                None,
                Some(format!(
                    "the curve files of {mic} state {}: type the one to use",
                    v.iter()
                        .map(|x| ac2_scene::cal::mv_per_pa(MvPerPa(*x)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            ),
        };
        let acoustic = s
            .session
            .open
            .as_ref()
            .and_then(|o| {
                s.calibrations.iter().find(|e| {
                    e.key.device == o.input_device && e.key.channel == input && e.key.mic == mic
                })
            })
            .filter(|e| matches!(e.spl.method, CalMethod::Acoustic { .. }))
            .map(ac2_scene::cal::calibrator);
        Self {
            input,
            mic: mic.to_owned(),
            connection: ElectricalConnection::InLine,
            focus: Field::Volts,
            volts: String::new(),
            freq: "1 kHz".into(),
            sensitivity: data_sheet.as_ref().map(|d| d.0.clone()).unwrap_or_default(),
            data_sheet,
            no_data_sheet,
            acoustic,
            confirm_replace: false,
            pending: None,
            error: None,
            notice: None,
        }
    }

    /// `In 2 · MM1 34804`.
    pub fn title(&self) -> String {
        format!(
            "Electrical calibration · in {} · {}",
            input_no(self.input),
            self.mic
        )
    }

    /// Where the sensitivity comes from: `data sheet (MM1 34804 0°)`, `typed`.
    pub fn sensitivity_source(&self) -> String {
        match &self.data_sheet {
            Some((v, from)) if *v == self.sensitivity.trim() => format!("data sheet ({from})"),
            _ => "typed".into(),
        }
    }

    /// The safety note for the chosen connection.
    pub fn safety(&self) -> &'static str {
        ac2_scene::cal::electrical_safety(self.connection)
    }

    /// `In-line: mic connected and powered, tone at the mic` / `Injected: …`.
    pub fn method_text(&self) -> &'static str {
        match self.connection {
            ElectricalConnection::InLine => {
                "in-line — mic connected and powered, a steady tone at the mic, meter on pins 2–3"
            }
            ElectricalConnection::Injected => {
                "injected — a generator in place of the mic, phantom power OFF"
            }
        }
    }

    fn touched(&mut self) {
        self.error = None;
        self.notice = None;
        self.confirm_replace = false;
    }

    /// ↑/↓, wrapping.
    pub fn move_focus(&mut self, d: i32) {
        let i = FIELDS.iter().position(|f| *f == self.focus).unwrap_or(0) as i32;
        self.focus = FIELDS[(i + d).rem_euclid(FIELDS.len() as i32) as usize];
    }

    /// ←/→ on the method: in-line ↔ injected.
    pub fn toggle_method(&mut self) {
        if self.focus == Field::Method {
            self.touched();
            self.connection = match self.connection {
                ElectricalConnection::InLine => ElectricalConnection::Injected,
                ElectricalConnection::Injected => ElectricalConnection::InLine,
            };
        }
    }

    fn field(&mut self) -> Option<&mut String> {
        match self.focus {
            Field::Method => None,
            Field::Volts => Some(&mut self.volts),
            Field::Freq => Some(&mut self.freq),
            Field::Sensitivity => Some(&mut self.sensitivity),
        }
    }

    pub fn type_text(&mut self, t: &str) {
        self.touched();
        if let Some(f) = self.field() {
            f.extend(t.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn backspace(&mut self) {
        self.touched();
        if let Some(f) = self.field() {
            f.pop();
        }
    }

    /// Enter: the request, or why not (shown in the dialog). An acoustic calibration of
    /// the input asks once first: it measured the whole chain and is the better one.
    pub fn submit(&mut self) -> Option<Command> {
        if self.pending.is_some() {
            return None;
        }
        self.error = None;
        let parsed = (|| {
            let volts = parse_volts(&self.volts).map_err(|e| (Field::Volts, e))?;
            let freq = parse_freq(&self.freq).map_err(|e| (Field::Freq, e))?;
            let from_sheet = self
                .data_sheet
                .as_ref()
                .is_some_and(|(v, _)| *v == self.sensitivity.trim());
            let mic_sensitivity = if from_sheet {
                None
            } else if self.sensitivity.trim().is_empty() {
                return Err((
                    Field::Sensitivity,
                    self.no_data_sheet
                        .clone()
                        .unwrap_or_else(|| "type the mic's sensitivity, e.g. 15.0 mV/Pa".into()),
                ));
            } else {
                Some(parse_sensitivity(&self.sensitivity).map_err(|e| (Field::Sensitivity, e))?)
            };
            Ok((volts, freq, mic_sensitivity))
        })();
        let (volts, freq, mic_sensitivity) = match parsed {
            Ok(p) => p,
            Err((f, e)) => {
                self.focus = f;
                self.error = Some(e);
                return None;
            }
        };
        if let Some(a) = &self.acoustic
            && !self.confirm_replace
        {
            self.confirm_replace = true;
            self.notice = Some(format!(
                "in {} has an acoustic calibration ({a}), the better one: Enter again to \
                 replace it with this electrical one",
                input_no(self.input)
            ));
            return None;
        }
        let replace_acoustic = self.confirm_replace;
        self.confirm_replace = false;
        self.notice = None;
        self.pending = Some(self.what());
        Some(Command::CalSplElectrical {
            input: self.input,
            mic: self.mic.clone(),
            connection: self.connection,
            volts,
            freq,
            mic_sensitivity,
            uncertainty: None,
            replace_acoustic,
        })
    }

    /// The request's `what`.
    pub fn what(&self) -> String {
        format!("{WHAT_PREFIX}{} ({})", input_no(self.input), self.mic)
    }

    /// A reply to a request with `what`. `true` when it was this dialog's and it is done
    /// (stored: the dialog closes); a refusal stays in the dialog to retry.
    pub fn reply(&mut self, what: &str, result: &Result<(), String>) -> bool {
        if self.pending.as_deref() != Some(what) {
            return false;
        }
        self.pending = None;
        match result {
            Ok(()) => true,
            Err(e) => {
                self.error = Some(e.clone());
                false
            }
        }
    }
}

/// A key in the dialog: the action for the calibrations view, if any. Text arrives
/// separately ([`ElectricalDialog::type_text`]).
pub fn key(
    d: &mut ElectricalDialog,
    chord: &crate::keys::Chord,
) -> Option<crate::cal_view::CalAction> {
    use eframe::egui::Key;
    match chord.key {
        Key::ArrowUp => d.move_focus(-1),
        Key::Tab if chord.shift => d.move_focus(-1),
        Key::ArrowDown | Key::Tab => d.move_focus(1),
        Key::ArrowLeft | Key::ArrowRight => d.toggle_method(),
        Key::Enter => {
            return d
                .submit()
                .map(|c| crate::cal_view::CalAction::Calibrate(c, d.what()));
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{
        CalEntry, CalKey, DeviceId, InputSetup, Mic, MicCurveRef, OpenSession, SplCal,
    };
    use ac2_proto::units::{Db, DbSpl, Dbfs, WallNs};

    fn state(stated: Option<f64>) -> State {
        let mut s = ac2_proto::samples::state();
        s.mics = vec![Mic {
            name: "MM1 34804".into(),
            curves: vec![MicCurveRef {
                label: "0°".into(),
                file_name: "449350_34804_0Grad.txt".into(),
                content_hash: "0".into(),
                points: 100,
                f_lo: Hz(20.0),
                f_hi: Hz(20_000.0),
                imported_at: WallNs(1),
                stated_sensitivity: stated,
            }],
        }];
        s.inputs = vec![InputSetup {
            channel: 1,
            mic: Some("MM1 34804".into()),
            curve: ac2_proto::model::CurveChoice::NotChosen,
        }];
        s.calibrations.clear();
        s
    }

    fn device(s: &State) -> DeviceId {
        s.session
            .open
            .as_ref()
            .map(|o: &OpenSession| o.input_device.clone())
            .unwrap_or_else(|| DeviceId("none".into()))
    }

    #[test]
    fn parses_what_meters_and_data_sheets_show() {
        assert_eq!(parse_volts("15.03 mV"), Ok(Volts(0.01503)));
        assert_eq!(parse_volts("0,015V"), Ok(Volts(0.015)));
        assert!((parse_volts("250 µV").expect("µV").0 - 250e-6).abs() < 1e-15);
        assert!(parse_volts("15").expect_err("unit").contains("mV or V"));
        assert!(parse_volts("0 mV").is_err());
        assert_eq!(parse_freq("1 kHz"), Ok(Hz(1000.0)));
        assert_eq!(parse_freq("400"), Ok(Hz(400.0)));
        assert_eq!(parse_sensitivity("15.0 mV/Pa"), Ok(MvPerPa(15.0)));
        let db = parse_sensitivity("-36.5 dBV/Pa").expect("dBV").0;
        assert!((db - 14.962).abs() < 1e-3);
        assert!(parse_sensitivity("15").is_err());
    }

    #[test]
    fn data_sheet_prefill_typed_and_submit() {
        let s = state(Some(15.0));
        let mut d = ElectricalDialog::new(&s, 1, "MM1 34804");
        assert_eq!(d.title(), "Electrical calibration · in 2 · MM1 34804");
        assert_eq!(d.sensitivity, "15.0 mV/Pa");
        assert_eq!(d.sensitivity_source(), "data sheet (MM1 34804 0°)");
        assert_eq!(d.focus, Field::Volts);
        assert!(d.safety().contains("pins 2 and 3"));
        // Nothing typed: the voltage is asked for, nothing sent.
        assert_eq!(d.submit(), None);
        assert!(d.error.as_deref().is_some_and(|e| e.contains("15.03 mV")));
        d.type_text("15 mV");
        let cmd = d.submit().expect("request");
        assert_eq!(
            cmd,
            Command::CalSplElectrical {
                input: 1,
                mic: "MM1 34804".into(),
                connection: ElectricalConnection::InLine,
                volts: Volts(0.015),
                freq: Hz(1000.0),
                mic_sensitivity: None,
                uncertainty: None,
                replace_acoustic: false,
            }
        );
        // One request at a time; a refusal stays in the dialog, a success closes it.
        assert_eq!(d.submit(), None);
        let what = d.what();
        assert!(!d.reply("something else", &Ok(())));
        assert!(!d.reply(&what, &Err("not steady yet".into())));
        assert_eq!(d.error.as_deref(), Some("not steady yet"));
        assert!(d.submit().is_some());
        assert!(d.reply(&what, &Ok(())));

        // Injected, the sensitivity typed: the warning changes, the source says typed.
        let mut d = ElectricalDialog::new(&s, 1, "MM1 34804");
        d.move_focus(-1);
        assert_eq!(d.focus, Field::Method);
        d.toggle_method();
        assert_eq!(d.connection, ElectricalConnection::Injected);
        assert!(d.safety().contains("phantom power OFF"));
        d.move_focus(1);
        d.type_text("100 mV");
        d.move_focus(1);
        d.backspace();
        d.backspace();
        d.backspace();
        d.backspace();
        d.backspace();
        d.type_text("400 Hz");
        d.move_focus(1);
        for _ in 0..d.sensitivity.chars().count() {
            d.backspace();
        }
        d.type_text("-40 dBV/Pa");
        assert_eq!(d.sensitivity_source(), "typed");
        let Some(Command::CalSplElectrical {
            connection,
            freq,
            mic_sensitivity,
            ..
        }) = d.submit()
        else {
            panic!("{:?}", d.error)
        };
        assert_eq!(connection, ElectricalConnection::Injected);
        assert_eq!(freq, Hz(400.0));
        assert!(mic_sensitivity.is_some_and(|m| (m.0 - 10.0).abs() < 1e-9));
    }

    #[test]
    fn no_data_sheet_and_an_acoustic_calibration() {
        let mut s = state(None);
        let mut d = ElectricalDialog::new(&s, 1, "MM1 34804");
        assert_eq!(d.sensitivity, "");
        d.type_text("15 mV");
        assert_eq!(d.submit(), None);
        assert_eq!(d.focus, Field::Sensitivity);
        assert!(d.error.as_deref().is_some_and(|e| e.contains("state none")));

        s.calibrations = vec![CalEntry {
            key: CalKey {
                device: device(&s),
                channel: 1,
                mic: "MM1 34804".into(),
            },
            spl: SplCal {
                sensitivity: Db(120.0),
                method: CalMethod::Acoustic {
                    calibrator_level: DbSpl(94.0),
                },
                freq: Hz(1000.0),
                measured: Dbfs(-26.0),
                calibrated_at: WallNs(0),
            },
        }];
        let mut d = ElectricalDialog::new(&s, 1, "MM1 34804");
        d.type_text("15 mV");
        d.move_focus(2);
        d.type_text("15 mV/Pa");
        // Asks once, then replaces on purpose.
        assert_eq!(d.submit(), None);
        assert!(
            d.notice
                .as_deref()
                .is_some_and(|n| n.contains("Enter again"))
        );
        let Some(Command::CalSplElectrical {
            replace_acoustic, ..
        }) = d.submit()
        else {
            panic!("{:?}", d.error)
        };
        assert!(replace_acoustic);
    }
}
