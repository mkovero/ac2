//! The calibrations view's notices: what a key did, or why it did nothing, on the line it
//! was pressed on. Inputs are numbered from 1 here, as everywhere the operator reads them;
//! the callers pass the 0-based channel.

use crate::cal::curve_name;

fn input_no(channel: u16) -> u32 {
    u32::from(channel) + 1
}

/// ←/→ on an input with no mic name.
pub fn no_mic_for_curve(channel: u16) -> String {
    format!(
        "input {} has no mic name: N names it, then ←/→ choose its curve",
        input_no(channel)
    )
}

/// The input's mic curve as just stepped to: `label` is the curve, `None` off.
pub fn curve_stepped(channel: u16, mic: &str, label: Option<&str>) -> String {
    match label {
        Some(label) => format!(
            "input {}: mic curve {}",
            input_no(channel),
            curve_name(mic, label)
        ),
        None => format!("input {}: mic curve off ({mic})", input_no(channel)),
    }
}

/// C or E on an input the open audio I/O does not capture.
pub fn not_captured(channel: u16) -> String {
    format!(
        "input {} is not in the open audio I/O: a calibration reads the input live",
        input_no(channel)
    )
}

/// E on an input with no mic name.
pub fn no_mic_for_electrical(channel: u16) -> String {
    format!(
        "input {} has no mic name: N names it, then E calibrates it",
        input_no(channel)
    )
}

/// A calibration stored: `what` the call, `after` what to do next.
pub fn stored(what: &str, after: &str) -> String {
    format!("{what}: stored. {after}")
}

/// What Delete deletes on a curve line.
pub fn delete_curve(mic: &str, label: &str) -> String {
    format!("curve {} of the mic library", curve_name(mic, label))
}

/// What Delete deletes on a sensitivity calibration line.
pub fn delete_sensitivity(mic: &str, channel: u16, device: &str) -> String {
    format!(
        "the sensitivity calibration of {mic} on input {} of {device}",
        input_no(channel)
    )
}

/// The first Delete press: asks for the second.
pub fn delete_again(what: &str) -> String {
    format!("Delete again to delete {what}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_read_from_one() {
        assert_eq!(
            no_mic_for_curve(0),
            "input 1 has no mic name: N names it, then ←/→ choose its curve"
        );
        assert_eq!(
            no_mic_for_electrical(1),
            "input 2 has no mic name: N names it, then E calibrates it"
        );
        assert_eq!(
            not_captured(2),
            "input 3 is not in the open audio I/O: a calibration reads the input live"
        );
    }

    #[test]
    fn curve_steps() {
        assert_eq!(
            curve_stepped(0, "MM1 34804", Some("90°")),
            "input 1: mic curve MM1 34804 90°"
        );
        assert_eq!(
            curve_stepped(0, "MM1 34804", None),
            "input 1: mic curve off (MM1 34804)"
        );
    }

    #[test]
    fn stored_and_delete() {
        assert_eq!(
            stored("cal.acoustic", "Done."),
            "cal.acoustic: stored. Done."
        );
        let what = delete_curve("MM1", "0°");
        assert_eq!(what, "curve MM1 0° of the mic library");
        assert_eq!(
            delete_again(&what),
            "Delete again to delete curve MM1 0° of the mic library"
        );
        assert_eq!(
            delete_sensitivity("MM1", 0, "Babyface"),
            "the sensitivity calibration of MM1 on input 1 of Babyface"
        );
    }
}
