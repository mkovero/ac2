//! Trace metadata rules shared by the daemon and the fake daemon: fresh edits, colours,
//! slots and what a lock protects.

use ac2_proto::model::{Polarity, Rgb, TraceEdit, TraceMeta};
use ac2_proto::units::{Db, Seconds, TraceId};

/// Okabe–Ito colours (distinguishable with the common colour-vision deficiencies), without
/// black; new traces cycle through them by id.
pub const PALETTE: [Rgb; 7] = [
    Rgb {
        r: 230,
        g: 159,
        b: 0,
    },
    Rgb {
        r: 86,
        g: 180,
        b: 233,
    },
    Rgb {
        r: 0,
        g: 158,
        b: 115,
    },
    Rgb {
        r: 240,
        g: 228,
        b: 66,
    },
    Rgb {
        r: 0,
        g: 114,
        b: 178,
    },
    Rgb {
        r: 213,
        g: 94,
        b: 0,
    },
    Rgb {
        r: 204,
        g: 121,
        b: 167,
    },
];

/// Longest trace name.
pub const MAX_NAME: usize = 128;

/// Edits of a new trace: visible, unlocked, as measured.
pub fn new_edit(id: TraceId, name: String, slot: Option<u8>) -> TraceEdit {
    TraceEdit {
        name,
        color: PALETTE[(id.0 as usize).wrapping_sub(1) % PALETTE.len()],
        visible: true,
        locked: false,
        order: id.0,
        offset: Db(0.0),
        polarity: Polarity::Normal,
        delay_nudge: Seconds(0.0),
        slot,
    }
}

/// Checks a name and slot.
pub fn check_edit(name: &str, slot: Option<u8>) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("a trace needs a name".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("trace names are at most {MAX_NAME} characters"));
    }
    if slot.is_some_and(|s| !(1..=9).contains(&s)) {
        return Err("slots are 1 … 9".into());
    }
    Ok(())
}

/// Checks the numeric display edits.
pub fn check_values(e: &TraceEdit) -> Result<(), String> {
    if !(e.offset.0.is_finite() && e.offset.0.abs() <= 200.0) {
        return Err("offset must be within ±200 dB".into());
    }
    if !(e.delay_nudge.0.is_finite() && e.delay_nudge.0.abs() <= 10.0) {
        return Err("delay nudge must be within ±10 s".into());
    }
    check_edit(&e.name, e.slot)
}

/// What a lock protects: the curve's identity and display (name, colour, offset, polarity,
/// nudge). Showing, hiding, reordering, slotting and unlocking stay possible.
pub fn lock_allows(old: &TraceEdit, new: &TraceEdit) -> bool {
    !old.locked
        || (old.name == new.name
            && old.color == new.color
            && old.offset == new.offset
            && old.polarity == new.polarity
            && old.delay_nudge == new.delay_nudge)
}

/// Other traces that hold `slot` and must give it up for trace `id`, with the slot cleared.
pub fn take_slot(traces: &[TraceMeta], id: TraceId, slot: Option<u8>) -> Vec<TraceMeta> {
    let Some(slot) = slot else {
        return Vec::new();
    };
    traces
        .iter()
        .filter(|t| t.id != id && t.edit.slot == Some(slot))
        .map(|t| {
            let mut t = t.clone();
            t.edit.slot = None;
            t
        })
        .collect()
}

/// A file name for an export of `name`: path separators and control characters replaced.
pub fn file_name(name: &str, ext: &str) -> String {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let base = base.trim().trim_start_matches('.');
    format!("{}.{ext}", if base.is_empty() { "trace" } else { base })
}
