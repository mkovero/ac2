//! The rig settings file's shape, and reading it section by section.
//!
//! Each top-level key is a section read on its own: one that cannot be read (a type this
//! build changed, a value a person edited wrong) costs that section only, never the
//! ceiling, the outputs, the monitor or the cues beside it. The rig meters are read one by
//! one, and within a meter its display choices (colour, layout) on their own: a meter that
//! has lost its colour still watches its limit.
//!
//! A rig meter is kept as [`RigMeter`], what the operator set for it, not as a session
//! measurement: a field a session measurement gains (compare picks, display offsets, the
//! transfer delay) never changes this file. A change to [`RigMeter`] or to a section's
//! shape bumps [`VERSION`], and the previous version is read by its own module
//! ([`super::v1`]).

use std::net::SocketAddr;

use ac2_proto::model::{
    ArtnetFade, ArtnetTarget, ColourFamily, MeasConfig, MeasKind, MeasScope, OutputSetup,
    PaneLayout, SessionConfig,
};
use ac2_proto::units::MeasId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::{RigMeter, RigSettings};

pub(super) const FORMAT: &str = "ac2-rig-settings";
pub(super) const VERSION: u32 = 2;

/// The file as written: every section always present (`null` for none), so that a key
/// missing on read is damage, not a default.
#[derive(Serialize)]
struct Out<'a> {
    format: &'static str,
    version: u32,
    ceiling_dbfs: Option<f64>,
    outputs: &'a [OutputSetup],
    meters: &'a [RigMeter],
    monitor_at_start: bool,
    monitor_session: Option<&'a SessionConfig>,
    osc_target: Option<SocketAddr>,
    artnet_target: Option<ArtnetTarget>,
    artnet_fade: ArtnetFade,
}

/// `s` as the file's bytes.
pub(super) fn write(s: &RigSettings) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec_pretty(&Out {
        format: FORMAT,
        version: VERSION,
        ceiling_dbfs: s.ceiling_dbfs,
        outputs: &s.outputs,
        meters: &s.meters,
        monitor_at_start: s.monitor_at_start,
        monitor_session: s.monitor_session.as_ref(),
        osc_target: s.osc_target,
        artnet_target: s.artnet_target,
        artnet_fade: s.artnet_fade,
    })
}

/// The sections of a file; `None`: that section was not read.
#[derive(Debug, Default)]
pub(super) struct Sections {
    pub(super) ceiling_dbfs: Option<Option<f64>>,
    pub(super) outputs: Option<Vec<OutputSetup>>,
    pub(super) meters: Option<Vec<RigMeter>>,
    pub(super) monitor_at_start: Option<bool>,
    pub(super) monitor_session: Option<Option<SessionConfig>>,
    pub(super) osc_target: Option<Option<SocketAddr>>,
    pub(super) artnet_target: Option<Option<ArtnetTarget>>,
    pub(super) artnet_fade: Option<ArtnetFade>,
}

impl Sections {
    /// Each section not read taken from `other`.
    pub(super) fn or(self, other: Self) -> Self {
        Self {
            ceiling_dbfs: self.ceiling_dbfs.or(other.ceiling_dbfs),
            outputs: self.outputs.or(other.outputs),
            meters: self.meters.or(other.meters),
            monitor_at_start: self.monitor_at_start.or(other.monitor_at_start),
            monitor_session: self.monitor_session.or(other.monitor_session),
            osc_target: self.osc_target.or(other.osc_target),
            artnet_target: self.artnet_target.or(other.artnet_target),
            artnet_fade: self.artnet_fade.or(other.artnet_fade),
        }
    }

    /// The names of the sections not read.
    pub(super) fn unread(&self) -> Vec<&'static str> {
        [
            ("ceiling_dbfs", self.ceiling_dbfs.is_none()),
            ("outputs", self.outputs.is_none()),
            ("meters", self.meters.is_none()),
            ("monitor_at_start", self.monitor_at_start.is_none()),
            ("monitor_session", self.monitor_session.is_none()),
            ("osc_target", self.osc_target.is_none()),
            ("artnet_target", self.artnet_target.is_none()),
            ("artnet_fade", self.artnet_fade.is_none()),
        ]
        .into_iter()
        .filter_map(|(k, unread)| unread.then_some(k))
        .collect()
    }

    /// The settings: each section not read as if it had never been set. An unread
    /// `ceiling_dbfs` is `None`, which starts the generator at the `--max-level` bound
    /// ([`super::start_ceiling`]): the same as a missing or damaged file, and never above
    /// what the operator allowed at start.
    pub(super) fn settings(self) -> RigSettings {
        RigSettings {
            ceiling_dbfs: self.ceiling_dbfs.flatten(),
            outputs: self.outputs.unwrap_or_default(),
            meters: self.meters.unwrap_or_default(),
            monitor_at_start: self.monitor_at_start.unwrap_or_default(),
            monitor_session: self.monitor_session.flatten(),
            osc_target: self.osc_target.flatten(),
            artnet_target: self.artnet_target.flatten(),
            artnet_fade: self.artnet_fade.unwrap_or_default(),
        }
    }
}

/// What a rig settings file holds.
#[derive(Debug)]
pub(super) enum Read {
    /// A file of a version this ac2d reads: its version, the sections read, and what could
    /// not be read (each entry names its section).
    File {
        version: u32,
        sections: Box<Sections>,
        lost: Vec<String>,
    },
    /// A rig settings file of a version this ac2d does not read (a newer build's).
    OtherVersion(u32),
}

/// Reads `text`. `Err`: not a rig settings file at all (torn, garbage, another format).
pub(super) fn read(text: &[u8]) -> Result<Read, String> {
    let mut obj = match serde_json::from_slice::<Value>(text).map_err(|e| e.to_string())? {
        Value::Object(o) => o,
        _ => return Err("not a JSON object".into()),
    };
    match obj.remove("format") {
        Some(Value::String(f)) if f == FORMAT => {}
        other => return Err(format!("not a rig settings file (format {other:?})")),
    }
    let version = obj
        .remove("version")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("no format version")?;
    match version {
        1 => super::v1::upgrade(&mut obj),
        VERSION => {}
        v => return Ok(Read::OtherVersion(v)),
    }
    let mut r = Reader {
        obj,
        lost: Vec::new(),
    };
    let sections = Sections {
        ceiling_dbfs: r.section("ceiling_dbfs", check_ceiling),
        outputs: r.section("outputs", |o: &Vec<OutputSetup>| check_outputs(o)),
        meters: r.meters(),
        monitor_at_start: r.section("monitor_at_start", |_| Ok(())),
        monitor_session: r.section("monitor_session", |s: &Option<SessionConfig>| {
            s.as_ref()
                .map_or(Ok(()), |c| crate::session::validate(c).map_err(|e| e.msg))
        }),
        osc_target: r.section("osc_target", check_osc),
        artnet_target: r.section("artnet_target", |t: &Option<ArtnetTarget>| {
            t.map_or(Ok(()), |t| {
                crate::control::check_artnet_addr(t.addr).map_err(|e| e.to_string())
            })
        }),
        artnet_fade: r.section("artnet_fade", |_| Ok(())),
    };
    let mut lost = r.lost;
    lost.extend(
        r.obj
            .keys()
            .map(|k| format!("{k}: not a rig setting this ac2d knows")),
    );
    Ok(Read::File {
        version,
        sections: Box::new(sections),
        lost,
    })
}

struct Reader {
    obj: Map<String, Value>,
    lost: Vec<String>,
}

impl Reader {
    /// The section `key`, read and checked; `None` (and the reason noted) when it cannot be.
    fn section<T: DeserializeOwned>(
        &mut self,
        key: &'static str,
        check: impl FnOnce(&T) -> Result<(), String>,
    ) -> Option<T> {
        field(&mut self.obj, key)
            .and_then(|t| check(&t).map(|()| t).map_err(|e| format!("{key}: {e}")))
            .map_err(|e| self.lost.push(e))
            .ok()
    }

    /// The rig meters, each read on its own: one that cannot be read is left out, noted.
    fn meters(&mut self) -> Option<Vec<RigMeter>> {
        let list: Vec<Value> = self.section("meters", |_| Ok(()))?;
        let mut out: Vec<RigMeter> = Vec::new();
        for v in list {
            match meter(v, &out) {
                Ok((m, notes)) => {
                    self.lost.extend(
                        notes
                            .into_iter()
                            .map(|n| format!("meters: meter {}: {n}", m.id)),
                    );
                    out.push(m);
                }
                Err(e) => self.lost.push(format!("meters: {e}")),
            }
        }
        out.sort_by_key(|m| m.id);
        Some(out)
    }
}

/// One rig meter, checked as the commands check it (a meter they would refuse never runs
/// from a file a person edited), and what of it could not be read.
fn meter(v: Value, kept: &[RigMeter]) -> Result<(RigMeter, Vec<String>), String> {
    let Value::Object(mut o) = v else {
        return Err("a meter that is not a JSON object".into());
    };
    let id: MeasId = field(&mut o, "id").map_err(|e| format!("a meter without an id ({e})"))?;
    let err = |e: String| format!("meter {id}: {e}");
    let config: MeasConfig = field(&mut o, "config").map_err(err)?;
    let running: bool = field(&mut o, "running").map_err(err)?;
    if kept.iter().any(|m| m.id == id) {
        return Err(err("listed twice".into()));
    }
    if config.scope != MeasScope::Rig || !matches!(config.kind, MeasKind::Spl { .. }) {
        return Err(err("not a rig SPL meter".into()));
    }
    crate::control::validate_meas(&config).map_err(|e| err(e.msg))?;
    // Display choices: without them the meter still meters, so only they are lost.
    let mut notes = Vec::new();
    let colour = field::<Option<ColourFamily>>(&mut o, "colour")
        .and_then(|c| match c {
            Some(c) if !c.is_valid() => Err(format!("colour: {c:?} is not a colour family")),
            c => Ok(c),
        })
        .unwrap_or_else(|e| {
            notes.push(e);
            None
        });
    let layout = field::<Option<PaneLayout>>(&mut o, "layout")
        .and_then(|l| match l {
            Some(l) => l
                .check()
                .map(|()| Some(l))
                .map_err(|e| format!("layout: {e}")),
            None => Ok(None),
        })
        .unwrap_or_else(|e| {
            notes.push(e);
            None
        });
    notes.extend(
        o.keys()
            .map(|k| format!("{k}: not a rig meter setting this ac2d knows")),
    );
    Ok((
        RigMeter {
            id,
            config,
            running,
            colour,
            layout,
        },
        notes,
    ))
}

/// `key` of `o`, taken out and read; the error names it.
fn field<T: DeserializeOwned>(o: &mut Map<String, Value>, key: &str) -> Result<T, String> {
    let v = o.remove(key).ok_or_else(|| format!("{key}: missing"))?;
    serde_json::from_value(v).map_err(|e| format!("{key}: {e}"))
}

fn check_ceiling(c: &Option<f64>) -> Result<(), String> {
    match c {
        Some(c) if !(c.is_finite() && *c <= 0.0) => {
            Err(format!("{c} is not a level at or below 0 dBFS"))
        }
        _ => Ok(()),
    }
}

fn check_osc(a: &Option<SocketAddr>) -> Result<(), String> {
    match a {
        Some(a) if a.ip().is_unspecified() || a.port() == 0 => {
            Err(format!("{a} is not an address to send to"))
        }
        _ => Ok(()),
    }
}

fn check_outputs(outputs: &[OutputSetup]) -> Result<(), String> {
    for (i, o) in outputs.iter().enumerate() {
        if outputs[..i].iter().any(|x| x.channel == o.channel) {
            return Err(format!("output {} is listed twice", o.channel + 1));
        }
        match &o.label {
            Some(l) => ac2_proto::model::check_output_label(l)
                .map_err(|e| format!("output {}: {e}", o.channel + 1))?,
            None => return Err(format!("output {} has no label", o.channel + 1)),
        }
    }
    Ok(())
}
