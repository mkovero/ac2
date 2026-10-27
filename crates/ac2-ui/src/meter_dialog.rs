//! The programme meter dialog: what a Programme pane meters (one of the session's pairs, or up
//! to eight inputs side by side, picked by name with their meters) and how the rig's
//! programme meters read. The second part is the rig's (`levels.set`): the daemon computes the
//! ballistics, so every app on the rig reads the same bar. It is marked as the rig's, and Enter
//! sends only what was changed here, so a value another client set meanwhile stays.

use ac2_proto::model::{
    Ballistic, CORRELATION_GATE_RANGE, CorrelationWindow, LOUDNESS_TARGET_RANGE, LevelsSettings,
    OVER_THRESHOLD_RANGE,
};
use ac2_proto::units::{Dbfs, Dbtp, Lufs};
use ac2_scene::programme::{form as words, rig};

use crate::forms::{Field, FieldId, Form, FormKind, Value};
use crate::state::{MAX_METERED, Metered, PaneId};

/// The dialog's own state beside its fields.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterForm {
    /// The session's pairs (left, right) and their names: the Meter field's options, before
    /// the inputs.
    pairs: Vec<((u16, u16), String)>,
    /// The session's inputs: channel and name as the dialogs show it.
    inputs: Vec<(u16, String)>,
    /// The over threshold's and the gate's options, dB; the loudness target's, LUFS.
    over: Vec<f64>,
    gate: Vec<f64>,
    target: Vec<f64>,
    /// The ember decay's options, seconds ([`crate::settings::ember_decay_steps`]).
    ember: Vec<f32>,
    /// The ember glow's options ([`crate::settings::ember_glow_steps`]).
    glow: Vec<f32>,
    /// The rig's settings when the dialog opened: a field still at its value is not sent.
    pub opened: LevelsSettings,
    /// The Programme pane whose meters change in place; `None`: a new pane.
    pub edit: Option<PaneId>,
}

impl MeterForm {
    /// The pair the Meter field `f` has picked; `None` for the inputs.
    pub(crate) fn pair_of(&self, f: &Field) -> Option<(u16, u16)> {
        match &f.value {
            Value::Choice { index, .. } => self.pairs.get(*index).map(|p| p.0),
            _ => None,
        }
    }
}

/// What the What part shows: the picked pair (an index of the pairs), else the inputs.
struct What {
    pair: Option<usize>,
    bars: Vec<Option<u16>>,
}

/// The dialog over the session's `pairs` and `inputs`, starting from `now` (an edited pane's
/// meters; for a new pane what to start from, else the first pair, else the first input), and
/// the rig's settings `rig`, and how this app's embers cool `embers` (decay and glow).
/// `edit`: the pane it changes in place.
pub fn form(
    pairs: Vec<((u16, u16), String)>,
    inputs: Vec<(u16, String)>,
    now: Option<Metered>,
    rig: LevelsSettings,
    embers: ac2_scene::gonio::Cooling,
    edit: Option<PaneId>,
) -> Form {
    let kind = match edit {
        Some(_) => FormKind::ProgrammeEdit,
        None => FormKind::Programme,
    };
    let captured = |c: &u16| inputs.iter().any(|(x, _)| x == c);
    let what = match now {
        Some(Metered::Pair { left, right }) => {
            match pairs.iter().position(|p| p.0 == (left, right)) {
                Some(i) => What {
                    pair: Some(i),
                    bars: Vec::new(),
                },
                None => What {
                    pair: None,
                    bars: vec![Some(left), Some(right)],
                },
            }
        }
        Some(m) => What {
            pair: None,
            bars: m.inputs().into_iter().map(Some).collect(),
        },
        None if !pairs.is_empty() => What {
            pair: Some(0),
            bars: Vec::new(),
        },
        None => What {
            pair: None,
            bars: inputs.first().map(|i| Some(i.0)).into_iter().collect(),
        },
    };
    let what = What {
        bars: what
            .bars
            .into_iter()
            .filter(|b| b.as_ref().is_none_or(captured))
            .collect(),
        ..what
    };
    let meter = MeterForm {
        over: steps(
            rig.over_threshold.0,
            rig::OVER_STEP_DB,
            OVER_THRESHOLD_RANGE,
        ),
        gate: steps(
            rig.correlation_gate.0,
            rig::GATE_STEP_DB,
            CORRELATION_GATE_RANGE,
        ),
        target: steps(
            rig.loudness_target.0,
            rig::TARGET_STEP_LU,
            LOUDNESS_TARGET_RANGE,
        ),
        ember: crate::settings::ember_decay_steps(),
        glow: crate::settings::ember_glow_steps(),
        pairs,
        inputs,
        opened: rig,
        edit,
    };
    let mut f = Form::new(kind, Vec::new());
    f.fields = what_fields(&meter, &what);
    f.fields.extend(rig_fields(&meter, rig));
    f.fields.push(ember_field(&meter.ember, embers.decay_s()));
    f.fields.push(glow_field(&meter.glow, embers.glow()));
    f.meter = Some(Box::new(meter));
    rehead(&mut f);
    f
}

/// The values of a stepped setting: its range in `by` steps, and `now` where it lies between
/// them (set by the CLI), so the dialog shows the rig's value as it is.
fn steps(now: f64, by: f64, (lo, hi): (f64, f64)) -> Vec<f64> {
    let n = ((hi - lo) / by).round() as usize;
    let mut v: Vec<f64> = (0..=n).map(|i| lo + i as f64 * by).collect();
    if !v.iter().any(|x| (x - now).abs() < 1e-9) {
        v.push(now);
        v.sort_by(f64::total_cmp);
    }
    v
}

fn what_fields(m: &MeterForm, w: &What) -> Vec<Field> {
    let mut v = Vec::new();
    if !m.pairs.is_empty() {
        let options: Vec<&str> = m
            .pairs
            .iter()
            .map(|p| p.1.as_str())
            .chain([words::INPUTS_OPTION])
            .collect();
        let mut f = Field::choice(
            FieldId::Metered,
            words::METER,
            &options,
            w.pair.unwrap_or(m.pairs.len()),
        );
        f.hint = match w.pair {
            Some(_) => words::PAIR_HINT,
            None => words::INPUTS_HINT,
        }
        .to_owned();
        v.push(f);
    }
    if w.pair.is_some() {
        return v;
    }
    // Every picked input, then one empty row to add the next input in.
    let mut bars = w.bars.clone();
    while bars.len() > 1 && bars.last() == Some(&None) {
        bars.pop();
    }
    if bars.last().is_none_or(Option::is_some) && bars.len() < MAX_METERED {
        bars.push(None);
    }
    for (i, b) in bars.iter().enumerate() {
        let hint = if i == 0 {
            words::FIRST_BAR_HINT
        } else {
            words::MORE_BAR_HINT
        };
        let mut f = Field::channel(FieldId::Bar(i as u8), "", &m.inputs, *b, hint);
        f.label = words::bar_label(i);
        v.push(f);
    }
    v
}

fn rig_fields(m: &MeterForm, l: LevelsSettings) -> Vec<Field> {
    let mut ballistic = Field::choice(
        FieldId::Ballistic,
        rig::BALLISTIC_TITLE,
        &Ballistic::ALL.map(Ballistic::word),
        Ballistic::ALL
            .iter()
            .position(|b| *b == l.ballistic)
            .unwrap_or(0),
    );
    ballistic.hint = rig::BALLISTIC_NOTE.to_owned();
    let mut window = Field::choice(
        FieldId::CorrelationWindow,
        rig::WINDOW_TITLE,
        &CorrelationWindow::ALL.map(CorrelationWindow::text),
        CorrelationWindow::ALL
            .iter()
            .position(|w| *w == l.correlation_window)
            .unwrap_or(0),
    );
    window.hint = rig::WINDOW_NOTE.to_owned();
    let stepped =
        |id, title: &str, note: &str, all: &[f64], now: f64, text: fn(f64) -> String| Field {
            id,
            label: title.to_owned(),
            value: Value::Choice {
                options: all.iter().map(|x| text(*x)).collect(),
                index: all.iter().position(|x| *x == now).unwrap_or(0),
            },
            hint: note.to_owned(),
        };
    vec![
        ballistic,
        stepped(
            FieldId::LoudnessTarget,
            rig::TARGET_TITLE,
            rig::TARGET_NOTE,
            &m.target,
            l.loudness_target.0,
            rig::target_text,
        ),
        window,
        stepped(
            FieldId::OverThreshold,
            rig::OVER_TITLE,
            rig::OVER_NOTE,
            &m.over,
            l.over_threshold.0,
            rig::over_text,
        ),
        stepped(
            FieldId::CorrelationGate,
            rig::GATE_TITLE,
            rig::GATE_NOTE,
            &m.gate,
            l.correlation_gate.0,
            rig::gate_text,
        ),
    ]
}

/// The index of the step of `all` nearest `now`: a stored value between steps shows as it.
fn nearest(all: &[f32], now: f32) -> usize {
    all.iter()
        .enumerate()
        .min_by(|a, b| (a.1 - now).abs().total_cmp(&(b.1 - now).abs()))
        .map_or(0, |(i, _)| i)
}

/// The goniometer's fade, the same setting as Settings › Display's.
fn ember_field(all: &[f32], now: f32) -> Field {
    let index = nearest(all, now);
    Field {
        id: FieldId::EmberDecay,
        label: words::EMBER_TITLE.to_owned(),
        value: Value::Choice {
            options: all
                .iter()
                .map(|s| crate::settings::ember_decay_text(*s))
                .collect(),
            index,
        },
        hint: words::EMBER_NOTE.to_owned(),
    }
}

/// The goniometer's glow, the same setting as Settings › Display's.
fn glow_field(all: &[f32], now: f32) -> Field {
    Field {
        id: FieldId::EmberGlow,
        label: words::GLOW_TITLE.to_owned(),
        value: Value::Choice {
            options: all
                .iter()
                .map(|g| crate::settings::ember_glow_text(*g))
                .collect(),
            index: nearest(all, now),
        },
        hint: words::GLOW_NOTE.to_owned(),
    }
}

/// The ember glow the form shows, a gain.
pub fn ember_glow(f: &Form) -> Option<f32> {
    let m = f.meter.as_deref()?;
    m.glow.get(f.choice_index(FieldId::EmberGlow)?).copied()
}

/// The ember decay the form shows, seconds.
pub fn ember_decay(f: &Form) -> Option<f32> {
    let m = f.meter.as_deref()?;
    m.ember.get(f.choice_index(FieldId::EmberDecay)?).copied()
}

/// The headings: what to meter over the first field, how it reads (the rig's) over the rig's,
/// and the display (this app's) over the ember decay.
fn rehead(f: &mut Form) {
    let first = f.fields.first().map(|x| x.id);
    f.headings = first
        .map(|id| (id, words::WHAT_HEADING.to_owned()))
        .into_iter()
        .chain([(
            FieldId::Ballistic,
            format!(
                "{} · Whose: {}",
                words::HOW_HEADING,
                crate::settings::THE_RIG
            ),
        )])
        .chain([(
            FieldId::EmberDecay,
            format!(
                "{} · Whose: {}",
                words::DISPLAY_HEADING,
                crate::settings::THIS_APP
            ),
        )])
        .collect();
}

fn what(f: &Form, m: &MeterForm) -> What {
    let pair = f
        .field(FieldId::Metered)
        .and_then(|x| match &x.value {
            Value::Choice { index, .. } => Some(*index),
            _ => None,
        })
        .filter(|i| *i < m.pairs.len());
    let bars = f
        .fields
        .iter()
        .filter(|x| matches!(x.id, FieldId::Bar(_)))
        .map(Field::channel_value)
        .collect();
    What { pair, bars }
}

/// ←/→ on the Meter field or an input row: the What part laid out again for the new pick. False
/// for any other field (a plain choice steps as in every form).
pub(crate) fn cycle(f: &mut Form, d: i32) -> bool {
    let Some(m) = f.meter.clone() else {
        return false;
    };
    let Some(id) = f.fields.get(f.focus).map(|x| x.id) else {
        return false;
    };
    let mut w = what(f, &m);
    match id {
        FieldId::Metered => {
            let n = m.pairs.len();
            let at = w.pair.unwrap_or(n) as i64;
            let to = (at + i64::from(d.signum())).clamp(0, n as i64) as usize;
            if to < n {
                w.pair = Some(to);
            } else if let Some(p) = w.pair.and_then(|i| m.pairs.get(i)) {
                // From a pair to inputs: its two sides are the first bars.
                w = What {
                    pair: None,
                    bars: vec![Some(p.0.0), Some(p.0.1)],
                };
            } else {
                w.pair = None;
            }
        }
        FieldId::Bar(b) => {
            let b = usize::from(b);
            let taken: Vec<u16> = w
                .bars
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != b)
                .filter_map(|(_, c)| *c)
                .collect();
            // A later row may name nothing; no two rows name one input.
            let opts: Vec<Option<u16>> = (b > 0)
                .then_some(None)
                .into_iter()
                .chain(
                    m.inputs
                        .iter()
                        .map(|(c, _)| Some(*c))
                        .filter(|c| c.is_some_and(|c| !taken.contains(&c))),
                )
                .collect();
            if opts.is_empty() {
                return true;
            }
            let cur = w.bars.get(b).copied().flatten();
            let to = match opts.iter().position(|o| *o == cur) {
                Some(i) => (i as i64 + i64::from(d.signum())).clamp(0, opts.len() as i64 - 1),
                // Input 1 with nothing picked: → the first input, ← the last.
                None if d < 0 => opts.len() as i64 - 1,
                None => 0,
            } as usize;
            if let Some(slot) = w.bars.get_mut(b) {
                *slot = opts[to];
            }
        }
        _ => return false,
    }
    let rigs: Vec<Field> = f
        .fields
        .iter()
        .filter(|x| is_rig(x.id) || matches!(x.id, FieldId::EmberDecay | FieldId::EmberGlow))
        .cloned()
        .collect();
    f.fields = what_fields(&m, &w);
    f.fields.extend(rigs);
    if let Some(i) = f.fields.iter().position(|x| x.id == id) {
        f.focus = i;
    }
    rehead(f);
    f.error = None;
    true
}

fn is_rig(id: FieldId) -> bool {
    matches!(
        id,
        FieldId::Ballistic
            | FieldId::LoudnessTarget
            | FieldId::CorrelationWindow
            | FieldId::OverThreshold
            | FieldId::CorrelationGate
    )
}

/// The rig's settings as the dialog shows them.
pub fn levels(f: &Form) -> Option<LevelsSettings> {
    let m = f.meter.as_deref()?;
    let at = |id| f.choice_index(id).unwrap_or(0);
    Some(LevelsSettings {
        ballistic: Ballistic::ALL[at(FieldId::Ballistic).min(Ballistic::ALL.len() - 1)],
        correlation_window: CorrelationWindow::ALL
            [at(FieldId::CorrelationWindow).min(CorrelationWindow::ALL.len() - 1)],
        over_threshold: Dbtp(m.over.get(at(FieldId::OverThreshold)).copied()?),
        correlation_gate: Dbfs(m.gate.get(at(FieldId::CorrelationGate)).copied()?),
        loudness_target: Lufs(m.target.get(at(FieldId::LoudnessTarget)).copied()?),
    })
}

/// The rig's settings to ask for: the rig's `now`, with each value the dialog changed from
/// what it opened with.
pub fn wanted(
    opened: LevelsSettings,
    shown: LevelsSettings,
    now: LevelsSettings,
) -> LevelsSettings {
    fn pick<T: PartialEq>(opened: T, shown: T, now: T) -> T {
        if shown != opened { shown } else { now }
    }
    LevelsSettings {
        ballistic: pick(opened.ballistic, shown.ballistic, now.ballistic),
        correlation_window: pick(
            opened.correlation_window,
            shown.correlation_window,
            now.correlation_window,
        ),
        over_threshold: pick(
            opened.over_threshold,
            shown.over_threshold,
            now.over_threshold,
        ),
        correlation_gate: pick(
            opened.correlation_gate,
            shown.correlation_gate,
            now.correlation_gate,
        ),
        loudness_target: pick(
            opened.loudness_target,
            shown.loudness_target,
            now.loudness_target,
        ),
    }
}

/// What Enter meters, or why it cannot.
pub fn metered(f: &Form) -> Result<Metered, String> {
    let Some(m) = f.meter.as_deref() else {
        return Err(words::CHOOSE_FIRST.to_owned());
    };
    let w = what(f, m);
    if let Some((left, right)) = w.pair.and_then(|i| m.pairs.get(i)).map(|p| p.0) {
        return Ok(Metered::Pair { left, right });
    }
    let list: Vec<u16> = w.bars.into_iter().flatten().collect();
    if list.is_empty() {
        return Err(words::CHOOSE_FIRST.to_owned());
    }
    Metered::inputs_of(&list).ok_or_else(|| words::BARS_DISTINCT.to_owned())
}

#[cfg(test)]
mod tests;
