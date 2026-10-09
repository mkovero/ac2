//! Free helpers of the reducer: parsing typed text, trace labels, slots and the pane a trace is drawn in.

use super::*;

/// A stored trace as messages and captions name it: `slot 3 (Main L S3)`, or its name.
pub fn trace_label(t: &TraceMeta) -> String {
    match t.edit.slot {
        Some(n) => format!("slot {n} ({})", t.edit.name),
        None => t.edit.name.clone(),
    }
}

/// What a trace command without a selected trace says.
pub(super) const SELECT_TRACE_FIRST: &str =
    "select a stored trace first (V, Alt+V for hidden ones, or click it in the list)";

/// A display offset as its prompt starts: empty for none, else `-3.5`.
pub(super) fn offset_text(v: f64) -> String {
    if v == 0.0 {
        String::new()
    } else {
        format::fixed(v, 1).replace(format::MINUS, "-")
    }
}

/// A slot as typed: `1` … `9`, or `none` (also empty, `-`, `off`) to free it.
pub fn parse_slot(text: &str) -> Result<Option<u8>, String> {
    let t = text.trim().to_ascii_lowercase();
    let t = t.strip_prefix("slot").map_or(t.as_str(), str::trim);
    if matches!(t, "" | "none" | "-" | "off") {
        return Ok(None);
    }
    match t.parse::<u8>() {
        Ok(n @ 1..=9) => Ok(Some(n)),
        _ => Err(format!("{text:?}: a slot is 1 … 9, or none")),
    }
}

/// Whether pane `p` draws stored trace `t` (when shown).
pub(super) fn drawn_in(t: &TraceMeta, p: PaneKind) -> bool {
    match p {
        PaneKind::Transfer | PaneKind::Ir => matches!(
            t.kind,
            TraceKind::Transfer | TraceKind::Target | TraceKind::Sweep
        ),
        PaneKind::Spectrum => matches!(t.kind, TraceKind::Spectrum { .. } | TraceKind::Rta { .. }),
        PaneKind::Distortion => t.kind == TraceKind::Sweep,
        PaneKind::Spl => false,
    }
}

/// A shown transfer-like stored curve: what the transfer pane draws when it is also in the
/// pane's group ([`super::AppState::on_transfer_pane`]).
pub fn transfer_kind_shown(t: &TraceMeta) -> bool {
    t.edit.visible
        && matches!(
            t.kind,
            TraceKind::Transfer | TraceKind::Target | TraceKind::Sweep
        )
}

pub fn open_session_hint(keymap: &Keymap) -> String {
    let first = |c| {
        keymap
            .chords(c, Scope::Global)
            .first()
            .map(|k: &Chord| k.label())
    };
    match (first(CommandId::OpenSession), first(CommandId::Palette)) {
        (Some(o), Some(p)) => format!("press {o} (or {p} → Open audio session)"),
        (Some(o), None) => format!("press {o}"),
        (None, Some(p)) => format!("{p} → Open audio session"),
        (None, None) => "command palette → Open audio session".into(),
    }
}

pub(super) fn slot_of(c: CommandId) -> u8 {
    use CommandId as C;
    match c {
        C::Slot1 | C::ShowSlot1 => 1,
        C::Slot2 | C::ShowSlot2 => 2,
        C::Slot3 | C::ShowSlot3 => 3,
        C::Slot4 | C::ShowSlot4 => 4,
        C::Slot5 | C::ShowSlot5 => 5,
        C::Slot6 | C::ShowSlot6 => 6,
        C::Slot7 | C::ShowSlot7 => 7,
        C::Slot8 | C::ShowSlot8 => 8,
        _ => 9,
    }
}

/// A session given by name, or by path (anything with a separator, or starting with `.` or
/// `~`); a path is made absolute, since the daemon does not share this process's working
/// directory.
pub fn parse_session_ref(text: &str) -> Result<SessionRef, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("type a session name or a directory path".into());
    }
    let is_path = t.contains('/')
        || t.contains('\\')
        || t.starts_with('.')
        || t.starts_with('~')
        || std::path::Path::new(t).is_absolute();
    if !is_path {
        return Ok(SessionRef::Name { name: t.to_owned() });
    }
    let p = match t.strip_prefix('~') {
        Some(rest) => std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|h| std::path::PathBuf::from(h).join(rest.trim_start_matches(['/', '\\'])))
            .ok_or_else(|| "no home directory to expand ~".to_string())?,
        None => std::path::PathBuf::from(t),
    };
    let p = std::path::absolute(&p).map_err(|e| e.to_string())?;
    Ok(SessionRef::Path {
        path: p.to_string_lossy().into_owned(),
    })
}

/// A number with an optional unit suffix (case-insensitive); accepts `−` and `,` decimal.
pub fn parse_number(text: &str, units: &[&str]) -> Result<f64, String> {
    let mut t = text.trim().to_ascii_lowercase().replace(format::MINUS, "-");
    for u in units {
        if let Some(s) = t.strip_suffix(u) {
            t = s.trim().to_string();
            break;
        }
    }
    let t = t.replace(',', ".");
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(format!("not a number: {:?}", text.trim())),
    }
}

/// The input a measurement's mic curve belongs to (a transfer function's measurement
/// input); a math channel has its operands', none of its own.
pub fn meas_input(k: &MeasKind) -> Option<u16> {
    match k {
        MeasKind::Transfer { config } => Some(config.measurement_input),
        MeasKind::Spectrum { config } => Some(config.input),
        MeasKind::Rta { config } => Some(config.input),
        MeasKind::Spl { config } => Some(config.input),
        MeasKind::Sweep { config } => Some(config.measurement_input),
        MeasKind::Math { .. } => None,
    }
}

/// The toast of a curve choice: `input 2: mic curve MM1 34804 90°`, `input 2: mic curve off`.
pub fn curve_what(row: &InputSetup) -> String {
    let n = u32::from(row.channel) + 1;
    match (&row.mic, &row.curve) {
        (Some(m), CurveChoice::Curve { label }) => {
            format!(
                "input {n}: mic curve {}",
                ac2_scene::cal::curve_name(m, label)
            )
        }
        (_, CurveChoice::NotChosen) => format!("input {n}: no mic curve chosen"),
        _ => format!("input {n}: mic curve off"),
    }
}

/// `3=M30, 4=ECM` (1-based) of the rows with a mic name, or `3=` for a cleared one.
pub fn mics_text(rows: &[InputSetup]) -> String {
    rows.iter()
        .map(|r| {
            format!(
                "{}={}",
                u32::from(r.channel) + 1,
                r.mic.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parses `3=M30, 4=ECM, 5=` into zero-based channels and names (`None` clears).
pub fn parse_mics(text: &str) -> Result<Vec<(u16, Option<String>)>, String> {
    let mut v: Vec<(u16, Option<String>)> = Vec::new();
    for part in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (ch, name) = part
            .split_once('=')
            .ok_or_else(|| format!("{part:?}: expected input=name, e.g. 3=M30"))?;
        let n: u16 = ch
            .trim()
            .parse()
            .map_err(|_| format!("not an input number: {:?}", ch.trim()))?;
        if n == 0 {
            return Err("inputs count from 1".into());
        }
        let name = name.trim();
        if name.chars().count() > 64 {
            return Err(format!(
                "mic name of input {n} is longer than 64 characters"
            ));
        }
        if v.iter().any(|(c, _)| *c == n - 1) {
            return Err(format!("input {n} given twice"));
        }
        v.push((n - 1, (!name.is_empty()).then(|| name.to_owned())));
    }
    if v.is_empty() {
        return Err("type at least one input=name".into());
    }
    Ok(v)
}

/// `80-800`, `80 – 800 Hz`: band edges in Hz, lower first.
pub fn parse_band(text: &str) -> Result<(f64, f64), String> {
    let t = text.replace(['–', '—'], "-");
    let (a, b) = t
        .split_once('-')
        .ok_or_else(|| format!("{:?}: expected low-high, e.g. 80-800", text.trim()))?;
    let lo = parse_number(a, &["hz"])?;
    let hi = parse_number(b, &["hz"])?;
    if !(lo > 0.0 && hi > lo) {
        return Err("band edges must be above 0 Hz, the upper above the lower".into());
    }
    Ok((lo, hi))
}

/// `1, 2` (one-based) → `[0, 1]`.
pub fn parse_outputs(text: &str) -> Result<Vec<u16>, String> {
    let mut v = Vec::new();
    for part in text.split([',', ' ']).filter(|s| !s.is_empty()) {
        let n: u16 = part
            .parse()
            .map_err(|_| format!("not a channel number: {part:?}"))?;
        if n == 0 {
            return Err("channels count from 1".into());
        }
        if !v.contains(&(n - 1)) {
            v.push(n - 1);
        }
    }
    if v.is_empty() {
        return Err("at least one output channel".into());
    }
    Ok(v)
}
