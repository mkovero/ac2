//! The rig's settings the daemon keeps across restarts: the system max level (the
//! generator ceiling), the output labels, the rig's own SPL meters and whether the daemon
//! reopens their audio at start. They belong to the rig, not to a session: a loaded session
//! must never raise the level limit, the wiring outlives any session, and a meter watching
//! a noise limit must keep watching whatever session a client loads.
//!
//! One small JSON file in the daemon's config directory, written atomically (temporary file,
//! synced, renamed). Like the calibration store, a file that cannot be read is never
//! written: the daemon starts at its `--max-level` bound with no labels, and every change is
//! refused with the reason, so a lowered limit is never silently replaced. A file of another
//! format version is set aside (renamed, never deleted).

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ac2_proto::model::{
    ArtnetFade, ArtnetTarget, MeasKind, MeasScope, OutputSetup, SessionConfig, StateFile,
};
use ac2_proto::{ErrorCode, ProtoError};
use ac2_traces::session::SavedMeasurement;
use serde::{Deserialize, Serialize};

use crate::util::perr;

const FORMAT: &str = "ac2-rig-settings";
const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RigFile {
    format: String,
    version: u32,
    /// The system max level last set, dBFS RMS; `None`: never set (the bound applies).
    ceiling_dbfs: Option<f64>,
    /// Labelled outputs, sorted by channel.
    outputs: Vec<OutputSetup>,
    /// The rig's SPL meters (`MeasScope::Rig`), with their ids.
    #[serde(default)]
    measurements: Vec<SavedMeasurement>,
    /// Reopen `monitor_session` capture-only at start.
    #[serde(default)]
    monitor_at_start: bool,
    /// The audio configuration of the last `session.open` while rig meters existed.
    #[serde(default)]
    monitor_session: Option<SessionConfig>,
    /// Where OSC cues go; absent: nowhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    osc_target: Option<SocketAddr>,
    /// Where the Art-Net universe goes; absent: nowhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artnet_target: Option<ArtnetTarget>,
    /// How long the DMX channels take to move to a new cue frame; absent (a file written
    /// before fades): [`ArtnetFade::DEFAULT`].
    #[serde(default)]
    artnet_fade: ArtnetFade,
}

/// Just enough of any file to tell its format and version.
#[derive(Debug, Deserialize)]
struct Header {
    format: String,
    version: u32,
}

/// What the file holds.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct RigSettings {
    /// The system max level last set; `None`: never set.
    pub(crate) ceiling_dbfs: Option<f64>,
    /// Labelled outputs, sorted by channel.
    pub(crate) outputs: Vec<OutputSetup>,
    /// The rig's SPL meters, with their ids; session files never hold them.
    pub(crate) measurements: Vec<SavedMeasurement>,
    /// Reopen `monitor_session` capture-only when the daemon starts.
    pub(crate) monitor_at_start: bool,
    /// The audio configuration of the last `session.open` while rig meters existed.
    pub(crate) monitor_session: Option<SessionConfig>,
    /// Where OSC cues go; `None`: nowhere.
    pub(crate) osc_target: Option<SocketAddr>,
    /// Where the Art-Net universe goes; `None`: nowhere.
    pub(crate) artnet_target: Option<ArtnetTarget>,
    /// How long the DMX channels take to move to a new cue frame.
    pub(crate) artnet_fade: ArtnetFade,
}

/// Where the rig settings live.
#[derive(Debug)]
pub(crate) struct RigStore {
    path: Option<PathBuf>,
    /// Why the file could not be read; the store is then read-only.
    unreadable: Option<String>,
    /// The file was damaged at start and set aside.
    damage: Option<crate::damage::Damage>,
}

impl RigStore {
    /// Settings kept in memory only (tests, embedded daemons without a config directory).
    pub(crate) fn memory() -> Self {
        Self {
            path: None,
            unreadable: None,
            damage: None,
        }
    }

    /// What was found damaged at start, once.
    pub(crate) fn take_damage(&mut self) -> Option<crate::damage::Damage> {
        self.damage.take()
    }

    /// Reads `path`. Missing → nothing set. Another format version → set aside, nothing
    /// set. Damaged (torn, garbage, values the commands would refuse) → set aside and the
    /// previous copy read ([`crate::damage::recover`]), else nothing set. A file that
    /// cannot be read at all (or moved) → nothing set and read-only.
    pub(crate) fn open(path: &Path) -> (Self, RigSettings) {
        let mut store = Self {
            path: Some(path.to_owned()),
            unreadable: None,
            damage: None,
        };
        let text = match std::fs::read(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return (store, RigSettings::default());
            }
            Err(e) => {
                store.refuse(format!("cannot read it: {e}"));
                return (store, RigSettings::default());
            }
        };
        if let Ok(h) = serde_json::from_slice::<Header>(&text)
            && h.format == FORMAT
            && h.version != VERSION
        {
            match set_aside(path, &format!("v{}", h.version)) {
                Some(to) => {
                    tracing::warn!(
                        "rig settings {} are format version {} (this ac2d reads {VERSION}); set \
                         aside as {} — the system max level starts at the --max-level bound \
                         and the output labels are empty",
                        path.display(),
                        h.version,
                        to.display()
                    );
                }
                None => store.refuse(format!(
                    "format version {} (this ac2d reads {VERSION}) and it cannot be renamed out \
                     of the way",
                    h.version
                )),
            }
            return (store, RigSettings::default());
        }
        match parse(&text) {
            Ok(s) => (store, s),
            Err(reason) => {
                match crate::damage::recover(path, StateFile::RigSettings, &reason, parse) {
                    Ok((s, d)) => {
                        store.damage = Some(d);
                        (store, s.unwrap_or_default())
                    }
                    Err(reason) => {
                        store.refuse(reason);
                        (store, RigSettings::default())
                    }
                }
            }
        }
    }

    pub(crate) fn refuse(&mut self, reason: String) {
        if let Some(p) = &self.path {
            tracing::error!(
                "rig settings {} are unreadable ({reason}); they will not be written — the \
                 system max level is the --max-level bound until the file is fixed or moved \
                 away and ac2d restarted",
                p.display()
            );
        }
        self.unreadable = Some(reason);
    }

    /// Ok unless the file could not be read at start.
    pub(crate) fn check(&self) -> Result<(), ProtoError> {
        match (&self.unreadable, &self.path) {
            (Some(reason), Some(path)) => Err(perr(
                ErrorCode::Refused,
                format!(
                    "rig settings {} are unreadable ({reason}); they are never overwritten — \
                     fix or move the file away and restart ac2d",
                    path.display()
                ),
            )),
            _ => Ok(()),
        }
    }

    /// Writes `s` atomically. Nothing is written (and the caller changes nothing) when the
    /// store is read-only or the write fails.
    pub(crate) fn persist(&self, s: &RigSettings) -> Result<(), ProtoError> {
        self.check()?;
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = RigFile {
            format: FORMAT.into(),
            version: VERSION,
            ceiling_dbfs: s.ceiling_dbfs,
            outputs: s.outputs.clone(),
            measurements: s.measurements.clone(),
            monitor_at_start: s.monitor_at_start,
            monitor_session: s.monitor_session.clone(),
            osc_target: s.osc_target,
            artnet_target: s.artnet_target,
            artnet_fade: s.artnet_fade,
        };
        let text = serde_json::to_vec_pretty(&file)
            .map_err(|e| perr(ErrorCode::Internal, format!("rig settings: {e}")))?;
        crate::damage::write_with_prev(path, &text).map_err(|e| {
            perr(
                ErrorCode::Internal,
                format!("cannot write rig settings {}: {e}", path.display()),
            )
        })
    }
}

/// The settings in `text`, checked.
fn parse(text: &[u8]) -> Result<RigSettings, String> {
    let f = serde_json::from_slice::<RigFile>(text).map_err(|e| e.to_string())?;
    if f.format != FORMAT {
        return Err(format!("not a rig settings file (format {:?})", f.format));
    }
    if f.version != VERSION {
        return Err(format!(
            "format version {} (this ac2d reads {VERSION})",
            f.version
        ));
    }
    let s = RigSettings {
        ceiling_dbfs: f.ceiling_dbfs,
        outputs: f.outputs,
        measurements: f.measurements,
        monitor_at_start: f.monitor_at_start,
        monitor_session: f.monitor_session,
        osc_target: f.osc_target,
        artnet_target: f.artnet_target,
        artnet_fade: f.artnet_fade,
    };
    check(&s)?;
    Ok(s)
}

/// A file a person edited may hold anything: the same rules as the commands.
fn check(s: &RigSettings) -> Result<(), String> {
    if let Some(c) = s.ceiling_dbfs
        && !(c.is_finite() && c <= 0.0)
    {
        return Err(format!(
            "ceiling_dbfs {c} is not a level at or below 0 dBFS"
        ));
    }
    if let Some(a) = s.osc_target
        && (a.ip().is_unspecified() || a.port() == 0)
    {
        return Err(format!("osc_target {a} is not an address to send to"));
    }
    if let Some(t) = s.artnet_target {
        crate::control::check_artnet_addr(t.addr).map_err(|e| format!("artnet_target {e}"))?;
    }
    for (i, o) in s.outputs.iter().enumerate() {
        if s.outputs[..i].iter().any(|x| x.channel == o.channel) {
            return Err(format!("output {} is listed twice", o.channel + 1));
        }
        match &o.label {
            Some(l) => ac2_proto::model::check_output_label(l)
                .map_err(|e| format!("output {}: {e}", o.channel + 1))?,
            None => return Err(format!("output {} has no label", o.channel + 1)),
        }
    }
    // A meter the commands would refuse never runs from a file a person edited.
    for (i, m) in s.measurements.iter().enumerate() {
        if s.measurements[..i].iter().any(|x| x.id == m.id) {
            return Err(format!("measurement {} is listed twice", m.id));
        }
        if m.config.scope != MeasScope::Rig || !matches!(m.config.kind, MeasKind::Spl { .. }) {
            return Err(format!("measurement {} is not a rig SPL meter", m.id));
        }
        crate::control::validate_meas(&m.config)
            .map_err(|e| format!("measurement {}: {}", m.id, e.msg))?;
    }
    if let Some(c) = &s.monitor_session {
        crate::session::validate(c).map_err(|e| format!("monitor session: {}", e.msg))?;
    }
    Ok(())
}

/// Renames `p` to `<p>.<tag>` (the time appended if that exists). Returns where it went.
fn set_aside(p: &Path, tag: &str) -> Option<PathBuf> {
    let name = p.file_name()?.to_string_lossy().into_owned();
    let mut to = p.with_file_name(format!("{name}.{tag}"));
    if to.exists() {
        to = p.with_file_name(format!(
            "{name}.{tag}-{}",
            crate::util::wall_ns() / 1_000_000_000
        ));
    }
    std::fs::rename(p, &to).ok().map(|()| to)
}

/// The system max level the daemon starts with: the one last set, never above `bound`
/// (a later, lower `--max-level` wins), else `bound`.
pub(crate) fn start_ceiling(saved: Option<f64>, bound: f64) -> f64 {
    saved.map_or(bound, |c| c.min(bound))
}

/// `current` with `rows` applied by channel (a `None` label removes the row), sorted.
pub(crate) fn upsert_outputs(current: &[OutputSetup], rows: &[OutputSetup]) -> Vec<OutputSetup> {
    let mut out: Vec<OutputSetup> = current
        .iter()
        .filter(|c| rows.iter().all(|r| r.channel != c.channel))
        .cloned()
        .collect();
    out.extend(rows.iter().filter(|r| r.label.is_some()).cloned());
    out.sort_by_key(|o| o.channel);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(channel: u16, l: &str) -> OutputSetup {
        OutputSetup {
            channel,
            label: Some(l.into()),
        }
    }

    #[test]
    fn roundtrip_missing_and_bound() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        let (store, s) = RigStore::open(&p);
        assert_eq!(s, RigSettings::default());
        let want = RigSettings {
            ceiling_dbfs: Some(-40.0),
            outputs: vec![label(0, "Main L"), label(3, "Sub")],
            osc_target: Some("192.168.1.20:9000".parse().expect("addr")),
            artnet_target: Some(ArtnetTarget {
                addr: "2.255.255.255:6454".parse().expect("addr"),
                universe: ac2_proto::model::PortAddress::new(1).expect("universe"),
            }),
            artnet_fade: ArtnetFade::new(0.0).expect("fade"),
            ..RigSettings::default()
        };
        store.persist(&want).expect("write");
        let (_, got) = RigStore::open(&p);
        assert_eq!(got, want);
        assert_eq!(start_ceiling(got.ceiling_dbfs, -10.0), -40.0);
        // A lower bound given at start wins over the value kept.
        assert_eq!(start_ceiling(got.ceiling_dbfs, -50.0), -50.0);
        assert_eq!(start_ceiling(None, -10.0), -10.0);
        // A file without a target (written before there were cues) reads as none.
        std::fs::write(
            &p,
            r#"{"format":"ac2-rig-settings","version":1,"ceiling_dbfs":null,"outputs":[]}"#,
        )
        .expect("write");
        assert_eq!(RigStore::open(&p).1, RigSettings::default());
    }

    /// A file written before rig meters and cues existed keeps its lowered ceiling and labels:
    /// resetting it would put the generator back at the `--max-level` bound, louder than the
    /// operator set.
    #[test]
    fn a_file_without_meters_or_cues_reads_intact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        std::fs::write(
            &p,
            r#"{"format":"ac2-rig-settings","version":1,"ceiling_dbfs":-30.0,
                "outputs":[{"channel":0,"label":"Main L"},{"channel":3,"label":"Sub"}]}"#,
        )
        .expect("write");
        let (store, got) = RigStore::open(&p);
        assert_eq!(
            got,
            RigSettings {
                ceiling_dbfs: Some(-30.0),
                outputs: vec![label(0, "Main L"), label(3, "Sub")],
                ..RigSettings::default()
            }
        );
        assert!(store.check().is_ok());
        // Written before fades: the default fade, not a jump and not a refusal.
        assert_eq!(got.artnet_fade, ArtnetFade::DEFAULT);
        assert_eq!(got.artnet_fade.seconds(), 1.0);
        assert_eq!(start_ceiling(got.ceiling_dbfs, -10.0), -30.0);
        assert!(!dir.path().join("rig.json.v1").exists());
    }

    fn rig_meter(id: u32, scope: MeasScope) -> SavedMeasurement {
        use ac2_proto::model::{MeasConfig, SplConfig, TimeWeighting, Weighting};
        SavedMeasurement {
            id: ac2_proto::units::MeasId(id),
            config: MeasConfig {
                name: "Stage".into(),
                kind: MeasKind::Spl {
                    config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
                },
                scope,
            },
            running: true,
            delay: None,
            colour: None,
            layout: None,
            compared: Vec::new(),
            offset: ac2_proto::units::Db(0.0),
        }
    }

    #[test]
    fn rig_meters_and_the_monitor_roundtrip_and_are_checked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        let (store, _) = RigStore::open(&p);
        let want = RigSettings {
            measurements: vec![rig_meter(4, MeasScope::Rig)],
            monitor_at_start: true,
            ..RigSettings::default()
        };
        store.persist(&want).expect("write");
        let (store, got) = RigStore::open(&p);
        assert_eq!(got, want);
        assert!(store.check().is_ok());

        // A session measurement, or one listed twice, makes the file damaged: it is set
        // aside and the copy before the last write read instead.
        for bad in [
            vec![rig_meter(4, MeasScope::Session)],
            vec![rig_meter(4, MeasScope::Rig), rig_meter(4, MeasScope::Rig)],
        ] {
            store
                .persist(&RigSettings {
                    measurements: bad,
                    ..RigSettings::default()
                })
                .expect("write");
            let (mut store, got) = RigStore::open(&p);
            assert_eq!(got, want);
            assert!(store.check().is_ok());
            let d = store.take_damage().expect("damage");
            assert!(d.reason.contains("previous copy"), "{}", d.reason);
            assert!(!p.exists());
            std::fs::remove_file(d.set_aside_as.expect("aside")).expect("remove");
        }
    }

    #[test]
    fn damaged_is_set_aside_and_other_versions_are_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        for bad in [
            &b"garbage"[..],
            b"",
            b"{\"format\":\"ac2-rig-se",
            b"\0\0\0\0",
        ] {
            std::fs::write(&p, bad).expect("write");
            let (mut store, s) = RigStore::open(&p);
            assert_eq!(s, RigSettings::default());
            assert!(store.check().is_ok());
            let d = store.take_damage().expect("damage");
            assert_eq!(d.kind, StateFile::RigSettings);
            assert_eq!(
                std::fs::read(d.set_aside_as.expect("aside")).expect("kept"),
                bad
            );
            store.persist(&RigSettings::default()).expect("writable");
            std::fs::remove_file(&p).expect("remove");
        }

        std::fs::write(
            &p,
            r#"{"format":"ac2-rig-settings","version":99,"ceiling_dbfs":-3.0,"outputs":[]}"#,
        )
        .expect("write");
        let (store, s) = RigStore::open(&p);
        assert_eq!(s, RigSettings::default());
        assert!(!p.exists());
        assert!(dir.path().join("rig.json.v99").exists());
        store.persist(&RigSettings::default()).expect("writable");

        // Values a command would refuse make the file unreadable, not half-applied.
        std::fs::write(
            &p,
            r#"{"format":"ac2-rig-settings","version":1,"ceiling_dbfs":3.0,"outputs":[],"measurements":[],"monitor_at_start":false,"monitor_session":null}"#,
        )
        .expect("write");
        let (mut store, s) = RigStore::open(&p);
        assert_eq!(s, RigSettings::default());
        assert!(store.check().is_ok());
        assert!(store.take_damage().is_some());
        std::fs::write(
            &p,
            r#"{"format":"ac2-rig-settings","version":1,"ceiling_dbfs":-3.0,"outputs":[],"artnet_fade":99.0}"#,
        )
        .expect("write");
        let (mut store, s) = RigStore::open(&p);
        assert_eq!(s, RigSettings::default());
        assert!(store.check().is_ok());
        assert!(store.take_damage().is_some());
    }

    #[test]
    fn upsert_sets_replaces_and_clears() {
        let cur = vec![label(0, "Main L"), label(2, "Fill")];
        let got = upsert_outputs(
            &cur,
            &[
                label(1, "Main R"),
                OutputSetup {
                    channel: 2,
                    label: None,
                },
                label(0, "Left"),
            ],
        );
        assert_eq!(got, vec![label(0, "Left"), label(1, "Main R")]);
    }
}
