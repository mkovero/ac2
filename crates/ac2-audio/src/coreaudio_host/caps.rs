//! A HAL device description → [`DeviceCaps`], and the stated latency.

use cahal::{Latency, StreamLayout, Transport};

use super::source::DevDesc;
use crate::backend::{
    BackendKind, ClockRelation, ClockSource, DeviceCaps, DeviceId, DirectionCaps, FrameRange,
    IndexExactness, RateRange, SampleFormat, StaticLatency,
};

/// The host name and device id prefix: a device is `coreaudio:<UID>`.
pub(crate) const HOST: &str = "coreaudio";

/// `coreaudio:<uid>`.
pub(crate) fn device_id(uid: &str) -> DeviceId {
    DeviceId(format!("{HOST}:{uid}"))
}

/// The UID in a `coreaudio:<UID>` id.
pub(crate) fn uid_of(id: &DeviceId) -> Option<&str> {
    id.0.strip_prefix(HOST)?.strip_prefix(':')
}

/// The device's names for its channels, or `None` when they cannot tell the channels
/// apart (missing, empty or repeated): the channels are then numbered.
pub(crate) fn channel_names(names: &[Option<String>], channels: u32) -> Option<Vec<String>> {
    if names.len() != channels as usize {
        return None;
    }
    let names: Vec<String> = names
        .iter()
        .map(|n| n.as_deref().unwrap_or("").trim().to_owned())
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    names
        .iter()
        .all(|n| !n.is_empty() && seen.insert(n.as_str()))
        .then_some(names)
}

fn rates(d: &DevDesc) -> Vec<RateRange> {
    let mut r: Vec<RateRange> = d
        .rates
        .iter()
        .map(|r| RateRange {
            min: r.min.round() as u32,
            max: r.max.round() as u32,
        })
        .collect();
    let now = d.nominal_rate.round() as u32;
    if !r.iter().any(|x| x.contains(now)) {
        r.push(RateRange { min: now, max: now });
    }
    r.sort_by_key(|x| (x.min, x.max));
    r.dedup();
    r
}

/// Latency of one direction's converter and transport: the device's, plus its first
/// stream's (the stream that carries channel 1).
fn transport(l: &Latency) -> u32 {
    l.device + l.streams.first().copied().unwrap_or(0)
}

/// The latency the backend states, ASIO's convention: `input + output` is the distance, in
/// index frames, from a sample written at index `i` to where a zero-length analog loop
/// brings it back.
///
/// The HAL places a cycle's input at `now − buffer − safety_in` and its output at
/// `now + buffer + safety_out`; a sample written at index `i` leaves `out − in` frames of
/// sample time later than the input of the same index, then passes the output's and the
/// input's converters. `out_minus_in` is that distance as measured on the first cycle;
/// without one (listing, before any cycle) it is the placement above.
pub(crate) fn stated_latency(
    buffer: u32,
    input: &Latency,
    output: &Latency,
    out_minus_in: Option<u32>,
) -> StaticLatency {
    let out_minus_in =
        out_minus_in.unwrap_or(2 * buffer + input.safety_offset + output.safety_offset);
    let input_part = buffer + input.safety_offset + transport(input);
    let output_part = out_minus_in.saturating_sub(buffer + input.safety_offset) + transport(output);
    StaticLatency::Frames {
        input: input_part,
        output: output_part,
        driver_buffering: None,
    }
}

fn direction(
    d: &DevDesc,
    layout: &StreamLayout,
    names: &[Option<String>],
    default: bool,
) -> Option<DirectionCaps> {
    let channels = layout.channels();
    if channels == 0 {
        return None;
    }
    Some(DirectionCaps {
        max_channels: u16::try_from(channels).unwrap_or(u16::MAX),
        rates: rates(d),
        buffer_frames: Some(FrameRange {
            min: d.buffer_range.min,
            max: d.buffer_range.max,
        }),
        formats: if layout.all_float32() {
            vec![SampleFormat::F32]
        } else {
            Vec::new()
        },
        default_rate: Some(d.nominal_rate.round() as u32),
        default_buffer: Some(d.buffer_frames),
        channel_names: channel_names(names, channels),
        system_default: default,
        spare_channels: 0,
    })
}

/// Notes for the operator about one device.
fn notes(d: &DevDesc, own_pid: i32) -> Vec<String> {
    let mut n = Vec::new();
    for (dir, l) in [("input", &d.input), ("output", &d.output)] {
        for (i, s) in l.streams.iter().enumerate() {
            if let cahal::StreamFormat::Other {
                format_id,
                flags,
                bits,
            } = s.format
            {
                n.push(format!(
                    "{dir} stream {} is {} ({bits} bit, flags {flags:#x}), not 32-bit float: \
                     ac2 does not open it",
                    i + 1,
                    cahal::FourCc(format_id)
                ));
            }
        }
    }
    if let Some(max) = d.variable_buffer {
        n.push(format!(
            "variable buffer sizes (up to {max} frames per cycle)"
        ));
    }
    if let Some(pid) = d.hog_pid
        && pid != own_pid
    {
        n.push(format!("in exclusive use (hog mode) by process {pid}"));
    }
    if d.transport == Transport::Aggregate {
        n.push(
            "an aggregate device: subdevices without drift correction run on clocks of \
             their own"
                .into(),
        );
    }
    n
}

/// The listing of one device. `default_in` / `default_out` mark the system defaults.
pub(crate) fn device_caps(
    d: &DevDesc,
    default_in: bool,
    default_out: bool,
    own_pid: i32,
) -> DeviceCaps {
    let input = direction(d, &d.input, &d.input_names, default_in);
    let output = direction(d, &d.output, &d.output_names, default_out);
    DeviceCaps {
        backend: BackendKind::CoreAudio,
        host: HOST.into(),
        id: device_id(&d.uid),
        name: d.name.clone(),
        duplex_clock: if input.is_some() && output.is_some() {
            ClockRelation::SingleCallback
        } else {
            ClockRelation::Unknown
        },
        input,
        output,
        index: IndexExactness::Exact,
        latency: stated_latency(d.buffer_frames, &d.input_latency, &d.output_latency, None),
        clock_sources: d
            .clock_sources
            .iter()
            .enumerate()
            .map(|(i, c)| ClockSource {
                name: match c.name.trim() {
                    "" => format!("Clock {}", i + 1),
                    s => s.to_owned(),
                },
                current: c.current,
            })
            .collect(),
        notes: notes(d, own_pid),
    }
}
