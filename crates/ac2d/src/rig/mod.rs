//! The rig's settings the daemon keeps across restarts: the system max level (the
//! generator ceiling), the output labels, the rig's own SPL meters, whether the daemon
//! reopens their audio at start, and where cues go. They belong to the rig, not to a
//! session: a loaded session must never raise the level limit, the wiring outlives any
//! session, and a meter watching a noise limit must keep watching whatever session a client
//! loads.
//!
//! One small JSON file in the daemon's config directory, written atomically (temporary file,
//! synced, renamed), the copy before the last write kept beside it. Read section by section
//! ([`file`]): a section that cannot be read costs only itself — the file is set aside as
//! damaged, that section taken from the previous copy where it reads there, the rest kept,
//! and the file written again with what was read. A file that cannot be read at all is set
//! aside and its previous copy read instead. A file that cannot be moved out of the way is
//! never written: every change is refused with the reason, so a lowered limit is never
//! silently replaced. A file of any other format version, older or newer, is set aside
//! (renamed, never deleted) and nothing is read from it.

mod file;

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ac2_proto::ProtoError;
use ac2_proto::model::{
    ArtnetFade, ArtnetTarget, ColourFamily, LevelsSettings, MeasConfig, Measurement, OutputSetup,
    PaneLayout, RigPeer, StateFile, StreamConfig,
};
use ac2_proto::units::{Db, MeasId};
use ac2_traces::session::SavedMeasurement;
use serde::Serialize;

use crate::util::{internal, refused};
use file::{Read, VERSION};

/// A rig SPL meter as the rig settings keep it: what the operator set for it, and nothing
/// a session measurement holds for its session (compare picks, display offset, delay).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct RigMeter {
    pub(crate) id: MeasId,
    pub(crate) config: MeasConfig,
    /// Running when last changed (restarted at start when a stream opens).
    pub(crate) running: bool,
    /// Colour family the operator chose; `None`: the one the id gives.
    pub(crate) colour: Option<ColourFamily>,
    /// The panes clients show while it is selected; `None`: none of its own.
    pub(crate) layout: Option<PaneLayout>,
}

impl RigMeter {
    /// What the rig settings keep of `m`.
    pub(crate) fn of(m: &Measurement) -> Self {
        Self {
            id: m.id,
            config: m.config.clone(),
            running: m.running,
            colour: m.colour,
            layout: m.layout.clone(),
        }
    }

    /// As a session file keeps a measurement (the rig log directory's manifest is one).
    pub(crate) fn saved(&self) -> SavedMeasurement {
        SavedMeasurement {
            id: self.id,
            config: self.config.clone(),
            running: self.running,
            delay: None,
            colour: self.colour,
            layout: self.layout.clone(),
            offset: Db(0.0),
            eq: Vec::new(),
            // An SPL meter makes no runs.
            next_run: 1,
        }
    }
}

/// What the file holds.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct RigSettings {
    /// The system max level last set; `None`: never set.
    pub(crate) ceiling_dbfs: Option<f64>,
    /// Labelled outputs, sorted by channel.
    pub(crate) outputs: Vec<OutputSetup>,
    /// The rig's SPL meters, by id; session files never hold them.
    pub(crate) meters: Vec<RigMeter>,
    /// Reopen `monitor_stream` capture-only when the daemon starts.
    pub(crate) monitor_at_start: bool,
    /// The audio configuration of the last `stream.open` while rig meters existed.
    pub(crate) monitor_stream: Option<StreamConfig>,
    /// Where OSC cues go; `None`: nowhere.
    pub(crate) osc_target: Option<SocketAddr>,
    /// Where the Art-Net universe goes; `None`: nowhere.
    pub(crate) artnet_target: Option<ArtnetTarget>,
    /// How long the DMX channels take to move to a new cue frame.
    pub(crate) artnet_fade: ArtnetFade,
    /// The programme meters: ballistic, correlation window, over threshold, gate.
    pub(crate) levels: LevelsSettings,
    /// The rigs this one may take audio from (`rig.pair`, `rig.unpair`).
    pub(crate) peers: Vec<RigPeer>,
}

/// Where the rig settings live.
#[derive(Debug)]
pub(crate) struct RigStore {
    path: Option<PathBuf>,
    /// Why the file could not be read; the store is then read-only.
    unreadable: Option<String>,
    /// The file was damaged at start and set aside.
    damage: Option<crate::damage::Damage>,
    /// The max level the file on disk holds, as last read whole or written: `None` when
    /// the file was missing, set aside, recovered or salvaged and not written since.
    stored: std::cell::Cell<Option<f64>>,
    /// What a write puts in the file's max level.
    level: LevelWrites,
}

/// What a write of the rig settings puts in the file's max level.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LevelWrites {
    /// The level in force: the outputs may reach loudspeakers, and it was set for them.
    InForce,
    /// The level the file held at start, unchanged (`None`: none). A level set while the
    /// outputs reach nothing (the simulated rig) says nothing about what loudspeakers on
    /// a real rig take: stored, the next start of a real daemon on the same settings would
    /// read it as the operator's and skip the quiet start.
    AsRead(Option<f64>),
}

impl RigStore {
    /// Settings kept in memory only (tests, ephemeral daemons).
    pub(crate) fn memory() -> Self {
        Self {
            path: None,
            unreadable: None,
            damage: None,
            stored: std::cell::Cell::new(None),
            level: LevelWrites::InForce,
        }
    }

    /// Where the daemon's outputs go, and the level the file held at start: where they
    /// reach nothing, the file keeps that level and the level in force lives only in memory.
    pub(crate) fn for_outputs(&mut self, reach: OutputsReach, read: Option<f64>) {
        self.level = match reach {
            OutputsReach::Loudspeakers => LevelWrites::InForce,
            OutputsReach::Nothing => LevelWrites::AsRead(read),
        };
    }

    /// Keeps a new max level in force (`gen.ceiling`, the start's own level) with the rest
    /// of `s`. Where the outputs reach nothing the level is not stored and nothing else
    /// changed, so nothing is written and nothing can refuse it.
    pub(crate) fn store_level(&self, s: &RigSettings) -> Result<(), ProtoError> {
        match self.level {
            LevelWrites::InForce => self.persist(s),
            LevelWrites::AsRead(_) => Ok(()),
        }
    }

    /// The max level the file on disk holds, as last read whole or written; `None` when
    /// it may hold none or another.
    pub(crate) fn stored_ceiling(&self) -> Option<f64> {
        self.stored.get()
    }

    /// What was found damaged at start, once.
    pub(crate) fn take_damage(&mut self) -> Option<crate::damage::Damage> {
        self.damage.take()
    }

    /// Reads `path`. Missing → nothing set. Another format version, older or newer → set
    /// aside, nothing set. Not a rig settings file at all (torn, garbage) → set aside and the previous
    /// copy read ([`crate::damage::recover`]), else nothing set. Sections that cannot be
    /// read → set aside, those sections from the previous copy where they read there, the
    /// rest kept, and the file written again. A file that cannot be read at all (or moved)
    /// → what could be read, and read-only.
    pub(crate) fn open(path: &Path) -> (Self, RigSettings) {
        let mut store = Self {
            path: Some(path.to_owned()),
            unreadable: None,
            damage: None,
            stored: std::cell::Cell::new(None),
            level: LevelWrites::InForce,
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
        let (sections, lost) = match file::read(&text) {
            Ok(Read::File { sections, lost }) => (*sections, lost),
            Ok(Read::OtherVersion(v)) => {
                match set_aside(path, &format!("v{v}")) {
                    Some(to) => {
                        tracing::warn!(
                            "rig settings {} are format version {v} (this ac2d reads \
                             {VERSION}); set aside as {} — the system max level starts as with \
                             none stored (quiet: −50 dBFS, or the --max-level bound if lower; \
                             the bound on a simulated rig) and the output labels are empty",
                            path.display(),
                            to.display()
                        );
                    }
                    None => store.refuse(format!(
                        "format version {v} (this ac2d reads {VERSION}) and it cannot be \
                         renamed out of the way"
                    )),
                }
                return (store, RigSettings::default());
            }
            Err(reason) => {
                return match crate::damage::recover(
                    path,
                    StateFile::RigSettings,
                    &reason,
                    read_whole,
                ) {
                    Ok((s, d)) => {
                        store.damage = Some(d);
                        (store, s.unwrap_or_default())
                    }
                    Err(reason) => {
                        store.refuse(reason);
                        (store, RigSettings::default())
                    }
                };
            }
        };
        if lost.is_empty() {
            let s = sections.settings();
            store.stored.set(s.ceiling_dbfs);
            return (store, s);
        }
        let (s, reason) = salvage(path, sections, &lost);
        match ac2_paths::set_aside_damaged(path) {
            Ok(to) => {
                let d = crate::damage::Damage {
                    kind: StateFile::RigSettings,
                    path: path.to_owned(),
                    set_aside_as: Some(to),
                    reason,
                };
                d.log();
                store.damage = Some(d);
            }
            Err(e) => {
                store.refuse(format!("{reason}, and it cannot be set aside: {e}"));
                return (store, s);
            }
        }
        // Written now, so that the next start reads it whole; what was not read stays in
        // the copy set aside.
        if let Err(e) = store.persist(&s) {
            tracing::error!("rig settings not written back: {}", e.msg);
        }
        (store, s)
    }

    pub(crate) fn refuse(&mut self, reason: String) {
        if let Some(p) = &self.path {
            tracing::error!(
                "rig settings {} are unreadable ({reason}); they will not be written — the \
                 system max level starts as with none stored (quiet: −50 dBFS, or the \
                 --max-level bound if lower; the bound on a simulated rig) and cannot be \
                 raised, as a raise must be stored, until the file is fixed or moved away \
                 and ac2d restarted",
                p.display()
            );
        }
        self.unreadable = Some(reason);
    }

    /// Ok unless the file could not be read at start.
    pub(crate) fn check(&self) -> Result<(), ProtoError> {
        match (&self.unreadable, &self.path) {
            (Some(reason), Some(path)) => Err(refused(format!(
                "rig settings {} are unreadable ({reason}); they are never overwritten — \
                 fix or move the file away and restart ac2d",
                path.display()
            ))),
            _ => Ok(()),
        }
    }

    /// Writes `s` atomically. `Err` (the store read-only, or a write failed) leaves the file
    /// as it was, and the caller changes nothing.
    pub(crate) fn persist(&self, s: &RigSettings) -> Result<(), ProtoError> {
        self.check()?;
        let as_read;
        let s = match self.level {
            LevelWrites::InForce => s,
            LevelWrites::AsRead(level) => {
                as_read = RigSettings {
                    ceiling_dbfs: level,
                    ..s.clone()
                };
                &as_read
            }
        };
        let Some(path) = &self.path else {
            self.stored.set(s.ceiling_dbfs);
            return Ok(());
        };
        let text = file::write(s).map_err(|e| internal(format!("rig settings: {e}")))?;
        let failed = |e: std::io::Error| {
            internal(format!("cannot write rig settings {}: {e}", path.display()))
        };
        // The previous copy is what a start reads when this file is torn: it must never
        // hold a higher max level than the one now in force, or a torn write would raise
        // it. A copy that may (an old file or copy above the new level, holding none, or
        // unreadable as settings) is replaced by the new text first, and only then the file
        // itself: a failure or crash at either step leaves the file as it was and the copy
        // no higher. A copy holding no level counts as higher, as a start without one
        // may come up at the bound (a daemon whose outputs reach nothing); one holding a
        // level counts as higher than a write of none (a simulated daemon's, keeping the
        // file's own none), whose start is quiet.
        let above = |bytes: Option<Vec<u8>>| {
            bytes.is_some_and(|b| {
                copy_above(read_whole(&b).ok().map(|p| p.ceiling_dbfs), s.ceiling_dbfs)
            })
        };
        let prev = crate::damage::prev_path(path);
        if above(std::fs::read(path).ok()) || above(std::fs::read(&prev).ok()) {
            ac2_paths::write_private_atomic(&prev, &text).map_err(failed)?;
            ac2_paths::write_private_atomic(path, &text).map_err(failed)?;
        } else {
            crate::damage::write_with_prev(path, &text).map_err(failed)?;
        }
        self.stored.set(s.ceiling_dbfs);
        Ok(())
    }
}

/// The settings of a file of which `lost` could not be read: each section not read taken
/// from the previous copy where it reads there, else as never set. Returns them and the
/// reason to report, naming every section and meter lost.
fn salvage(path: &Path, sections: file::Sections, lost: &[String]) -> (RigSettings, String) {
    let unread = sections.unread();
    let sections = match previous(path) {
        Some(prev) if !unread.is_empty() => sections.or(prev),
        _ => sections,
    };
    let still = sections.unread();
    let from_prev: Vec<_> = unread.iter().filter(|k| !still.contains(k)).collect();
    let mut what = String::from("read the rest");
    if !from_prev.is_empty() {
        what += &format!(
            "; {} from the previous copy {}",
            join(&from_prev),
            crate::damage::prev_path(path).display()
        );
    }
    if !still.is_empty() {
        what += &format!("; started without {}", join(&still));
    }
    (
        sections.settings(),
        format!("not read: {}; {what}", lost.join("; ")),
    )
}

/// The settings in a previous copy, each section not read there as never set.
fn read_whole(text: &[u8]) -> Result<RigSettings, String> {
    match file::read(text)? {
        Read::File { sections, .. } => Ok((*sections).settings()),
        Read::OtherVersion(v) => Err(format!("format version {v} (this ac2d reads {VERSION})")),
    }
}

/// Whether a copy on disk whose max level reads `was` (`None`: unreadable as settings;
/// `Some(None)`: no level) may let a torn write start louder than `now`, the level being
/// written: then the copy is replaced first.
fn copy_above(was: Option<Option<f64>>, now: Option<f64>) -> bool {
    match (was, now) {
        (Some(Some(was)), Some(now)) => was > now,
        (Some(Some(_)), None) => true,
        (_, None) => false,
        _ => true,
    }
}

/// The sections of the previous copy of `path`, if it is a rig settings file this ac2d reads.
fn previous(path: &Path) -> Option<file::Sections> {
    let text = std::fs::read(crate::damage::prev_path(path)).ok()?;
    match file::read(&text) {
        Ok(Read::File { sections, .. }) => Some(*sections),
        _ => None,
    }
}

fn join(keys: &[impl AsRef<str>]) -> String {
    keys.iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(", ")
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

/// The system max level a rig starts with when none was stored and its outputs can reach
/// loudspeakers, dBFS RMS (never above the `--max-level` bound). A rig nobody has set up
/// yet, or one whose file was lost, may have any amplifier and speaker behind its outputs:
/// it must not start able to play loud. −50 dBFS still reaches the speakers audibly enough
/// to check the wiring, and the operator raises it, live, to what the system takes.
pub(crate) const UNSTORED_START_DBFS: f64 = -50.0;

/// Where a daemon's outputs can go, which decides how it starts with no level stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputsReach {
    /// At least one offered backend is a real audio API: loudspeakers may be behind it.
    Loudspeakers,
    /// Only the simulated device or a replay: nothing outside the process hears it.
    Nothing,
}

impl OutputsReach {
    /// Of the backends a daemon offers: any one that reaches loudspeakers decides it, as a
    /// stream may open on it.
    pub(crate) fn of(kinds: impl IntoIterator<Item = ac2_audio::BackendKind>) -> Self {
        if kinds
            .into_iter()
            .any(ac2_audio::BackendKind::reaches_loudspeakers)
        {
            Self::Loudspeakers
        } else {
            Self::Nothing
        }
    }
}

/// The level a start takes with none stored: quiet ([`UNSTORED_START_DBFS`], or `bound`
/// when lower) where loudspeakers may listen; `bound` where nothing does, so a simulated
/// rig starts ready to measure.
pub(crate) fn unstored_ceiling(bound: f64, reach: OutputsReach) -> f64 {
    match reach {
        OutputsReach::Loudspeakers => UNSTORED_START_DBFS.min(bound),
        OutputsReach::Nothing => bound,
    }
}

/// The system max level the daemon starts with: the one last set, never above `bound`
/// (a later, lower `--max-level` wins), else [`unstored_ceiling`]; the start stores what
/// this returns. A level the file holds but that cannot be read is `None` here, so the
/// start is the same as with no file.
pub(crate) fn start_ceiling(saved: Option<f64>, bound: f64, reach: OutputsReach) -> f64 {
    saved.map_or_else(|| unstored_ceiling(bound, reach), |c| c.min(bound))
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
    use ac2_proto::model::{MeasKind, MeasScope};

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
        let (mut store, got) = RigStore::open(&p);
        assert_eq!(got, want);
        assert!(store.take_damage().is_none());
        assert_eq!(
            start_ceiling(got.ceiling_dbfs, -10.0, OutputsReach::Loudspeakers),
            -40.0
        );
        // A lower bound given at start wins over the value kept.
        assert_eq!(
            start_ceiling(got.ceiling_dbfs, -50.0, OutputsReach::Loudspeakers),
            -50.0
        );
        // Nothing stored: quiet where loudspeakers may listen, never above a bound lower
        // still; at the bound where nothing does.
        let hw = OutputsReach::Loudspeakers;
        assert_eq!(start_ceiling(None, -10.0, hw), UNSTORED_START_DBFS);
        assert_eq!(start_ceiling(None, -60.0, hw), -60.0);
        assert_eq!(start_ceiling(None, -10.0, OutputsReach::Nothing), -10.0);
        use ac2_audio::BackendKind as K;
        assert_eq!(
            OutputsReach::of([K::Fake, K::Replay]),
            OutputsReach::Nothing
        );
        assert_eq!(OutputsReach::of([K::Fake, K::Jack]), hw);
    }

    fn rig_meter(id: u32, scope: MeasScope) -> RigMeter {
        use ac2_proto::model::{SplConfig, TimeWeighting, Weighting};
        RigMeter {
            id: MeasId(id),
            config: MeasConfig {
                name: "Stage".into(),
                kind: MeasKind::Spl {
                    config: SplConfig::on_input(1, Weighting::A, TimeWeighting::Fast),
                },
                scope,
            },
            running: true,
            colour: None,
            layout: None,
        }
    }

    /// A meter the commands would refuse, or one listed twice, is left out and reported;
    /// the other meters and every other section are read.
    #[test]
    fn a_bad_meter_costs_only_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        let (store, _) = RigStore::open(&p);
        let want = RigSettings {
            ceiling_dbfs: Some(-20.0),
            meters: vec![rig_meter(4, MeasScope::Rig)],
            monitor_at_start: true,
            ..RigSettings::default()
        };
        store.persist(&want).expect("write");
        let (store, got) = RigStore::open(&p);
        assert_eq!(got, want);

        for (bad, why) in [
            (
                rig_meter(5, MeasScope::Session),
                "meter 5: not a rig SPL meter",
            ),
            (rig_meter(4, MeasScope::Rig), "meter 4: listed twice"),
        ] {
            let mut meters = want.meters.clone();
            meters.push(bad);
            store
                .persist(&RigSettings {
                    meters,
                    ..want.clone()
                })
                .expect("write");
            let (mut store, got) = RigStore::open(&p);
            assert_eq!(got, want);
            assert!(store.check().is_ok());
            let d = store.take_damage().expect("damage");
            assert!(d.reason.contains(why), "{}", d.reason);
            // Written again with what was read: the next start finds nothing wrong.
            let (mut again, s) = RigStore::open(&p);
            assert_eq!(s, want);
            assert!(again.take_damage().is_none());
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
            b"[]",
            b"{\"format\":\"something else\"}",
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

        // Another version, older or newer: set aside whole, nothing read from it, and not
        // reported as damage — it is a file of another build, not a broken one.
        for v in [VERSION - 1, 1, VERSION + 88] {
            let text = format!(
                r#"{{"format":"ac2-rig-settings","version":{v},"ceiling_dbfs":-3.0,"outputs":[]}}"#
            );
            std::fs::write(&p, &text).expect("write");
            let (mut store, s) = RigStore::open(&p);
            assert_eq!(s, RigSettings::default());
            assert!(store.check().is_ok());
            assert!(store.take_damage().is_none());
            assert!(!p.exists());
            assert_eq!(
                std::fs::read_to_string(dir.path().join(format!("rig.json.v{v}"))).expect("kept"),
                text
            );
            store.persist(&RigSettings::default()).expect("writable");
            std::fs::remove_file(&p).expect("remove");
        }
    }

    /// Values a command would refuse cost their section only. The ceiling is taken from
    /// the previous copy where it reads there, else none: the start then is as with none stored.
    #[test]
    fn a_refused_value_costs_its_section() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        let (store, _) = RigStore::open(&p);
        let good = RigSettings {
            ceiling_dbfs: Some(-25.0),
            outputs: vec![label(0, "Main L")],
            ..RigSettings::default()
        };
        store.persist(&good).expect("write");
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&p).expect("read")).expect("json");
        v["ceiling_dbfs"] = 3.0.into();
        v["artnet_fade"] = 99.0.into();
        v["outputs"] = serde_json::json!([{ "channel": 1, "label": "Main R" }]);
        // Written as a new copy, so that the good file is the previous copy.
        store
            .persist(&RigSettings {
                outputs: vec![label(1, "Main R")],
                ..good.clone()
            })
            .expect("write");
        std::fs::write(&p, serde_json::to_vec(&v).expect("json")).expect("write");
        let (mut store, s) = RigStore::open(&p);
        let d = store.take_damage().expect("damage");
        assert!(
            d.reason.contains("ceiling_dbfs: 3 is not a level"),
            "{}",
            d.reason
        );
        assert!(d.reason.contains("artnet_fade:"), "{}", d.reason);
        assert_eq!(
            s,
            RigSettings {
                ceiling_dbfs: Some(-25.0),
                outputs: vec![label(1, "Main R")],
                ..RigSettings::default()
            }
        );

        // No previous copy to take it from: no level, so the start is quiet.
        std::fs::remove_file(crate::damage::prev_path(&p)).expect("remove prev");
        std::fs::write(&p, serde_json::to_vec(&v).expect("json")).expect("write");
        let (mut store, s) = RigStore::open(&p);
        let d = store.take_damage().expect("damage");
        assert!(
            d.reason
                .contains("started without ceiling_dbfs, artnet_fade"),
            "{}",
            d.reason
        );
        assert_eq!(s.ceiling_dbfs, None);
        assert_eq!(
            start_ceiling(s.ceiling_dbfs, -10.0, OutputsReach::Loudspeakers),
            UNSTORED_START_DBFS
        );
        assert_eq!(s.outputs, vec![label(1, "Main R")]);
    }

    /// A copy holding a level is above a write of none: a simulated daemon writing the
    /// file's own none replaces the copy too, so a torn file cannot bring a level back.
    #[test]
    fn a_copy_with_a_level_is_above_a_write_of_none() {
        assert!(copy_above(Some(Some(-30.0)), None));
        assert!(!copy_above(Some(None), None));
        assert!(!copy_above(None, None));
        assert!(copy_above(Some(Some(-30.0)), Some(-40.0)));
        assert!(!copy_above(Some(Some(-50.0)), Some(-40.0)));
        assert!(copy_above(Some(None), Some(-40.0)));
        assert!(copy_above(None, Some(-40.0)));

        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("rig.json");
        let (store, _) = RigStore::open(&p);
        let loud = RigSettings {
            ceiling_dbfs: Some(-30.0),
            ..RigSettings::default()
        };
        store.persist(&loud).expect("write");
        let prev = crate::damage::prev_path(&p);
        std::fs::copy(&p, &prev).expect("copy");
        let level = |p: &Path| {
            read_whole(&std::fs::read(p).expect("read"))
                .expect("whole")
                .ceiling_dbfs
        };
        assert_eq!(level(&prev), Some(-30.0));
        let (mut store, _) = RigStore::open(&p);
        store.for_outputs(OutputsReach::Nothing, None);
        store.persist(&loud).expect("write");
        assert_eq!((level(&p), level(&prev)), (None, None));
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
