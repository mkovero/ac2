//! What a network client reads of the daemon host's file system: the base name of every
//! path, never where it lies. A session, a recording or a damaged file is told by its name;
//! the directories above it say who runs the rig and how its host is laid out, and a
//! network client cannot name a path anyway (`SessionRef::Path`, `RecordingRef::Path` are
//! the local pair's). The local pair (the OS user on the host) reads them whole.
//!
//! Applied to every reply to a network client ([`super::Control::answer`]) and to every
//! event sent on the network pair ([`change`]).

use ac2_proto::event::Change;
use ac2_proto::model::{
    DamagedFile, RecordingRun, Rig, SessionOrigin, State, Stream, WorkingSession,
};
use ac2_proto::{ErrorDetail, ProtoError, ReplyBody};

use super::transfer::file_part;

fn base(path: &mut String) {
    let b = file_part(path).to_owned();
    *path = b;
}

fn working(w: &mut WorkingSession) {
    if let Some(SessionOrigin::Path { path }) = &mut w.origin {
        base(path);
    }
}

fn recording(r: &mut RecordingRun) {
    base(&mut r.path);
}

fn stream(s: &mut Stream) {
    if let Some(r) = s.open.as_mut().and_then(|o| o.replay.as_mut()) {
        base(&mut r.path);
    }
}

fn damaged(d: &mut DamagedFile) {
    base(&mut d.path);
    if let Some(a) = &mut d.set_aside_as {
        base(a);
    }
}

fn rig(r: &mut Rig) {
    r.damaged_files.iter_mut().for_each(damaged);
}

fn state(st: &mut State) {
    working(&mut st.working);
    if let Some(r) = &mut st.recording {
        recording(r);
    }
    stream(&mut st.stream);
    rig(&mut st.rig);
}

/// A snapshot as a network client is sent it: inline in a reply, or as the bytes of a
/// snapshot transfer, which no reply redaction reaches.
pub(super) fn snapshot(s: &mut ac2_proto::StateSnapshot) {
    state(&mut s.state);
}

/// `c` as a network client is sent it, when that differs from what the local pair is sent.
pub(crate) fn change(c: &Change) -> Option<Change> {
    let mut c = match c {
        Change::Working(w) if matches!(w.origin, Some(SessionOrigin::Path { .. })) => c.clone(),
        Change::Recording(_) => c.clone(),
        Change::Stream(s) if s.open.as_ref().is_some_and(|o| o.replay.is_some()) => c.clone(),
        Change::Rig(r) if !r.damaged_files.is_empty() => c.clone(),
        _ => return None,
    };
    match &mut c {
        Change::Working(w) => working(w),
        Change::Recording(r) => recording(r),
        Change::Stream(s) => stream(s),
        Change::Rig(r) => rig(r),
        _ => {}
    }
    Some(c)
}

/// A reply as a network client is sent it.
pub(super) fn reply(result: &mut Result<ReplyBody, ProtoError>) {
    match result {
        Ok(body) => body_of(body),
        Err(e) => {
            if let Some(ErrorDetail::CalStore { path, .. }) = &mut e.detail {
                base(path);
            }
        }
    }
}

fn body_of(body: &mut ReplyBody) {
    match body {
        ReplyBody::Stream(s) => stream(s),
        ReplyBody::Snapshot(s) => state(&mut s.state),
        ReplyBody::Events(evs) => {
            for e in evs {
                if let Some(c) = change(&e.change) {
                    e.change = c;
                }
            }
        }
        ReplyBody::SessionFile(f) => base(&mut f.path),
        ReplyBody::Sessions(fs) => fs.iter_mut().for_each(|f| base(&mut f.path)),
        ReplyBody::Recording(r) => recording(r),
        ReplyBody::Recordings(rs) => rs.iter_mut().for_each(|r| base(&mut r.path)),
        _ => {}
    }
}
