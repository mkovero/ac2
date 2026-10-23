//! The electrical calibration dialog of the calibrations view (`docs/design/q7-calibration.md`
//! §11): a voltage measured at an input with a meter while the daemon reads the input's
//! level, and the mic's sensitivity, give the input's sensitivity calibration without an
//! acoustic calibrator.
//!
//! Keyboard-first like the other dialogs: ↑/↓ (Tab) move between the fields, ←/→ choose
//! where the voltage is measured (in-line or injected), typing edits the focused field,
//! Enter reads and stores, Esc closes. The sensitivity starts as the data sheet's (the mic's
//! curve files), named as such; editing it makes it typed. The mic's output impedance and
//! the preamp's input impedance are optional (both or neither): the data-sheet sensitivity is
//! open circuit, and the preamp loads the mic; the dialog shows the correction they make
//! as they are typed.
//!
//! Pure data: the reducer turns Enter into a `cal.spl_electrical` request and hands the
//! reply back through [`ElectricalDialog::reply`].

use ac2_proto::Command;
use ac2_proto::cal::{self, DataSheet};
use ac2_proto::model::{CalMethod, ElectricalConnection, MicLoading, State};
use ac2_proto::units::MvPerPa;
use ac2_scene::unit_text::{Freq, MicSensitivityArg, OhmsArg, VoltsArg, in_field};

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
    /// The mic's output impedance (optional).
    MicImpedance,
    /// The preamp's input impedance (optional).
    InputImpedance,
}

const FIELDS: [Field; 6] = [
    Field::Method,
    Field::Volts,
    Field::Freq,
    Field::Sensitivity,
    Field::MicImpedance,
    Field::InputImpedance,
];

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
    /// The mic's output impedance as typed; blank with `input_impedance`: no loading.
    pub mic_impedance: String,
    /// The preamp's input impedance as typed.
    pub input_impedance: String,
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
            mic_impedance: String::new(),
            input_impedance: String::new(),
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

    /// The loading as typed: `Ok(None)` both blank, or the field to fix and why.
    fn loading(&self) -> Result<Option<MicLoading>, (Field, String)> {
        let (m, i) = (self.mic_impedance.trim(), self.input_impedance.trim());
        match (m.is_empty(), i.is_empty()) {
            (true, true) => Ok(None),
            (true, false) => Err((
                Field::MicImpedance,
                "give the mic's output impedance too, or leave both blank".into(),
            )),
            (false, true) => Err((
                Field::InputImpedance,
                "give the preamp's input impedance too, or leave both blank".into(),
            )),
            (false, false) => Ok(Some(MicLoading {
                mic_impedance: in_field::<OhmsArg>(m, "ohm")
                    .map_err(|e| (Field::MicImpedance, e.0))?
                    .0,
                input_impedance: in_field::<OhmsArg>(i, "ohm")
                    .map_err(|e| (Field::InputImpedance, e.0))?
                    .0,
            })),
        }
    }

    /// The loading correction as typed, live: `loaded 330 Ω → 2.0 kΩ −1.3 dB`, `blank: no
    /// loading correction`, or what is missing.
    pub fn loading_text(&self) -> String {
        match self.loading() {
            Ok(None) => "blank: no loading correction".into(),
            Ok(Some(l)) => ac2_scene::cal::loading(&l),
            Err((_, e)) => e,
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
            Field::MicImpedance => Some(&mut self.mic_impedance),
            Field::InputImpedance => Some(&mut self.input_impedance),
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
            if self.volts.trim().is_empty() {
                return Err((
                    Field::Volts,
                    "type the voltage the meter shows, e.g. 15.03 mV".into(),
                ));
            }
            let volts = self
                .volts
                .parse::<VoltsArg>()
                .map_err(|e| (Field::Volts, e.0))?
                .0;
            let freq = in_field::<Freq>(&self.freq, "hz")
                .map_err(|e| (Field::Freq, e.0))?
                .0;
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
                Some(
                    self.sensitivity
                        .parse::<MicSensitivityArg>()
                        .map_err(|e| (Field::Sensitivity, e.0))?
                        .0,
                )
            };
            let loading = self.loading()?;
            Ok((volts, freq, mic_sensitivity, loading))
        })();
        let (volts, freq, mic_sensitivity, loading) = match parsed {
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
            loading,
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
    use ac2_proto::units::{Db, DbSpl, Dbfs, Hz, Ohms, Volts, WallNs};

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
            kind: ac2_proto::model::InputKind::Mic {
                mic: Some("MM1 34804".into()),
                curve: ac2_proto::model::CurveChoice::NotChosen,
            },
            pair: None,
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
                loading: None,
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
    fn loading_both_or_neither_shown_live() {
        let s = state(Some(15.0));
        let mut d = ElectricalDialog::new(&s, 1, "MM1 34804");
        assert_eq!(d.loading_text(), "blank: no loading correction");
        d.type_text("15 mV");
        d.move_focus(3);
        assert_eq!(d.focus, Field::MicImpedance);
        d.type_text("330 ohm");
        assert_eq!(
            d.loading_text(),
            "give the preamp's input impedance too, or leave both blank"
        );
        // One impedance alone is refused at that field, nothing sent.
        assert_eq!(d.submit(), None);
        assert_eq!(d.focus, Field::InputImpedance);
        d.type_text("2 k");
        assert_eq!(d.loading_text(), "loaded 330 Ω → 2.0 kΩ −1.3 dB");
        let Some(Command::CalSplElectrical {
            mic_sensitivity,
            loading,
            ..
        }) = d.submit()
        else {
            panic!("{:?}", d.error)
        };
        assert_eq!(mic_sensitivity, None);
        assert_eq!(
            loading,
            Some(MicLoading {
                mic_impedance: Ohms(330.0),
                input_impedance: Ohms(2000.0),
            })
        );
        // Down from the last field wraps to the method.
        d.move_focus(1);
        assert_eq!(d.focus, Field::Method);
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
