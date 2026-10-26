//! Picking a listed device for one direction, shared by `session open` (the daemon's
//! listing) and `selftest duplex` (the backend's own).
//!
//! Some hosts list each direction as its own device (an input-only and an output-only
//! device, sometimes with the same name), so a name only means something together with the
//! direction it is wanted for.

use ac2_audio::{DeviceCaps, Direction};
use ac2_proto::model::DeviceInfo;

use crate::CliError;

/// A listed device as far as choosing one goes.
pub(crate) trait Listed {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    /// `None` when the device has no side for `dir`, else whether the host defaults to it.
    fn side(&self, dir: Direction) -> Option<bool>;
}

impl Listed for DeviceInfo {
    fn id(&self) -> &str {
        &self.id.0
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn side(&self, dir: Direction) -> Option<bool> {
        match dir {
            Direction::Input => self.input.as_ref(),
            Direction::Output => self.output.as_ref(),
        }
        .map(|d| d.system_default)
    }
}

impl Listed for DeviceCaps {
    fn id(&self) -> &str {
        &self.id.0
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn side(&self, dir: Direction) -> Option<bool> {
        match dir {
            Direction::Input => self.input.as_ref(),
            Direction::Output => self.output.as_ref(),
        }
        .map(|d| d.system_default)
    }
}

fn side_word(dir: Direction) -> &'static str {
    match dir {
        Direction::Input => "capture",
        Direction::Output => "playback",
    }
}

/// The device `sel` names for `dir`: an id, else a name, else a name ignoring case, each
/// among the devices that have a `dir` side only.
pub(crate) fn named<'a, D: Listed>(
    devices: &[&'a D],
    sel: &str,
    dir: Direction,
) -> Result<&'a D, CliError> {
    let usable = || devices.iter().copied().filter(|d| d.side(dir).is_some());
    if let Some(d) = usable()
        .find(|d| d.id() == sel)
        .or_else(|| usable().find(|d| d.name() == sel))
        .or_else(|| usable().find(|d| d.name().eq_ignore_ascii_case(sel)))
    {
        return Ok(d);
    }
    let side = side_word(dir);
    if let Some(d) = devices
        .iter()
        .find(|d| d.id() == sel || d.name().eq_ignore_ascii_case(sel))
    {
        return Err(CliError::Usage(format!(
            "{} ({}) has no {side} side (see `ac2 devices`)",
            d.name(),
            d.id()
        )));
    }
    let known: Vec<String> = usable()
        .map(|d| format!("{} ({})", d.id(), d.name()))
        .collect();
    Err(CliError::Usage(format!(
        "no {side} device {sel:?}; listed: {}",
        if known.is_empty() {
            "none".to_owned()
        } else {
            known.join(", ")
        }
    )))
}

/// The device to use for `dir` when none is named: the host's default for that direction,
/// else the only device with a `dir` side. `None` when there is none or several and the
/// host names no default.
pub(crate) fn default_for<'a, D: Listed>(devices: &[&'a D], dir: Direction) -> Option<&'a D> {
    let usable = || devices.iter().copied().filter(|d| d.side(dir).is_some());
    usable().find(|d| d.side(dir) == Some(true)).or_else(|| {
        let mut it = usable();
        match (it.next(), it.next()) {
            (Some(d), None) => Some(d),
            _ => None,
        }
    })
}

/// How many listed devices have a `dir` side.
pub(crate) fn count<D: Listed>(devices: &[&D], dir: Direction) -> usize {
    devices.iter().filter(|d| d.side(dir).is_some()).count()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ac2_proto::model::{BackendKind, ClockRelation, DeviceId, DirectionInfo, IndexExactness};

    fn dir(n: u16, default: bool) -> Option<DirectionInfo> {
        (n > 0).then(|| DirectionInfo {
            max_channels: n,
            rates_hz: vec![],
            buffer_frames: None,
            default_rate_hz: None,
            default_buffer_frames: None,
            channel_names: None,
            system_default: default,
            spare_channels: 0,
        })
    }

    /// A listed device: `inputs`/`outputs` channels, `default` on each side it has.
    pub(crate) fn info(
        id: &str,
        name: &str,
        inputs: u16,
        outputs: u16,
        default: bool,
    ) -> DeviceInfo {
        DeviceInfo {
            backend: BackendKind::CoreAudio,
            host: "coreaudio".into(),
            id: DeviceId(id.into()),
            name: name.into(),
            input: dir(inputs, default),
            output: dir(outputs, default),
            duplex_clock: ClockRelation::SingleCallback,
            index: IndexExactness::Exact,
            clock_sources: vec![],
            notes: vec![],
        }
    }

    /// One interface listed as a playback-only device first, then a capture-only device
    /// with the very same name.
    fn split() -> Vec<DeviceInfo> {
        vec![
            info("usb-out", "Speakers (USB Audio)", 0, 2, true),
            info("usb-in", "Speakers (USB Audio)", 2, 0, true),
            info("webcam", "Microphone (Webcam)", 1, 0, false),
        ]
    }

    #[test]
    fn a_shared_name_resolves_by_direction() {
        let devs = split();
        let refs: Vec<&DeviceInfo> = devs.iter().collect();
        let cap = named(&refs, "Speakers (USB Audio)", Direction::Input).expect("capture");
        assert_eq!(cap.id.0, "usb-in");
        let play = named(&refs, "speakers (usb audio)", Direction::Output).expect("playback");
        assert_eq!(play.id.0, "usb-out");
    }

    #[test]
    fn a_device_without_the_side_is_refused_by_name() {
        let devs = split();
        let refs: Vec<&DeviceInfo> = devs.iter().collect();
        let e = named(&refs, "Microphone (Webcam)", Direction::Output).expect_err("refused");
        assert!(e.to_string().contains("no playback side"), "{e}");
        let e = named(&refs, "usb-out", Direction::Input).expect_err("refused");
        assert!(e.to_string().contains("no capture side"), "{e}");
        assert!(named(&refs, "nope", Direction::Input).is_err());
    }

    #[test]
    fn defaults_follow_the_host_per_direction() {
        let devs = split();
        let refs: Vec<&DeviceInfo> = devs.iter().collect();
        assert_eq!(
            default_for(&refs, Direction::Input).map(|d| d.id.0.as_str()),
            Some("usb-in")
        );
        assert_eq!(
            default_for(&refs, Direction::Output).map(|d| d.id.0.as_str()),
            Some("usb-out")
        );
        // No host default and two capture devices: nothing to guess.
        let devs = [info("a", "A", 2, 0, false), info("b", "B", 2, 0, false)];
        let refs: Vec<&DeviceInfo> = devs.iter().collect();
        assert!(default_for(&refs, Direction::Input).is_none());
        // The only capture device is the default even when the host names none.
        let devs = [info("a", "A", 0, 2, false), info("b", "B", 2, 0, false)];
        let refs: Vec<&DeviceInfo> = devs.iter().collect();
        assert_eq!(
            default_for(&refs, Direction::Input).map(|d| d.id.0.as_str()),
            Some("b")
        );
    }
}
