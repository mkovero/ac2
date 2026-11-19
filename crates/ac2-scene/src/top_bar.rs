//! The window's top bar as one line of parts: the rig shown, where its daemon runs and how
//! it is doing, then the session.
//!
//! `pupu ▾ | remote: ac2pi-0ab1c6 (STIM OFF) · 96 kHz · 256 frames · CPU 43 % · DSP 0 % ·
//! 0 xruns · 85 fps | untitled · unsaved changes`
//!
//! Plain data in, parts out: the view places them (and scrolls the line when it is wider
//! than its room), so the bar reads the same in every state the tests can name without a
//! window.

use crate::audio::SessionLine;
use crate::chrome::{LinkTone, RIG_NOT_AUTHORIZED, RigDown};
use crate::health::HealthLine;

/// The mDNS suffix a host name is shown without: `pupu.local` reads as a rig on this
/// computer, which is what `local` means in the bar.
const MDNS_SUFFIX: &str = ".local";

/// Where a rig is, as the app shows it everywhere: the host name its mDNS advert carries
/// when one was seen for it (`mdns_host`), else the `address` the operator typed. Either
/// without the `.local` suffix; a numeric address is shown as it is.
pub fn host_name(address: &str, mdns_host: Option<&str>) -> String {
    let name = mdns_host.unwrap_or(address).trim_end_matches('.');
    let cut = name.len().saturating_sub(MDNS_SUFFIX.len());
    match name.get(cut..) {
        Some(tail) if cut > 0 && tail.eq_ignore_ascii_case(MDNS_SUFFIX) => name[..cut].to_owned(),
        _ => name.to_owned(),
    }
}

/// Where the rig shown's daemon runs, as the app reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place<'a> {
    /// On this computer, over its own transport: the daemon the app starts beside itself.
    Local,
    /// Over the network, at `host` ([`host_name`]).
    Remote { host: &'a str },
}

/// The link to the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarLink<'a> {
    Connecting,
    /// The last attempt failed with `error`.
    Failed {
        error: &'a str,
        down: RigDown,
    },
    /// `responding`: its heartbeat is fresh; `synced`: the mirrored state has caught up.
    Connected {
        responding: bool,
        synced: bool,
    },
}

/// The generator of the rig shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarStimulus<'a> {
    /// `STIM OFF`, `ARMED`, `FIRING`, …
    pub badge: &'a str,
    pub phase: StimulusPhase,
    /// What plays and where (`pink −20.0 dBFS → Main L, Main R`): said after the badge
    /// unless off.
    pub plays: &'a str,
    pub tip: &'a str,
    /// Another client holds the generator.
    pub held_by: Option<&'a str>,
}

/// How loud the stimulus part reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StimulusPhase {
    Off,
    /// Armed, or on its way to armed or to playing: nothing sounds yet.
    Armed,
    Firing,
}

/// The audio I/O of the rig shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarAudio<'a> {
    /// Open, on `device`.
    Open {
        device: &'a str,
        line: &'a SessionLine,
    },
    /// Open and stopped: what the bar says of it, and the banner behind it.
    Stopped { text: &'a str, hover: &'a str },
    /// None open: what is missing and the key that opens it.
    None { text: &'a str },
}

/// The working session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarSession<'a> {
    /// `friday`, `untitled · unsaved changes`.
    pub text: &'a str,
    pub unsaved: bool,
    pub detail: &'a str,
}

/// How a part is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Dim,
    Warn,
    Fault,
    /// The stimulus colour: a replayed recording stands in for the inputs.
    Armed,
}

/// What a part of the line stands after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lead {
    /// The line's first part.
    First,
    /// A space: it belongs to the part before it.
    Space,
    /// ` · `: another figure of the same segment.
    Dot,
    /// A rule: the next segment.
    Rule,
}

/// What a click on a part opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// Settings › Connection.
    Connection,
    /// The Sessions dialog.
    Sessions,
}

/// One part of the line: a text in one colour with its own tooltip and click.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub lead: Lead,
    pub text: String,
    pub tone: Tone,
    /// Strong: the stimulus while it is armed or plays.
    pub strong: bool,
    pub hover: Option<String>,
    pub click: Option<Click>,
    /// The health meter: `text` in spans, which the view lays out in fixed slots so a
    /// figure changing moves nothing after it.
    pub figures: Option<HealthLine>,
}

impl Part {
    fn new(lead: Lead, text: impl Into<String>, tone: Tone) -> Self {
        Self {
            lead,
            text: text.into(),
            tone,
            strong: false,
            hover: None,
            click: None,
            figures: None,
        }
    }

    fn hover(mut self, h: impl Into<String>) -> Self {
        self.hover = Some(h.into());
        self
    }

    fn click(mut self, c: Click) -> Self {
        self.click = Some(c);
        self
    }
}

/// Something more the line says after the session: an autosave, a recording, a replay,
/// what the stimulus keys do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub tone: Tone,
    pub hover: Option<String>,
}

/// What the bar is built from.
#[derive(Clone, Debug, PartialEq)]
pub struct BarInputs<'a> {
    /// The rig shown, as the drop-down names it.
    pub rig: &'a str,
    pub place: Place<'a>,
    /// The link's target in full, with the numeric address: for the hover.
    pub target: &'a str,
    /// The daemon's software and version, once it said hello: for the hover.
    pub server: Option<&'a str>,
    pub link: BarLink<'a>,
    /// `None`: no daemon state mirrored yet.
    pub stimulus: Option<BarStimulus<'a>>,
    pub audio: Option<BarAudio<'a>>,
    /// Transfer functions across rigs stopped by the clocks: why and what to do.
    pub banners: &'a [String],
    pub health: Option<HealthLine>,
    pub session: Option<BarSession<'a>>,
    pub notes: Vec<Note>,
}

/// The bar: the rig shown with its link's tone, fixed at the left, and the line after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub rig: String,
    /// The dot on the rig's name.
    pub tone: LinkTone,
    pub line: Vec<Part>,
}

impl Bar {
    /// The line as one string, a rule as ` | `.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for p in &self.line {
            s.push_str(match p.lead {
                Lead::First => "",
                Lead::Space => " ",
                Lead::Dot => " · ",
                Lead::Rule => " | ",
            });
            s.push_str(&p.text);
        }
        s
    }
}

/// `local`, `remote: ac2pi-0ab1c6`; a host called as the rig is not said twice: `remote`.
fn place_text(rig: &str, place: Place<'_>) -> String {
    match place {
        Place::Local => "local".to_owned(),
        Place::Remote { host } if host.eq_ignore_ascii_case(rig) => "remote".to_owned(),
        Place::Remote { host } => format!("remote: {host}"),
    }
}

/// The bar for `i`.
pub fn bar(i: &BarInputs<'_>) -> Bar {
    let place = place_text(i.rig, i.place);
    // The software and the numeric address are for whoever asks: the hover and Settings.
    let whole = match i.server {
        Some(s) => format!("{s} · {}", i.target),
        None => i.target.to_owned(),
    };
    let about = format!("{whole} · click: Settings › Connection");
    // Link trouble takes the daemon's segment over: nothing it would say after is current.
    let trouble = match i.link {
        BarLink::Connecting => {
            let whom = match i.place {
                Place::Local => "the local daemon",
                Place::Remote { host } => host,
            };
            Some((LinkTone::Warn, format!("connecting to {whom}…"), None))
        }
        BarLink::Failed {
            error,
            down: RigDown::NotAuthorized,
        } => Some((
            LinkTone::Fault,
            format!("{place} · {RIG_NOT_AUTHORIZED}"),
            Some(error),
        )),
        BarLink::Failed {
            down: RigDown::NetworkOff,
            ..
        } => Some((
            LinkTone::Warn,
            format!("{place} · {}", crate::network::RIG_NETWORK_OFF),
            None,
        )),
        BarLink::Failed {
            error,
            down: RigDown::Unreachable,
        } => Some((
            LinkTone::Fault,
            format!("{place} · {error} · retrying"),
            None,
        )),
        BarLink::Connected {
            responding: false, ..
        } => Some((LinkTone::Fault, format!("{place} · not responding"), None)),
        BarLink::Connected { synced: false, .. } => {
            Some((LinkTone::Warn, format!("{place} · syncing"), None))
        }
        BarLink::Connected { .. } => None,
    };
    let mut line = Vec::new();
    let tone = match trouble {
        Some((tone, text, more)) => {
            let t = match tone {
                LinkTone::Fault => Tone::Fault,
                _ => Tone::Warn,
            };
            let hover = match more {
                Some(m) => format!("{m}\n{about}"),
                None => about,
            };
            line.push(
                Part::new(Lead::First, text, t)
                    .hover(hover)
                    .click(Click::Connection),
            );
            tone
        }
        None => {
            line.push(
                Part::new(Lead::First, place, Tone::Plain)
                    .hover(about)
                    .click(Click::Connection),
            );
            daemon_parts(i, &mut line);
            LinkTone::Ok
        }
    };
    if let Some(s) = i.session {
        let tone = if s.unsaved { Tone::Warn } else { Tone::Plain };
        line.push(
            Part::new(Lead::Rule, s.text, tone)
                .hover(s.detail)
                .click(Click::Sessions),
        );
        for n in &i.notes {
            let mut p = Part::new(Lead::Dot, n.text.clone(), n.tone);
            p.hover.clone_from(&n.hover);
            line.push(p);
        }
    }
    Bar {
        rig: i.rig.to_owned(),
        tone,
        line,
    }
}

/// What a daemon whose link is fine says after its place: the stimulus, the audio I/O, a
/// transfer function stopped across rigs, the health meter.
fn daemon_parts(i: &BarInputs<'_>, line: &mut Vec<Part>) {
    if let Some(s) = i.stimulus {
        let (tone, text) = match s.phase {
            StimulusPhase::Off => (Tone::Dim, format!("({})", s.badge)),
            StimulusPhase::Armed => (Tone::Warn, format!("({} {})", s.badge, s.plays)),
            StimulusPhase::Firing => (Tone::Fault, format!("({} {})", s.badge, s.plays)),
        };
        let mut p = Part::new(Lead::Space, text, tone);
        p.strong = s.phase != StimulusPhase::Off;
        // Off, the level and outputs it would play at are a hover away.
        let hover = match s.phase {
            StimulusPhase::Off => format!("{} · {}", s.plays, s.tip),
            _ => s.tip.to_owned(),
        };
        line.push(p.hover(hover));
        if let Some(who) = s.held_by {
            line.push(Part::new(Lead::Space, format!("held by {who}"), Tone::Warn));
        }
    }
    match i.audio {
        Some(BarAudio::Open { device, line: l }) => {
            let tone = if l.changed { Tone::Warn } else { Tone::Dim };
            let text = match &l.change {
                Some(c) => format!("{} · {c}", l.short),
                None => l.short.clone(),
            };
            let hover = match &l.detail {
                Some(d) => format!("{device} · {d}"),
                None => device.to_owned(),
            };
            line.push(Part::new(Lead::Dot, text, tone).hover(hover));
        }
        Some(BarAudio::Stopped { text, hover }) => {
            line.push(Part::new(Lead::Dot, text, Tone::Fault).hover(hover));
        }
        Some(BarAudio::None { text }) => line.push(Part::new(Lead::Dot, text, Tone::Dim)),
        None => {}
    }
    if !i.banners.is_empty() {
        let text = i.banners.join(" · ");
        line.push(Part::new(Lead::Dot, text.clone(), Tone::Warn).hover(text));
    }
    if let Some(h) = &i.health {
        let mut p = Part::new(Lead::Dot, h.text(), Tone::Dim).hover(h.detail.clone());
        p.figures = Some(h.clone());
        line.push(p);
    }
}

#[cfg(test)]
mod tests;
