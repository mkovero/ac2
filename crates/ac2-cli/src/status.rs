//! `ac2 status` as a terminal operator reads it: the rig and daemon first, then the audio
//! I/O, the rigs this one takes audio or levels from (each once), the measurements between
//! rigs, the inputs and the autosave, each a section of aligned rows. Trouble starts its
//! line with `!`. The words are `ac2-scene`'s, the short forms where a column has no room
//! for a sentence; `-v` adds the identifiers and the long sentences.

mod layout;

use ac2_proto::Welcome;
use ac2_proto::cal::state_input_use;
use ac2_proto::model::{
    AutosaveState, ClockRelation, CrossTfState, InputKind, PeerRelation, RemoteInput, RemoteLink,
    RemoteState, Session, State, StopWhy, remote_slot,
};
use ac2_proto::units::WallNs;
use ac2_scene::between_rigs as br;
use ac2_scene::format;
use ac2_scene::time::ClockOffset;

pub use layout::{Line, Row, Section, render};

/// The page width the default view keeps to.
pub const PAGE: usize = 100;

/// The daemon's build against this `ac2`'s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Build {
    /// The same build.
    Same,
    /// A local daemon of another build: an older `ac2d` still running after an upgrade.
    Stale { daemon: String, ours: String },
    /// A remote daemon of another build.
    Other { daemon: String, ours: String },
    /// The daemon did not report a build id.
    Unknown,
}

/// What the page is made from.
pub struct Input<'a> {
    pub welcome: &'a Welcome,
    pub session: &'a Session,
    pub state: &'a State,
    pub build: Build,
    /// Wall time now, on this machine's clock.
    pub now: WallNs,
    /// The local UTC offset at a wall time, for a time of day.
    pub offset_s: &'a dyn Fn(WallNs) -> i32,
    pub verbose: bool,
}

/// The status page.
pub fn text(i: &Input<'_>) -> String {
    let remote_inputs = i
        .session
        .open
        .as_ref()
        .map(|o| o.config.remote_inputs.as_slice())
        .unwrap_or_default();
    render(
        &[
            header(i),
            audio(i, remote_inputs),
            rigs(i, remote_inputs),
            measurements(i),
            inputs(i, remote_inputs),
            autosave(i),
        ],
        // `-v` keeps each of the scene's sentences whole on its line.
        if i.verbose { usize::MAX } else { PAGE },
    )
}

fn header(i: &Input<'_>) -> Section {
    let w = i.welcome;
    let mut rows = Vec::new();
    match &i.build {
        Build::Same => {}
        Build::Stale { daemon, ours } => rows.push(
            Row::new([
                "STALE",
                &format!("daemon build {daemon}, this ac2 is {ours}"),
            ])
            .trouble(true)
            .more(Line::trouble(
                "restart it: `ac2 daemon stop && ac2 daemon start`",
                true,
            )),
        ),
        Build::Other { daemon, ours } => {
            rows.push(Row::new(["build", &format!("{daemon} (this ac2: {ours})")]))
        }
        Build::Unknown => rows.push(Row::new([
            "build",
            "unknown (the daemon did not report a build id)",
        ])),
    }
    if i.verbose {
        rows.push(Row::new(["rig id", &w.rig_id.to_string()]));
        rows.push(Row::new(["client id", &w.client_id.0]));
        rows.push(Row::new([
            "incarnation",
            &format!("{:016x}", w.daemon_incarnation.0),
        ]));
        rows.push(Row::new(["epoch", &w.session_epoch.to_string()]));
        rows.push(Row::new(["rev", &w.rev.to_string()]));
    }
    Section {
        heading: w.name.clone(),
        summary: Some(Line::plain(w.server.clone())),
        rows,
    }
}

/// Channels (zero-based) as one-based numbers, a run of consecutive ones as a range:
/// `1–8, 11`.
pub fn channel_ranges(ch: &[u16]) -> String {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for c in ch.iter().map(|c| u32::from(*c) + 1) {
        match runs.last_mut() {
            Some((_, hi)) if *hi + 1 == c => *hi = c,
            _ => runs.push((c, c)),
        }
    }
    runs.iter()
        .map(|&(lo, hi)| match hi - lo {
            0 => lo.to_string(),
            1 => format!("{lo}, {hi}"),
            _ => format!("{lo}–{hi}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A session channel's name: `3` for this rig's input, `ketunkolo in 1` for another rig's.
fn input_name(r: &RemoteState, remote: &[RemoteInput], ch: u16) -> String {
    match remote_slot(ch).and_then(|s| remote.iter().find(|x| x.slot == s)) {
        Some(x) => ac2_scene::remote_inputs::remote_name(&br::rig_name(r, x), x.channel, None),
        None => (u32::from(ch) + 1).to_string(),
    }
}

fn audio(i: &Input<'_>, remote: &[RemoteInput]) -> Section {
    let st = i.state;
    let mut rows = Vec::new();
    let Some(o) = &i.session.open else {
        return Section {
            heading: "AUDIO".into(),
            summary: Some(Line::plain("closed")),
            rows: if i.verbose {
                vec![Row::new(["epoch", &i.session.epoch.to_string()])]
            } else {
                Vec::new()
            },
        };
    };
    let rate = ac2_scene::audio::rate_text(o.sample_rate_hz);
    let summary = match &o.replay {
        Some(r) => format!("replaying {} ({:?}) · {rate}", r.name, r.pace),
        None => format!(
            "open · {} · {rate} · {} samples",
            o.input_device.0, o.buffer_frames
        ),
    };
    if let Some(s) = &i.session.stopped {
        let t = ac2_scene::audio::audio_stopped_text(s, i.now, i.offset_s);
        let mut row = Row::new(["audio", &t.status]).trouble(true);
        if i.verbose {
            row = row.more(Line::trouble(t.detail, true));
        }
        rows.push(row);
    }
    if let Some(r) = &o.replay {
        rows.push(Row::new(["file", &r.path]));
        rows.push(Row::new([
            "recorded",
            &ac2_scene::recording::clock(r.frames as f64 / f64::from(o.sample_rate_hz.max(1))),
        ]));
    }
    let local: Vec<u16> = o
        .config
        .input_channels
        .iter()
        .copied()
        .filter(|c| remote_slot(*c).is_none())
        .collect();
    let mut ins = Vec::new();
    if !local.is_empty() {
        ins.push(channel_ranges(&local));
    }
    for x in remote {
        ins.push(input_name(&st.remote, remote, x.session_channel()));
    }
    let mut in_row = Row::new([
        "in",
        &if ins.is_empty() {
            "none".to_owned()
        } else {
            ins.join(" · ")
        },
    ]);
    if !remote.is_empty() {
        in_row = in_row.more(Line::plain(ac2_scene::cross_rig::link_offset_line(
            o.config.link_offset,
            &st.remote,
        )));
    }
    rows.push(in_row);
    if o.replay.is_none() {
        let n = o.config.output_channels;
        let out = if n == 0 {
            "none".to_owned()
        } else if o.output_device != o.input_device {
            format!(
                "{n} · {} (another device: the two clocks may drift)",
                o.output_device.0
            )
        } else {
            n.to_string()
        };
        rows.push(Row::new(["out", &out]));
    }
    rows.push(clock_row(i, o));
    driver_rows(o, &mut rows);
    Section {
        heading: "AUDIO".into(),
        summary: Some(Line::plain(summary)),
        rows,
    }
}

/// The session's clock domain, its drift, then the sample and system clocks in the
/// Connection page's words.
fn clock_row(i: &Input<'_>, o: &ac2_proto::model::OpenSession) -> Row {
    let t = &i.state.timing;
    let (relation, long) = match o.clock {
        ClockRelation::SingleCallback => ("one clock", "one callback for input and output"),
        ClockRelation::Unknown => (
            "input and output may be on different clocks",
            "the backend does not say they share one",
        ),
    };
    let (drift, warn) = match &t.drift {
        None if o.config.loopback.is_none() => ("drift not measured (no loopback)".into(), false),
        None => ("drift not measured yet (needs a stimulus)".into(), false),
        Some(d) => (
            format!(
                "drift {} ppm over {}{}",
                format::signed(d.ppm, 1),
                format::duration(d.span.0),
                if d.warning {
                    " · WARNING: output and input on different clocks"
                } else {
                    ""
                }
            ),
            d.warning,
        ),
    };
    let mut row = Row::new(["clock", &format!("{relation} · {drift}")]).trouble(warn);
    if i.verbose {
        row = row.more(Line::plain(long));
    }
    for l in ac2_scene::clock::status_lines(&i.state.clocks) {
        row = row.more(Line::plain(l));
    }
    row
}

/// A driver-reporting session's clock source, spare ports, stated latency, and a change
/// or reset the driver made under it.
fn driver_rows(o: &ac2_proto::model::OpenSession, rows: &mut Vec<Row>) {
    if let Some(c) = &o.clock_source {
        rows.push(Row::new(["source", c]));
    }
    for p in &o.spare_ports {
        rows.push(Row::new(["spare", &ac2_scene::audio::spare_port_text(p)]));
    }
    if let Some(l) = &o.stated_latency {
        rows.push(Row::new([
            "latency",
            &ac2_scene::audio::stated_latency_text(l, o.sample_rate_hz),
        ]));
    }
    for n in [
        ac2_scene::audio::driver_reset_text(o),
        ac2_scene::audio::driver_change_text(o),
    ]
    .into_iter()
    .flatten()
    {
        rows.push(Row::new(["NOTE", &n]));
    }
}

fn relation_trouble(rel: &PeerRelation) -> bool {
    matches!(
        rel,
        PeerRelation::NotLocked { .. } | PeerRelation::CantTell { .. }
    )
}

fn link_trouble(l: &RemoteLink) -> bool {
    !matches!(l, RemoteLink::Up | RemoteLink::Connecting)
}

/// Each rig once: the session's relation and link with one line per input taken from
/// it, else what its levels say (`levels only`).
fn rigs(i: &Input<'_>, remote: &[RemoteInput]) -> Section {
    let r = &i.state.remote;
    let mut rows = Vec::new();
    let levels_line = |idx: usize, name: &str| {
        ac2_scene::standing::status_lines(r, i.now)
            .get(idx)
            .map(|l| {
                let pre = format!("from levels {name} · ");
                format!("levels: {}", l.strip_prefix(&pre).unwrap_or(l))
            })
            .unwrap_or_default()
    };
    for rig in &r.rigs {
        let standing = r.standing.iter().position(|s| s.rig == rig.rig);
        let link = match rig.link {
            RemoteLink::Up => format!("link {}", br::link_text(rig)),
            ref l => format!("link {}", ac2_scene::standing::link_short(l)),
        };
        let rel = &rig.clock.relation;
        let mut row = Row::new([
            rig.name.as_str(),
            &br::clock_short(&rig.name, &rig.clock, 1),
            &link,
        ])
        .trouble(relation_trouble(rel) || link_trouble(&rig.link));
        for x in remote.iter().filter(|x| x.rig == rig.rig) {
            row = row.more(input_line(r, x, &rig.link, i.verbose));
        }
        if let Some(s) = standing.map(|k| &r.standing[k])
            && std::mem::discriminant(&s.clock.relation) != std::mem::discriminant(rel)
        {
            row = row.more(Line::trouble(
                format!("levels: {}", br::clock_short(&s.name, &s.clock, 2)),
                relation_trouble(&s.clock.relation),
            ));
        }
        if i.verbose {
            let at = |t: WallNs| Some(ac2_scene::time::local_clock(t, i.now, |x| (i.offset_s)(x)));
            let block = br::rig_block(r, rig, remote, &at);
            let pre = format!("{}: ", rig.name);
            for (k, l) in block.into_iter().enumerate() {
                let l = if k == 0 {
                    l.strip_prefix(&pre).map(str::to_owned).unwrap_or(l)
                } else {
                    l
                };
                row = row.more(Line::plain(l));
            }
            if let Some(k) = standing {
                row = row.more(Line::plain(levels_line(k, &rig.name)));
            }
        }
        rows.push(row);
    }
    for (k, s) in r.standing.iter().enumerate() {
        if r.rigs.iter().any(|g| g.rig == s.rig) {
            continue;
        }
        let mut row = Row::new([
            s.name.as_str(),
            &br::clock_short(&s.name, &s.clock, 2),
            &format!(
                "levels only · link {}",
                ac2_scene::standing::link_short(&s.link)
            ),
        ])
        .trouble(relation_trouble(&s.clock.relation) || link_trouble(&s.link));
        if i.verbose {
            row = row.more(Line::plain(levels_line(k, &s.name)));
            row = row.more(Line::plain(ac2_scene::standing::FROM_LEVELS));
        }
        rows.push(row);
    }
    Section {
        heading: "RIGS".into(),
        summary: None,
        rows,
    }
}

/// A remote input under its rig: `in 1 streaming · 1 late frames in the last hour · 0
/// slips`; placed on arrival is said (the normal placing, by capture time, is not, but
/// for `-v`); a link not up says why no audio comes.
fn input_line(r: &RemoteState, x: &RemoteInput, link: &RemoteLink, verbose: bool) -> Line {
    let name = format!("in {}", u32::from(x.channel) + 1);
    let state = br::input_state(r, x, &|_| None);
    let trouble = state.tone.is_problem();
    let st = r.inputs.iter().find(|s| s.slot == x.slot);
    let text = match (link, st) {
        (RemoteLink::Up, Some(_)) if verbose => format!("{name} {}", state.text),
        (RemoteLink::Up, Some(st)) => {
            let mut t = br::audio_text(st.audio, &br::rig_name(r, x), x.channel);
            if let Some(a @ ac2_proto::model::Anchor::Arrival { .. }) = &st.anchor {
                t.push_str(&format!(", {}", br::anchor_text(a)));
            }
            format!("{name} {t} · {}", br::input_counts(st))
        }
        _ => format!("{name}: {}", state.text),
    };
    Line::trouble(text, trouble)
}

/// The transfer functions between rigs: whether each averages and from which rig, then
/// what happened to it and its delay.
fn measurements(i: &Input<'_>) -> Section {
    let st = i.state;
    let r = &st.remote;
    let ours = &i.welcome.name;
    let mut rows = Vec::new();
    for x in &r.transfers {
        let Some(m) = st.measurements.iter().find(|m| m.id == x.meas) else {
            continue;
        };
        let label = &m.config.name;
        let title = ac2_scene::cross_rig::pane_title(label, x, r);
        let side = title
            .strip_prefix(&format!("{label} · "))
            .unwrap_or(&title)
            .to_owned();
        let state = ac2_scene::cross_rig::stop_verdict(x).unwrap_or("averaging");
        let stopped = matches!(
            x.state,
            CrossTfState::Stopped {
                why: StopWhy::NotLocked | StopWhy::CantTell,
                ..
            }
        );
        let mut row = Row::new([label.as_str(), state, &side]).trouble(stopped);
        let lines = ac2_scene::cross_rig::status_lines(x, r, "`ac2 delay find`");
        for l in lines.iter().skip(1) {
            let trouble = l.starts_with("link offset too small") || l.contains("slipped");
            row = row.more(Line::trouble(l.clone(), trouble));
        }
        if let Some(d) = &m.delay {
            let basis = if i.verbose {
                ac2_scene::cross_rig::delay_basis(d.applied.0, ours, &br::name_of(r, x.rig))
            } else {
                ac2_scene::cross_rig::delay_basis_short(d.applied.0)
            };
            row = row.more(Line::plain(format!("delay {basis}")));
        }
        if i.verbose
            && let Some(b) = ac2_scene::cross_rig::banner(label, x, r, ours)
        {
            row = row.more(Line::plain(b));
        }
        rows.push(row);
    }
    Section {
        heading: "MEASUREMENTS".into(),
        summary: None,
        rows,
    }
}

/// The channels the input list names: every input with a setup row, and every input the
/// open session captures.
fn input_channels(s: &State) -> Vec<u16> {
    let mut ch: Vec<u16> = s.inputs.iter().map(|i| i.channel).collect();
    if let Some(o) = &s.session.open {
        ch.extend(o.config.input_channels.iter().copied());
    }
    ch.sort_unstable();
    ch.dedup();
    ch
}

/// What a mic is, and a mic's curve and sensitivity in short; a line input `dBFS`, or its
/// line calibration.
fn reference_short(s: &State, ch: u16, now: WallNs) -> String {
    use ac2_proto::cal::Sensitivity;
    let u = state_input_use(s, ch);
    let sens = ac2_scene::cal::sensitivity_short(&u.sensitivity, now, ClockOffset(0));
    match (u.kind, &u.sensitivity) {
        (InputKind::Mic { .. }, _) => match ac2_scene::cal::curve_short(&u.curve) {
            Some(c) => format!("{c} · {sens}"),
            None => sens,
        },
        (_, Sensitivity::Line(_)) => sens,
        (InputKind::Line | InputKind::Digital, _) => "dBFS".to_owned(),
    }
}

/// The calibration in full, for `-v`: whose it is and how it was taken, the chain an
/// electrical calibration rests on included. Nothing for an input without one.
fn reference_long(s: &State, ch: u16, now: WallNs) -> Vec<String> {
    let u = state_input_use(s, ch);
    let Some(e) = u.sensitivity.entry() else {
        return Vec::new();
    };
    vec![
        ac2_scene::cal::sensitivity_state(&u.sensitivity, now, ClockOffset(0)),
        ac2_scene::cal::method_detail(e),
    ]
}

fn kind_text(k: &InputKind) -> String {
    match k.mic() {
        Some(m) => format!("mic {m}"),
        None => crate::output::kind_word(k).to_owned(),
    }
}

fn inputs(i: &Input<'_>, remote: &[RemoteInput]) -> Section {
    let s = i.state;
    let mut rows = Vec::new();
    if !s.inputs.is_empty() || i.session.open.is_some() {
        let name = |ch| input_name(&s.remote, remote, ch);
        let mut shown: Vec<u16> = Vec::new();
        for ch in input_channels(s) {
            if shown.contains(&ch) {
                continue;
            }
            let kind = kind_text(&ac2_proto::cal::input_kind(&s.inputs, ch));
            let row = match s.pairs.iter().find(|p| p.left == ch || p.right == ch) {
                Some(p) => {
                    shown.extend([p.left, p.right]);
                    let label = match (remote_slot(p.left), remote_slot(p.right)) {
                        (None, None) => format!("{}+{}", name(p.left), name(p.right)),
                        _ => format!("{} + {}", name(p.left), name(p.right)),
                    };
                    let pair = p
                        .name
                        .as_deref()
                        .map_or_else(|| "pair".to_owned(), |n| format!("pair {n}"));
                    let (l, r) = (
                        reference_short(s, p.left, i.now),
                        reference_short(s, p.right, i.now),
                    );
                    let mut row = if l == r {
                        Row::new([label, kind, pair, l])
                    } else {
                        Row::new([label, kind, pair, format!("L {l}")])
                            .more(Line::plain(format!("R {r}")))
                    };
                    if i.verbose {
                        for (side, c) in [("L", p.left), ("R", p.right)] {
                            for l in reference_long(s, c, i.now) {
                                row = row.more(Line::plain(format!("{side} {l}")));
                            }
                        }
                    }
                    row
                }
                None => {
                    let mut row =
                        Row::new([name(ch), kind, String::new(), reference_short(s, ch, i.now)]);
                    if i.verbose {
                        for l in reference_long(s, ch, i.now) {
                            row = row.more(Line::plain(l));
                        }
                    }
                    row
                }
            };
            rows.push(row);
        }
    }
    Section {
        heading: "INPUTS".into(),
        summary: None,
        rows,
    }
}

fn autosave(i: &Input<'_>) -> Section {
    let st = i.state;
    let a = &st.autosave;
    let (text, failed) = match &a.state {
        AutosaveState::Failed { reason } => (format!("FAILED: {reason}"), true),
        _ => (
            ac2_scene::autosave::autosave_label(a, i.now, ClockOffset(0))
                .map_or_else(|| "off".to_owned(), |l| l.text),
            false,
        ),
    };
    let mut rows = Vec::new();
    for l in ac2_scene::autosave::log_failure_lines(
        &a.log_failures,
        |m| {
            st.measurements
                .iter()
                .find(|x| x.id == m)
                .map(|x| x.config.name.clone())
        },
        i.now,
        ClockOffset(0),
    ) {
        rows.push(Row::new(["log", &format!("NOT SAVED: {l}")]).trouble(true));
    }
    for l in ac2_scene::autosave::damaged_file_lines(&st.rig.damaged_files) {
        rows.push(Row::new(["damaged", &l]).trouble(true));
    }
    Section {
        heading: "AUTOSAVE".into(),
        summary: Some(Line::trouble(text, failed)),
        rows,
    }
}

#[cfg(test)]
mod tests;
