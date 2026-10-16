//! Texts of the rig monitor and its OSC cues, shared by the measurement tree, the Rig
//! monitor settings page and the SPL limit editor: a cue state's word and colours, a cue
//! message as one typed line (`/light/1 1.0`) and back, the OSC target, the sending status.

use std::net::SocketAddr;

use ac2_proto::Warning;
use ac2_proto::model::{CueState, MeasScope, Measurement, OscArg, OscMessage, OscPath, Rig};
use ac2_proto::units::MeasId;

use crate::primitives::Color;
use crate::theme::Theme;

/// The scope toggle of an SPL meter, as its row in the limit editor reads.
pub const KEEP_ON_RIG: &str = "Keep on rig (survives session loads)";
/// The rig group's header in the measurement tree.
pub const RIG: &str = "Rig";
/// The header of the session's measurements, under the rig group.
pub const SESSION: &str = "Session";
/// The message Send test starts with.
pub const DEFAULT_TEST: &str = "/ac2/test 1";
/// The Rig monitor settings page, as the sidebar and the tree name it.
pub const PAGE: &str = "Rig monitor";
/// The Rig monitor page's rows.
pub const TARGET_TITLE: &str = "OSC target";
pub const MONITOR_TITLE: &str = "Monitor at start";
pub const TEST_TITLE: &str = "Send test";
/// Under the target row.
pub const TARGET_NOTE: &str =
    "IP address and port of the receiver (a lighting desk, a stage display); empty: off";
/// Under the test row.
pub const TEST_NOTE: &str = "Enter sends this message once: an address, then its arguments";
/// The cues' section of an SPL meter's limit editor.
pub const CUES_TITLE: &str = "OSC cues";
/// What a cue line is, under the cues' heading.
pub const CUES_NOTE: &str = "sent to the rig's OSC target when a state begins, then every second: \
                             an address, then arguments (/light/1 1.0); Level appends the level \
                             in dB SPL";
/// The heading over the rig's meters on the Rig monitor page.
pub const METERS_TITLE: &str = "Meters on the rig";
/// The Rig monitor page with no rig meter.
pub const NO_RIG_METERS: &str =
    "No meter on the rig: on an SPL meter's SPL / Leq page, turn on “Keep on rig”.";

/// The order a limit's cue fields are listed in: what matters most first.
pub const STATES: [CueState; 4] = [
    CueState::Over,
    CueState::Near,
    CueState::Ok,
    CueState::Fault,
];

/// A cue state as its chip and every text say it, in the words of the Leq tiles.
pub fn state_word(s: CueState) -> &'static str {
    match s {
        CueState::Ok => "OK",
        CueState::Near => "NEAR",
        CueState::Over => "OVER",
        CueState::Fault => "FAULT",
    }
}

/// A cue field's label in the limit editor.
pub fn field_title(s: CueState) -> &'static str {
    match s {
        CueState::Over => "Over",
        CueState::Near => "Near",
        CueState::Ok => "OK",
        CueState::Fault => "Fault",
    }
}

/// Background and text of a cue chip: over and near as the Leq tiles colour them; OK plain
/// with the level-OK colour; fault plain in the fault colour (the rig cannot judge, which
/// is not a level over its limit).
pub fn cue_colors(s: CueState, theme: &Theme) -> (Color, Color) {
    match s {
        CueState::Over => (theme.banner_fault.background, theme.banner_fault.text),
        CueState::Near => (theme.banner_warning.background, theme.banner_warning.text),
        CueState::Ok => (theme.plot_background, theme.level_ok),
        CueState::Fault => (theme.plot_background, theme.banner_fault.background),
    }
}

/// The state a meter's chip shows: its meter-wide cue's, else the worst of its cued
/// limits'; `None` when nothing of it is cued.
pub fn meter_state(rig: &Rig, meas: MeasId) -> Option<CueState> {
    let mine = rig.cues.iter().filter(|c| c.meas == meas);
    mine.clone()
        .find(|c| c.subject == ac2_proto::model::CueSubject::Meter)
        .map(|c| c.state)
        .or_else(|| mine.map(|c| c.state).max())
}

/// Whether `m` is kept by the rig rather than the session.
pub fn on_rig(m: &Measurement) -> bool {
    m.config.scope == MeasScope::Rig
}

/// An argument as typed: an integer bare, a float always with its point (so it reads
/// back a float), a string bare unless it would read back as a number or holds a space
/// or a quote: then quoted.
fn arg_text(a: &OscArg) -> String {
    match a {
        OscArg::Int(i) => i.to_string(),
        OscArg::Float(f) => {
            let s = f.to_string();
            if s.contains(['.', 'e', 'E']) || !f.is_finite() {
                s
            } else {
                format!("{s}.0")
            }
        }
        OscArg::Str(s) => {
            let plain = !s.is_empty()
                && !s
                    .chars()
                    .any(|c| c.is_whitespace() || c == '"' || c == '\\')
                && matches!(typed_arg(s), OscArg::Str(_));
            if plain {
                s.clone()
            } else {
                let esc = s.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{esc}\"")
            }
        }
    }
}

/// An unquoted token: an `i32`, else a finite float written as a number, else a string.
fn typed_arg(t: &str) -> OscArg {
    if let Ok(i) = t.parse::<i32>() {
        return OscArg::Int(i);
    }
    let numeric = t
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'));
    match t.parse::<f32>() {
        Ok(f) if numeric && f.is_finite() => OscArg::Float(f),
        _ => OscArg::Str(t.to_owned()),
    }
}

/// A cue message as one line: its address, then its arguments (`/light/1 1.0 "red"`).
/// Whether the level is appended is a toggle of its own, not part of the line.
pub fn message_text(m: &OscMessage) -> String {
    let mut s = m.path.as_str().to_owned();
    for a in &m.args {
        s.push(' ');
        s.push_str(&arg_text(a));
    }
    s
}

/// A typed cue line read back: empty is no message; else an address and its arguments,
/// split at spaces, `"…"` for a string with spaces (`\"`, `\\` inside). An integer reads
/// `i`, a number with a point `f`, anything else a string.
pub fn parse_message(text: &str, append_level: bool) -> Result<Option<OscMessage>, String> {
    let tokens = tokens(text)?;
    let Some((path, rest)) = tokens.split_first() else {
        return Ok(None);
    };
    let Token::Bare(path) = path else {
        return Err("an OSC address starts with '/'".into());
    };
    let path = OscPath::new(path.clone()).map_err(|e| e.to_string())?;
    let args = rest
        .iter()
        .map(|t| match t {
            Token::Bare(s) => typed_arg(s),
            Token::Quoted(s) => OscArg::Str(s.clone()),
        })
        .collect();
    Ok(Some(OscMessage {
        path,
        args,
        append_level,
    }))
}

enum Token {
    Bare(String),
    Quoted(String),
}

fn tokens(text: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            let mut s = String::new();
            loop {
                match chars.next() {
                    None => return Err("a quoted argument is missing its closing \"".into()),
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some(e) => s.push(e),
                        None => return Err("a quoted argument is missing its closing \"".into()),
                    },
                    Some(x) => s.push(x),
                }
            }
            out.push(Token::Quoted(s));
        } else {
            let mut s = String::new();
            while let Some(&x) = chars.peek() {
                if x.is_whitespace() {
                    break;
                }
                s.push(x);
                chars.next();
            }
            out.push(Token::Bare(s));
        }
    }
    Ok(out)
}

/// The OSC target as typed: empty (or `off`) sends nowhere; else an IP address and port.
pub fn parse_target(text: &str) -> Result<Option<SocketAddr>, String> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    match t.parse::<SocketAddr>() {
        Ok(a) if a.port() != 0 => Ok(Some(a)),
        _ => Err(format!(
            "{t:?}: type an IP address and port, like 192.168.1.20:9000 (empty: off)"
        )),
    }
}

/// The OSC target in the field: the address, or empty when off.
pub fn target_text(t: Option<SocketAddr>) -> String {
    t.map_or_else(String::new, |a| a.to_string())
}

/// `OSC to 192.168.1.20:9000 · 12 sent · last error: …`; off: `OSC off`.
pub fn status_line(rig: &Rig) -> String {
    let mut s = match rig.osc_target {
        Some(a) => format!("OSC to {a} · {}", sent_text(rig.osc.sent)),
        None if rig.osc.sent > 0 => format!("OSC off · {}", sent_text(rig.osc.sent)),
        None => "OSC off".to_owned(),
    };
    if let Some(e) = &rig.osc.last_error {
        s.push_str(&format!(" · last error: {e}"));
    }
    s
}

fn sent_text(n: u64) -> String {
    if n == 1 {
        "1 message sent".into()
    } else {
        format!("{n} messages sent")
    }
}

/// The monitor-at-start choice as shown.
pub fn monitor_text(on: bool) -> &'static str {
    if on {
        "on: the rig meters run from daemon start, no client needed"
    } else {
        "off: the rig meters run while a client has the audio session open"
    }
}

/// The scope toggle's value as shown.
pub fn scope_text(on_rig: bool) -> &'static str {
    if on_rig {
        "on: kept by the rig, survives session loads"
    } else {
        "off: saved and loaded with the session"
    }
}

/// The append-level toggle's value as shown.
pub fn append_text(on: bool) -> &'static str {
    if on { "+ level" } else { "no level" }
}

/// The rig group's header detail, longest first: the meters and how sending fares.
pub fn rig_details(meters: usize, rig: &Rig) -> Vec<String> {
    let n = if meters == 1 {
        "1 meter".to_owned()
    } else {
        format!("{meters} meters")
    };
    let osc = match rig.osc_target {
        Some(a) => format!("OSC to {a}"),
        None => "OSC off".to_owned(),
    };
    let mut long = format!("{n} · {osc}");
    if rig.osc.last_error.is_some() {
        long.push_str(" · send error");
    }
    vec![long, n]
}

/// A rig meter's line on the Rig monitor page: `FOH SPL · running · OVER`.
pub fn meter_line(m: &Measurement, state: Option<CueState>) -> String {
    let run = if m.running { "running" } else { "stopped" };
    match state {
        Some(s) => format!("{} · {run} · {}", m.config.name, state_word(s)),
        None => format!("{} · {run} · no cue", m.config.name),
    }
}

/// The toast after the daemon refused an edit made on settings another client changed
/// first: the shown settings are the rig's again.
pub fn conflict_toast(what: &str) -> String {
    format!("{what}: changed elsewhere — reloaded")
}

/// The toast when the refused edit stays on its page, rebased on the newer settings.
pub fn conflict_kept_toast(what: &str) -> String {
    format!("{what}: changed elsewhere — not applied, your edits kept")
}

/// The line under a page whose Apply was refused as made on settings another client changed
/// first: the edits stay, and the next Apply is made on the newer settings.
pub const CHANGED_ELSEWHERE: &str = "changed elsewhere: Apply again overwrites with your edits";

/// [`CHANGED_ELSEWHERE`] on the Rig monitor page, where Enter applies a row.
pub const CHANGED_ELSEWHERE_ENTER: &str =
    "changed elsewhere: Enter again overwrites with your edit";

/// A reply's warning in words, the rig meters it lists by `names` (in its order); the same
/// sentences the CLI prints.
pub fn reply_warning(w: &Warning, names: &[String]) -> String {
    match w {
        Warning::RigMonitorBlind { .. } => format!(
            "rig SPL meter(s) {} stop metering until a session opens again; their OSC cues \
             report fault",
            names.join(", ")
        ),
        Warning::RigMonitorMoved { .. } => format!(
            "rig SPL meter(s) {} now meter this device's inputs",
            names.join(", ")
        ),
    }
}

/// The meters a reply's warning lists.
pub fn warning_meas(w: &Warning) -> &[MeasId] {
    match w {
        Warning::RigMonitorBlind { meas } | Warning::RigMonitorMoved { meas } => meas,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::{CueSubject, OutputStatus, PeakQuantity, RigCue};

    fn msg(path: &str, args: Vec<OscArg>) -> OscMessage {
        OscMessage {
            path: OscPath::new(path).expect("test value"),
            args,
            append_level: false,
        }
    }

    #[test]
    fn a_message_reads_back_as_typed() {
        let cases = [
            msg("/light/1", vec![OscArg::Float(1.0)]),
            msg("/ac2/test", vec![OscArg::Int(1)]),
            msg(
                "/x",
                vec![
                    OscArg::Int(-3),
                    OscArg::Float(-0.25),
                    OscArg::Str("red".into()),
                    OscArg::Str("12".into()),
                    OscArg::Str("two words".into()),
                    OscArg::Str("say \"hi\"".into()),
                    OscArg::Str(String::new()),
                    OscArg::Str("inf".into()),
                ],
            ),
            msg("/bare", vec![]),
        ];
        for m in cases {
            let t = message_text(&m);
            assert_eq!(parse_message(&t, false), Ok(Some(m.clone())), "{t}");
        }
        assert_eq!(
            message_text(&msg("/light/1", vec![OscArg::Float(1.0)])),
            "/light/1 1.0"
        );
    }

    #[test]
    fn typed_arguments_take_their_type() {
        let m = parse_message("  /light/1 1 1.0 1e3 red \"a b\" ", true)
            .expect("test value")
            .expect("test value");
        assert_eq!(m.path.as_str(), "/light/1");
        assert_eq!(
            m.args,
            vec![
                OscArg::Int(1),
                OscArg::Float(1.0),
                OscArg::Float(1000.0),
                OscArg::Str("red".into()),
                OscArg::Str("a b".into()),
            ]
        );
        assert!(m.append_level);
        assert_eq!(parse_message("   ", false), Ok(None));
    }

    #[test]
    fn a_bad_address_says_why() {
        assert_eq!(
            parse_message("light 1", false),
            Err("an OSC address starts with '/'".into())
        );
        assert_eq!(
            parse_message("/light/* 1", false),
            Err("'*' is not allowed in an OSC address".into())
        );
        assert_eq!(
            parse_message("\"/x\"", false),
            Err("an OSC address starts with '/'".into())
        );
        assert!(parse_message("/x \"open", false).is_err());
    }

    #[test]
    fn the_target_is_an_address_or_off() {
        assert_eq!(parse_target(""), Ok(None));
        assert_eq!(parse_target(" off "), Ok(None));
        assert_eq!(
            parse_target("192.168.1.20:9000"),
            Ok(Some("192.168.1.20:9000".parse().expect("test value")))
        );
        assert_eq!(
            parse_target("[fe80::1]:9000").map(target_text),
            Ok("[fe80::1]:9000".into())
        );
        for bad in ["lights.local:9000", "192.168.1.20", "10.0.0.1:0"] {
            assert!(parse_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_status_line_says_target_sent_and_error() {
        let mut rig = Rig::default();
        assert_eq!(status_line(&rig), "OSC off");
        rig.osc_target = Some("10.0.0.5:9000".parse().expect("test value"));
        rig.osc = OutputStatus {
            sent: 12,
            last_error: None,
        };
        assert_eq!(status_line(&rig), "OSC to 10.0.0.5:9000 · 12 messages sent");
        rig.osc.last_error = Some("network unreachable".into());
        assert_eq!(
            status_line(&rig),
            "OSC to 10.0.0.5:9000 · 12 messages sent · last error: network unreachable"
        );
        assert_eq!(
            rig_details(2, &rig),
            ["2 meters · OSC to 10.0.0.5:9000 · send error", "2 meters"]
        );
    }

    #[test]
    fn the_chip_shows_the_meter_cue_else_the_worst_limit() {
        let cue = |meas, subject, state| RigCue {
            meas: MeasId(meas),
            subject,
            state,
        };
        let peak = CueSubject::Peak {
            quantity: PeakQuantity::LcPeak,
        };
        let mut rig = Rig::default();
        assert_eq!(meter_state(&rig, MeasId(1)), None);
        rig.cues = vec![
            cue(1, peak, CueState::Near),
            cue(2, peak, CueState::Over),
            cue(1, CueSubject::Predicted, CueState::Ok),
        ];
        assert_eq!(meter_state(&rig, MeasId(1)), Some(CueState::Near));
        rig.cues.push(cue(1, CueSubject::Meter, CueState::Ok));
        assert_eq!(meter_state(&rig, MeasId(1)), Some(CueState::Ok));
        assert_eq!(state_word(CueState::Over), "OVER");
    }

    #[test]
    fn chip_colours_follow_the_leq_tiles() {
        let t = Theme::dark();
        let tile = |s| crate::leq::tile_colors(s, &t);
        use crate::leq::TileState;
        assert_eq!(cue_colors(CueState::Over, &t), tile(TileState::Over));
        assert_eq!(cue_colors(CueState::Near, &t), tile(TileState::Near));
        assert_eq!(cue_colors(CueState::Ok, &t).0, tile(TileState::Ok).0);
    }

    #[test]
    fn reply_warnings_name_the_meters() {
        let names = ["Stage".to_owned(), "FOH".to_owned()];
        let blind = Warning::RigMonitorBlind {
            meas: vec![MeasId(1), MeasId(2)],
        };
        assert_eq!(warning_meas(&blind), &[MeasId(1), MeasId(2)]);
        assert_eq!(
            reply_warning(&blind, &names),
            "rig SPL meter(s) Stage, FOH stop metering until a session opens again; their OSC \
             cues report fault"
        );
        let moved = Warning::RigMonitorMoved {
            meas: vec![MeasId(1)],
        };
        assert_eq!(
            reply_warning(&moved, &names[..1]),
            "rig SPL meter(s) Stage now meter this device's inputs"
        );
        assert_eq!(
            conflict_kept_toast("FOH SPL: Leq windows set"),
            "FOH SPL: Leq windows set: changed elsewhere — not applied, your edits kept"
        );
    }
}
