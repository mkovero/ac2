//! The dynamics dialog: a processor's attack and release measured with tone bursts. Like a
//! sweep it is made without playing; Space on its Dynamics pane arms a run. The pilot and
//! burst levels have no default: they depend on the processor's threshold, which only the
//! operator knows.

use super::*;
use ac2_proto::model::{DynamicsConfig, DynamicsOnset};
use ac2_scene::format::{MINUS, fixed};
use ac2_scene::unit_text::{Freq, LevelDbfs, Time, in_field};

/// The burst lengths a new measurement offers: decades of 1-3-10 from 1 ms to 10 s, the
/// span over which limiters' attack and release times fall.
const DEFAULT_DURATIONS: &str = "1ms, 3ms, 10ms, 30ms, 100ms, 300ms, 1s, 3s, 10s";
const ONSETS: [(&str, DynamicsOnset); 2] = [
    ("zero crossing", DynamicsOnset::Zero),
    ("peak", DynamicsOnset::Peak),
];

/// `-20` from −20 (the field is typed with an ASCII minus).
fn typed(v: f64) -> String {
    fixed(v, 1).replace(MINUS, "-")
}

fn seconds_text(s: f64) -> String {
    if s < 1.0 {
        format!("{}ms", fixed(s * 1e3, 0))
    } else {
        format!("{}s", fixed(s, if s.fract() == 0.0 { 0 } else { 1 }))
    }
}

fn list<T>(s: &str, mut each: impl FnMut(&str) -> Result<T, String>) -> Result<Vec<T>, String> {
    s.split([',', ' '])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(&mut each)
        .collect()
}

impl Form {
    pub(crate) fn is_dynamics(&self) -> bool {
        matches!(self.kind, FormKind::Dynamics | FormKind::DynamicsEdit)
    }

    /// The dialog over the session's captured `inputs` and its `outputs`, the reference and
    /// outputs picked as for a sweep (the loopback's, when the session has one).
    pub fn dynamics(
        open: Option<&OpenSession>,
        made: usize,
        inputs: &[(u16, String)],
        outputs: &[(u16, String)],
    ) -> Self {
        let sweep = Self::sweep(open, 0, inputs, outputs, &[], None);
        let pick = |id: FieldId| sweep.channel(id);
        let fields = vec![
            Field::channel(
                FieldId::Reference,
                "Reference",
                inputs,
                pick(FieldId::Reference),
                "the loopback (programme copy)",
            ),
            Field::channel(
                FieldId::Measurement,
                "Measurement",
                inputs,
                pick(FieldId::Measurement),
                "the processor's output",
            ),
            Field::channel(
                FieldId::Output,
                "Output",
                outputs,
                pick(FieldId::Output),
                "what plays the bursts: the processor's input and the loopback",
            ),
            Field::text(FieldId::Carrier, "Carrier", "1 kHz", ""),
            Field::choice(FieldId::Onset, "Onset", &ONSETS.map(|o| o.0), 0),
            Field::text(
                FieldId::Pilot,
                "Pilot",
                "",
                "dBFS, required: the rest level, below the threshold",
            ),
            Field::text(
                FieldId::Bursts,
                "Bursts",
                "",
                "dBFS, required: levels above the threshold, in play order (-14, -8)",
            ),
            Field::text(FieldId::Durations, "Lengths", DEFAULT_DURATIONS, ""),
            Field::text(
                FieldId::RestMax,
                "Longest rest",
                "30s",
                "a rest not recovered by then ends anyway",
            ),
            Field::text(FieldId::Name, "Name", format!("Dynamics {}", made + 1), ""),
        ];
        let mut f = Self::new(FormKind::Dynamics, fields);
        let first: Vec<u16> = f.channel(FieldId::Measurement).into_iter().collect();
        f.set_sweep_inputs(&first);
        f.set_sweep_outputs(&sweep.sweep_outputs());
        f
    }

    /// The dialog editing dynamics measurement `m`: its settings as the fields.
    pub fn edit_dynamics(
        m: &Measurement,
        open: Option<&OpenSession>,
        inputs: &[(u16, String)],
        outputs: &[(u16, String)],
    ) -> Result<Self, String> {
        let MeasKind::Dynamics { config: c } = &m.config.kind else {
            return Err(format!("{} is not a dynamics measurement", m.config.name));
        };
        let mut inputs = inputs.to_vec();
        for &i in std::iter::once(&c.reference_input).chain(&c.measurement_inputs) {
            if !inputs.iter().any(|(x, _)| *x == i) {
                inputs.push((i, format!("In {} (not captured)", i + 1)));
            }
        }
        let outputs = super::build::with_outputs(outputs, &c.outputs);
        let mut f = Self::dynamics(open, 0, &inputs, &outputs);
        f.kind = FormKind::DynamicsEdit;
        f.sweep_edit = Some(m.id);
        let carrier = format!("{} Hz", fixed(c.carrier.0, 0));
        let bursts: Vec<String> = c.bursts.iter().map(|b| typed(b.0)).collect();
        let lengths: Vec<String> = c.durations.iter().map(|d| seconds_text(d.0)).collect();
        for field in &mut f.fields {
            match (field.id, &mut field.value) {
                (
                    FieldId::Reference,
                    Value::Channel {
                        channels, index, ..
                    },
                ) => {
                    *index = channels.iter().position(|x| *x == c.reference_input);
                }
                (FieldId::Carrier, Value::Text(t)) => t.clone_from(&carrier),
                (FieldId::Onset, Value::Choice { index, .. }) => {
                    *index = ONSETS.iter().position(|o| o.1 == c.onset).unwrap_or(0);
                }
                (FieldId::Pilot, Value::Text(t)) => *t = typed(c.pilot.0),
                (FieldId::Bursts, Value::Text(t)) => *t = bursts.join(", "),
                (FieldId::Durations, Value::Text(t)) => *t = lengths.join(", "),
                (FieldId::RestMax, Value::Text(t)) => *t = seconds_text(c.rest_max.0),
                (FieldId::Name, Value::Text(t)) => t.clone_from(&m.config.name),
                _ => {}
            }
        }
        f.set_sweep_inputs(&c.measurement_inputs);
        f.set_sweep_outputs(&c.outputs);
        Ok(f)
    }

    /// `meas.create` (or `meas.update`) of the dialog. The daemon checks the programme as a
    /// whole (carrier against the rate, burst counts); what is checked here is what a field
    /// alone can say. Levels above the maximum are kept: a run checks them when it plays.
    pub fn dynamics_meas(&self, open: Option<&OpenSession>) -> Result<MeasConfig, String> {
        let open = open.ok_or("no audio I/O: open one first")?;
        let reference = self
            .channel(FieldId::Reference)
            .ok_or("choose the reference: the input that records the loopback")?;
        let measurement = self.sweep_inputs();
        let c = DynamicsConfig {
            reference_input: reference,
            measurement_inputs: measurement,
            outputs: Vec::new(),
            carrier: ac2_proto::units::Hz(0.0),
            onset: DynamicsOnset::Zero,
            pilot: Dbfs(0.0),
            bursts: Vec::new(),
            durations: Vec::new(),
            rest_max: Seconds(DynamicsConfig::DEFAULT_REST_MAX),
        };
        c.check_inputs()?;
        let outputs = self.play_outputs(Some(open))?;
        let carrier = in_field::<Freq>(self.text(FieldId::Carrier), "hz")?.0;
        let pilot = self.text(FieldId::Pilot).trim();
        if pilot.is_empty() {
            return Err("type the pilot level (dBFS): it depends on the threshold".into());
        }
        let pilot = in_field::<LevelDbfs>(pilot, "dbfs")?.0;
        let bursts = list(self.text(FieldId::Bursts), |t| {
            Ok(in_field::<LevelDbfs>(t, "dbfs")?.0)
        })?;
        if bursts.is_empty() {
            return Err("type the burst levels (dBFS), loudest last or in any order".into());
        }
        let durations = list(self.text(FieldId::Durations), |t| {
            Ok(in_field::<Time>(t, "s")?.0)
        })?;
        if durations.is_empty() {
            return Err("type the burst lengths (e.g. 10ms, 100ms, 1s)".into());
        }
        let rest_max = in_field::<Time>(self.text(FieldId::RestMax), "s")?.0;
        let name = self.text(FieldId::Name).trim();
        if name.is_empty() {
            return Err("type a name".into());
        }
        let onset = ONSETS[self.choice_index(FieldId::Onset).unwrap_or(0).min(1)].1;
        Ok(MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: name.to_owned(),
            kind: MeasKind::Dynamics {
                config: DynamicsConfig {
                    outputs,
                    carrier,
                    onset,
                    pilot,
                    bursts,
                    durations,
                    rest_max,
                    ..c
                },
            },
        })
    }
}
